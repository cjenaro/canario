/// GTK4 Application — thin wrapper around canario-core.
///
/// Translates between the core's `Event` channel and GTK4 widgets.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use gio::ApplicationFlags;
use glib::ControlFlow;
use gtk4::prelude::*;
use libadwaita as adw;

use canario_core::{Canario, Event};

use crate::ui::indicator::RecordingIndicator;
use crate::ui::settings::SettingsWindow;
use crate::ui::tray::{CanarioTray, TrayAction};

type TrayHandle = ksni::blocking::Handle<CanarioTray>;

pub struct CanarioGtkApp {
    app: adw::Application,
    canario: Canario,
    rx: Receiver<Event>,
    tray_handle: Arc<Mutex<Option<TrayHandle>>>,
    is_recording_flag: Arc<AtomicBool>,
    tray_rx: std::sync::mpsc::Receiver<TrayAction>,
    tray_visibility_tx: std::sync::mpsc::Sender<bool>,
}

impl CanarioGtkApp {
    pub fn new(canario: Canario, rx: Receiver<Event>) -> Self {
        let app = adw::Application::new(Some("com.canario.Canario"), ApplicationFlags::FLAGS_NONE);
        let is_recording_flag = Arc::new(AtomicBool::new(false));
        let (tray_tx, tray_rx) = std::sync::mpsc::channel();
        let (tray_visibility_tx, tray_visibility_rx) = std::sync::mpsc::channel();

        let gtk_app = Self {
            app,
            canario,
            rx,
            tray_handle: Arc::new(Mutex::new(None)),
            is_recording_flag,
            tray_rx,
            tray_visibility_tx,
        };

        // Serialize tray creation and removal off the GTK thread. A visibility
        // change during D-Bus startup is applied as soon as startup finishes.
        let flag = gtk_app.is_recording_flag.clone();
        let handle = gtk_app.tray_handle.clone();
        std::thread::spawn(move || {
            while let Ok(visible) = tray_visibility_rx.recv() {
                if visible {
                    match start_tray(flag.clone(), tray_tx.clone()) {
                        Ok(tray) => *handle.lock().unwrap() = Some(tray),
                        Err(e) => tracing::error!("Tray icon failed: {}", e),
                    }
                } else if let Some(tray) = handle.lock().unwrap().take() {
                    tray.shutdown().wait();
                }
            }
            if let Some(tray) = handle.lock().unwrap().take() {
                tray.shutdown().wait();
            }
        });

        gtk_app.setup_signals();
        gtk_app
    }

    fn setup_signals(&self) {
        let canario = self.canario.clone();

        self.app.connect_startup(move |app| {
            tracing::info!("Canario GTK startup");

            // Start hotkey listener
            if let Err(e) = canario.start_hotkey() {
                tracing::warn!("Hotkey listener failed to start: {}", e);
            }

            // Keep the application alive for the entire tray lifetime.
            //
            // Canario is a tray app: most of the time it has NO windows open,
            // and GTK exits the main loop when the last window closes. Holding
            // a use-count on the Application prevents that. The hold is
            // intentionally never released — the guard lives until process
            // exit, when the OS reclaims it. `std::mem::forget` just makes
            // that "leak on purpose" explicit; there is no cleaner GTK idiom
            // for "run forever with zero windows".
            std::mem::forget(app.hold());
        });

        // On first launch (no model downloaded yet), open Settings so the
        // user is prompted to download the ASR model.
        let canario_activate = self.canario.clone();
        self.app.connect_activate(move |app| {
            if !canario_activate.is_model_downloaded() || !canario_activate.config().show_tray_icon
            {
                SettingsWindow::present(app, &canario_activate);
            }
        });
    }

    pub fn run(self) -> anyhow::Result<()> {
        let rx = self.rx;
        let tray_rx = self.tray_rx;
        let app = self.app;
        let canario = self.canario;
        let tray_handle = self.tray_handle;
        let is_recording_flag = self.is_recording_flag;
        let tray_visibility_tx = self.tray_visibility_tx;
        let mut tray_visible = None;

        let app_clone = app.clone();

        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            let visible = canario.config().show_tray_icon;
            if tray_visible != Some(visible) {
                let _ = tray_visibility_tx.send(visible);
                tray_visible = Some(visible);
            }

            // Drain pending tray actions (sent from the ksni tray thread).
            while let Ok(action) = tray_rx.try_recv() {
                match action {
                    TrayAction::ToggleRecording => {
                        canario.toggle_recording();
                    }
                    TrayAction::ShowSettings => {
                        SettingsWindow::present(&app_clone, &canario);
                    }
                    TrayAction::Quit => {
                        canario.shutdown();
                        app_clone.quit();
                        return ControlFlow::Break;
                    }
                }
            }

            // Poll core events
            while let Ok(event) = rx.try_recv() {
                handle_event(
                    &app_clone,
                    &canario,
                    &tray_handle,
                    &is_recording_flag,
                    event,
                );
            }
            ControlFlow::Continue
        });

        app.run_with_args::<String>(&[]);
        Ok(())
    }
}

fn start_tray(
    is_recording: Arc<AtomicBool>,
    action_tx: std::sync::mpsc::Sender<TrayAction>,
) -> anyhow::Result<TrayHandle> {
    use ksni::blocking::TrayMethods;
    let tray = CanarioTray::new(is_recording, action_tx);
    let handle = tray
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn tray: {}", e))?;
    Ok(handle)
}

fn refresh_tray(tray_handle: &Arc<Mutex<Option<TrayHandle>>>) {
    if let Some(handle) = tray_handle.lock().unwrap().as_ref() {
        handle.update(|_| {});
    }
}

fn handle_event(
    app: &adw::Application,
    canario: &Canario,
    tray_handle: &Arc<Mutex<Option<TrayHandle>>>,
    is_recording_flag: &Arc<AtomicBool>,
    event: Event,
) {
    match event {
        Event::RecordingStarted => {
            is_recording_flag.store(true, Ordering::SeqCst);
            RecordingIndicator::show(app);
            refresh_tray(tray_handle);
        }

        Event::RecordingStopped => {
            is_recording_flag.store(false, Ordering::SeqCst);
            RecordingIndicator::hide(app);
            refresh_tray(tray_handle);
        }

        Event::RecordingCancelled => {
            // Recording was discarded (Escape) — hide the indicator,
            // reset the tray, and do NOT paste or add to history.
            tracing::info!("Recording cancelled — audio discarded");
            is_recording_flag.store(false, Ordering::SeqCst);
            RecordingIndicator::hide(app);
            refresh_tray(tray_handle);
        }

        Event::TranscriptionReady {
            text,
            duration_secs,
        } => {
            tracing::info!("✅ Transcription: {}", text);

            let config = canario.config();
            if config.auto_paste {
                match canario_core::paste_text(&text) {
                    Ok(pasted) => {
                        if pasted {
                            tracing::info!("📋 Auto-typed");
                            if config.sound_effects {
                                super::app::app_beep_confirm();
                            }
                        } else {
                            tracing::info!("📋 Copied to clipboard (Ctrl+V to paste)");
                        }
                    }
                    Err(e) => tracing::error!("Paste failed: {}", e),
                }
            }

            // Store in history
            canario.add_history(text, duration_secs, None);
        }

        Event::Error { message: err } => {
            tracing::error!("Error: {}", err);
        }

        Event::AudioLevel { level } => {
            RecordingIndicator::update_level(app, level);
        }

        // Live caption preview for long recordings — rendered by the
        // Electron overlay. The GTK indicator shows no text yet, so the
        // partial is only logged (follow-up: caption view in GTK).
        Event::PartialTranscript { text } => {
            tracing::debug!("Live partial transcript: {}", text);
        }

        // Model download events are routed straight to the settings window's
        // registered download-status widgets (see model_manager.rs).
        Event::ModelDownloadProgress { progress: p } => {
            crate::ui::model_manager::download_progress(p);
        }

        Event::ModelDownloadComplete => {
            tracing::info!("Model download complete");
            crate::ui::model_manager::download_complete();
        }

        Event::ModelDownloadFailed { error: err } => {
            tracing::error!("Model download failed: {}", err);
            crate::ui::model_manager::download_failed(&err);
        }

        Event::HotkeyTriggered => {
            canario.toggle_recording();
        }
    }
}

/// Play confirmation beep (callable without &self)
pub fn app_beep_confirm() {
    canario_core::audio_effects::beep_confirm();
}
