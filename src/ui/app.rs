/// GTK4 Application — the main entry point for the GUI app.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use gio::ApplicationFlags;
use glib::ControlFlow;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::AppConfig;
use crate::hotkey::{HotkeyAction, HotkeyConfig, HotkeyListener};
use crate::ui::recording::{self, RecordingHandle};
use crate::ui::settings::SettingsWindow;
use crate::ui::indicator::RecordingIndicator;
use crate::ui::tray::CanarioTray;
use crate::ui::{AppMessage, AppState, AppStatus};
use crate::history::History;

/// Handle to the ksni tray service (blocking variant, safe to clone + share)
type TrayHandle = ksni::blocking::Handle<CanarioTray>;

pub struct CanarioApp {
    app: adw::Application,
    state: Arc<Mutex<AppState>>,
    tx: Sender<AppMessage>,
    rx: Receiver<AppMessage>,
    recording_handle: Arc<Mutex<Option<RecordingHandle>>>,
    is_recording_flag: Arc<AtomicBool>,
    tray_handle: Arc<Mutex<Option<TrayHandle>>>,
    #[allow(dead_code)] // Kept alive to maintain hotkey listener
    hotkey: Arc<Mutex<Option<HotkeyListener>>>,
    history: Arc<Mutex<History>>,
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
            tray_handle: Arc::new(Mutex::new(None)),
            hotkey: Arc::new(Mutex::new(None)),
            history: Arc::new(Mutex::new(History::load())),
        };

        canario.setup_signals();
        canario
    }

    fn setup_signals(&self) {
        let state = self.state.clone();
        let tx = self.tx.clone();
        let is_recording_flag = self.is_recording_flag.clone();
        let tray_handle = self.tray_handle.clone();
        let hotkey = self.hotkey.clone();

        self.app.connect_startup(move |app| {
            tracing::info!("Canario startup");

            // Install .desktop file on first launch
            if let Err(e) = crate::config::autostart::install_desktop_file() {
                tracing::warn!("Failed to install .desktop file: {}", e);
            }

            // Install icon
            let icon_svg = include_bytes!("../../assets/canario.svg");
            if let Err(e) = crate::config::autostart::install_icon(icon_svg) {
                tracing::warn!("Failed to install icon: {}", e);
            }

            {
                let mut s = state.lock().unwrap();
                if s.is_model_ready() {
                    s.status = AppStatus::Ready;
                    tracing::info!("Model already downloaded, ready to record");
                } else {
                    tracing::info!("No model found — open Settings to download one");
                }
            }

            // Start system tray — store the handle so we can refresh it
            let tray_tx = tx.clone();
            let flag = is_recording_flag.clone();
            let th = tray_handle.clone();
            std::thread::spawn(move || {
                match start_tray(tray_tx, flag) {
                    Ok(handle) => {
                        *th.lock().unwrap() = Some(handle);
                        tracing::info!("System tray icon started");
                    }
                    Err(e) => {
                        tracing::error!("Tray icon failed: {}", e);
                    }
                }
            });

            // Start hotkey listener
            let hk_tx = tx.clone();
            let s = state.lock().unwrap();
            let hotkey_config = HotkeyConfig::from_app_config(
                &s.config.hotkey,
                s.config.minimum_key_time,
                s.config.double_tap_lock,
                s.config.double_tap_only,
            );
            drop(s);

            let hk = hotkey.clone();
            std::thread::spawn(move || {
                let mut listener = HotkeyListener::new();
                match listener.start(hotkey_config, move |action| {
                    tracing::info!("Hotkey action: {:?}", action);
                    match action {
                        HotkeyAction::StartRecording => {
                            let _ = hk_tx.send(AppMessage::ToggleRecording);
                        }
                        HotkeyAction::StopRecording => {
                            let _ = hk_tx.send(AppMessage::ToggleRecording);
                        }
                        HotkeyAction::CancelRecording => {
                            let _ = hk_tx.send(AppMessage::ToggleRecording);
                        }
                    }
                }) {
                    Ok(()) => {
                        tracing::info!("Global hotkey listener started");
                        *hk.lock().unwrap() = Some(listener);
                    }
                    Err(e) => tracing::warn!("Hotkey listener failed to start: {}", e),
                }
            });

            std::mem::forget(app.hold());
        });

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

    pub fn run(self) -> anyhow::Result<()> {
        let rx = self.rx;
        let state = self.state;
        let app = self.app;
        let recording_handle = self.recording_handle;
        let tx = self.tx;
        let is_recording_flag = self.is_recording_flag;
        let tray_handle = self.tray_handle;
        let hotkey = self.hotkey;
        let history = self.history;

        let app_clone = app.clone();

        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            while let Ok(msg) = rx.try_recv() {
                handle_message(
                    &app_clone,
                    &state,
                    &recording_handle,
                    &tx,
                    &is_recording_flag,
                    &tray_handle,
                    &hotkey,
                    &history,
                    msg,
                );
            }
            ControlFlow::Continue
        });

        app.run_with_args::<String>(&[]);
        Ok(())
    }
}

/// Spawn tray and return the handle immediately (doesn't block).
fn start_tray(
    tx: Sender<AppMessage>,
    is_recording: Arc<AtomicBool>,
) -> anyhow::Result<TrayHandle> {
    use ksni::blocking::TrayMethods;

    let tray = CanarioTray::new(tx, is_recording);
    let handle = tray.spawn().map_err(|e| {
        anyhow::anyhow!("Failed to spawn tray: {}", e)
    })?;

    Ok(handle)
}

/// Force the tray to re-read the shared recording flag and update
/// its icon, tooltip, and menu labels.
fn refresh_tray(tray_handle: &Arc<Mutex<Option<TrayHandle>>>) {
    if let Some(handle) = tray_handle.lock().unwrap().as_ref() {
        // update() calls the Tray methods again so ksni re-exports
        // the icon_name, tool_tip, and menu over D-Bus.
        handle.update(|_| {});
    }
}

fn handle_message(
    app: &adw::Application,
    state: &Arc<Mutex<AppState>>,
    recording_handle: &Arc<Mutex<Option<RecordingHandle>>>,
    tx: &Sender<AppMessage>,
    is_recording_flag: &Arc<AtomicBool>,
    tray_handle: &Arc<Mutex<Option<TrayHandle>>>,
    hotkey_listener: &Arc<Mutex<Option<HotkeyListener>>>,
    history: &Arc<Mutex<History>>,
    msg: AppMessage,
) {
    match msg {
        AppMessage::ShowSettings => {
            SettingsWindow::present(app, state.clone(), tx.clone(), history.clone());
        }

        AppMessage::ToggleRecording => {
            let s = state.lock().unwrap();
            let mut handle = recording_handle.lock().unwrap();

            if s.is_recording {
                // ── Stop ──────────────────────────────────────
                if let Some(h) = handle.take() {
                    tracing::info!("Stopping recording...");
                    h.stop();
                }
                is_recording_flag.store(false, Ordering::SeqCst);
                drop(s);
                let mut s = state.lock().unwrap();
                s.is_recording = false;
                s.status = AppStatus::Transcribing;
                RecordingIndicator::hide(app);
                refresh_tray(tray_handle);
            } else if s.can_record() {
                // ── Start ─────────────────────────────────────
                let model_dir = s.config.local_model_dir();
                let post_processor = s.config.post_processor.clone();
                let sound_effects = s.config.sound_effects;

                match recording::start_recording(model_dir, tx.clone(), post_processor, sound_effects) {
                    Ok(h) => {
                        *handle = Some(h);
                        is_recording_flag.store(true, Ordering::SeqCst);
                        drop(s);
                        let mut s = state.lock().unwrap();
                        s.is_recording = true;
                        s.status = AppStatus::Recording;
                        RecordingIndicator::show(app);
                        tracing::info!("Recording started");
                        refresh_tray(tray_handle);
                    }
                    Err(e) => {
                        tracing::error!("Failed to start recording: {}", e);
                        drop(s);
                        let mut s = state.lock().unwrap();
                        s.status = AppStatus::Error(format!("{}", e));
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
            refresh_tray(tray_handle);
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
            refresh_tray(tray_handle);
            tracing::info!("Recording stopped, ready");
        }

        AppMessage::RecordingError(err) => {
            let mut s = state.lock().unwrap();
            s.status = AppStatus::Error(err.clone());
            tracing::error!("Recording error: {}", err);
            // Do NOT paste or store in history
        }

        AppMessage::TranscriptionReady(text) => {
            let mut s = state.lock().unwrap();
            let sound_effects = s.config.sound_effects;
            s.last_transcription = text.clone();
            s.status = AppStatus::Ready;
            tracing::info!("✅ Transcription: {}", text);

            if s.config.auto_paste {
                match crate::ui::paste::paste_text(&text) {
                    Ok(pasted) => {
                        if pasted {
                            tracing::info!("📋 Auto-typed");
                            if sound_effects {
                                crate::audio::effects::beep_confirm();
                            }
                        } else {
                            tracing::info!("📋 Copied to clipboard (Ctrl+V to paste)");
                        }
                    }
                    Err(e) => tracing::error!("Paste failed: {}", e),
                }
            }

            // Store in history
            history.lock().unwrap().add(
                text,
                0.0, // duration not tracked here yet
                None, // source_app not detected yet
            );
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

        AppMessage::HotkeyChanged(new_hotkey) => {
            // Update config
            {
                let mut s = state.lock().unwrap();
                s.config.hotkey = new_hotkey.clone();
                let _ = s.config.save();
            }

            // Stop old listener
            if let Some(listener) = hotkey_listener.lock().unwrap().take() {
                listener.stop();
                drop(listener);
                tracing::info!("Old hotkey listener stopped");
            }

            // Start new listener
            let s = state.lock().unwrap();
            let hotkey_config = HotkeyConfig::from_app_config(
                &s.config.hotkey,
                s.config.minimum_key_time,
                s.config.double_tap_lock,
                s.config.double_tap_only,
            );
            drop(s);

            let hk = hotkey_listener.clone();
            let hk_tx = tx.clone();
            std::thread::spawn(move || {
                let mut listener = HotkeyListener::new();
                match listener.start(hotkey_config, move |action| {
                    match action {
                        HotkeyAction::StartRecording
                        | HotkeyAction::StopRecording
                        | HotkeyAction::CancelRecording => {
                            let _ = hk_tx.send(AppMessage::ToggleRecording);
                        }
                    }
                }) {
                    Ok(()) => {
                        tracing::info!("Hotkey listener restarted with {:?}", new_hotkey);
                        *hk.lock().unwrap() = Some(listener);
                    }
                    Err(e) => tracing::warn!("Hotkey restart failed: {}", e),
                }
            });
        }
    }
}
