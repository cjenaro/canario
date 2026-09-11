/// Recording engine — captures audio, transcribes, emits events.
///
/// Capture runs through the warm-mic machinery (canario-vew, see
/// [`crate::mic_warm`]): one parked capture stream feeds a ring
/// buffer, a press marks the start offset, and this loop drains the
/// ring into the recording buffer until stop. Results are
/// communicated via the `Sender<Event>` channel.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig};

use crate::config::{ModelPaths, TransformSettings};
use crate::event::Event;
use crate::inference::postprocess::PostProcessor;
use crate::mic_warm::{self, MicSession, MicVerdict};
use crate::timing;

// ── Stop signalling ───────────────────────────────────────────────────
// The capture loop used to poll a plain `AtomicBool` every 50 ms, so
// every stop paid a uniform 0–50 ms wait before transcription began
// (canario-b1g baseline: 14.3 ms mean / 28.5 max, 50 ms by
// construction). `StopSignal` replaces the poll with a condvar wake:
// `RecordingHandle::stop`/`cancel` flip the flag and notify while
// holding the mutex the waiter owns while checking it, so the capture
// thread reacts in microseconds and the `stop_observed` timing mark
// measures the true wake latency.

/// Shared stop/cancel signal between the recording API (`RecordingHandle`)
/// and the capture thread (plus the live-captions worker, which reuses
/// the same wait for its cadence slices).
struct StopSignal {
    /// Set when the capture loop should stop and transcribe.
    stop: AtomicBool,
    /// Set when stopping should also discard the audio (no transcription).
    cancel: AtomicBool,
    /// Notified after every flag flip. Pairing mutex exists only for
    /// the condvar protocol — the flags are the payload.
    cond: Condvar,
    mutex: Mutex<()>,
}

impl StopSignal {
    fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            cond: Condvar::new(),
            mutex: Mutex::new(()),
        }
    }

    /// Signal the capture loop to stop and transcribe.
    ///
    /// The flag flip and the notify run under the mutex the waiter
    /// holds while checking the flag, so a stop can never land in the
    /// "checked the flag → not yet asleep" window where the notify
    /// would be missed.
    fn signal_stop(&self) {
        let _guard = self.mutex.lock();
        self.stop.store(true, Ordering::SeqCst);
        self.cond.notify_all();
    }

    /// Signal stop **and** discard — same wake path as [`Self::signal_stop`].
    fn signal_cancel(&self) {
        let _guard = self.mutex.lock();
        self.cancel.store(true, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        self.cond.notify_all();
    }

    /// Has a stop (or cancel) been signalled?
    fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Was the stop a cancel (discard) rather than a transcribe?
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Block until a stop/cancel signal arrives or `timeout` elapses.
    ///
    /// Returns `true` when stopped (disambiguate with
    /// [`Self::is_cancelled`]), `false` on timeout. Callers use the
    /// timeout as their tick: the capture loop paces `Event::AudioLevel`
    /// this way, and the live-captions worker slices its decode cadence.
    /// Spurious wakes are re-checked, so `true` always means the flag
    /// is actually set.
    fn wait_for_stop(&self, timeout: Duration) -> bool {
        let mut guard = self.mutex.lock();
        let deadline = Instant::now() + timeout;
        while !self.is_stopped() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let woke = self.cond.wait_for(&mut guard, remaining);
            if woke.timed_out() {
                return self.is_stopped();
            }
        }
        true
    }
}

/// Handle to stop a running recording and track thread completion.
pub struct RecordingHandle {
    signal: Arc<StopSignal>,
    /// Set to `false` by the thread when it finishes (recording + transcription).
    busy: Arc<AtomicBool>,
}

impl RecordingHandle {
    /// Signal the recording thread to stop.
    ///
    /// Wakes the capture loop's condvar immediately — no poll to wait out.
    pub fn stop(&self) {
        self.signal.signal_stop();
    }

    /// Signal the recording thread to stop AND discard the audio:
    /// the mic is released but the buffer is never transcribed, and
    /// `Event::RecordingCancelled` is emitted instead of
    /// `TranscriptionReady`/`RecordingStopped`.
    pub fn cancel(&self) {
        self.signal.signal_cancel();
    }

    /// Is the thread still running (capturing or transcribing)?
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
}

/// Start recording from the microphone.
///
/// Captures audio until `RecordingHandle::stop()` is called, then
/// transcribes the entire buffer, applies post-processing, runs the
/// LLM transformation pipeline when `transform.enabled` (fgm.3 D3 —
/// before the `TranscriptionReady` event), and sends events via `tx`.
pub fn start_recording(
    model_paths: ModelPaths,
    tx: std::sync::mpsc::Sender<Event>,
    post_processor: PostProcessor,
    transform: TransformSettings,
    sound_effects: bool,
    sound_volume: f32,
) -> anyhow::Result<RecordingHandle> {
    let signal = Arc::new(StopSignal::new());
    let busy = Arc::new(AtomicBool::new(true));
    let signal_clone = signal.clone();
    let busy_clone = busy.clone();

    // Play start beep
    if sound_effects {
        crate::audio::effects::beep_start(sound_volume);
    }

    // Inference thread count (0 = auto). Part of the recognizer cache
    // key, so a change reloads the model.
    let num_threads = configured_num_threads();

    std::thread::spawn(move || {
        tracing::info!("Recording thread starting...");
        let result = recording_loop(
            model_paths,
            tx.clone(),
            signal_clone,
            &post_processor,
            transform,
            sound_effects,
            sound_volume,
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

    Ok(RecordingHandle { signal, busy })
}

/// Cadence of `Event::AudioLevel` while recording — also the timeout the
/// capture loop's condvar wait uses, so level ticks keep their original
/// 50 ms rhythm while a stop signal still wakes the loop immediately.
const AUDIO_LEVEL_INTERVAL_MS: u64 = 50;

/// The main recording loop — runs in a background thread.
#[allow(clippy::too_many_arguments)] // thread-spawn plumbing, not an API
fn recording_loop(
    model_paths: ModelPaths,
    tx: std::sync::mpsc::Sender<Event>,
    signal: Arc<StopSignal>,
    post_processor: &PostProcessor,
    transform: TransformSettings,
    sound_effects: bool,
    sound_volume: f32,
    num_threads: i32,
) -> anyhow::Result<()> {
    timing::mark("recording_thread_start");

    // ── Acquire the mic (canario-vew) ───────────────────────────────
    // Warm path: the parked stream is already running into the ring,
    // so this is a µs offset-mark (mic_warm_reused). Cold path: the
    // device open + stream play happen inline here, exactly like the
    // pre-vew code (mic_device_opened / mic_stream_started), and the
    // stream then parks after the recording for the next press.
    let mut session: MicSession = mic_warm::begin_recording()?;
    let mic_sr = session.sample_rate();

    // The recording's own buffer, drained from the ring every tick
    // below (plus once more at stop, so no tail audio is lost to the
    // tick cadence). Live captions and the stop path read this, same
    // as when the mic callback appended to it directly.
    let audio_buf: Arc<parking_lot::Mutex<Vec<f32>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));

    // ── Live captions for long sessions ─────────────────────────────
    // Detached worker: decodes a sliding window of the buffer and emits
    // PartialTranscript events once the recording passes the config
    // threshold. Self-terminates when `stop` is set (stop or cancel);
    // decodes share the recognizer cache with the final decode, whose
    // lock serializes the two so they never overlap.
    spawn_live_captions_worker(
        audio_buf.clone(),
        mic_sr,
        signal.clone(),
        tx.clone(),
        model_paths.clone(),
        post_processor.clone(),
        num_threads,
    );

    // ── Wait for stop signal ────────────────────────────────────────
    // Condvar wake (canario-b1g): `RecordingHandle::stop`/`cancel`
    // notify this wait directly, so the loop reacts in microseconds
    // instead of finishing a 50 ms poll sleep — `stop_observed` below
    // now measures the true wake latency. The `wait_for_stop` timeout
    // only paces `Event::AudioLevel`, which stays on its original
    // 50 ms cadence: each timeout is one level tick, exactly like the
    // old loop body before its sleep.
    let mut taken = session.start_offset();
    let mut prev_written = session.written();
    let mut last_progress = Instant::now();
    let mut reopens_done = 0u32;
    let mut abort_reason: Option<String> = None;
    let mut log_timer = Instant::now();
    loop {
        // Drain the warm ring into this recording's buffer. `taken`
        // walks forward from the press offset; overrun means the loop
        // stalled past the ring depth (early audio lost — logged).
        let (chunk, overran) = session.drain_new(&mut taken);
        if overran {
            tracing::warn!("Warm-mic ring overran the drain cursor — early audio lost");
        }
        audio_buf.lock().extend_from_slice(&chunk);

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

        if log_timer.elapsed() >= Duration::from_secs(3) {
            tracing::info!(
                "Recording: {} samples ({:.1}s), RMS={:.3}",
                buf_len,
                buf_len as f64 / mic_sr as f64,
                rms,
            );
            log_timer = Instant::now();
        }

        // ── Device-failure fallback (canario-vew) ───────────────────
        // An error callback plus a silent stream means the device is
        // gone (hotplug). Correctness over warmth: reopen once and
        // keep capturing into the same recording; give up after that
        // and transcribe the partial audio.
        let written_now = session.written();
        if written_now != prev_written {
            prev_written = written_now;
            last_progress = Instant::now();
        }
        let stalled_ms = last_progress.elapsed().as_millis() as u64;
        if mic_warm::mid_recording_verdict(session.error_delta() > 0, stalled_ms)
            == MicVerdict::Reopen
        {
            if reopens_done >= mic_warm::MAX_REOPENS_PER_RECORDING {
                abort_reason =
                    Some("microphone stream failed twice; audio may be incomplete".into());
                break;
            }
            match session.reopen_after_error() {
                Ok(()) => {
                    reopens_done += 1;
                    last_progress = Instant::now();
                }
                Err(e) => {
                    abort_reason = Some(format!("microphone reopen failed: {}", e));
                    break;
                }
            }
        }

        if signal.wait_for_stop(Duration::from_millis(AUDIO_LEVEL_INTERVAL_MS)) {
            break;
        }
    }

    timing::mark("stop_observed");

    // Final drain: the mic callback no longer appends to `audio_buf`
    // directly (it feeds the ring), so pull whatever arrived since the
    // last tick before cloning for transcription.
    let (tail, _) = session.drain_new(&mut taken);
    audio_buf.lock().extend_from_slice(&tail);

    if let Some(reason) = &abort_reason {
        tracing::error!("Aborting recording: {}", reason);
        // Also ends the live-captions worker, which waits on `stop`.
        signal.signal_stop();
        let _ = tx.send(Event::Error {
            message: format!("Recording interrupted: {}", reason),
        });
    }

    // ── Stop: park the mic, transcribe from the offset ──────────────
    // The stream is not released here — it parks for the warm window
    // (or releases now if the default device changed); see mic_warm.
    // Cancel path: discard the buffer — skip the stop beep, resample,
    // and transcription entirely.
    session.finish();

    if signal.is_cancelled() {
        audio_buf.lock().clear();
        tracing::info!("Recording cancelled — discarding captured audio");
        timing::mark("recording_cancelled");
        let _ = tx.send(Event::RecordingCancelled);
        return Ok(());
    }

    let raw_audio = audio_buf.lock().clone();
    timing::mark("audio_cloned");

    // Kick off the stop double-beep. Non-blocking (bead canario-9mw):
    // the whole sequence — tones + inter-tone gap — runs on its own
    // detached thread, so resample + decode below start immediately.
    // The marks now measure only the (microsecond) handoff.
    if sound_effects {
        timing::mark("beep_stop_start");
        crate::audio::effects::beep_stop(sound_volume);
        timing::mark("beep_stop_done");
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

    // ── Transcription begins (canario-dmp.9) ───────────────────────
    // The capture is now committed to the decode pipeline (resample →
    // decode → transform). Signal it BEFORE the decode work starts so
    // frontends can flip their overlay to a transcribing state;
    // deriving that state from a successful stop response is equally
    // valid — both paths are documented in PRD-ELECTRON.md Appendix B.
    let _ = tx.send(Event::TranscriptionStarted);

    // ── Resample to 16kHz if needed ─────────────────────────────────
    let audio_16k = if mic_sr != 16000 {
        tracing::info!("Resampling {}Hz → 16000Hz...", mic_sr);
        timing::mark("resample_start");
        let out = crate::inference::resample::resample(&raw_audio, mic_sr, 16000)?;
        timing::mark("resample_done");
        out
    } else {
        raw_audio
    };

    // ── Transcribe (recognizer is cached across recordings) ─────────
    let audio_secs = audio_16k.len() as f64 / 16000.0;
    tracing::info!("Transcribing {:.1}s of audio...", audio_secs);

    // The closure returns the raw transcript so post-processing and the
    // fgm.3 transform pipeline run AFTER the recognizer cache lock is
    // released — a provider call (up to `timeout_ms`, D5d) must never
    // hold the decode cache hostage (live captions share it).
    let transcript = with_recognizer(&model_paths, num_threads, |recognizer| {
        let rec_stream = recognizer.create_stream();
        rec_stream.accept_waveform(16000, &audio_16k);
        let decode_start = std::time::Instant::now();
        timing::mark("decode_start");
        recognizer.decode(&rec_stream);
        timing::mark("decode_end");
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

        rec_stream
            .get_result()
            .map(|result| result.text.trim().to_string())
            .filter(|text| !text.is_empty())
    })?;

    match transcript {
        Some(raw_transcript) => {
            let transcript = post_processor.process(&raw_transcript);
            tracing::info!(
                "✅ Transcription ({} chars): {}",
                transcript.chars().count(),
                transcript
            );
            emit_transcription_ready(&tx, transcript, duration, &transform);
        }
        None => tracing::info!("(no speech detected)"),
    }

    let _ = tx.send(Event::RecordingStopped);
    timing::mark("recording_stopped");
    Ok(())
}

// ── Transformation pipeline (fgm.3, fgm.1 D3/D5d) ─────────────────────
//
// Runs between the final decode and the TranscriptionReady event: the
// event's `text` is the transformed transcript (what gets pasted and
// stored — D3), with the raw transcript riding along as `raw_text`
// only when it differs. On ANY failure — provider unreachable, HTTP
// error, timeout, even failing to build the throwaway runtime — the
// raw transcript flows on (D5d): dictation never blocks, audio is
// never lost. Failures surface as a debug log plus the event's
// `transform_failed` flag (fgm.4's fallback affordance); no Event
// variant was added for this — one flag on the existing event keeps
// the protocol minimal.

/// Map [`crate::transform::apply_transformation`]'s reserved
/// infrastructure error (the tokio runtime build failing): D5d applies
/// here too — even that falls back to the raw transcript instead of
/// blocking the dictation.
fn infra_error_outcome(transcript: &str, err: anyhow::Error) -> crate::transform::TransformOutcome {
    crate::transform::TransformOutcome::Raw {
        text: transcript.to_owned(),
        reason: crate::transform::RawReason::Provider(err.to_string()),
    }
}

/// Transform `transcript` (the post-processed dictation — the text
/// that would be pasted without a transformation) when `transform` is
/// enabled, then emit [`Event::TranscriptionReady`].
///
/// The focused app and the in-memory credential are read here, at
/// transform time: the paste that follows lands in whatever window is
/// focused NOW, so the rule match and the paste target agree, and the
/// credential store (D2) needs no threading through the recording API.
fn emit_transcription_ready(
    tx: &std::sync::mpsc::Sender<Event>,
    transcript: String,
    duration_secs: f64,
    transform: &TransformSettings,
) {
    let mut text = transcript.clone();
    let mut raw_text = None;
    let mut transform_failed = false;

    if transform.enabled {
        let focused = crate::transform::focused_app();
        let credential = crate::transform::credential();
        timing::mark("transform_start");
        let outcome = crate::transform::apply_transformation(
            transform,
            &transform.rules,
            credential.as_deref(),
            &transcript,
            focused.as_deref(),
        )
        // D5d applies to the runtime too: even an infrastructure error
        // falls back to raw instead of blocking the dictation.
        .unwrap_or_else(|e| infra_error_outcome(&transcript, e));
        timing::mark("transform_done");

        match outcome {
            crate::transform::TransformOutcome::Transformed(transformed) => {
                // raw_text only when the transformation actually changed
                // something (D3: no storage doubling for no-ops).
                if transformed != transcript {
                    raw_text = Some(transcript.clone());
                    text = transformed;
                }
            }
            crate::transform::TransformOutcome::Raw { text: raw, reason } => {
                text = raw;
                if reason.is_failure() {
                    transform_failed = true;
                }
                tracing::debug!("transformation skipped: {}", reason);
            }
        }
    }

    timing::mark("transcript_ready");
    let _ = tx.send(Event::TranscriptionReady {
        text,
        duration_secs,
        raw_text,
        transform_failed,
    });
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
    /// Shared with the capture loop — `wait_for_stop` wakes it the same
    /// instant a stop/cancel fires (canario-b1g), instead of re-polling
    /// the flag between decode cadence slices.
    stop: Arc<StopSignal>,
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
    stop: Arc<StopSignal>,
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
        // Wait out the cadence in small slices, each a condvar wait on
        // the shared stop signal — a stop (or cancel) interrupts the
        // worker within the slice's wake latency rather than being
        // noticed at the next 150 ms poll.
        let mut waited_ms = 0;
        while waited_ms < LIVE_INTERVAL_MS
            && !stop.wait_for_stop(Duration::from_millis(LIVE_POLL_MS))
        {
            waited_ms += LIVE_POLL_MS;
        }
        if stop.is_stopped() {
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
                if !stop.is_stopped() && !text.is_empty() && text != last_text {
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
        timing::mark("recognizer_load_start");
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
        timing::mark("recognizer_load_done");
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
    if let Err(reason) = crate::inference::validate::validate_model_files(model_paths) {
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

    timing::mark("recognizer_load_start");
    // Validation ran above, so `Err` here is at most a race (a file
    // replaced between the check and the load) — same handling as a
    // plain load failure: leave the cache untouched.
    let outcome = match store_recognizer(&mut cache, model_paths, num_threads) {
        Ok(Some(())) => PrewarmOutcome::Warmed,
        Ok(None) | Err(_) => PrewarmOutcome::LoadFailed,
    };
    timing::mark("recognizer_load_done");
    outcome
}

// ── Model file validation ────────────────────────────────────────────
// Validation lives in `crate::inference::validate`, shared with
// `TranscriptionEngine::load_model`. sherpa-onnx is not defensive at
// load time: its C++ `ReadTokens` (symbol-table.cc, pinned v1.12.38)
// calls SHERPA_ONNX_EXIT on input outside its line grammar, killing
// the *whole app* instead of surfacing an error. Every path that
// builds a recognizer (dictation via `with_recognizer`, background
// prewarm via `prewarm_once`) goes through `create_recognizer`, so
// validating the files there turns a bad tokens file into a regular
// error.

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
    crate::inference::validate::validate_model_files(paths).map_err(anyhow::Error::msg)?;

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
    use crate::config::{TransformProvider, TransformSettings};
    use crate::inference::validate::test_support::{
        model_paths_in, write_garbage_onnx_model, write_model_files, MINIMAL_ONNX,
    };
    use crate::transform::test_support::{spawn_black_hole_server, spawn_one_shot_server};
    use crate::transform::{TransformOutcome, TransformRule};

    #[test]
    #[ignore = "requires CANARIO_TEST_MODEL_DIR pointing to a downloaded model"]
    fn cached_model_passes_recognizer_creation_guard() {
        let dir = std::env::var_os("CANARIO_TEST_MODEL_DIR").expect("set CANARIO_TEST_MODEL_DIR");
        let paths = model_paths_in(std::path::Path::new(&dir));
        let recognizer = create_recognizer(&paths, 2).expect("valid model should pass validation");
        assert!(recognizer.is_some(), "valid model should load in sherpa");
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

    // ── StopSignal wake semantics (canario-b1g) ──────────────────────
    // The condvar replaced a 50 ms flag poll, so the contract to pin
    // down is: a signal wakes a waiting thread (instead of being
    // noticed on the next poll tick or, worse, a missed-notify
    // timeout), and a wait without a signal keeps ticking (timeout)
    // so AudioLevel keeps its cadence.

    /// A stop signalled *before* the wait must return immediately,
    /// without touching the timeout — the capture thread's first wait
    /// after a (theoretical) pre-signalled start must not sleep.
    #[test]
    fn stop_signal_presignalled_returns_without_waiting() {
        let signal = StopSignal::new();
        signal.signal_stop();

        let start = Instant::now();
        let stopped = signal.wait_for_stop(Duration::from_secs(30));

        assert!(stopped, "flag already set — wait must return true");
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "returned without waiting out the 30 s timeout"
        );
    }

    /// The core of the fix: a stop signalled *while* the capture thread
    /// is waiting must wake it well inside the 5 s timeout a missed
    /// notify would hit (in practice the wake is microseconds; the
    /// benchmark gates the millisecond number, this test gates the
    /// mechanism).
    #[test]
    fn stop_signal_wakes_a_waiting_thread() {
        let signal = Arc::new(StopSignal::new());
        let waiter = {
            let signal = signal.clone();
            std::thread::spawn(move || {
                let start = Instant::now();
                let stopped = signal.wait_for_stop(Duration::from_secs(5));
                (stopped, start.elapsed())
            })
        };

        std::thread::sleep(Duration::from_millis(50)); // let it block
        signal.signal_stop();
        let (stopped, elapsed) = waiter.join().unwrap();

        assert!(stopped, "notify must wake the waiter with true");
        assert!(
            elapsed < Duration::from_secs(1),
            "woke in {:?} — far under the 5 s missed-notify timeout",
            elapsed
        );
        assert!(signal.is_stopped());
        assert!(!signal.is_cancelled(), "plain stop must not discard");
    }

    /// Cancel takes the same wake path AND marks the recording for
    /// discard, so the loop can skip transcription.
    #[test]
    fn stop_signal_cancel_wakes_and_marks_discard() {
        let signal = Arc::new(StopSignal::new());
        let waiter = {
            let signal = signal.clone();
            std::thread::spawn(move || signal.wait_for_stop(Duration::from_secs(5)))
        };

        std::thread::sleep(Duration::from_millis(50));
        signal.signal_cancel();
        let stopped = waiter.join().unwrap();

        assert!(stopped, "cancel must wake the waiter like stop");
        assert!(signal.is_stopped());
        assert!(signal.is_cancelled(), "cancel must flag discard");
    }

    /// No signal → timeout returns `false` after (roughly) the full
    /// timeout: this is the AudioLevel tick, so returning early (a
    /// busy-spin) or late (a doubled sleep) both break the cadence.
    #[test]
    fn stop_signal_timeout_returns_false_after_the_timeout() {
        let signal = StopSignal::new();

        let start = Instant::now();
        let stopped = signal.wait_for_stop(Duration::from_millis(50));

        assert!(!stopped, "no signal — must time out with false");
        assert!(
            start.elapsed() >= Duration::from_millis(40),
            "waited {:?} — should honor (not shrink) the 50 ms tick",
            start.elapsed()
        );
    }

    /// The cadence pattern the capture loop runs: several timed-out
    /// ticks, then a signal mid-sequence wakes the very next wait —
    /// the waiter must not need to re-arm between ticks.
    #[test]
    fn stop_signal_times_out_then_wakes_on_the_next_wait() {
        let signal = Arc::new(StopSignal::new());

        assert!(!signal.wait_for_stop(Duration::from_millis(25)), "tick 1");
        assert!(!signal.wait_for_stop(Duration::from_millis(25)), "tick 2");

        let signaler = {
            let signal = signal.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(10));
                signal.signal_stop();
            })
        };
        assert!(
            signal.wait_for_stop(Duration::from_secs(5)),
            "signal during the third wait must wake it"
        );
        signaler.join().unwrap();
    }

    // ── Transformation pipeline emission (fgm.3 D3/D5d) ──────────────
    //
    // These drive `emit_transcription_ready` — the exact function the
    // recording loop calls between the final decode and the event —
    // with a live default rule so they are independent of the test
    // machine's session (Wayland focused=None, X11 focused=Some both
    // match a leading default rule). A full through-the-sidecar
    // recording cannot be driven hermetically (it needs a microphone,
    // a model download and actual speech); this seam is the pipeline's
    // real production path minus the decode.

    /// Everything on except the provider, which each test points where
    /// it needs. The default rule leads so the focused app — whatever
    /// this machine detects — always matches.
    fn transform_settings(base_url: String, timeout_ms: u64) -> TransformSettings {
        TransformSettings {
            enabled: true,
            provider: TransformProvider {
                base_url,
                model: "llama3".into(),
            },
            timeout_ms,
            rules: vec![
                TransformRule {
                    app_match: String::new(),
                    instruction: "be terse".into(),
                },
                TransformRule {
                    app_match: "never-matches-xyzzy".into(),
                    instruction: "unused".into(),
                },
            ],
        }
    }

    /// D5d, the whole point: transform ENABLED but the provider is
    /// unreachable (127.0.0.1:9 — the discard port, nothing listens)
    /// with a short timeout — the TranscriptionReady event STILL flows,
    /// with the raw transcript, no raw_text (nothing changed) and the
    /// failure flag set for the fgm.4 fallback affordance.
    #[test]
    fn emit_transcription_ready_survives_an_unreachable_provider() {
        let (tx, rx) = std::sync::mpsc::channel();
        let settings = transform_settings("http://127.0.0.1:9/v1".into(), 250);

        let started = Instant::now();
        emit_transcription_ready(&tx, "hello world".into(), 1.25, &settings);

        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Event::TranscriptionReady {
                text,
                duration_secs,
                raw_text,
                transform_failed,
            }) => {
                assert_eq!(text, "hello world", "the raw transcript must flow on");
                assert_eq!(duration_secs, 1.25);
                assert_eq!(raw_text, None, "nothing was transformed");
                assert!(transform_failed, "D5d: the failure must be flagged");
            }
            other => panic!("expected TranscriptionReady, got {other:?}"),
        }
        // The refused connection (plus the clamped 250 ms timeout
        // window) must not wedge the pipeline.
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    /// The happy path (D3): the provider's text becomes the event's
    /// `text`, the pre-transform transcript rides along as `raw_text`
    /// (only because it differs), and no failure is flagged.
    #[test]
    fn emit_transcription_ready_emits_transformed_text_with_raw() {
        let (addr, captured) =
            spawn_one_shot_server(r#"{"choices":[{"message":{"content":"Hello, world."}}]}"#);
        let (tx, rx) = std::sync::mpsc::channel();
        let settings = transform_settings(format!("http://{addr}/v1"), 2000);

        emit_transcription_ready(&tx, "hello wrld".into(), 2.0, &settings);

        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Event::TranscriptionReady {
                text,
                raw_text,
                transform_failed,
                ..
            }) => {
                assert_eq!(text, "Hello, world.");
                assert_eq!(raw_text.as_deref(), Some("hello wrld"));
                assert!(!transform_failed);
            }
            other => panic!("expected TranscriptionReady, got {other:?}"),
        }
        // The provider really was asked: the request carried the
        // rendered instruction + the transcript (D5b payload).
        let (_headers, body) = captured.recv_timeout(Duration::from_secs(5)).unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("be terse"), "system message: {system}");
        assert_eq!(body["messages"][1]["content"], "hello wrld");
    }

    /// D5a: with the transform disabled the event is the plain raw
    /// dictation — no provider call happens at all (proven against a
    /// black-hole server: a call would have cost the 250 ms timeout),
    /// no new fields fire.
    #[test]
    fn emit_transcription_ready_disabled_makes_no_provider_call() {
        let addr = spawn_black_hole_server();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut settings = transform_settings(format!("http://{addr}/v1"), 250);
        settings.enabled = false;

        let started = Instant::now();
        emit_transcription_ready(&tx, "raw dictation".into(), 1.0, &settings);

        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Event::TranscriptionReady {
                text,
                raw_text,
                transform_failed,
                ..
            }) => {
                assert_eq!(text, "raw dictation");
                assert_eq!(raw_text, None);
                assert!(!transform_failed);
            }
            other => panic!("expected TranscriptionReady, got {other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "a disabled transform must not pay the provider timeout (took {:?})",
            started.elapsed()
        );
    }

    /// A transformation that returns the transcript unchanged must not
    /// double-store it (D3): raw_text stays `None` when nothing
    /// differs, and the event is not flagged as failed.
    #[test]
    fn emit_transcription_ready_noop_transformation_carries_no_raw() {
        let (addr, _captured) =
            spawn_one_shot_server(r#"{"choices":[{"message":{"content":"same text"}}]}"#);
        let (tx, rx) = std::sync::mpsc::channel();
        let settings = transform_settings(format!("http://{addr}/v1"), 2000);

        emit_transcription_ready(&tx, "same text".into(), 1.0, &settings);

        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Event::TranscriptionReady {
                text,
                raw_text,
                transform_failed,
                ..
            }) => {
                assert_eq!(text, "same text");
                assert_eq!(raw_text, None, "nothing changed — no raw_text");
                assert!(!transform_failed);
            }
            other => panic!("expected TranscriptionReady, got {other:?}"),
        }
    }

    /// D5d, timeout flavor: a provider that accepts but never answers
    /// is cut off at the timeout and the raw transcript still flows —
    /// dictation waits at most `timeout_ms`, never forever.
    #[test]
    fn emit_transcription_ready_times_out_to_raw() {
        let addr = spawn_black_hole_server();
        let (tx, rx) = std::sync::mpsc::channel();
        let settings = transform_settings(format!("http://{addr}/v1"), 250);

        let started = Instant::now();
        emit_transcription_ready(&tx, "hello world".into(), 1.0, &settings);

        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Event::TranscriptionReady {
                text,
                raw_text,
                transform_failed,
                ..
            }) => {
                assert_eq!(text, "hello world");
                assert_eq!(raw_text, None);
                assert!(transform_failed, "a timeout is a flagged failure");
            }
            other => panic!("expected TranscriptionReady, got {other:?}"),
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(200),
            "returned after {elapsed:?} — before the timeout could fire"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "returned after {elapsed:?} — the timeout did not bound the pipeline"
        );
    }

    /// The infrastructure-error mapping (`apply_transformation`'s
    /// reserved `Err` — the tokio runtime build failing): even that
    /// maps to a flagged raw event, never a lost dictation. The Err
    /// itself cannot be forced hermetically, so this pins the exact
    /// `infra_error_outcome` helper the emit path wires in.
    #[test]
    fn infra_error_outcome_maps_to_a_flagged_raw_event() {
        let outcome = infra_error_outcome(
            "hello world",
            anyhow::anyhow!("failed to init tokio runtime"),
        );
        let TransformOutcome::Raw { text, reason } = outcome else {
            panic!("infra errors map to Raw");
        };
        assert_eq!(text, "hello world");
        assert!(reason.is_failure());
        assert_eq!(
            reason.to_string(),
            "transform failed: failed to init tokio runtime"
        );
    }
}
