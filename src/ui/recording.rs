/// Recording engine for the GUI app.
///
/// Simple approach: record raw audio while active, transcribe on stop.
/// VAD streaming is great for the CLI but for a GUI with explicit
/// start/stop buttons, capturing everything and transcribing at the
/// end is more reliable and matches the user's mental model.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig};

use crate::ui::AppMessage;
use crate::inference::postprocess::PostProcessor;

/// Shared state that the background recording thread checks to know
/// when to stop.
pub struct RecordingHandle {
    stop: Arc<AtomicBool>,
}

impl RecordingHandle {
    /// Signal the recording thread to stop
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Start recording from the microphone.
///
/// Captures audio until `RecordingHandle::stop()` is called, then
/// transcribes the entire buffer, applies post-processing, and sends
/// the result via `tx`.
pub fn start_recording(
    model_dir: PathBuf,
    tx: std::sync::mpsc::Sender<AppMessage>,
    post_processor: PostProcessor,
    sound_effects: bool,
) -> anyhow::Result<RecordingHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    // Play start beep
    if sound_effects {
        crate::audio::effects::beep_start();
    }

    std::thread::spawn(move || {
        tracing::info!("Recording thread starting...");
        if let Err(e) = recording_loop(model_dir, tx.clone(), stop_clone, &post_processor) {
            tracing::error!("Recording thread error: {}", e);
            let _ = tx.send(AppMessage::RecordingError(format!("{}", e)));
            let _ = tx.send(AppMessage::RecordingStopped);
        }
    });

    Ok(RecordingHandle { stop })
}

/// The main recording loop — runs in a background thread.
fn recording_loop(
    model_dir: PathBuf,
    tx: std::sync::mpsc::Sender<AppMessage>,
    stop: Arc<AtomicBool>,
    post_processor: &PostProcessor,
) -> anyhow::Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    // ── Open mic first (fast) ───────────────────────────────────────
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No input device found"))?;
    let supported = device.default_input_config()?;
    let mic_sr = supported.sample_rate().0;
    let channels = supported.channels() as usize;

    tracing::info!("Recording from '{}' at {}Hz", device.name().unwrap_or_default(), mic_sr);

    // Shared audio buffer — the callback pushes, we read on stop
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
        // Send audio level updates for the indicator
        let buf_snapshot = audio_buf.lock();
        let buf_len = buf_snapshot.len();
        // Compute RMS from the last ~4000 samples (roughly last 100ms at 44100Hz)
        let recent_start = buf_len.saturating_sub(4000);
        let recent = &buf_snapshot[recent_start..];
        let rms = if recent.is_empty() {
            0.0
        } else {
            (recent.iter().map(|s| s * s).sum::<f32>() / recent.len() as f32).sqrt()
        };
        drop(buf_snapshot);

        let level = (rms * 5.0).min(1.0) as f64;
        let _ = tx.send(AppMessage::AudioLevel(level));

        // Periodic diagnostic
        if log_timer.elapsed() >= std::time::Duration::from_secs(3) {
            tracing::info!(
                "Recording: {} samples ({:.1}s) captured, RMS={:.3}",
                buf_len,
                buf_len as f64 / mic_sr as f64,
                rms,
            );
            log_timer = std::time::Instant::now();
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // ── Stop: grab the audio and release the mic ────────────────────
    let raw_audio = audio_buf.lock().clone();
    drop(stream); // release mic

    let duration = raw_audio.len() as f64 / mic_sr as f64;
    tracing::info!("Stopped. Captured {} samples ({:.1}s)", raw_audio.len(), duration);

    if duration < 0.2 {
        tracing::warn!("Recording too short, nothing to transcribe");
        let _ = tx.send(AppMessage::RecordingStopped);
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

    // Save debug WAV so we can test offline
    let debug_wav = std::env::temp_dir().join("canario_debug.wav");
    if let Err(e) = save_wav(&debug_wav, &audio_16k, 16000) {
        tracing::warn!("Failed to save debug WAV: {}", e);
    } else {
        tracing::info!("Debug WAV saved to {:?}", debug_wav);
    }

    let rec_stream = recognizer.create_stream();
    rec_stream.accept_waveform(16000, &audio_16k);
    recognizer.decode(&rec_stream);

    if let Some(result) = rec_stream.get_result() {
        let raw_text = result.text.trim().to_string();
        if !raw_text.is_empty() {
            // Apply post-processing (word remappings / removals)
            let text = post_processor.process(&raw_text);
            tracing::info!("✅ Transcription: {}", text);
            let _ = tx.send(AppMessage::TranscriptionReady(text));
        } else {
            tracing::info!("(no speech detected)");
        }
    }

    let _ = tx.send(AppMessage::RecordingStopped);
    Ok(())
}

/// Create the ASR recognizer from model files
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

/// Save f32 samples as a 16-bit PCM WAV file
fn save_wav(path: &PathBuf, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
    let data_size = (samples.len() * 2) as u32;
    let file_size = 36 + data_size;

    let mut out = std::fs::File::create(path)?;
    use std::io::Write;

    // RIFF header
    out.write_all(b"RIFF")?;
    out.write_all(&(file_size).to_le_bytes())?;
    out.write_all(b"WAVE")?;

    // fmt chunk
    out.write_all(b"fmt ")?;
    out.write_all(&16u32.to_le_bytes())?; // chunk size
    out.write_all(&1u16.to_le_bytes())?;  // PCM format
    out.write_all(&1u16.to_le_bytes())?;  // mono
    out.write_all(&sample_rate.to_le_bytes())?;
    let byte_rate = sample_rate * 2; // 1 channel * 2 bytes/sample
    out.write_all(&(byte_rate).to_le_bytes())?;
    out.write_all(&2u16.to_le_bytes())?;  // block align
    out.write_all(&16u16.to_le_bytes())?; // bits per sample

    // data chunk
    out.write_all(b"data")?;
    out.write_all(&data_size.to_le_bytes())?;

    for &sample in samples {
        let s = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.write_all(&s.to_le_bytes())?;
    }

    Ok(())
}

/// Simple linear interpolation resampling
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
