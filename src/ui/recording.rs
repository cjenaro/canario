/// Recording engine for the GUI app.
///
/// Manages mic capture + VAD + transcription in background threads,
/// communicating results back to the GTK main loop via an mpsc channel.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, SileroVadModelConfig, VadModelConfig,
    VoiceActivityDetector,
};

use crate::ui::AppMessage;

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

    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// Start recording from the microphone with VAD.
///
/// Returns a `RecordingHandle` that can be used to stop recording.
/// Transcription results are sent via `tx` as `AppMessage::TranscriptionReady`.
pub fn start_recording(
    model_dir: PathBuf,
    tx: std::sync::mpsc::Sender<AppMessage>,
) -> anyhow::Result<RecordingHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    std::thread::spawn(move || {
        tracing::info!("Recording thread starting...");
        if let Err(e) = recording_loop(model_dir, tx.clone(), stop_clone) {
            tracing::error!("Recording thread error: {}", e);
            // Send the error as a fake transcription so the user sees something
            let _ = tx.send(AppMessage::TranscriptionReady(format!("❌ Error: {}", e)));
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
) -> anyhow::Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    // ── Load ASR model ──────────────────────────────────────────────
    tracing::info!("Loading ASR model from {:?}...", model_dir);
    let recognizer = create_recognizer(&model_dir)
        .ok_or_else(|| anyhow::anyhow!(
            "Failed to load ASR model. Files not found in {:?}. \
             Download the model from Settings first.", model_dir
        ))?;
    tracing::info!("ASR model loaded");

    // ── Load VAD ────────────────────────────────────────────────────
    tracing::info!("Loading VAD model...");
    let vad = create_vad()?;
    tracing::info!("VAD model loaded");

    // ── Open mic ────────────────────────────────────────────────────
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No input device found"))?;
    let supported = device.default_input_config()?;
    let mic_sr = supported.sample_rate().0;
    let channels = supported.channels() as usize;

    tracing::info!("Recording from '{}' at {}Hz", device.name().unwrap_or_default(), mic_sr);
    tracing::info!("Speak naturally — VAD will detect speech segments");

    // Shared audio buffer
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

    // ── VAD + transcription loop ────────────────────────────────────
    let window_size = 512; // Silero VAD window at 16kHz = 32ms

    while !stop.load(Ordering::SeqCst) {
        // Drain captured audio
        let new_audio: Vec<f32> = {
            let mut buf = audio_buf.lock();
            std::mem::take(&mut *buf)
        };

        if new_audio.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        // Compute audio level for the indicator
        let rms = (new_audio.iter().map(|s| s * s).sum::<f32>() / new_audio.len() as f32).sqrt();
        let level = (rms * 10.0).min(1.0) as f64; // scale up for visibility
        let _ = tx.send(AppMessage::AudioLevel(level));

        // Resample to 16kHz if needed
        let audio_16k = if mic_sr != 16000 {
            simple_resample(&new_audio, mic_sr, 16000)
        } else {
            new_audio
        };

        // Feed to VAD in chunks
        for chunk in audio_16k.chunks(window_size) {
            if chunk.len() == window_size {
                vad.accept_waveform(chunk);
            } else {
                let mut padded = chunk.to_vec();
                padded.resize(window_size, 0.0f32);
                vad.accept_waveform(&padded);
            }

            // Check for complete speech segments
            while !vad.is_empty() {
                if let Some(segment) = vad.front() {
                    let samples = segment.samples().to_vec();
                    let duration = samples.len() as f64 / 16000.0;
                    vad.pop();

                    if duration < 0.1 {
                        continue;
                    }

                    tracing::info!("VAD segment: {:.1}s — transcribing...", duration);

                    let rec_stream = recognizer.create_stream();
                    rec_stream.accept_waveform(16000, &samples);
                    recognizer.decode(&rec_stream);

                    if let Some(result) = rec_stream.get_result() {
                        let text = result.text.trim().to_string();
                        if !text.is_empty() {
                            tracing::info!("Transcription: {}", text);
                            let _ = tx.send(AppMessage::TranscriptionReady(text));
                        }
                    }
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // ── Flush remaining audio ───────────────────────────────────────
    tracing::info!("Recording stopping — flushing remaining audio...");
    drop(stream);

    // Flush VAD — get any remaining speech
    vad.flush();
    while !vad.is_empty() {
        if let Some(segment) = vad.front() {
            let samples = segment.samples().to_vec();
            let duration = samples.len() as f64 / 16000.0;
            vad.pop();

            if duration < 0.1 {
                continue;
            }

            let rec_stream = recognizer.create_stream();
            rec_stream.accept_waveform(16000, &samples);
            recognizer.decode(&rec_stream);

            if let Some(result) = rec_stream.get_result() {
                let text = result.text.trim().to_string();
                if !text.is_empty() {
                    tracing::info!("Final transcription: {}", text);
                    let _ = tx.send(AppMessage::TranscriptionReady(text));
                }
            }
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

/// Create the Silero VAD
fn create_vad() -> anyhow::Result<VoiceActivityDetector> {
    let vad_path = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("canario")
        .join("models")
        .join("silero_vad.onnx");

    let mut vad_config = VadModelConfig::default();
    vad_config.silero_vad = SileroVadModelConfig {
        model: Some(vad_path.to_string_lossy().to_string()),
        threshold: 0.5,
        min_silence_duration: 0.4,
        min_speech_duration: 0.15,
        window_size: 512,
        max_speech_duration: 30.0,
    };
    vad_config.sample_rate = 16000;
    vad_config.num_threads = 1;
    vad_config.debug = false;

    VoiceActivityDetector::create(&vad_config, 60.0)
        .ok_or_else(|| anyhow::anyhow!("Failed to create VAD"))
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
