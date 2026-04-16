/// GTK4 Application — the main entry point for the GUI app.
///
/// Architecture:
///   - Main thread: GTK4 main loop (blocking)
///   - Background thread: ksni tray service (blocking D-Bus)
///   - Communication via std::sync::mpsc + glib::timeout_add_local polling
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use gio::ApplicationFlags;
use glib::ControlFlow;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::AppConfig;
use crate::ui::settings::SettingsWindow;
use crate::ui::indicator::RecordingIndicator;
use crate::ui::tray::CanarioTray;
use crate::ui::{AppMessage, AppState, AppStatus};

pub struct CanarioApp {
    app: adw::Application,
    state: Arc<Mutex<AppState>>,
    tx: Sender<AppMessage>,
    rx: Receiver<AppMessage>,
}

impl CanarioApp {
    pub fn new() -> Self {
        let app = adw::Application::new(
            Some("com.canario.Canario"),
            ApplicationFlags::FLAGS_NONE,
        );

        let config = AppConfig::load().unwrap_or_default();
        let state = Arc::new(Mutex::new(AppState::new(config)));

        let (tx, rx) = mpsc::channel::<AppMessage>();

        let canario = Self {
            app,
            state: state.clone(),
            tx,
            rx,
        };

        canario.setup_signals();
        canario
    }

    fn setup_signals(&self) {
        let state = self.state.clone();
        let tx = self.tx.clone();

        self.app.connect_startup(move |app| {
            tracing::info!("Canario startup");

            // Load model status
            {
                let mut s = state.lock().unwrap();
                if s.is_model_ready() {
                    s.status = AppStatus::Ready;
                    tracing::info!("Model already downloaded, ready to record");
                } else {
                    tracing::info!("No model found — open Settings to download one");
                }
            }

            // Start system tray in a background thread
            let tray_tx = tx.clone();
            std::thread::spawn(move || {
                if let Err(e) = start_tray(tray_tx) {
                    tracing::error!("Tray icon failed: {}", e);
                    tracing::info!("Running without system tray icon");
                }
            });

            // Keep the app running even without visible windows.
            // Forget the guard so the hold is never released — the app
            // exits only when app.quit() is called from the tray.
            std::mem::forget(app.hold());
        });

        // connect_activate is called on first launch and when the app is
        // activated again (e.g. desktop file activation). We use it to
        // show the settings window if no model is configured yet.
        let state_for_activate = self.state.clone();
        let tx_for_activate = self.tx.clone();
        self.app.connect_activate(move |_app| {
            let s = state_for_activate.lock().unwrap();
            if !s.is_model_ready() {
                drop(s);
                let _ = tx_for_activate.send(AppMessage::ShowSettings);
            }
        });
    }

    /// Run the GTK4 main loop. Blocks until the app quits.
    pub fn run(self) -> anyhow::Result<()> {
        let rx = self.rx;
        let app = self.app;
        let state = self.state;

        // Poll for messages from background threads
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            // Drain all pending messages
            while let Ok(msg) = rx.try_recv() {
                handle_message(&app, &state, msg);
            }
            ControlFlow::Continue
        });

        app.run_with_args::<String>(&[]);
        Ok(())
    }
}

/// Start the system tray icon in a background thread.
/// Blocks the current thread forever to keep the ksni Handle alive.
fn start_tray(tx: Sender<AppMessage>) -> anyhow::Result<()> {
    use ksni::blocking::TrayMethods;

    let tray = CanarioTray::new(tx);
    let _handle = tray.spawn().map_err(|e| {
        anyhow::anyhow!("Failed to spawn tray: {}", e)
    })?;

    tracing::info!("System tray icon started");

    // Block this thread forever so the handle is never dropped.
    // Using a channel is more robust than thread::park() which can
    // wake spuriously.
    let (_shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
    let _ = shutdown_rx.recv();

    Ok(())
}

/// Handle a message from background threads
fn handle_message(
    app: &adw::Application,
    state: &Arc<Mutex<AppState>>,
    msg: AppMessage,
) {
    match msg {
        AppMessage::ShowSettings => {
            SettingsWindow::present(app, state.clone());
        }

        AppMessage::ToggleRecording => {
            let mut s = state.lock().unwrap();
            if s.is_recording {
                s.is_recording = false;
                s.status = AppStatus::Transcribing;
                tracing::info!("Recording stopped — transcribing...");
                // TODO: wire to actual transcription engine
                s.status = AppStatus::Ready;
            } else if s.can_record() {
                s.is_recording = true;
                s.status = AppStatus::Recording;
                tracing::info!("Recording started");
                RecordingIndicator::show(app);
            } else {
                tracing::warn!("Cannot record in current state: {:?}", s.status);
            }
        }

        AppMessage::Quit => {
            tracing::info!("Quitting...");
            app.quit();
        }

        AppMessage::RecordingStarted => {
            let mut s = state.lock().unwrap();
            s.is_recording = true;
            s.status = AppStatus::Recording;
            RecordingIndicator::show(app);
        }

        AppMessage::RecordingStopped => {
            let mut s = state.lock().unwrap();
            s.is_recording = false;
            s.status = AppStatus::Transcribing;
            RecordingIndicator::hide(app);
            // TODO: wire to actual transcription engine
        }

        AppMessage::TranscriptionReady(text) => {
            let mut s = state.lock().unwrap();
            s.is_transcribing = false;
            s.last_transcription = text.clone();
            s.status = AppStatus::Ready;
            tracing::info!("Transcription: {}", text);

            if s.config.auto_paste {
                if let Err(e) = crate::ui::paste::paste_text(&text) {
                    tracing::error!("Paste failed: {}", e);
                }
            }
        }

        AppMessage::DownloadProgress(p) => {
            let mut s = state.lock().unwrap();
            s.status = AppStatus::Downloading(p);
        }

        AppMessage::DownloadComplete => {
            let mut s = state.lock().unwrap();
            s.status = AppStatus::Ready;
            tracing::info!("Model download complete");
        }

        AppMessage::DownloadFailed(err) => {
            let mut s = state.lock().unwrap();
            s.status = AppStatus::Error(err.clone());
            tracing::error!("Model download failed: {}", err);
        }

        AppMessage::AudioLevel(level) => {
            RecordingIndicator::update_level(app, level);
        }
    }
}
