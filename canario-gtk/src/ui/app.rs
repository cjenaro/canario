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
}

impl CanarioGtkApp {
    pub fn new(canario: Canario, rx: Receiver<Event>) -> Self {
        let app = adw::Application::new(
            Some("com.canario.Canario"),
            ApplicationFlags::FLAGS_NONE,
        );
        let is_recording_flag = Arc::new(AtomicBool::new(false));
        let (tray_tx, tray_rx) = std::sync::mpsc::channel();

        let gtk_app = Self {
            app,
            canario,
            rx,
            tray_handle: Arc::new(Mutex::new(None)),
            is_recording_flag,
            tray_rx,
        };

        gtk_app.setup_signals(tray_tx);
        gtk_app
    }

    fn setup_signals(&self, tray_tx: std::sync::mpsc::Sender<TrayAction>) {
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
            let tt = tray_tx.clone();
            std::thread::spawn(move || {
                match start_tray(flag, tt) {
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

        // BUG-002: Auto-open Settings on first launch when no model downloaded
        let canario_activate = self.canario.clone();
        self.app.connect_activate(move |app| {
            if !canario_activate.is_model_downloaded() {
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

        let app_clone = app.clone();

        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            // BUG-001: Poll tray action channel
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

        Event::Error { message: err } => {
            tracing::error!("Error: {}", err);
        }

        Event::AudioLevel { level } => {
            RecordingIndicator::update_level(app, level);
        }

        // BUG-003: Model download progress — find by widget name
        Event::ModelDownloadProgress { progress: p } => {
            if let Some(win) = app
                .windows()
                .into_iter()
                .find(|w| w.widget_name() == "canario-settings")
            {
                update_download_progress(&win, p);
            }
        }

        Event::ModelDownloadComplete => {
            tracing::info!("Model download complete");
            if let Some(win) = app
                .windows()
                .into_iter()
                .find(|w| w.widget_name() == "canario-settings")
            {
                update_download_complete(&win);
            }
        }

        Event::ModelDownloadFailed { error: err } => {
            tracing::error!("Model download failed: {}", err);
            if let Some(win) = app
                .windows()
                .into_iter()
                .find(|w| w.widget_name() == "canario-settings")
            {
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

// ── BUG-003: Widget-tree helpers that search by widget name ──────────────

fn find_widget_by_name(widget: &gtk4::Widget, name: &str) -> Option<gtk4::Widget> {
    if widget.widget_name() == name {
        return Some(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(found) = find_widget_by_name(&c, name) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

fn update_download_progress(win: &gtk4::Window, progress: f64) {
    if let Some(pb) = find_widget_by_name(win.upcast_ref(), "model-download-progress")
        .and_then(|w| w.downcast::<gtk4::ProgressBar>().ok())
    {
        pb.set_visible(true);
        pb.set_fraction(progress);
        pb.set_show_text(true);
    }
}

fn update_download_complete(win: &gtk4::Window) {
    if let Some(pb) = find_widget_by_name(win.upcast_ref(), "model-download-progress")
        .and_then(|w| w.downcast::<gtk4::ProgressBar>().ok())
    {
        pb.set_fraction(1.0);
        pb.set_visible(false);
    }
    if let Some(label) = find_widget_by_name(win.upcast_ref(), "model-status-label")
        .and_then(|w| w.downcast::<gtk4::Label>().ok())
    {
        label.set_label("✅ Ready");
    }
    if let Some(dl_btn) = find_widget_by_name(win.upcast_ref(), "model-download-btn")
        .and_then(|w| w.downcast::<gtk4::Button>().ok())
    {
        dl_btn.set_visible(false);
        dl_btn.set_sensitive(true);
    }
    if let Some(del_btn) = find_widget_by_name(win.upcast_ref(), "model-delete-btn")
        .and_then(|w| w.downcast::<gtk4::Button>().ok())
    {
        del_btn.set_visible(true);
    }
}

fn update_download_failed(win: &gtk4::Window, err: &str) {
    if let Some(pb) = find_widget_by_name(win.upcast_ref(), "model-download-progress")
        .and_then(|w| w.downcast::<gtk4::ProgressBar>().ok())
    {
        pb.set_visible(false);
    }
    if let Some(label) = find_widget_by_name(win.upcast_ref(), "model-status-label")
        .and_then(|w| w.downcast::<gtk4::Label>().ok())
    {
        label.set_label(&format!("❌ Failed: {}", err));
    }
    if let Some(dl_btn) = find_widget_by_name(win.upcast_ref(), "model-download-btn")
        .and_then(|w| w.downcast::<gtk4::Button>().ok())
    {
        dl_btn.set_visible(true);
        dl_btn.set_sensitive(true);
    }
    if let Some(del_btn) = find_widget_by_name(win.upcast_ref(), "model-delete-btn")
        .and_then(|w| w.downcast::<gtk4::Button>().ok())
    {
        del_btn.set_visible(false);
    }
}
