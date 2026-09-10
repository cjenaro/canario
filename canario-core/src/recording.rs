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
        store_recognizer(&mut cache, model_paths, num_threads).ok_or_else(|| {
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
    /// The tokens file is not parseable text. sherpa's C++
    /// `ReadTokens` **exits the process** on unparseable input, so the
    /// optional warmup refuses to touch such a model; a genuinely
    /// broken model still fails loudly at dictation time via
    /// `with_recognizer`.
    InvalidTokens,
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
                | PrewarmOutcome::InvalidTokens
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
    // ...and never hand sherpa a tokens file it cannot parse: its C++
    // ReadTokens exits the whole process on bad input, and an optional
    // background warmup must not be able to take the app down.
    if !tokens_looks_valid(&model_paths.tokens) {
        return PrewarmOutcome::InvalidTokens;
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

    if store_recognizer(&mut cache, model_paths, num_threads).is_some() {
        PrewarmOutcome::Warmed
    } else {
        PrewarmOutcome::LoadFailed
    }
}

/// Cheap sanity check that the tokens file looks parseable: non-empty
/// UTF-8 text with at least one non-blank line. sherpa's C++
/// `ReadTokens` calls `exit(1)` on unparseable input, so the prewarm
/// refuses files that could kill the process — real errors still reach
/// the user through `with_recognizer` at dictation time. A valid file
/// that merely looks odd costs at most a skipped warmup (the first
/// dictation then loads on demand).
fn tokens_looks_valid(tokens: &std::path::Path) -> bool {
    match std::fs::read_to_string(tokens) {
        Ok(text) => text.lines().any(|line| !line.trim().is_empty()),
        Err(_) => false,
    }
}

/// Create the ASR recognizer from model files.
fn create_recognizer(paths: &ModelPaths, num_threads: i32) -> Option<OfflineRecognizer> {
    if !paths.all_exist() {
        return None;
    }

    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer.encoder = Some(paths.encoder.to_string_lossy().to_string());
    config.model_config.transducer.decoder = Some(paths.decoder.to_string_lossy().to_string());
    config.model_config.transducer.joiner = Some(paths.joiner.to_string_lossy().to_string());
    config.model_config.tokens = Some(paths.tokens.to_string_lossy().to_string());
    config.model_config.model_type = Some("nemo_transducer".to_string());
    config.model_config.num_threads = num_threads;
    config.model_config.debug = false;

    OfflineRecognizer::create(&config)
}

/// Build a recognizer for `(model_paths, num_threads)` and store it in
/// the cache. Returns `None` on failure WITHOUT touching the cache —
/// a failed load must never evict a working recognizer.
fn store_recognizer(
    cache: &mut Option<CachedRecognizer>,
    model_paths: &ModelPaths,
    num_threads: i32,
) -> Option<()> {
    let recognizer = create_recognizer(model_paths, num_threads)?;
    *cache = Some(CachedRecognizer {
        model_paths: model_paths.clone(),
        num_threads,
        recognizer: SendableRecognizer(recognizer),
    });
    Some(())
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

    /// Create all four model files with the given bytes.
    fn write_model_files(paths: &ModelPaths, bytes: &[u8]) {
        for f in [&paths.encoder, &paths.decoder, &paths.joiner, &paths.tokens] {
            std::fs::write(f, bytes).unwrap();
        }
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
        write_model_files(&paths, b"placeholder");

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
        // Binary garbage is not UTF-8, so it fails the sanity check
        // before any sherpa call.
        write_model_files(&paths, b"\xff\xfe\x00definitely not text");

        let outcome = prewarm_once(&paths, 4, recognizer_config_epoch());

        assert_eq!(outcome, PrewarmOutcome::InvalidTokens);
        assert!(RECOGNIZER_CACHE.lock().is_none());
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

    /// The tokens sanity gate is what keeps the prewarm from ever
    /// feeding sherpa a file its C++ would exit() on.
    #[test]
    fn tokens_looks_valid_rejects_missing_empty_and_binary() {
        let dir = tempfile::tempdir().unwrap();

        let missing = dir.path().join("missing.txt");
        assert!(!tokens_looks_valid(&missing));

        let empty = dir.path().join("empty.txt");
        std::fs::write(&empty, b"").unwrap();
        assert!(!tokens_looks_valid(&empty));

        // Invalid UTF-8 (a lead byte is missing from this emoji).
        let binary = dir.path().join("binary.txt");
        std::fs::write(&binary, [0u8, 159, 146, 150]).unwrap();
        assert!(!tokens_looks_valid(&binary));

        let valid = dir.path().join("tokens.txt");
        std::fs::write(&valid, "▁t 0\n▁h 1\n").unwrap();
        assert!(tokens_looks_valid(&valid));
    }
}
