/// Recording engine — captures audio, transcribes, emits events.
///
/// Simple approach: record raw audio while active, transcribe on stop.
/// Communicates results via the `Sender<Event>` channel.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig};

use crate::event::Event;
use crate::inference::postprocess::PostProcessor;

/// Handle to stop a running recording and track thread completion.
pub struct RecordingHandle {
    stop: Arc<AtomicBool>,
    /// Set to `false` by the thread when it finishes (recording + transcription).
    busy: Arc<AtomicBool>,
}

impl RecordingHandle {
    /// Signal the recording thread to stop.
    pub fn stop(&self) {
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
    model_dir: PathBuf,
    tx: std::sync::mpsc::Sender<Event>,
    post_processor: PostProcessor,
    sound_effects: bool,
) -> anyhow::Result<RecordingHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicBool::new(true));
    let stop_clone = stop.clone();
    let busy_clone = busy.clone();

    // Play start beep
    if sound_effects {
        crate::audio::effects::beep_start();
    }

    std::thread::spawn(move || {
        tracing::info!("Recording thread starting...");
        let result = recording_loop(model_dir, tx.clone(), stop_clone, &post_processor, sound_effects);
        if let Err(e) = &result {
            tracing::error!("Recording thread error: {}", e);
            let _ = tx.send(Event::Error { message: format!("{}", e) });
            let _ = tx.send(Event::RecordingStopped);
        }
        // Mark thread as no longer busy (whether success or failure)
        busy_clone.store(false, Ordering::SeqCst);
    });

    Ok(RecordingHandle { stop, busy })
}

/// The main recording loop — runs in a background thread.
fn recording_loop(
    model_dir: PathBuf,
    tx: std::sync::mpsc::Sender<Event>,
    stop: Arc<AtomicBool>,
    post_processor: &PostProcessor,
    sound_effects: bool,
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

    tracing::info!("Recording from '{}' at {}Hz", device.name().unwrap_or_default(), mic_sr);

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
                            frame.iter().map(|&s| s as f32 / i16::MAX as f32).sum::<f32>()
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
                buf_len, buf_len as f64 / mic_sr as f64, rms,
            );
            log_timer = std::time::Instant::now();
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // ── Stop: grab audio, release mic ───────────────────────────────
    let raw_audio = audio_buf.lock().clone();
    drop(stream);

    // Play stop beep (double-beep) before transcription begins
    if sound_effects {
        crate::audio::effects::beep_stop();
    }

    let duration = raw_audio.len() as f64 / mic_sr as f64;
    tracing::info!("Stopped. Captured {} samples ({:.1}s)", raw_audio.len(), duration);

    if duration < 0.2 {
        tracing::warn!("Recording too short, nothing to transcribe");
        let _ = tx.send(Event::RecordingStopped);
        return Ok(());
    }

    // ── Resample to 16kHz if needed ─────────────────────────────────
    let audio_16k = if mic_sr != 16000 {
        tracing::info!("Resampling {}Hz → 16000Hz...", mic_sr);
        simple_resample(&raw_audio, mic_sr, 16000)
    } else {
        raw_audio
    };

    // ── Load model and transcribe ───────────────────────────────────
    tracing::info!("Loading ASR model...");
    let recognizer = create_recognizer(&model_dir).ok_or_else(|| {
        anyhow::anyhow!(
            "ASR model not found in {:?}. Download from Settings.",
            model_dir
        )
    })?;

    tracing::info!("Transcribing {:.1}s of audio...", audio_16k.len() as f64 / 16000.0);

    let rec_stream = recognizer.create_stream();
    rec_stream.accept_waveform(16000, &audio_16k);
    recognizer.decode(&rec_stream);

    if let Some(result) = rec_stream.get_result() {
        let raw_text = result.text.trim().to_string();
        if !raw_text.is_empty() {
            let text = post_processor.process(&raw_text);
            tracing::info!("✅ Transcription: {}", text);
            let _ = tx.send(Event::TranscriptionReady {
                text,
                duration_secs: duration,
            });
        } else {
            tracing::info!("(no speech detected)");
        }
    }

    let _ = tx.send(Event::RecordingStopped);
    Ok(())
}

/// Create the ASR recognizer from model files.
fn create_recognizer(model_dir: &PathBuf) -> Option<OfflineRecognizer> {
    let encoder = model_dir.join("encoder.int8.onnx");
    let decoder = model_dir.join("decoder.int8.onnx");
    let joiner = model_dir.join("joiner.int8.onnx");
    let tokens = model_dir.join("tokens.txt");

    if !encoder.exists() || !decoder.exists() || !tokens.exists() {
        return None;
    }

    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer.encoder = Some(encoder.to_string_lossy().to_string());
    config.model_config.transducer.decoder = Some(decoder.to_string_lossy().to_string());
    config.model_config.transducer.joiner = Some(joiner.to_string_lossy().to_string());
    config.model_config.tokens = Some(tokens.to_string_lossy().to_string());
    config.model_config.model_type = Some("nemo_transducer".to_string());
    config.model_config.num_threads = 4;
    config.model_config.debug = false;

    OfflineRecognizer::create(&config)
}

/// Simple linear interpolation resampling.
fn simple_resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    let ratio = to_rate as f64 / from_rate as f64;
    let new_len = (samples.len() as f64 * ratio) as usize;
    (0..new_len)
        .map(|i| {
            let src_pos = i as f64 / ratio;
            let idx = src_pos as usize;
            let frac = src_pos - idx as f64;
            let s0 = samples.get(idx).copied().unwrap_or(0.0);
            let s1 = samples.get(idx + 1).copied().unwrap_or(0.0);
            (s0 as f64 * (1.0 - frac) + s1 as f64 * frac) as f32
        })
        .collect()
}
