/// GTK4 Application — the main entry point for the GUI app.
///
/// Architecture:
///   - Main thread: GTK4 main loop (blocking)
///   - Background thread: ksni tray service (D-Bus)
///   - Background thread: mic capture + VAD + transcription (on demand)
///   - Communication via std::sync::mpsc + glib::timeout_add_local polling
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use gio::ApplicationFlags;
use glib::ControlFlow;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::AppConfig;
use crate::ui::recording::{self, RecordingHandle};
use crate::ui::settings::SettingsWindow;
use crate::ui::indicator::RecordingIndicator;
use crate::ui::tray::CanarioTray;
use crate::ui::{AppMessage, AppState, AppStatus};

pub struct CanarioApp {
    app: adw::Application,
    state: Arc<Mutex<AppState>>,
    tx: Sender<AppMessage>,
    rx: Receiver<AppMessage>,
    /// Handle to the active recording thread (if any)
    recording_handle: Arc<Mutex<Option<RecordingHandle>>>,
    /// Shared recording flag — read by the tray to update its menu
    is_recording_flag: Arc<AtomicBool>,
}

impl CanarioApp {
    pub fn new() -> Self {
        let app = adw::Application::new(
            Some("com.canario.Canario"),
            ApplicationFlags::FLAGS_NONE,
        );

        let config = AppConfig::load().unwrap_or_default();
        let state = Arc::new(Mutex::new(AppState::new(config)));
        let is_recording_flag = Arc::new(AtomicBool::new(false));

        let (tx, rx) = mpsc::channel::<AppMessage>();

        let canario = Self {
            app,
            state: state.clone(),
            tx,
            rx,
            recording_handle: Arc::new(Mutex::new(None)),
            is_recording_flag: is_recording_flag.clone(),
        };

        canario.setup_signals();
        canario
    }

    fn setup_signals(&self) {
        let state = self.state.clone();
        let tx = self.tx.clone();
        let is_recording_flag = self.is_recording_flag.clone();

        self.app.connect_startup(move |app| {
            tracing::info!("Canario startup");

            {
                let mut s = state.lock().unwrap();
                if s.is_model_ready() {
                    s.status = AppStatus::Ready;
                    tracing::info!("Model already downloaded, ready to record");
                } else {
                    tracing::info!("No model found — open Settings to download one");
                }
            }

            // Start system tray — pass the shared recording flag
            let tray_tx = tx.clone();
            let flag = is_recording_flag.clone();
            std::thread::spawn(move || {
                if let Err(e) = start_tray(tray_tx, flag) {
                    tracing::error!("Tray icon failed: {}", e);
                    tracing::info!("Running without system tray icon");
                }
            });

            std::mem::forget(app.hold());
        });

        // Show settings if no model
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
        let state = self.state;
        let app = self.app;
        let recording_handle = self.recording_handle;
        let tx = self.tx;
        let is_recording_flag = self.is_recording_flag;

        let app_clone = app.clone();

        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            while let Ok(msg) = rx.try_recv() {
                handle_message(&app_clone, &state, &recording_handle, &tx, &is_recording_flag, msg);
            }
            ControlFlow::Continue
        });

        app.run_with_args::<String>(&[]);
        Ok(())
    }
}

fn start_tray(tx: Sender<AppMessage>, is_recording: Arc<AtomicBool>) -> anyhow::Result<()> {
    use ksni::blocking::TrayMethods;

    let tray = CanarioTray::new(tx, is_recording);
    let _handle = tray.spawn().map_err(|e| {
        anyhow::anyhow!("Failed to spawn tray: {}", e)
    })?;

    tracing::info!("System tray icon started");

    let (_shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
    let _ = shutdown_rx.recv();

    Ok(())
}

fn handle_message(
    app: &adw::Application,
    state: &Arc<Mutex<AppState>>,
    recording_handle: &Arc<Mutex<Option<RecordingHandle>>>,
    tx: &Sender<AppMessage>,
    is_recording_flag: &Arc<AtomicBool>,
    msg: AppMessage,
) {
    match msg {
        AppMessage::ShowSettings => {
            SettingsWindow::present(app, state.clone());
        }

        AppMessage::ToggleRecording => {
            let s = state.lock().unwrap();
            let mut handle = recording_handle.lock().unwrap();

            if s.is_recording {
                // ── Stop recording ───────────────────────────────
                if let Some(h) = handle.take() {
                    tracing::info!("Stopping recording...");
                    h.stop();
                }
                is_recording_flag.store(false, Ordering::SeqCst);
            } else if s.can_record() {
                // ── Start recording ──────────────────────────────
                let model_dir = s.config.local_model_dir();

                match recording::start_recording(model_dir, tx.clone()) {
                    Ok(h) => {
                        *handle = Some(h);
                        is_recording_flag.store(true, Ordering::SeqCst);
                        drop(s);
                        let mut s = state.lock().unwrap();
                        s.is_recording = true;
                        s.status = AppStatus::Recording;
                        RecordingIndicator::show(app);
                        tracing::info!("Recording started");
                    }
                    Err(e) => {
                        tracing::error!("Failed to start recording: {}", e);
                        drop(s);
                        let mut s = state.lock().unwrap();
                        s.status = AppStatus::Error(format!("Recording failed: {}", e));
                    }
                }
            } else {
                tracing::warn!("Cannot record in current state: {:?}", s.status);
            }
        }

        AppMessage::Quit => {
            {
                let mut handle = recording_handle.lock().unwrap();
                if let Some(h) = handle.take() {
                    h.stop();
                }
            }
            is_recording_flag.store(false, Ordering::SeqCst);
            tracing::info!("Quitting...");
            app.quit();
        }

        AppMessage::RecordingStopped => {
            let mut s = state.lock().unwrap();
            s.is_recording = false;
            s.is_transcribing = false;
            s.status = AppStatus::Ready;
            is_recording_flag.store(false, Ordering::SeqCst);
            RecordingIndicator::hide(app);
            tracing::info!("Recording stopped, ready");
        }

        AppMessage::TranscriptionReady(text) => {
            let mut s = state.lock().unwrap();
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
