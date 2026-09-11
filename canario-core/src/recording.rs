/// Recording engine — captures audio, transcribes, emits events.
///
/// Simple approach: record raw audio while active, transcribe on stop.
/// Communicates results via the `Sender<Event>` channel.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig};

use crate::config::ModelPaths;
use crate::event::Event;
use crate::inference::postprocess::PostProcessor;

/// Handle to stop a running recording and track thread completion.
pub struct RecordingHandle {
    stop: Arc<AtomicBool>,
    /// Set when the recording should be discarded (no transcription).
    cancel: Arc<AtomicBool>,
    /// Set to `false` by the thread when it finishes (recording + transcription).
    busy: Arc<AtomicBool>,
}

impl RecordingHandle {
    /// Signal the recording thread to stop.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Signal the recording thread to stop AND discard the audio:
    /// the mic is released but the buffer is never transcribed, and
    /// `Event::RecordingCancelled` is emitted instead of
    /// `TranscriptionReady`/`RecordingStopped`.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Is the thread still running (capturing or transcribing)?
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
}

/// Start recording from the microphone.
///
/// Captures audio until `RecordingHandle::stop()` is called, then
/// transcribes the entire buffer, applies post-processing, and sends
/// events via `tx`.
pub fn start_recording(
    model_paths: ModelPaths,
    tx: std::sync::mpsc::Sender<Event>,
    post_processor: PostProcessor,
    sound_effects: bool,
) -> anyhow::Result<RecordingHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let cancel = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicBool::new(true));
    let stop_clone = stop.clone();
    let cancel_clone = cancel.clone();
    let busy_clone = busy.clone();

    // Play start beep
    if sound_effects {
        crate::audio::effects::beep_start();
    }

    // Inference thread count (0 = auto). Part of the recognizer cache
    // key, so a change reloads the model.
    let num_threads = configured_num_threads();

    std::thread::spawn(move || {
        tracing::info!("Recording thread starting...");
        let result = recording_loop(
            model_paths,
            tx.clone(),
            stop_clone,
            cancel_clone,
            &post_processor,
            sound_effects,
            num_threads,
        );
        if let Err(e) = &result {
            tracing::error!("Recording thread error: {}", e);
            let _ = tx.send(Event::Error {
                message: format!("{}", e),
            });
            let _ = tx.send(Event::RecordingStopped);
        }
        // Mark thread as no longer busy (whether success or failure)
        busy_clone.store(false, Ordering::SeqCst);
    });

    Ok(RecordingHandle { stop, cancel, busy })
}

/// The main recording loop — runs in a background thread.
fn recording_loop(
    model_paths: ModelPaths,
    tx: std::sync::mpsc::Sender<Event>,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    post_processor: &PostProcessor,
    sound_effects: bool,
    num_threads: i32,
) -> anyhow::Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    // ── Open mic ────────────────────────────────────────────────────
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No input device found"))?;
    let supported = device.default_input_config()?;
    let mic_sr = supported.sample_rate().0;
    let channels = supported.channels() as usize;

    tracing::info!(
        "Recording from '{}' at {}Hz",
        device.name().unwrap_or_default(),
        mic_sr
    );

    let audio_buf: Arc<parking_lot::Mutex<Vec<f32>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));

    let audio_buf_clone = audio_buf.clone();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &supported.into(),
            move |data: &[f32], _| {
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                    .collect();
                audio_buf_clone.lock().extend_from_slice(&mono);
            },
            |err| tracing::error!("Audio error: {}", err),
            None,
        )?,
        cpal::SampleFormat::I16 => {
            let buf = audio_buf.clone();
            device.build_input_stream(
                &supported.into(),
                move |data: &[i16], _| {
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|frame| {
                            frame
                                .iter()
                                .map(|&s| s as f32 / i16::MAX as f32)
                                .sum::<f32>()
                                / channels as f32
                        })
                        .collect();
                    buf.lock().extend_from_slice(&mono);
                },
                |err| tracing::error!("Audio error: {}", err),
                None,
            )?
        }
        _ => anyhow::bail!("Unsupported sample format"),
    };

    stream.play()?;

    // ── Live captions for long sessions ─────────────────────────────
    // Detached worker: decodes a sliding window of the buffer and emits
    // PartialTranscript events once the recording passes the config
    // threshold. Self-terminates when `stop` is set (stop or cancel);
    // decodes share the recognizer cache with the final decode, whose
    // lock serializes the two so they never overlap.
    spawn_live_captions_worker(
        audio_buf.clone(),
        mic_sr,
        stop.clone(),
        tx.clone(),
        model_paths.clone(),
        post_processor.clone(),
        num_threads,
    );

    // ── Wait for stop signal ────────────────────────────────────────
    let mut log_timer = std::time::Instant::now();
    while !stop.load(Ordering::SeqCst) {
        let buf_snapshot = audio_buf.lock();
        let buf_len = buf_snapshot.len();
        let recent_start = buf_len.saturating_sub(4000);
        let recent = &buf_snapshot[recent_start..];
        let rms = if recent.is_empty() {
            0.0
        } else {
            (recent.iter().map(|s| s * s).sum::<f32>() / recent.len() as f32).sqrt()
        };
        drop(buf_snapshot);

        let level = (rms * 5.0).min(1.0) as f64;
        let _ = tx.send(Event::AudioLevel { level });

        if log_timer.elapsed() >= std::time::Duration::from_secs(3) {
            tracing::info!(
                "Recording: {} samples ({:.1}s), RMS={:.3}",
                buf_len,
                buf_len as f64 / mic_sr as f64,
                rms,
            );
            log_timer = std::time::Instant::now();
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // ── Stop: grab audio, release mic ───────────────────────────────
    // Cancel path: release the mic and discard the buffer — skip the
    // stop beep, resample, and transcription entirely.
    if cancel.load(Ordering::SeqCst) {
        drop(stream);
        audio_buf.lock().clear();
        tracing::info!("Recording cancelled — discarding captured audio");
        let _ = tx.send(Event::RecordingCancelled);
        return Ok(());
    }

    let raw_audio = audio_buf.lock().clone();
    drop(stream);

    // Play stop beep (double-beep) before transcription begins
    if sound_effects {
        crate::audio::effects::beep_stop();
    }

    let duration = raw_audio.len() as f64 / mic_sr as f64;
    tracing::info!(
        "Stopped. Captured {} samples ({:.1}s)",
        raw_audio.len(),
        duration
    );

    if duration < 0.2 {
        tracing::warn!("Recording too short, nothing to transcribe");
        let _ = tx.send(Event::RecordingStopped);
        return Ok(());
    }

    // ── Resample to 16kHz if needed ─────────────────────────────────
    let audio_16k = if mic_sr != 16000 {
        tracing::info!("Resampling {}Hz → 16000Hz...", mic_sr);
        crate::inference::resample::resample(&raw_audio, mic_sr, 16000)?
    } else {
        raw_audio
    };

    // ── Transcribe (recognizer is cached across recordings) ─────────
    let audio_secs = audio_16k.len() as f64 / 16000.0;
    tracing::info!("Transcribing {:.1}s of audio...", audio_secs);

    with_recognizer(&model_paths, num_threads, |recognizer| {
        let rec_stream = recognizer.create_stream();
        rec_stream.accept_waveform(16000, &audio_16k);
        let decode_start = std::time::Instant::now();
        recognizer.decode(&rec_stream);
        let decode_secs = decode_start.elapsed().as_secs_f64();
        tracing::info!(
            "Decoded {:.1}s of audio in {:.2}s ({:.2}x realtime)",
            audio_secs,
            decode_secs,
            if decode_secs > 0.0 {
                audio_secs / decode_secs
            } else {
                0.0
            },
        );

        if let Some(result) = rec_stream.get_result() {
            let raw_text = result.text.trim().to_string();
            if !raw_text.is_empty() {
                let text = post_processor.process(&raw_text);
                tracing::info!(
                    "✅ Transcription ({} chars): {}",
                    text.chars().count(),
                    text
                );
                let _ = tx.send(Event::TranscriptionReady {
                    text,
                    duration_secs: duration,
                });
            } else {
                tracing::info!("(no speech detected)");
            }
        }
    })?;

    let _ = tx.send(Event::RecordingStopped);
    Ok(())
}

// ── Live captions ────────────────────────────────────────────────────
// Long recordings get a live text preview: a worker thread re-decodes a
// sliding window of the captured buffer and emits `PartialTranscript`
// events. The final full-buffer decode (and the pasted/stored result)
// is unchanged — partials are preview-only and never reach history.

/// How much audio each partial decode sees, at most. Caps decode cost
/// so long rambling sessions stay roughly realtime instead of falling
/// further behind as the buffer grows.
const LIVE_WINDOW_SECS: f64 = 20.0;

/// Pause between partial decodes, counted after a decode finishes.
const LIVE_INTERVAL_MS: u64 = 1200;

/// Sleep granularity while waiting out the cadence — keeps the worker
/// responsive to `stop` instead of blocking a full interval.
const LIVE_POLL_MS: u64 = 150;

/// Sample range of the buffer a partial decode should cover: the whole
/// buffer while it fits in `window_secs`, else its trailing window.
fn live_window_range(buf_len: usize, mic_sr: u32, window_secs: f64) -> std::ops::Range<usize> {
    let window_samples = (window_secs.max(0.0) * mic_sr as f64) as usize;
    if buf_len <= window_samples {
        0..buf_len
    } else {
        buf_len - window_samples..buf_len
    }
}

/// Everything the live-captions worker needs, bundled to keep the
/// loop signature small.
struct LiveCaptionsCtx {
    audio_buf: Arc<parking_lot::Mutex<Vec<f32>>>,
    mic_sr: u32,
    stop: Arc<AtomicBool>,
    tx: std::sync::mpsc::Sender<Event>,
    model_paths: ModelPaths,
    post_processor: PostProcessor,
    num_threads: i32,
    threshold_secs: f64,
}

/// Spawn the live-captions worker for an in-flight recording. No-op
/// (returns) when disabled by config. Reads config from disk, matching
/// `configured_num_threads` — the same on-disk source the final decode
/// uses, so a mid-session config change can't split behavior.
fn spawn_live_captions_worker(
    audio_buf: Arc<parking_lot::Mutex<Vec<f32>>>,
    mic_sr: u32,
    stop: Arc<AtomicBool>,
    tx: std::sync::mpsc::Sender<Event>,
    model_paths: ModelPaths,
    post_processor: PostProcessor,
    num_threads: i32,
) {
    let (enabled, threshold_secs) = crate::config::AppConfig::load()
        .map(|c| (c.live_captions, c.live_captions_threshold_secs))
        .unwrap_or((true, 8.0));
    if !enabled {
        tracing::debug!("Live captions disabled in config");
        return;
    }
    tracing::debug!(
        "Live captions armed: threshold {:.1}s, window {:.0}s, interval {}ms",
        threshold_secs,
        LIVE_WINDOW_SECS,
        LIVE_INTERVAL_MS
    );

    let ctx = LiveCaptionsCtx {
        audio_buf,
        mic_sr,
        stop,
        tx,
        model_paths,
        post_processor,
        num_threads,
        threshold_secs,
    };
    let spawned = std::thread::Builder::new()
        .name("live-captions".to_string())
        .spawn(move || live_captions_loop(ctx));
    if let Err(e) = spawned {
        // Preview only — recording works fine without it.
        tracing::warn!("Could not spawn live-captions worker: {}", e);
    }
}

/// The live-captions worker loop. Emits `PartialTranscript` events for
/// the trailing window of the buffer once it grows past the threshold.
fn live_captions_loop(ctx: LiveCaptionsCtx) {
    let LiveCaptionsCtx {
        audio_buf,
        mic_sr,
        stop,
        tx,
        model_paths,
        post_processor,
        num_threads,
        threshold_secs,
    } = ctx;

    let threshold_samples = (threshold_secs.max(0.0) * mic_sr as f64) as usize;
    let mut last_text = String::new();

    loop {
        // Wait out the cadence in small slices so `stop` (or cancel)
        // interrupts within one poll instead of a full interval.
        let mut waited_ms = 0;
        while waited_ms < LIVE_INTERVAL_MS && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(LIVE_POLL_MS));
            waited_ms += LIVE_POLL_MS;
        }
        if stop.load(Ordering::SeqCst) {
            return;
        }

        // Only long sessions get captions — short dictations never pay
        // a partial decode.
        let window = {
            let buf = audio_buf.lock();
            if buf.len() < threshold_samples {
                continue;
            }
            let range = live_window_range(buf.len(), mic_sr, LIVE_WINDOW_SECS);
            buf[range].to_vec()
        };
        if window.is_empty() {
            continue;
        }

        let audio_16k = if mic_sr != 16000 {
            match crate::inference::resample::resample(&window, mic_sr, 16000) {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!("Live captions stopping (resample failed): {}", e);
                    return;
                }
            }
        } else {
            window
        };

        // Shares the recognizer cache with the final decode; the cache
        // lock serializes the two, so a stop at most waits out this
        // decode instead of racing it.
        let decoded = with_recognizer(&model_paths, num_threads, |recognizer| {
            let rec_stream = recognizer.create_stream();
            rec_stream.accept_waveform(16000, &audio_16k);
            recognizer.decode(&rec_stream);
            rec_stream.get_result().map(|r| r.text.trim().to_string())
        });

        match decoded {
            Ok(Some(raw_text)) if !raw_text.is_empty() => {
                let text = post_processor.process(&raw_text);
                // Emit only fresh text, and never after stop: a late
                // partial must not re-show captions that the final
                // result (or RecordingStopped) already cleared.
                if !stop.load(Ordering::SeqCst) && !text.is_empty() && text != last_text {
                    tracing::debug!(
                        "Live caption ({} chars, {:.1}s window)",
                        text.chars().count(),
                        audio_16k.len() as f64 / 16000.0
                    );
                    last_text = text.clone();
                    let _ = tx.send(Event::PartialTranscript { text });
                }
            }
            Ok(_) => {}
            Err(e) => {
                // E.g. model files missing — the final decode surfaces
                // the real, actionable error. Previewing is best-effort.
                tracing::warn!("Live captions stopping: {}", e);
                return;
            }
        }
    }
}

/// Cached recognizer entry — keyed by model paths + thread count so a
/// model/config switch triggers a reload.
struct CachedRecognizer {
    model_paths: ModelPaths,
    num_threads: i32,
    recognizer: SendableRecognizer,
}

/// `OfflineRecognizer` wraps a raw C pointer, so upstream doesn't mark it
/// `Send`. Per-decode state lives in the stream, not the recognizer, so it
/// is safe to move across threads and reuse as long as decodes are
/// serialized — the cache lock in `with_recognizer` guarantees that (and
/// `Canario` never runs two transcriptions concurrently anyway).
struct SendableRecognizer(OfflineRecognizer);
unsafe impl Send for SendableRecognizer {}

/// Recognizer cache — loading the model from disk takes hundreds of ms,
/// so one recognizer is kept alive across transcriptions.
static RECOGNIZER_CACHE: parking_lot::Mutex<Option<CachedRecognizer>> =
    parking_lot::Mutex::new(None);

/// Bumped every time the recognizer's identity changes via config
/// (resolved model paths or thread count). A background prewarm
/// captures the epoch when it starts and abandons the warm-up if the
/// epoch moved by the time it holds the cache lock — that is what
/// keeps a stale background request from evicting a fresher cache
/// entry after a quick model switch.
static RECOGNIZER_CONFIG_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Run `f` with the cached recognizer for `model_paths`, loading (or
/// reloading, if the model paths or thread count changed) on cache miss.
/// The cache lock is held while `f` runs, serializing decodes.
fn with_recognizer<R>(
    model_paths: &ModelPaths,
    num_threads: i32,
    f: impl FnOnce(&OfflineRecognizer) -> R,
) -> anyhow::Result<R> {
    let mut cache = RECOGNIZER_CACHE.lock();

    let stale = cache
        .as_ref()
        .is_none_or(|c| c.model_paths != *model_paths || c.num_threads != num_threads);
    if stale {
        tracing::info!("Loading ASR model (encoder: {:?})...", model_paths.encoder);
        // Validation failures are corrupt-file errors (bad tokens, bad
        // ONNX): actionable for the user, so surface them instead of
        // the generic "not found" message.
        let stored = store_recognizer(&mut cache, model_paths, num_threads).map_err(|e| {
            anyhow::anyhow!(
                "ASR model files are corrupt or unreadable: {}. Re-download the model from Settings.",
                e
            )
        })?;
        stored.ok_or_else(|| {
            anyhow::anyhow!(
                "ASR model files not found. Download from Settings or configure custom model paths."
            )
        })?;
        tracing::info!("ASR model loaded");
    } else {
        tracing::debug!("Reusing cached ASR recognizer");
    }

    let cached = cache.as_ref().expect("cache populated above");
    Ok(f(&cached.recognizer.0))
}

/// Tell the recognizer cache that the recognizer identity just changed
/// (model paths or thread count). Invalidates in-flight background
/// prewarms for the previous identity.
pub fn recognizer_config_changed() {
    RECOGNIZER_CONFIG_EPOCH.fetch_add(1, Ordering::SeqCst);
}

/// Current recognizer-config epoch (see [`RECOGNIZER_CONFIG_EPOCH`]).
fn recognizer_config_epoch() -> u64 {
    RECOGNIZER_CONFIG_EPOCH.load(Ordering::SeqCst)
}

/// What a background prewarm of the recognizer cache did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrewarmOutcome {
    /// Model loaded and stored in the cache.
    Warmed,
    /// The cache already held this exact recognizer — nothing to do.
    AlreadyWarm,
    /// The model files are not on disk — nothing to warm.
    MissingModelFiles,
    /// A model file failed the safety validation (unparseable tokens,
    /// or an encoder/decoder/joiner that is not protobuf-shaped).
    /// sherpa's C++ `ReadTokens` **exits the process** on unparseable
    /// input, so the optional warmup refuses to touch such a model; a
    /// genuinely broken model still fails loudly at dictation time via
    /// `with_recognizer`.
    InvalidModelFiles,
    /// Config changed while the prewarm was in flight; abandoned
    /// without touching the cache.
    StaleConfig,
    /// `OfflineRecognizer::create` failed; any existing cache entry
    /// was left in place.
    LoadFailed,
}

/// Pre-warm the recognizer cache in a background thread so the first
/// dictation after startup (or after a model download) doesn't pay the
/// model-load cost (hundreds of ms).
///
/// Best-effort and silent by design: missing files, stale requests and
/// load failures are logged, never surfaced to the user. Real errors
/// reach the user through `with_recognizer` at dictation time, where
/// they are actionable.
///
/// Only referenced by `Canario`'s non-test prewarm hook; gated out of
/// test builds so the (unused) symbol doesn't trip dead-code analysis.
#[cfg(not(test))]
pub fn prewarm_recognizer_cache(model_paths: ModelPaths) {
    let spawned = std::thread::Builder::new()
        .name("recognizer-prewarm".to_string())
        .spawn(move || {
            // The thread count comes from the same on-disk config that
            // `start_recording` reads, so the prewarmed entry's cache
            // key matches what dictation will request.
            let num_threads = configured_num_threads();
            let outcome = prewarm_once(&model_paths, num_threads, recognizer_config_epoch());
            match outcome {
                PrewarmOutcome::Warmed => tracing::info!("Recognizer cache pre-warmed"),
                PrewarmOutcome::AlreadyWarm => tracing::debug!("Recognizer cache already warm"),
                PrewarmOutcome::MissingModelFiles
                | PrewarmOutcome::InvalidModelFiles
                | PrewarmOutcome::StaleConfig => {
                    tracing::debug!("Prewarm skipped: {:?}", outcome);
                }
                PrewarmOutcome::LoadFailed => {
                    tracing::warn!("Prewarm could not load the model; cache left untouched");
                }
            }
        });
    if let Err(e) = spawned {
        // Not worth a user-facing error — the first dictation will
        // simply load the model on demand.
        tracing::warn!("Could not spawn recognizer prewarm thread: {}", e);
    }
}

/// Synchronous prewarm core. The load runs while holding the cache
/// lock, so a concurrent dictation either waits for a load it would
/// have paid anyway, or finds the entry already warm — loads are
/// serialized and never duplicated.
fn prewarm_once(model_paths: &ModelPaths, num_threads: i32, epoch: u64) -> PrewarmOutcome {
    // Never attempt to load a model that isn't fully on disk...
    if !model_paths.all_exist() {
        return PrewarmOutcome::MissingModelFiles;
    }
    // ...and never hand sherpa files it cannot parse: its C++
    // ReadTokens exits the whole process on bad tokens input, and an
    // optional background warmup must not be able to take the app
    // down. `create_recognizer` repeats this check, so the prewarm is
    // protected even if this early-out is ever bypassed.
    if let Err(reason) = validate_model_files(model_paths) {
        tracing::debug!("Prewarm validation failed: {}", reason);
        return PrewarmOutcome::InvalidModelFiles;
    }

    let mut cache = RECOGNIZER_CACHE.lock();

    // Config changed since this prewarm was requested: a newer prewarm
    // or a dictation owns the cache now. Abandon without writing.
    if recognizer_config_epoch() != epoch {
        return PrewarmOutcome::StaleConfig;
    }

    // A previous prewarm or a dictation already warmed this exact
    // recognizer — don't pay the load again.
    if cache
        .as_ref()
        .is_some_and(|c| c.model_paths == *model_paths && c.num_threads == num_threads)
    {
        return PrewarmOutcome::AlreadyWarm;
    }

    // Validation ran above, so `Err` here is at most a race (a file
    // replaced between the check and the load) — same handling as a
    // plain load failure: leave the cache untouched.
    match store_recognizer(&mut cache, model_paths, num_threads) {
        Ok(Some(())) => PrewarmOutcome::Warmed,
        Ok(None) | Err(_) => PrewarmOutcome::LoadFailed,
    }
}

// ── Model file validation ────────────────────────────────────────────
// sherpa-onnx is not defensive at load time: its C++ `ReadTokens`
// (symbol-table.cc, pinned v1.12.38) calls SHERPA_ONNX_EXIT on input
// outside its line grammar, killing the *whole app* instead of
// surfacing an error. Every path that builds a recognizer (dictation
// via `with_recognizer`, background prewarm via `prewarm_once`) goes
// through `create_recognizer`, so validating the files there turns a
// bad tokens file into a regular error.
//
// Scope/limitations: the tokens check mirrors sherpa's exact line
// grammar, so anything that passes it cannot trip SHERPA_ONNX_EXIT in
// `ReadTokens`. The ONNX check is only a *structural* sanity check
// (top-level protobuf wire format) — it catches garbage bytes, text
// files and truncated downloads, but a wire-valid file that is
// semantically broken can still fail inside ORT. ORT failures normally
// come back as `OfflineRecognizer::create` returning None (a normal
// error for us); no pure-Rust pre-check can rule out every possible
// C++ CHECK/abort in a third-party runtime.

/// Hard cap on tokens file size. Real sherpa vocabularies are tens of
/// KB; without a cap, a multi-GB garbage "tokens" file would be read
/// fully into memory before failing the UTF-8 check.
const MAX_TOKENS_BYTES: u64 = 64 * 1024 * 1024;

/// Validate all four model files before sherpa sees them. The tokens
/// check exactly matches sherpa's exit-on-bad-input grammar; the ONNX
/// check is a structural sanity check only (see `validate_onnx`).
/// Returns a human-readable reason on failure (surfaced to the user at
/// dictation time).
fn validate_model_files(paths: &ModelPaths) -> Result<(), String> {
    validate_tokens(&paths.tokens)?;
    for onnx in [&paths.encoder, &paths.decoder, &paths.joiner] {
        validate_onnx(onnx)?;
    }
    Ok(())
}

/// The tokens file must be UTF-8 text matching the exact line grammar
/// of sherpa's C++ `ReadTokens` (symbol-table.cc, v1.12.38):
///
/// ```text
/// <symbol> <id>   — symbol, ASCII whitespace, int32 id
/// <id>            — a lone integer: the whitespace token " " with that id
/// (blank)         — tolerated by sherpa, skipped here
/// ```
///
/// `ReadTokens` calls SHERPA_ONNX_EXIT on anything outside that grammar:
/// an unparseable or partially-parseable id (`abc`, `12abc`), or extra
/// trailing fields (`symbol extra 1`). Fields are split on ASCII
/// whitespace only, like C++ `operator>>` in the classic locale —
/// Unicode whitespace (e.g. U+00A0) does NOT separate fields for
/// sherpa, so splitting on it here could accept a line sherpa exits on.
///
/// One deliberately stricter case: a lone NON-numeric field. sherpa
/// `atoi`s it to id 0 and loads the (junk) model; we reject it, since
/// no real vocabulary has such lines and the model would be useless.
fn validate_tokens(tokens: &std::path::Path) -> Result<(), String> {
    let name = || tokens.display().to_string();
    let len = std::fs::metadata(tokens)
        .map_err(|e| format!("{}: cannot stat ({})", name(), e))?
        .len();
    if len == 0 {
        return Err(format!("{}: file is empty", name()));
    }
    if len > MAX_TOKENS_BYTES {
        return Err(format!(
            "{}: {} bytes is far too large for a tokens file",
            name(),
            len
        ));
    }
    let text =
        std::fs::read_to_string(tokens).map_err(|_| format!("{}: not valid UTF-8 text", name()))?;
    let mut any_token = false;
    for (i, line) in text.lines().enumerate() {
        // C++ classic-locale whitespace includes vertical tab, which
        // Rust's split_ascii_whitespace deliberately excludes.
        let mut fields = line
            .split(|c: char| c.is_ascii_whitespace() || c == '\u{b}')
            .filter(|field| !field.is_empty());
        let Some(symbol) = fields.next() else {
            continue; // blank line: tolerated by sherpa
        };
        any_token = true;
        match fields.next() {
            // Lone field: the whitespace token, id = the field itself.
            None => {
                if symbol.parse::<i32>().is_err() {
                    return Err(format!(
                        "{}: line {} is a lone non-numeric field",
                        name(),
                        i + 1
                    ));
                }
            }
            // "symbol id": the id must parse FULLY as int32 (sherpa
            // exits on `12abc`), and a third field exits it outright.
            Some(id) => {
                if id.parse::<i32>().is_err() {
                    return Err(format!(
                        "{}: line {} has no parseable integer id",
                        name(),
                        i + 1
                    ));
                }
                if fields.next().is_some() {
                    return Err(format!(
                        "{}: line {} has extra trailing fields",
                        name(),
                        i + 1
                    ));
                }
            }
        }
    }
    if !any_token {
        return Err(format!("{}: contains no tokens", name()));
    }
    Ok(())
}

/// The ONNX encoder/decoder/joiner must look like a protobuf-encoded
/// `ModelProto`. ONNX has **no fixed magic bytes** — a `.onnx` file is
/// just serialized protobuf — so this walks the protobuf wire format
/// instead: every top-level field must have a legal tag and stay within
/// the file, and field 1 (`ir_version`, a `required` varint in
/// onnx.proto) must be present. That catches garbage bytes, text files
/// and truncated downloads without claiming a magic header exists.
///
/// This is a structural sanity check, not a proof of loadability:
/// nested messages are skipped unparsed, so a wire-valid but
/// semantically broken model can still fail inside ORT (normally as
/// `OfflineRecognizer::create` returning None — a regular error for
/// us, since sherpa surfaces ORT failures rather than exiting).
///
/// Reads only tags and lengths (skipping field payloads with `seek`),
/// so even a multi-hundred-MB model validates without being read.
fn validate_onnx(path: &std::path::Path) -> Result<(), String> {
    use std::io::{BufReader, Seek, SeekFrom};

    let name = || path.display().to_string();
    let file = std::fs::File::open(path).map_err(|e| format!("{}: cannot open ({})", name(), e))?;
    let file_len = file
        .metadata()
        .map_err(|e| format!("{}: cannot stat ({})", name(), e))?
        .len();
    if file_len == 0 {
        return Err(format!("{}: file is empty", name()));
    }

    let mut reader = BufReader::new(file);
    let mut saw_ir_version = false;
    loop {
        let pos = reader
            .stream_position()
            .map_err(|e| format!("{}: read error ({})", name(), e))?;
        if pos >= file_len {
            break;
        }
        let key = read_protobuf_varint(&mut reader)
            .ok_or_else(|| format!("{}: truncated or invalid protobuf tag", name()))?;
        let field = key >> 3;
        let wire_type = key & 0x7;
        if field == 0 {
            return Err(format!("{}: invalid protobuf field tag 0", name()));
        }
        match wire_type {
            // Varint
            0 => {
                read_protobuf_varint(&mut reader)
                    .ok_or_else(|| format!("{}: truncated varint field", name()))?;
                if field == 1 {
                    saw_ir_version = true;
                }
            }
            // 64-bit / 32-bit fixed. Bounds are measured AFTER the
            // (possibly multi-byte) tag varint — `pos` predates it.
            1 | 5 => {
                let bytes: u64 = if wire_type == 1 { 8 } else { 4 };
                let after_tag = reader
                    .stream_position()
                    .map_err(|e| format!("{}: read error ({})", name(), e))?;
                if bytes > file_len - after_tag {
                    return Err(format!("{}: truncated fixed-width field", name()));
                }
                reader
                    .seek(SeekFrom::Current(bytes as i64))
                    .map_err(|e| format!("{}: read error ({})", name(), e))?;
            }
            // Length-delimited (strings, nested messages, raw tensors)
            2 => {
                let len = read_protobuf_varint(&mut reader)
                    .ok_or_else(|| format!("{}: truncated length prefix", name()))?;
                let after_len = reader
                    .stream_position()
                    .map_err(|e| format!("{}: read error ({})", name(), e))?;
                if len > file_len - after_len {
                    return Err(format!(
                        "{}: field declares {} bytes past end of file (truncated?)",
                        name(),
                        len
                    ));
                }
                reader
                    .seek(SeekFrom::Current(len as i64))
                    .map_err(|e| format!("{}: read error ({})", name(), e))?;
            }
            // Groups (deprecated) and reserved wire types never appear
            // in ONNX models.
            _ => return Err(format!("{}: unsupported protobuf wire type", name())),
        }
    }
    if !saw_ir_version {
        return Err(format!(
            "{}: no ir_version field — not an ONNX ModelProto",
            name()
        ));
    }
    Ok(())
}

/// Read one protobuf varint (up to 10 bytes for u64). Returns `None`
/// on truncation or overflow.
fn read_protobuf_varint(reader: &mut impl std::io::Read) -> Option<u64> {
    let mut value: u64 = 0;
    let mut buf = [0u8; 1];
    for shift in (0..=63).step_by(7) {
        reader.read_exact(&mut buf).ok()?;
        let byte = buf[0];
        if shift == 63 {
            // 10th byte: only one value bit fits in a u64; a set
            // continuation bit or higher value bits mean overflow.
            if byte > 1 {
                return None;
            }
            value |= (byte as u64) << shift;
        } else {
            value |= ((byte & 0x7f) as u64) << shift;
        }
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Create the ASR recognizer from model files.
///
/// - `Err`: files exist but failed validation — tokens outside
///   sherpa's `ReadTokens` grammar (which would exit the process) or
///   structurally invalid ONNX. These never reach sherpa.
/// - `Ok(None)`: files missing, or sherpa itself declined the model.
fn create_recognizer(
    paths: &ModelPaths,
    num_threads: i32,
) -> anyhow::Result<Option<OfflineRecognizer>> {
    if !paths.all_exist() {
        return Ok(None);
    }
    validate_model_files(paths).map_err(anyhow::Error::msg)?;

    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer.encoder = Some(paths.encoder.to_string_lossy().to_string());
    config.model_config.transducer.decoder = Some(paths.decoder.to_string_lossy().to_string());
    config.model_config.transducer.joiner = Some(paths.joiner.to_string_lossy().to_string());
    config.model_config.tokens = Some(paths.tokens.to_string_lossy().to_string());
    config.model_config.model_type = Some("nemo_transducer".to_string());
    config.model_config.num_threads = num_threads;
    config.model_config.debug = false;

    Ok(OfflineRecognizer::create(&config))
}

/// Build a recognizer for `(model_paths, num_threads)` and store it in
/// the cache. Returns `Ok(None)` on a load failure WITHOUT touching the
/// cache — a failed load must never evict a working recognizer.
/// `Err` (corrupt files) likewise leaves the cache untouched.
fn store_recognizer(
    cache: &mut Option<CachedRecognizer>,
    model_paths: &ModelPaths,
    num_threads: i32,
) -> anyhow::Result<Option<()>> {
    let Some(recognizer) = create_recognizer(model_paths, num_threads)? else {
        return Ok(None);
    };
    *cache = Some(CachedRecognizer {
        model_paths: model_paths.clone(),
        num_threads,
        recognizer: SendableRecognizer(recognizer),
    });
    Ok(Some(()))
}

/// Inference thread count from the on-disk config (0 = auto, config
/// load failure → 4). Read from disk — the same source
/// `start_recording` and the prewarm use — so the recognizer cache key
/// is consistent across all of them.
fn configured_num_threads() -> i32 {
    crate::config::AppConfig::load()
        .map(|c| resolve_num_threads(c.num_threads))
        .unwrap_or(4)
}

/// Resolve the configured inference thread count (0 = auto).
fn resolve_num_threads(configured: u32) -> i32 {
    if configured == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4)
    } else {
        configured as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four model file paths inside `dir` (files NOT created).
    fn model_paths_in(dir: &std::path::Path) -> ModelPaths {
        ModelPaths {
            encoder: dir.join("encoder.int8.onnx"),
            decoder: dir.join("decoder.int8.onnx"),
            joiner: dir.join("joiner.int8.onnx"),
            tokens: dir.join("tokens.txt"),
        }
    }

    #[test]
    #[ignore = "requires CANARIO_TEST_MODEL_DIR pointing to a downloaded model"]
    fn cached_model_passes_recognizer_creation_guard() {
        let dir = std::env::var_os("CANARIO_TEST_MODEL_DIR").expect("set CANARIO_TEST_MODEL_DIR");
        let paths = model_paths_in(std::path::Path::new(&dir));
        let recognizer = create_recognizer(&paths, 2).expect("valid model should pass validation");
        assert!(recognizer.is_some(), "valid model should load in sherpa");
    }

    /// Create all four model files with the given bytes.
    fn write_model_files(paths: &ModelPaths, bytes: &[u8]) {
        for f in [&paths.encoder, &paths.decoder, &paths.joiner, &paths.tokens] {
            std::fs::write(f, bytes).unwrap();
        }
    }

    /// Smallest byte string that passes `validate_onnx`: a single
    /// top-level varint field 1 (`ir_version`). ONNX is protobuf with
    /// no magic bytes, so this is all the check can require.
    const MINIMAL_ONNX: &[u8] = &[0x08, 0x01];

    /// Model files whose tokens are fine but whose ONNX files are
    /// garbage (so nothing ever reaches sherpa).
    fn write_garbage_onnx_model(paths: &ModelPaths) {
        for f in [&paths.encoder, &paths.decoder, &paths.joiner] {
            std::fs::write(f, b"this is not a protobuf message").unwrap();
        }
        std::fs::write(&paths.tokens, "▁t 0\n▁h 1\n").unwrap();
    }

    /// A prewarm for a model that is not on disk must do nothing — no
    /// load attempt, no cache write, no error. (This is the path taken
    /// at startup before the first model download.)
    #[test]
    fn prewarm_skips_when_model_files_missing() {
        let paths = model_paths_in(std::path::Path::new("/nonexistent-canario-test-model"));

        let outcome = prewarm_once(&paths, 4, recognizer_config_epoch());

        assert_eq!(outcome, PrewarmOutcome::MissingModelFiles);
        assert!(RECOGNIZER_CACHE.lock().is_none());
    }

    /// A prewarm requested before a config change must abandon (without
    /// touching the cache) even when its model files exist — otherwise a
    /// stale background request could evict a fresher cache entry after
    /// a quick model switch.
    #[test]
    fn prewarm_abandons_when_config_changed_since_request() {
        let dir = tempfile::tempdir().unwrap();
        let paths = model_paths_in(dir.path());
        // Plausible files, so validation passes and the epoch check is
        // what decides the outcome.
        for f in [&paths.encoder, &paths.decoder, &paths.joiner] {
            std::fs::write(f, MINIMAL_ONNX).unwrap();
        }
        std::fs::write(&paths.tokens, "▁t 0\n").unwrap();

        let epoch_at_request = recognizer_config_epoch();
        recognizer_config_changed(); // a config update races the prewarm

        let outcome = prewarm_once(&paths, 4, epoch_at_request);
        assert_eq!(outcome, PrewarmOutcome::StaleConfig);
        assert!(RECOGNIZER_CACHE.lock().is_none());
    }

    /// An unparseable tokens file must never reach sherpa: its C++
    /// `ReadTokens` **exits the process** on bad input (verified the
    /// hard way — an earlier version of this suite fed it garbage and
    /// the whole test binary died with status 255). The prewarm skips
    /// such files and leaves the cache untouched; dictation still
    /// surfaces the real problem via `with_recognizer`.
    #[test]
    fn prewarm_skips_unparseable_tokens_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = model_paths_in(dir.path());
        // Binary garbage is not UTF-8, so it fails validation before
        // any sherpa call.
        write_model_files(&paths, b"\xff\xfe\x00definitely not text");

        let outcome = prewarm_once(&paths, 4, recognizer_config_epoch());

        assert_eq!(outcome, PrewarmOutcome::InvalidModelFiles);
        assert!(RECOGNIZER_CACHE.lock().is_none());
    }

    /// A model whose tokens parse but whose ONNX files are garbage is
    /// just as dangerous (ORT may abort on it), so the prewarm refuses
    /// it too.
    #[test]
    fn prewarm_skips_corrupt_onnx_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = model_paths_in(dir.path());
        write_garbage_onnx_model(&paths);

        let outcome = prewarm_once(&paths, 4, recognizer_config_epoch());

        assert_eq!(outcome, PrewarmOutcome::InvalidModelFiles);
        assert!(RECOGNIZER_CACHE.lock().is_none());
    }

    /// Regression for the dictation path (canario-vje): before the
    /// guard lived in `create_recognizer`, a corrupt tokens file went
    /// straight to sherpa's `ReadTokens`, which `exit(1)`ed the whole
    /// process. Now the same input is a plain `Err` — the recording
    /// thread turns it into `Event::Error`. (If this test binary
    /// survives, the guard held; before the fix it would die here.)
    #[test]
    fn create_recognizer_rejects_corrupt_tokens_instead_of_exiting() {
        let dir = tempfile::tempdir().unwrap();
        let paths = model_paths_in(dir.path());
        write_model_files(&paths, b"\xff\xfe\x00definitely not text");

        let result = create_recognizer(&paths, 4);

        // (Can't `expect_err`: OfflineRecognizer has no Debug impl.)
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("corrupt tokens must be an error, not a load attempt"),
        };
        assert!(err.to_string().contains("tokens.txt"), "error: {}", err);
    }

    /// Same guarantee when the tokens are fine but an ONNX file is
    /// corrupt: the error names the offending file.
    #[test]
    fn create_recognizer_rejects_corrupt_onnx_instead_of_exiting() {
        let dir = tempfile::tempdir().unwrap();
        let paths = model_paths_in(dir.path());
        write_garbage_onnx_model(&paths);

        let result = create_recognizer(&paths, 4);

        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("corrupt onnx must be an error, not a load attempt"),
        };
        assert!(err.to_string().contains("encoder"), "error: {}", err);
    }

    /// Missing files stay on the old path: `Ok(None)`, mapped by
    /// `with_recognizer` to the "download the model" error.
    #[test]
    fn create_recognizer_missing_files_is_none_not_error() {
        let paths = model_paths_in(std::path::Path::new("/nonexistent-canario-test-model"));
        let result = create_recognizer(&paths, 4);
        assert!(matches!(result, Ok(None)));
    }

    /// `0` means "auto" (available parallelism); anything else is used
    /// verbatim. Part of the cache key, so the mapping must be stable.
    #[test]
    fn resolve_num_threads_maps_zero_to_auto() {
        let auto = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4);
        assert_eq!(resolve_num_threads(0), auto);
        assert_eq!(resolve_num_threads(1), 1);
        assert_eq!(resolve_num_threads(8), 8);
    }

    /// The tokens gate is what keeps sherpa's exit()-on-bad-input
    /// `ReadTokens` from ever seeing these files.
    #[test]
    fn validate_tokens_rejects_missing_empty_and_binary() {
        let dir = tempfile::tempdir().unwrap();

        let missing = dir.path().join("missing.txt");
        assert!(validate_tokens(&missing).is_err());

        let empty = dir.path().join("empty.txt");
        std::fs::write(&empty, b"").unwrap();
        assert!(validate_tokens(&empty).is_err());

        // Only blank lines: no tokens at all.
        let blank = dir.path().join("blank.txt");
        std::fs::write(&blank, "\n  \n\t\n").unwrap();
        assert!(validate_tokens(&blank).is_err());

        // Invalid UTF-8 (a lead byte is missing from this emoji).
        let binary = dir.path().join("binary.txt");
        std::fs::write(&binary, [0u8, 159, 146, 150]).unwrap();
        assert!(validate_tokens(&binary).is_err());

        let valid = dir.path().join("tokens.txt");
        std::fs::write(&valid, "▁t 0\n▁h 1\n").unwrap();
        assert!(validate_tokens(&valid).is_ok());
    }

    /// The tokens gate mirrors sherpa's `ReadTokens` line grammar
    /// (symbol-table.cc v1.12.38), because lines outside it make the
    /// C++ code SHERPA_ONNX_EXIT the whole process.
    #[test]
    fn validate_tokens_requires_symbol_id_lines() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, content: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, content).unwrap();
            p
        };

        // Non-integer id: `iss >> id` fails, trailing data → exit.
        assert!(validate_tokens(&write("bad_id.txt", "▁t abc\n")).is_err());
        // Partially-parseable id: C++ reads 12, then 'a' is trailing
        // data → exit.
        assert!(validate_tokens(&write("partial_id.txt", "▁t 12abc\n")).is_err());
        // Extra trailing fields — even with a parseable LAST field —
        // exit sherpa (`iss >> std::ws` leaves `extra`/`1` unread).
        // Reading only the last field would falsely accept these.
        assert!(validate_tokens(&write("extra_mid.txt", "▁t extra 1\n")).is_err());
        assert!(validate_tokens(&write("vertical_tab_extra.txt", "▁t\u{b}extra 1\n")).is_err());
        assert!(validate_tokens(&write("extra_end.txt", "▁t 1 extra\n")).is_err());
        // Lone non-numeric field: sherpa atoi()s it to 0 and loads a
        // junk model; we reject (stricter, but no real vocab has this).
        assert!(validate_tokens(&write("lone_junk.txt", "hello\n")).is_err());
        // Unicode whitespace does NOT separate fields for C++: the
        // "id" here is `1\u{A0}` — unparseable, sherpa exits.
        assert!(validate_tokens(&write("nbsp.txt", "▁t 1\u{A0}\n")).is_err());

        // ASCII whitespace variants (tab, CRLF, leading space) are fine.
        assert!(validate_tokens(&write("tabs.txt", "▁t\t5\n")).is_ok());
        assert!(validate_tokens(&write("vertical_tab.txt", "▁t\u{b}5\n")).is_ok());
        assert!(validate_tokens(&write("crlf.txt", "  ▁t 5\r\n")).is_ok());
        // Negative ids parse as int32 and sherpa loads them without
        // exiting — accepted (the guard targets exit-causing input).
        assert!(validate_tokens(&write("neg_id.txt", "▁t -1\n")).is_ok());
        // Lone integer = the whitespace token with that id.
        assert!(validate_tokens(&write("space_tok.txt", "▁t 0\n5\n")).is_ok());
        // Multi-blank-line padding is tolerated.
        assert!(validate_tokens(&write("ok.txt", "<blk> 0\n\n▁world 2\n")).is_ok());
    }

    /// A garbage "tokens" file the size of a model must be rejected
    /// from its metadata alone — never read into memory.
    #[test]
    fn validate_tokens_rejects_oversized_files_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.txt");
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(MAX_TOKENS_BYTES + 1).unwrap();
        let err = validate_tokens(&path).unwrap_err();
        assert!(err.contains("too large"), "error: {}", err);
    }

    /// ONNX is protobuf with no magic bytes: acceptance hinges on a
    /// well-formed top-level wire stream containing field 1
    /// (`ir_version`), not on any fixed header.
    #[test]
    fn validate_onnx_accepts_well_formed_protobuf() {
        let dir = tempfile::tempdir().unwrap();

        // Just ir_version = 1.
        let minimal = dir.path().join("minimal.onnx");
        std::fs::write(&minimal, MINIMAL_ONNX).unwrap();
        assert!(validate_onnx(&minimal).is_ok());

        // ir_version, then a length-delimited field 7 (graph) whose
        // payload is skipped without parsing, then a varint field 14.
        let with_graph = dir.path().join("graph.onnx");
        std::fs::write(
            &with_graph,
            [0x08, 0x07, 0x3A, 0x03, 0xAA, 0xBB, 0xCC, 0x70, 0x2A],
        )
        .unwrap();
        assert!(validate_onnx(&with_graph).is_ok());

        // ir_version plus fixed32/fixed64 fields (legal wire types),
        // including a multi-byte tag (field 16 ≥ 16 → 2-byte varint).
        let fixed = dir.path().join("fixed.onnx");
        std::fs::write(
            &fixed,
            [
                [0x08, 0x01].as_slice(),               // field 1 varint
                &[0x11, 1, 2, 3, 4, 5, 6, 7, 8],       // field 2, 64-bit
                &[0x1D, 1, 2, 3, 4],                   // field 3, 32-bit
                &[0x81, 0x01, 1, 2, 3, 4, 5, 6, 7, 8], // field 16, 64-bit
            ]
            .concat(),
        )
        .unwrap();
        assert!(validate_onnx(&fixed).is_ok());
    }

    /// Regression: fixed-width bounds must be measured AFTER the tag
    /// varint. With a multi-byte tag (field number ≥ 16), measuring
    /// from the pre-tag position lets a payload run past EOF
    /// undetected (the seek beyond end-of-file succeeds silently).
    #[test]
    fn validate_onnx_rejects_fixed_width_truncated_by_multibyte_tag() {
        let dir = tempfile::tempdir().unwrap();
        // ir_version=1, then field 16 (tag = (16<<3)|1 = 129, varint
        // 0x81 0x01) wire type 1 — only 6 of the 8 payload bytes present.
        let path = dir.path().join("truncated-fixed.onnx");
        std::fs::write(&path, [0x08, 0x01, 0x81, 0x01, 1, 2, 3, 4, 5, 6]).unwrap();
        let err = validate_onnx(&path).unwrap_err();
        assert!(err.contains("truncated"), "error: {}", err);
    }

    /// The rejection cases: everything that must fail BEFORE sherpa or
    /// ORT can see it.
    #[test]
    fn validate_onnx_rejects_garbage_truncated_and_non_model_files() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, content: &[u8]| {
            let p = dir.path().join(name);
            std::fs::write(&p, content).unwrap();
            p
        };

        assert!(validate_onnx(&dir.path().join("missing.onnx")).is_err());
        assert!(validate_onnx(&write("empty.onnx", b"")).is_err());
        // Valid UTF-8 text (passes the tokens check!) is not a model.
        assert!(validate_onnx(&write("text.onnx", b"hello world\n")).is_err());
        // Well-formed protobuf, but no ir_version → not a ModelProto.
        assert!(validate_onnx(&write("no_ir.onnx", &[0x12, 0x01, 0x00])).is_err());
        // Field tag 0 is illegal protobuf.
        assert!(validate_onnx(&write("tag0.onnx", &[0x00, 0x01])).is_err());
        // Deprecated group wire types never appear in ONNX.
        assert!(validate_onnx(&write("group.onnx", &[0x0B, 0x01])).is_err());
        // ir_version, then a field claiming 16 payload bytes with only
        // 1 left — a truncated download.
        assert!(validate_onnx(&write("truncated.onnx", &[0x08, 0x01, 0x3A, 0x10, 0x00])).is_err());
        // Tag varint cut off mid-continuation.
        assert!(validate_onnx(&write("bad_tag.onnx", &[0x88])).is_err());
    }

    /// Regression guard for the process-exit repro: the exact bytes
    /// that killed the test binary with status 255 must now fail
    /// validation on every model file role they could occupy.
    #[test]
    fn validate_model_files_rejects_the_status_255_repro_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let paths = model_paths_in(dir.path());
        write_model_files(&paths, b"\xff\xfe\x00definitely not text");
        let err = validate_model_files(&paths).unwrap_err();
        assert!(err.contains("tokens.txt"), "error: {}", err);

        // Garbage ONNX is caught even with valid tokens.
        write_garbage_onnx_model(&paths);
        assert!(validate_model_files(&paths).is_err());

        // And a fully plausible set passes: real tokens + minimal
        // protobuf-shaped ONNX files.
        for f in [&paths.encoder, &paths.decoder, &paths.joiner] {
            std::fs::write(f, MINIMAL_ONNX).unwrap();
        }
        std::fs::write(&paths.tokens, "▁t 0\n").unwrap();
        assert!(validate_model_files(&paths).is_ok());
    }

    /// A buffer shorter than the window decodes in full — early in a
    /// session there is nothing to slide over.
    #[test]
    fn live_window_covers_short_buffers_entirely() {
        assert_eq!(live_window_range(1_000, 16_000, 20.0), 0..1_000);
        assert_eq!(live_window_range(0, 48_000, 20.0), 0..0);
    }

    /// Once the buffer outgrows the window, decodes see exactly the
    /// trailing window (here: 30s of 16kHz audio, 20s window → the
    /// last 320_000 samples).
    #[test]
    fn live_window_returns_tail_for_long_buffers() {
        assert_eq!(live_window_range(480_000, 16_000, 20.0), 160_000..480_000);
        // Window size is respected at non-16k rates too.
        assert_eq!(
            live_window_range(1_440_000, 48_000, 20.0),
            480_000..1_440_000
        );
    }

    /// A zero-second window must degenerate to an empty tail, never a
    /// underflow/panic on `buf_len - window_samples`.
    #[test]
    fn live_window_zero_secs_yields_empty_tail() {
        let range = live_window_range(480_000, 16_000, 0.0);
        assert_eq!(range, 480_000..480_000);
        assert!(range.is_empty());
    }
}
