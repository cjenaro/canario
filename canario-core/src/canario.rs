/// The main Canario backend.
///
/// Owns all state, communicates with frontends via events.
/// Thread-safe and `Clone` — pass it around freely.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::config::AppConfig;
use crate::event::Event;
use crate::history::History;
use crate::hotkey::{HotkeyAction, HotkeyConfig, HotkeyListener};
use crate::recording::RecordingHandle;

struct Inner {
    config: Mutex<AppConfig>,
    history: Mutex<History>,
    recording_handle: Mutex<Option<RecordingHandle>>,
    is_recording: AtomicBool,
    event_tx: Sender<Event>,
    hotkey: Mutex<Option<HotkeyListener>>,
}

/// The main backend. Create one per application.
///
/// ```no_run
/// let (canario, events) = Canario::new()?;
/// // canario is Clone — share it across threads
/// let canario2 = canario.clone();
/// ```no_run
#[derive(Clone)]
pub struct Canario {
    inner: Arc<Inner>,
}

impl Canario {
    /// Create a new Canario backend.
    ///
    /// Returns `(Canario, Receiver<Event>)`. The `Canario` handle is used
    /// to call methods; the `Receiver` delivers events to your frontend.
    ///
    /// Config is loaded from `~/.config/canario/config.json` (or defaults).
    /// History is loaded from `~/.local/share/canario/history.json`.
    pub fn new() -> anyhow::Result<(Self, Receiver<Event>)> {
        let config = AppConfig::load()?;
        let history = History::load();
        let (tx, rx) = std::sync::mpsc::channel();

        Ok((
            Self {
                inner: Arc::new(Inner {
                    config: Mutex::new(config),
                    history: Mutex::new(history),
                    recording_handle: Mutex::new(None),
                    is_recording: AtomicBool::new(false),
                    event_tx: tx,
                    hotkey: Mutex::new(None),
                }),
            },
            rx,
        ))
    }

    // ── Recording ────────────────────────────────────────────────────

    /// Start recording from the default microphone.
    ///
    /// Audio is captured in the background. Call `stop_recording()` to
    /// stop and transcribe. Events: `RecordingStarted`, `AudioLevel`,
    /// `TranscriptionReady`, `RecordingStopped`, `Error`.
    pub fn start_recording(&self) -> anyhow::Result<()> {
        if self.is_recording() {
            return Err(anyhow::anyhow!("Already recording"));
        }

        // Don't start a new recording if the old thread is still transcribing.
        // Must acquire the lock only ONCE to avoid deadlock (Mutex is not reentrant).
        {
            let mut guard = self.inner.recording_handle.lock().unwrap();
            if let Some(handle) = guard.as_ref() {
                if handle.is_busy() {
                    return Err(anyhow::anyhow!("Transcription in progress, please wait"));
                }
            }
            // Clean up stale handle (whether or not one existed)
            *guard = None;
        }

        let config = self.config().clone();
        let model_dir = config.local_model_dir();
        let post_processor = config.post_processor.clone();
        let sound_effects = config.sound_effects;
        let tx = self.inner.event_tx.clone();

        let handle = crate::recording::start_recording(
            model_dir,
            tx,
            post_processor,
            sound_effects,
        )?;

        *self.inner.recording_handle.lock().unwrap() = Some(handle);
        self.inner.is_recording.store(true, Ordering::SeqCst);
        let _ = self.inner.event_tx.send(Event::RecordingStarted);
        Ok(())
    }

    /// Stop recording and begin transcription.
    ///
    /// The recording thread will emit `Event::RecordingStopped` when
    /// transcription is complete (or immediately for short recordings).
    pub fn stop_recording(&self) {
        if let Some(h) = self.inner.recording_handle.lock().unwrap().as_ref() {
            h.stop();
        }
        // Don't send RecordingStopped here — the recording thread sends it
        // after transcription is done. Only update the in-memory flag.
        self.inner.is_recording.store(false, Ordering::SeqCst);
    }

    /// Toggle recording: start if idle, stop if recording.
    ///
    /// Returns `true` if now recording, `false` if now stopped.
    pub fn toggle_recording(&self) -> bool {
        if self.is_recording() {
            self.stop_recording();
            false
        } else if self.is_model_downloaded() {
            if self.start_recording().is_ok() {
                true
            } else {
                false
            }
        } else {
            let _ = self.inner.event_tx.send(Event::Error {
                message: "Model not downloaded. Open settings to download.".into(),
            });
            false
        }
    }

    /// Is the mic currently recording?
    pub fn is_recording(&self) -> bool {
        self.inner.is_recording.load(Ordering::SeqCst)
    }

    // ── Config ───────────────────────────────────────────────────────

    /// Get a snapshot of the current config.
    pub fn config(&self) -> AppConfig {
        self.inner.config.lock().unwrap().clone()
    }

    /// Update config atomically. Saves to disk.
    ///
    /// ```no_run
    /// canario.update_config(|c| c.auto_paste = false).unwrap();
    /// ```no_run
    pub fn update_config(&self, f: impl FnOnce(&mut AppConfig)) -> anyhow::Result<()> {
        let mut config = self.inner.config.lock().unwrap();
        f(&mut config);
        config.save()?;
        Ok(())
    }

    // ── Model management ─────────────────────────────────────────────

    /// Is the configured ASR model downloaded and ready?
    pub fn is_model_downloaded(&self) -> bool {
        self.inner.config.lock().unwrap().is_model_downloaded()
    }

    /// Start downloading the selected model in the background.
    ///
    /// Emits `ModelDownloadProgress`, `ModelDownloadComplete`, or
    /// `ModelDownloadFailed`.
    pub fn download_model(&self) -> anyhow::Result<()> {
        let config = self.config();
        let model_dir = config.local_model_dir();
        let repo = config.model_hf_repo().to_string();
        let tx = self.inner.event_tx.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                crate::inference::download_model_with_progress(
                    &model_dir,
                    &repo,
                    &tx,
                ).await
            });
            match result {
                Ok(()) => { let _ = tx.send(Event::ModelDownloadComplete); }
                Err(e) => { let _ = tx.send(Event::ModelDownloadFailed { error: e.to_string() }); }
            }
        });

        Ok(())
    }

    /// Delete the downloaded model files.
    pub fn delete_model(&self) -> anyhow::Result<()> {
        let config = self.config();
        let model_dir = config.local_model_dir();
        if model_dir.exists() {
            std::fs::remove_dir_all(&model_dir)?;
        }
        Ok(())
    }

    // ── History ──────────────────────────────────────────────────────

    /// Add a transcription to history.
    pub fn add_history(&self, text: String, duration_secs: f64, source_app: Option<String>) {
        self.inner.history.lock().unwrap().add(text, duration_secs, source_app);
    }

    /// Get recent history entries (most recent first).
    pub fn recent_history(&self, limit: usize) -> Vec<crate::history::HistoryEntry> {
        self.inner.history.lock().unwrap().recent_owned(limit)
    }

    /// Search history by text content.
    pub fn search_history(&self, query: &str) -> Vec<crate::history::HistoryEntry> {
        self.inner.history.lock().unwrap().search_owned(query)
    }

    /// Delete a history entry by ID.
    pub fn delete_history(&self, id: &str) {
        self.inner.history.lock().unwrap().delete(id);
    }

    /// Clear all history.
    pub fn clear_history(&self) {
        self.inner.history.lock().unwrap().clear();
    }

    /// History entry count.
    pub fn history_count(&self) -> usize {
        self.inner.history.lock().unwrap().entries.len()
    }

    // ── Hotkey ───────────────────────────────────────────────────────

    /// Start listening for the global hotkey.
    ///
    /// Emits `HotkeyTriggered` when the hotkey is pressed.
    pub fn start_hotkey(&self) -> anyhow::Result<()> {
        let config = self.config();
        let hk_config = HotkeyConfig::from_app_config(
            &config.hotkey,
            config.minimum_key_time,
            config.double_tap_lock,
            config.double_tap_only,
        );

        let tx = self.inner.event_tx.clone();
        let mut listener = HotkeyListener::new();
        listener.start(hk_config, move |action| {
            match action {
                HotkeyAction::StartRecording
                | HotkeyAction::StopRecording
                | HotkeyAction::CancelRecording => {
                    let _ = tx.send(Event::HotkeyTriggered);
                }
            }
        })?;

        *self.inner.hotkey.lock().unwrap() = Some(listener);
        Ok(())
    }

    /// Stop the global hotkey listener.
    pub fn stop_hotkey(&self) {
        if let Some(listener) = self.inner.hotkey.lock().unwrap().take() {
            listener.stop();
        }
    }

    /// Restart hotkey listener with current config.
    pub fn restart_hotkey(&self) -> anyhow::Result<()> {
        self.stop_hotkey();
        self.start_hotkey()
    }

    // ── Lifecycle ────────────────────────────────────────────────────

    /// Shut down cleanly (stops recording + hotkey).
    pub fn shutdown(&self) {
        self.stop_recording();
        self.stop_hotkey();
    }

    /// Install .desktop file and icon for the current user.
    pub fn install_desktop_files(&self) -> anyhow::Result<()> {
        crate::config::autostart::install_desktop_file()?;
        Ok(())
    }
}
