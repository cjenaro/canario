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
use crate::ui::tray::CanarioTray;

type TrayHandle = ksni::blocking::Handle<CanarioTray>;

pub struct CanarioGtkApp {
    app: adw::Application,
    canario: Canario,
    rx: Receiver<Event>,
    tray_handle: Arc<Mutex<Option<TrayHandle>>>,
    is_recording_flag: Arc<AtomicBool>,
}

impl CanarioGtkApp {
    pub fn new(canario: Canario, rx: Receiver<Event>) -> Self {
        let app = adw::Application::new(
            Some("com.canario.Canario"),
            ApplicationFlags::FLAGS_NONE,
        );
        let is_recording_flag = Arc::new(AtomicBool::new(false));

        let gtk_app = Self {
            app,
            canario,
            rx,
            tray_handle: Arc::new(Mutex::new(None)),
            is_recording_flag,
        };

        gtk_app.setup_signals();
        gtk_app
    }

    fn setup_signals(&self) {
        let canario = self.canario.clone();
        let tx_tray = self.is_recording_flag.clone();
        let tray_handle = self.tray_handle.clone();

        self.app.connect_startup(move |app| {
            tracing::info!("Canario GTK startup");

            // Start hotkey listener
            if let Err(e) = canario.start_hotkey() {
                tracing::warn!("Hotkey listener failed to start: {}", e);
            }

            // Start system tray
            let flag = tx_tray.clone();
            let th = tray_handle.clone();
            std::thread::spawn(move || {
                match start_tray(flag) {
                    Ok(handle) => {
                        *th.lock().unwrap() = Some(handle);
                        tracing::info!("System tray icon started");
                    }
                    Err(e) => {
                        tracing::error!("Tray icon failed: {}", e);
                    }
                }
            });

            std::mem::forget(app.hold());
        });

        let canario_activate = self.canario.clone();
        self.app.connect_activate(move |_app| {
            if !canario_activate.is_model_downloaded() {
                // Show settings to prompt model download
                // We'll send a message via glib idle
            }
        });
    }

    pub fn run(self) -> anyhow::Result<()> {
        let rx = self.rx;
        let app = self.app;
        let canario = self.canario;
        let tray_handle = self.tray_handle;
        let is_recording_flag = self.is_recording_flag;

        let app_clone = app.clone();

        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
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

fn start_tray(is_recording: Arc<AtomicBool>) -> anyhow::Result<TrayHandle> {
    use ksni::blocking::TrayMethods;
    let tray = CanarioTray::new(is_recording);
    let handle = tray.spawn().map_err(|e| anyhow::anyhow!("Failed to spawn tray: {}", e))?;
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

        Event::TranscriptionReady { text, duration_secs } => {
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

        Event::Error(err) => {
            tracing::error!("Error: {}", err);
        }

        Event::AudioLevel(level) => {
            RecordingIndicator::update_level(app, level);
        }

        Event::ModelDownloadProgress(p) => {
            // Forward to settings window if open
            if let Some(win) = app.windows().into_iter().find(|w| w.widget_name() == "canario-settings") {
                // Find and update progress bar
                update_download_progress(&win, p);
            }
        }

        Event::ModelDownloadComplete => {
            tracing::info!("Model download complete");
            if let Some(win) = app.windows().into_iter().find(|w| w.widget_name() == "canario-settings") {
                update_download_complete(&win);
            }
        }

        Event::ModelDownloadFailed(err) => {
            tracing::error!("Model download failed: {}", err);
            if let Some(win) = app.windows().into_iter().find(|w| w.widget_name() == "canario-settings") {
                update_download_failed(&win, &err);
            }
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

fn update_download_progress(win: &gtk4::Window, progress: f64) {
    // Walk widget tree to find progress bar
    if let Some(pb) = find_progress_bar(win.upcast_ref()) {
        pb.set_visible(true);
        pb.set_fraction(progress);
        pb.set_show_text(true);
    }
}

fn update_download_complete(win: &gtk4::Window) {
    if let Some(pb) = find_progress_bar(win.upcast_ref()) {
        pb.set_fraction(1.0);
    }
}

fn update_download_failed(win: &gtk4::Window, _err: &str) {
    if let Some(pb) = find_progress_bar(win.upcast_ref()) {
        pb.set_visible(false);
    }
}

fn find_progress_bar(widget: &gtk4::Widget) -> Option<gtk4::ProgressBar> {
    if let Ok(pb) = widget.clone().downcast::<gtk4::ProgressBar>() {
        return Some(pb);
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(found) = find_progress_bar(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}
