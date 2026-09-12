/// The main Canario backend.
///
/// Owns all state, communicates with frontends via events.
/// Thread-safe and `Clone` — pass it around freely.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::audio::mute::MuteGuard;
use crate::config::{AppConfig, AudioBehavior, ModelPaths, ModelVariant};
use crate::event::Event;
use crate::history::History;
use crate::hotkey::{HotkeyAction, HotkeyConfig, HotkeyListener, HotkeyStatus};
use crate::recording::RecordingHandle;
use crate::timing;
use serde::Serialize;

/// Lock a mutex, recovering from poisoning instead of panicking.
/// A poisoned mutex just means a thread panicked while holding it;
/// the state inside is still usable.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Shared cancel logic for `Canario::cancel_recording` and the hotkey
/// callback (which holds a `Weak<Inner>` to avoid a reference cycle
/// through the listener's stored closure).
fn cancel_recording_inner(inner: &Inner) {
    if let Some(h) = lock(&inner.recording_handle).as_ref() {
        tracing::info!("Recording cancel requested — audio will be discarded");
        h.cancel();
    }
    restore_audio_mute(inner);
    inner.is_recording.store(false, Ordering::SeqCst);
}

/// Restore system audio if it was muted for the recording
/// (`AudioBehavior::Mute`). Safe no-op when nothing was muted.
fn restore_audio_mute(inner: &Inner) {
    if let Some(guard) = lock(&inner.mute_guard).take() {
        guard.restore();
    }
}

/// Snapshot of the recording/download lifecycle (the `status` command).
///
/// Lets a (re)mounting frontend reconcile its state machine with core
/// truth instead of guessing from events it may have missed — e.g. a
/// settings-window reload while core is mid-recording (canario-dmp.5).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LifecycleStatus {
    /// Mic is currently capturing.
    pub recording: bool,
    /// A recording thread is still transcribing a finished capture.
    pub transcribing: bool,
    /// A model download is in flight.
    pub downloading: bool,
}

/// Shared tokio runtime for background downloads (built once, reused).
fn download_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Runtime::new().expect("failed to create tokio runtime"))
}

/// The parts of the config that identify a cached recognizer: the
/// resolved model paths plus the inference thread count (both halves of
/// the recognizer cache key). Used by [`Canario::update_config`] to
/// detect changes that should invalidate and re-warm the cache.
/// `None` paths = no usable model (e.g. Custom variant with
/// unconfigured paths).
fn recognizer_cache_key(config: &AppConfig) -> (Option<ModelPaths>, u32) {
    (config.model_paths().ok(), config.num_threads)
}

/// Push `config.input_device` into mic_warm's process-wide preferred
/// device (canario-1hq.2), so recordings use it without a restart.
/// Called wherever the in-memory config snapshot is (re)written:
/// construction, updates, reloads — the recording pipeline reads the
/// preference through `mic_warm::begin_recording`, which is why
/// recording.rs needs no changes.
fn sync_mic_preference(config: &AppConfig) {
    crate::mic_warm::set_preferred_device(Some(config.input_device.clone()));
}

struct Inner {
    config: Mutex<AppConfig>,
    history: Mutex<History>,
    recording_handle: Mutex<Option<RecordingHandle>>,
    is_recording: AtomicBool,
    /// Set while system audio is muted for a recording
    /// (`AudioBehavior::Mute`); restores the prior state on stop/cancel.
    mute_guard: Mutex<Option<MuteGuard>>,
    download_cancel: Arc<AtomicBool>,
    download_running: AtomicBool,
    event_tx: Sender<Event>,
    hotkey: Mutex<Option<HotkeyListener>>,
}

/// The main backend. Create one per application.
///
/// ```no_run
/// use canario_core::Canario;
///
/// let (canario, events) = Canario::new().unwrap();
/// // canario is Clone — share it across threads
/// let canario2 = canario.clone();
/// ```
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

        // canario-1hq.2: the configured input device becomes this
        // process's warm-mic preference. Gated to non-test builds —
        // unit tests construct `Canario` against the developer's real
        // config, and the store is process-wide (one test's
        // construction would clobber another's assertion; the store's
        // semantics are covered by mic_warm's own tests under a shared
        // lock).
        #[cfg(not(test))]
        sync_mic_preference(&config);

        // Plugin chain (canario-11h.2): install the process-wide
        // manager from the loaded config — same not-test gating for
        // the same reason (hermetic plugin tests install their own
        // managers under the plugins STORE_LOCK).
        #[cfg(not(test))]
        crate::plugins::init(
            crate::config::AppConfig::config_dir().join("plugins"),
            config.plugins.clone(),
        );

        // Pre-warm the recognizer cache in the background (when the
        // selected model is already on disk) so even the first
        // dictation is fast. Cheap existence check here; the heavy
        // model load happens on the prewarm thread.
        Self::prewarm_recognizer_if_ready(&config);

        Ok((
            Self {
                inner: Arc::new(Inner {
                    config: Mutex::new(config),
                    history: Mutex::new(history),
                    recording_handle: Mutex::new(None),
                    is_recording: AtomicBool::new(false),
                    mute_guard: Mutex::new(None),
                    download_cancel: Arc::new(AtomicBool::new(false)),
                    download_running: AtomicBool::new(false),
                    event_tx: tx,
                    hotkey: Mutex::new(None),
                }),
            },
            rx,
        ))
    }

    // ── Recognizer cache prewarming ──────────────────────────────────

    /// Spawn a background prewarm of the recognizer cache when the
    /// selected model's files are already on disk. Called at startup,
    /// after a successful model download, and when a config update
    /// changes the recognizer's identity.
    ///
    /// Disabled under `cargo test`: unit tests construct `Canario` on
    /// machines that may have a real multi-hundred-MB model installed,
    /// and the suite must stay fast and hermetic. The prewarm logic
    /// itself is unit-tested in `recording`.
    #[cfg(not(test))]
    fn prewarm_recognizer_if_ready(config: &AppConfig) {
        // Benchmark knob (scripts/bench-pipeline --cold): a cold run must
        // pay the model load inside its first dictation, not hide it in
        // a startup prewarm.
        if crate::timing::bench_disable_prewarm() {
            tracing::debug!("Prewarm skipped: CANARIO_BENCH_DISABLE_PREWARM is set");
            return;
        }
        match config.model_paths() {
            Ok(paths) if paths.all_exist() => {
                crate::recording::prewarm_recognizer_cache(paths);
            }
            _ => tracing::debug!("Prewarm skipped: selected model not on disk yet"),
        }
    }

    /// `cargo test` twin of `prewarm_recognizer_if_ready`: a no-op, so
    /// tests never spawn real model loads.
    #[cfg(test)]
    fn prewarm_recognizer_if_ready(_config: &AppConfig) {}

    // ── Recording ────────────────────────────────────────────────────

    /// Start recording from the default microphone.
    ///
    /// Audio is captured in the background. Call `stop_recording()` to
    /// stop and transcribe. Events: `RecordingStarted`, `AudioLevel`,
    /// `TranscriptionStarted` (when the finished capture begins
    /// transcribing), `TranscriptionReady`, `RecordingStopped`, `Error`.
    pub fn start_recording(&self) -> anyhow::Result<()> {
        timing::mark("start_recording_called");
        if self.is_recording() {
            return Err(anyhow::anyhow!("Already recording"));
        }

        // Don't start a new recording if the old thread is still transcribing.
        // Must acquire the lock only ONCE to avoid deadlock (Mutex is not reentrant).
        {
            let mut guard = lock(&self.inner.recording_handle);
            if let Some(handle) = guard.as_ref() {
                if handle.is_busy() {
                    return Err(anyhow::anyhow!("Transcription in progress, please wait"));
                }
            }
            // Clean up stale handle (whether or not one existed)
            *guard = None;
        }

        let config = self.config().clone();
        let model_paths = config.model_paths()?;
        let post_processor = config.post_processor.clone();
        let transform = config.transform.clone();
        let sound_effects = config.sound_effects;
        let sound_volume = config.sound_effects_volume;

        // Mute system audio for the recording if configured. The guard
        // restores the prior state on stop/cancel (or on failure below).
        let mute_guard = match config.recording_audio_behavior {
            AudioBehavior::Mute => crate::audio::mute::mute_for_recording(),
            AudioBehavior::DoNothing => None,
        };

        let tx = self.inner.event_tx.clone();

        let handle = match crate::recording::start_recording(
            model_paths,
            tx,
            post_processor,
            transform,
            sound_effects,
            sound_volume,
        ) {
            Ok(handle) => handle,
            Err(e) => {
                // Recording never started — undo the mute immediately.
                if let Some(guard) = mute_guard {
                    guard.restore();
                }
                return Err(e);
            }
        };

        *lock(&self.inner.mute_guard) = mute_guard;
        *lock(&self.inner.recording_handle) = Some(handle);
        self.inner.is_recording.store(true, Ordering::SeqCst);
        tracing::info!("Recording started");
        timing::mark("recording_started_event");
        let _ = self.inner.event_tx.send(Event::RecordingStarted);
        Ok(())
    }

    /// Stop recording and begin transcription.
    ///
    /// The recording thread will emit `Event::RecordingStopped` when
    /// transcription is complete (or immediately for short recordings).
    pub fn stop_recording(&self) {
        timing::mark("stop_recording_called");
        if let Some(h) = lock(&self.inner.recording_handle).as_ref() {
            tracing::info!("Recording stop requested");
            h.stop();
        }
        restore_audio_mute(&self.inner);
        // Don't send RecordingStopped here — the recording thread sends it
        // after transcription is done. Only update the in-memory flag.
        self.inner.is_recording.store(false, Ordering::SeqCst);
    }

    /// Cancel the current recording: stop capturing and DISCARD the audio.
    ///
    /// Unlike `stop_recording()`, no transcription is run and nothing is
    /// pasted or stored. The recording thread emits `RecordingCancelled`.
    /// Safe to call when idle (no-op).
    pub fn cancel_recording(&self) {
        cancel_recording_inner(&self.inner);
    }

    /// Toggle recording: start if idle, stop if recording.
    ///
    /// Returns `true` if now recording, `false` if now stopped.
    pub fn toggle_recording(&self) -> bool {
        if self.is_recording() {
            self.stop_recording();
            false
        } else if self.is_model_downloaded() {
            self.start_recording().is_ok()
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

    /// Snapshot of the recording/download lifecycle for the `status`
    /// command: the authoritative truth a (re)mounting frontend
    /// reconciles its state machine against (canario-dmp.5).
    pub fn lifecycle_status(&self) -> LifecycleStatus {
        let transcribing = lock(&self.inner.recording_handle)
            .as_ref()
            .map(|handle| handle.is_busy())
            .unwrap_or(false);
        LifecycleStatus {
            recording: self.is_recording(),
            transcribing,
            downloading: self.is_downloading(),
        }
    }

    // ── Config ───────────────────────────────────────────────────────

    /// Get a snapshot of the current config.
    pub fn config(&self) -> AppConfig {
        lock(&self.inner.config).clone()
    }

    /// Re-read `config.json` from disk into the in-memory snapshot and
    /// return the fresh value.
    ///
    /// The file can change under a running instance — the other
    /// frontend, a manual edit, the CLI — so [`Self::config`] alone
    /// would serve a boot-time snapshot forever. The sidecar's command
    /// loop is serial, so this cannot interleave with
    /// [`Self::update_config`]; a changed recognizer identity
    /// invalidates the recognizer cache exactly like
    /// [`Self::update_config`] does, regardless of who wrote the file.
    ///
    /// Emits [`Event::ConfigChanged`] only when the reloaded config
    /// genuinely differs from the in-memory snapshot (canario-dmp.20)
    /// — `get_config` polls this method, so an unchanged file must
    /// not spam the event.
    pub fn refresh_config(&self) -> anyhow::Result<AppConfig> {
        let loaded = crate::config::AppConfig::load()?;
        let mut config = lock(&self.inner.config);
        let cache_key_before = recognizer_cache_key(&config);
        let cache_key_after = recognizer_cache_key(&loaded);
        // Serialize both sides for comparison — `AppConfig` carries no
        // `PartialEq`, and the JSON view is exactly the persistence
        // format, so "differs on the wire" means "differs on disk".
        // A serialization failure on both sides compares equal (no
        // event) — failing to compare must not fabricate a change.
        let changed = serde_json::to_value(&*config).ok() != serde_json::to_value(&loaded).ok();
        *config = loaded.clone();
        // External edits (another frontend, the CLI, a manual edit) may
        // have changed the input device — keep the warm-mic preference
        // in sync (canario-1hq.2).
        sync_mic_preference(&loaded);
        drop(config);

        if cache_key_after != cache_key_before {
            crate::recording::recognizer_config_changed();
            Self::prewarm_recognizer_if_ready(&self.config());
        }
        // External change detected on reload (another frontend, the
        // CLI, a manual edit): same lock ordering as update_config —
        // the event leaves only after the config lock is dropped.
        if changed {
            let _ = self.inner.event_tx.send(Event::ConfigChanged);
        }
        Ok(loaded)
    }

    /// Update config atomically. Saves to disk.
    ///
    /// Emits [`Event::ConfigChanged`] after a successful save
    /// (canario-dmp.20) — payload-free; consumers pull
    /// [`Self::refresh_config`] / `get_config` for the new state.
    ///
    /// ```no_run
    /// # use canario_core::Canario;
    /// # let (canario, _events) = Canario::new().unwrap();
    /// canario.update_config(|c| c.auto_paste = false).unwrap();
    /// ```
    pub fn update_config(&self, f: impl FnOnce(&mut AppConfig)) -> anyhow::Result<()> {
        let mut config = lock(&self.inner.config);
        let cache_key_before = recognizer_cache_key(&config);
        // Serialized plugins block before/after: the process-wide
        // plugin manager is re-initialized only when its settings
        // actually changed (a re-init kills resident plugin children —
        // it must not ride along with unrelated config writes).
        #[cfg(not(test))]
        let plugins_before = serde_json::to_value(&config.plugins).ok();
        f(&mut config);
        #[cfg(not(test))]
        let plugins_after = serde_json::to_value(&config.plugins).ok();
        config.save()?;
        // canario-1hq.2: a device switch takes effect on the next
        // recording — push it into the warm-mic preference now (a
        // parked stream on another device is re-evaluated at press
        // time, released, and the wanted device opened fresh).
        sync_mic_preference(&config);
        let cache_key_after = recognizer_cache_key(&config);
        drop(config);

        // The recognizer identity changed: invalidate any in-flight
        // background prewarm for the old identity, then warm the new
        // one if its files are on disk. Order matters — the epoch bump
        // must happen before the new prewarm captures its epoch.
        if cache_key_after != cache_key_before {
            crate::recording::recognizer_config_changed();
            Self::prewarm_recognizer_if_ready(&self.config());
        }

        // canario-dmp.20: broadcast the write. Unconditional on a
        // successful save — the contract is "any config.json write by
        // this instance" — and sent only after the config lock is
        // dropped, so a receiver handling the event may re-enter
        // (e.g. call get_config) without deadlocking.
        let _ = self.inner.event_tx.send(Event::ConfigChanged);

        // Plugin settings changed: swap in a fresh manager (kills the
        // old children; enabling a plugin applies on the next
        // dictation without a restart). Same not-test posture as the
        // install in `new`.
        #[cfg(not(test))]
        if plugins_before != plugins_after {
            crate::plugins::init(
                crate::config::AppConfig::config_dir().join("plugins"),
                self.config().plugins.clone(),
            );
        }
        Ok(())
    }

    // ── Audio input devices (canario-1hq.2) ──────────────────────────

    /// Enumerate the available audio input devices (deduplicated by
    /// name) for the settings device picker. An enumeration failure
    /// degrades to an empty list — never an error — so the picker
    /// renders "System default" only instead of failing.
    pub fn list_input_devices(&self) -> Vec<crate::mic_warm::MicDevice> {
        crate::mic_warm::list_input_devices()
    }

    // ── Model management ─────────────────────────────────────────────

    /// Is the configured ASR model downloaded and ready?
    pub fn is_model_downloaded(&self) -> bool {
        lock(&self.inner.config).is_model_downloaded()
    }

    /// Start downloading the selected model in the background.
    ///
    /// Streams files to disk (resumable via `.part` files), retries
    /// transient network errors, and can be aborted with
    /// [`Canario::cancel_download`].
    ///
    /// Emits `ModelDownloadProgress`, `ModelDownloadComplete`, or
    /// `ModelDownloadFailed` (also used for user cancellation).
    pub fn download_model(&self) -> anyhow::Result<()> {
        let config = self.config();

        // Custom models are local-only: there is no repo to download from
        // (`model_hf_repo()` returns "" for Custom, which would produce a
        // malformed URL).
        if config.model == ModelVariant::Custom {
            return Err(anyhow::anyhow!(
                "Custom models are local-only — set custom model paths instead of downloading"
            ));
        }

        // Only one download at a time.
        if self.inner.download_running.swap(true, Ordering::SeqCst) {
            return Err(anyhow::anyhow!("Download already in progress"));
        }

        let model_dir = config.local_model_dir();
        let repo = config.model_hf_repo().to_string();
        let tx = self.inner.event_tx.clone();
        let cancel = self.inner.download_cancel.clone();
        cancel.store(false, Ordering::SeqCst);

        let inner = self.inner.clone();
        tracing::info!("Model download started: repo={}, dir={:?}", repo, model_dir);
        std::thread::spawn(move || {
            let result = download_runtime().block_on(async {
                crate::inference::download_model_with_progress(&model_dir, &repo, &tx, cancel).await
            });
            // Clear the running flag BEFORE the events: frontends re-derive
            // their button state from is_downloading() when handling
            // ModelDownloadComplete/Failed, and must not observe the stale
            // "still downloading" state (e.g. a Download button left
            // insensitive until the next unrelated refresh).
            inner.download_running.store(false, Ordering::SeqCst);

            match result {
                Ok(()) => {
                    tracing::info!("Model download finished successfully");
                    let _ = tx.send(Event::ModelDownloadComplete);
                    // The model just landed on disk — warm it now so the
                    // first dictation is as fast as every other one.
                    // Re-read the config in case the selection changed
                    // while the download ran.
                    let config = lock(&inner.config).clone();
                    Self::prewarm_recognizer_if_ready(&config);
                }
                Err(e) => {
                    tracing::error!("Model download failed: {}", e);
                    let _ = tx.send(Event::ModelDownloadFailed {
                        error: e.to_string(),
                    });
                }
            }
        });

        Ok(())
    }

    /// Cancel an in-flight model download.
    ///
    /// The download thread aborts at the next chunk boundary and emits
    /// `ModelDownloadFailed`. Partially downloaded files are kept as
    /// `.part` files, so a later [`Canario::download_model`] resumes
    /// where it left off. Does nothing if no download is running.
    pub fn cancel_download(&self) {
        if self.inner.download_running.load(Ordering::SeqCst) {
            tracing::info!("Model download cancellation requested");
            self.inner.download_cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Is a model download currently running?
    pub fn is_downloading(&self) -> bool {
        self.inner.download_running.load(Ordering::SeqCst)
    }

    /// Delete the downloaded model files.
    pub fn delete_model(&self) -> anyhow::Result<()> {
        let config = self.config();
        // Custom model paths point at the user's own files — never delete
        // those, only built-in downloads are ours to remove.
        if config.model == ModelVariant::Custom {
            return Err(anyhow::anyhow!(
                "Custom models are local-only — delete the files yourself if needed"
            ));
        }
        let model_dir = config.local_model_dir();
        if model_dir.exists() {
            std::fs::remove_dir_all(&model_dir)?;
            tracing::info!("Model files deleted: {:?}", model_dir);
        }
        Ok(())
    }

    // ── History ──────────────────────────────────────────────────────

    /// Add a transcription to history (fgm.3 D3: `text` is the
    /// canonical — transformed — transcript; `raw_text` rides along
    /// only when a transformation changed it, and the store drops it
    /// when it equals `text`).
    pub fn add_history(
        &self,
        text: String,
        duration_secs: f64,
        source_app: Option<String>,
        raw_text: Option<String>,
    ) {
        lock(&self.inner.history).add(text, duration_secs, source_app, raw_text);
    }

    /// Get recent history entries (most recent first).
    pub fn recent_history(&self, limit: usize) -> Vec<crate::history::HistoryEntry> {
        lock(&self.inner.history).recent_owned(limit)
    }

    /// Search history by text content.
    pub fn search_history(&self, query: &str) -> Vec<crate::history::HistoryEntry> {
        lock(&self.inner.history).search_owned(query)
    }

    /// Delete a history entry by ID.
    pub fn delete_history(&self, id: &str) {
        lock(&self.inner.history).delete(id);
    }

    /// Clear all history.
    pub fn clear_history(&self) {
        lock(&self.inner.history).clear();
    }

    /// History entry count.
    pub fn history_count(&self) -> usize {
        lock(&self.inner.history).entries.len()
    }

    // ── Hotkey ───────────────────────────────────────────────────────

    /// Start listening for the global hotkey.
    ///
    /// Emits `HotkeyTriggered` when the hotkey starts/stops recording
    /// (frontends should call `toggle_recording()`). Escape during
    /// recording is handled in-core: the recording is discarded and
    /// `RecordingCancelled` is emitted instead.
    pub fn start_hotkey(&self) -> anyhow::Result<()> {
        let config = self.config();
        let hk_config = HotkeyConfig::from_app_config(
            &config.hotkey,
            config.minimum_key_time,
            config.double_tap_lock,
            config.double_tap_only,
            config.double_tap_timeout_ms,
            config.modifier_threshold_ms,
        );

        let tx = self.inner.event_tx.clone();
        // Weak ref so the listener's stored closure doesn't keep `Inner`
        // alive forever (Inner → listener → closure → Inner cycle).
        let weak_inner = Arc::downgrade(&self.inner);
        let mut listener = HotkeyListener::new();
        listener.start(hk_config, move |action| {
            match action {
                HotkeyAction::StartRecording | HotkeyAction::StopRecording => {
                    // Marks the moment the hotkey backend dispatched the
                    // action (its poll loop already noticed the key) —
                    // the latest core-side witness of the physical press.
                    timing::mark(match action {
                        HotkeyAction::StartRecording => "hotkey_start_action",
                        _ => "hotkey_stop_action",
                    });
                    let _ = tx.send(Event::HotkeyTriggered);
                }
                HotkeyAction::CancelRecording => {
                    timing::mark("hotkey_cancel_action");
                    // Handle cancellation in-core: stop capturing and
                    // discard the buffer without transcribing. The
                    // recording thread emits `RecordingCancelled`.
                    if let Some(inner) = weak_inner.upgrade() {
                        cancel_recording_inner(&inner);
                    }
                }
            }
        })?;

        *lock(&self.inner.hotkey) = Some(listener);
        Ok(())
    }

    /// Stop the global hotkey listener.
    pub fn stop_hotkey(&self) {
        if let Some(listener) = lock(&self.inner.hotkey).take() {
            listener.stop();
        }
    }

    /// Restart hotkey listener with current config.
    pub fn restart_hotkey(&self) -> anyhow::Result<()> {
        self.stop_hotkey();
        self.start_hotkey()
    }

    /// Current health of the global hotkey backend.
    ///
    /// [`Self::start_hotkey`] runs the evdev `/dev/input` access probe
    /// synchronously, so a query issued right after `start_hotkey()` /
    /// `restart_hotkey()` returns cannot race the listener threads.
    /// Frontends use this to surface the "user not in the input group"
    /// failure (with its fix command) instead of leaving it in the logs.
    pub fn hotkey_status(&self) -> HotkeyStatus {
        match lock(&self.inner.hotkey).as_ref() {
            Some(listener) => listener.status(),
            None => HotkeyStatus::not_started(),
        }
    }

    // ── Lifecycle ────────────────────────────────────────────────────

    /// Shut down cleanly (stops recording + hotkey).
    pub fn shutdown(&self) {
        self.stop_recording();
        self.stop_hotkey();
    }

    /// Install .desktop file and icon for the current user.
    ///
    /// Also absorbs any legacy Electron-written login entry on the way
    /// (canario-dmp.17) so frontends calling this at startup migrate
    /// for free. A failed migration only logs — the menu entry must
    /// still be installed.
    pub fn install_desktop_files(&self) -> anyhow::Result<()> {
        if let Err(e) = crate::config::autostart::migrate_legacy_autostart() {
            tracing::warn!("Legacy autostart migration failed: {}", e);
        }
        crate::config::autostart::install_desktop_file()?;
        Ok(())
    }

    /// Toggle login autostart through the one shared implementation and
    /// keep config.autostart in agreement. Filesystem changes happen
    /// first; the flag is persisted only if they succeeded, so a failed
    /// toggle leaves state untouched. exec=None symlinks the installed
    /// menu entry (Exec=canario); Some(exec) writes a standalone entry.
    pub fn set_autostart(&self, enabled: bool, exec: Option<&str>) -> anyhow::Result<()> {
        if enabled {
            crate::config::autostart::enable_autostart(exec)?;
        } else {
            crate::config::autostart::disable_autostart()?;
        }
        self.update_config(|cfg| cfg.autostart = enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cancel event must serialize with the same serde tag convention
    /// as the other events — the Electron sidecar forwards this verbatim
    /// to the renderer.
    #[test]
    fn recording_cancelled_serializes_with_event_tag() {
        let json = serde_json::to_string(&Event::RecordingCancelled).unwrap();
        assert_eq!(json, r#"{"event":"RecordingCancelled"}"#);
    }

    /// Before start_hotkey the status must read "not-started" so the
    /// Electron renderer can distinguish "not yet attempted" from a
    /// degraded backend when it queries on mount.
    #[test]
    fn hotkey_status_defaults_to_not_started() {
        let (canario, _rx) = Canario::new().unwrap();
        let status = canario.hotkey_status();
        assert_eq!(status.backend, "not-started");
        assert!(!status.permission_denied);
        assert_eq!(status.fix_command, None);
    }

    /// Cancelling while idle must be a safe no-op: no panic, no events,
    /// recording flag stays false.
    #[test]
    fn cancel_recording_when_idle_is_noop() {
        let (canario, rx) = Canario::new().unwrap();
        canario.cancel_recording();
        assert!(!canario.is_recording());
        assert!(
            rx.try_recv().is_err(),
            "no events should be emitted when cancelling an idle backend"
        );
    }

    /// The hotkey mapping's cancel path clears the recording flag even
    /// when no recording thread exists yet (e.g. Escape raced with stop).
    #[test]
    fn cancel_recording_inner_clears_flag() {
        let (canario, _rx) = Canario::new().unwrap();
        canario.inner.is_recording.store(true, Ordering::SeqCst);
        cancel_recording_inner(&canario.inner);
        assert!(!canario.is_recording());
    }

    /// Custom models are local-only: download must be rejected with a
    /// clear error before any download state is touched (regression:
    /// `model_hf_repo()` returns "" for Custom → malformed URL).
    #[test]
    fn download_model_rejects_custom_variant() {
        let (canario, _rx) = Canario::new().unwrap();
        // In-memory only — don't write the user's config file.
        lock(&canario.inner.config).model = ModelVariant::Custom;

        let err = canario.download_model().unwrap_err();
        assert!(
            err.to_string().contains("local-only"),
            "unexpected error: {}",
            err
        );
        assert!(!canario.is_downloading());
    }

    /// Deleting must never touch user-owned custom model files.
    #[test]
    fn delete_model_rejects_custom_variant() {
        let (canario, _rx) = Canario::new().unwrap();
        lock(&canario.inner.config).model = ModelVariant::Custom;

        let err = canario.delete_model().unwrap_err();
        assert!(
            err.to_string().contains("local-only"),
            "unexpected error: {}",
            err
        );
    }

    /// A Custom variant with unconfigured paths fails fast with a clear
    /// error instead of starting a recording that can't transcribe.
    #[test]
    fn start_recording_rejects_custom_variant_without_paths() {
        let (canario, _rx) = Canario::new().unwrap();
        lock(&canario.inner.config).model = ModelVariant::Custom;

        let err = canario.start_recording().unwrap_err();
        assert!(
            err.to_string().contains("custom_encoder_path"),
            "unexpected error: {}",
            err
        );
        assert!(!canario.is_recording());
    }

    /// Only changes to the recognizer's identity (resolved model paths
    /// or thread count) may invalidate and re-warm the recognizer
    /// cache; unrelated settings changes must not.
    #[test]
    fn recognizer_cache_key_tracks_only_recognizer_identity() {
        let base = AppConfig::default();

        // Unrelated settings → identical key.
        let mut unrelated = base.clone();
        unrelated.auto_paste = !base.auto_paste;
        unrelated.sound_effects = !base.sound_effects;
        assert_eq!(
            recognizer_cache_key(&base),
            recognizer_cache_key(&unrelated)
        );

        // Model switch → different key.
        let mut switched = base.clone();
        switched.model = ModelVariant::ParakeetV2;
        assert_ne!(recognizer_cache_key(&base), recognizer_cache_key(&switched));

        // Thread count → different key (it is part of the cache key).
        let mut threads = base.clone();
        threads.num_threads = base.num_threads + 1;
        assert_ne!(recognizer_cache_key(&base), recognizer_cache_key(&threads));

        // Unconfigured Custom is its own (unusable) identity, never
        // equal to a usable one; configured paths make it usable.
        let mut custom = base.clone();
        custom.model = ModelVariant::Custom;
        assert_ne!(recognizer_cache_key(&base), recognizer_cache_key(&custom));
        assert!(recognizer_cache_key(&custom).0.is_none());

        custom.custom_encoder_path = Some("/tmp/enc.onnx".into());
        custom.custom_decoder_path = Some("/tmp/dec.onnx".into());
        custom.custom_tokens_path = Some("/tmp/tokens.txt".into());
        assert!(recognizer_cache_key(&custom).0.is_some());
    }

    /// canario-1hq.2: config → warm-mic preference sync (the helper
    /// update_config / refresh_config / non-test construction call).
    /// Tested in memory — update_config itself persists to the real
    /// config.json, which unit tests must not write. Serialized
    /// against mic_warm's own store tests by the shared lock: the
    /// store is process-wide and tests run in parallel.
    #[test]
    fn mic_preference_follows_the_configured_input_device() {
        let _guard = crate::mic_warm::PREFERRED_TEST_LOCK.lock();
        // Default config (and old configs without the key) → the
        // system default.
        sync_mic_preference(&AppConfig::default());
        assert_eq!(crate::mic_warm::preferred_device(), None);
        // Blank/whitespace counts as the system default too.
        sync_mic_preference(&AppConfig {
            input_device: "   ".into(),
            ..AppConfig::default()
        });
        assert_eq!(crate::mic_warm::preferred_device(), None);
        // A named device becomes the preference (trimmed).
        let named = |name: &str| AppConfig {
            input_device: name.into(),
            ..AppConfig::default()
        };
        sync_mic_preference(&named("  Yeti SB  "));
        assert_eq!(
            crate::mic_warm::preferred_device().as_deref(),
            Some("Yeti SB")
        );
        // Clearing back to "" returns to the system default.
        sync_mic_preference(&named(""));
        assert_eq!(crate::mic_warm::preferred_device(), None);
    }
}
