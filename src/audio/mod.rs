use anyhow::Result;
use ringbuf::HeapRb;
use ringbuf::traits::{Split, Consumer, Producer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{error, info};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const SAMPLE_RATE: u32 = 16_000;
const RING_BUFFER_SECONDS: f64 = 2.0;

/// Raw audio sample
pub type Sample = f32;

/// Shared state for the audio capture system
pub struct AudioCapture {
    running: Arc<AtomicBool>,
    recording: Arc<AtomicBool>,
    producer: Option<ringbuf::HeapProd<Sample>>,
    consumer: Option<ringbuf::HeapCons<Sample>>,
}

impl AudioCapture {
    pub fn new() -> Self {
        let capacity = (SAMPLE_RATE as f64 * RING_BUFFER_SECONDS) as usize;
        let rb = HeapRb::<Sample>::new(capacity);
        let (prod, cons) = rb.split();

        Self {
            running: Arc::new(AtomicBool::new(false)),
            recording: Arc::new(AtomicBool::new(false)),
            producer: Some(prod),
            consumer: Some(cons),
        }
    }

    /// Start the microphone capture loop
    pub fn start(&mut self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No input device available"))?;

        info!("Using input device: {}", device.name().unwrap_or_default());

        let supported_config = device.default_input_config()?;
        let mic_sample_rate = supported_config.sample_rate().0;
        let channels = supported_config.channels() as usize;

        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let recording = self.recording.clone();

        let mut producer = self.producer.take().expect("producer already taken");

        let stream = match supported_config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &supported_config.into(),
                move |data: &[f32], _| {
                    if !running.load(Ordering::SeqCst) {
                        return;
                    }
                    // Convert to mono
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                        .collect();
                    if recording.load(Ordering::SeqCst) {
                        for &sample in &mono {
                            let _ = producer.try_push(sample);
                        }
                    }
                },
                |err| error!("Audio capture error: {}", err),
                None,
            )?,
            cpal::SampleFormat::I16 => device.build_input_stream(
                &supported_config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if !running.load(Ordering::SeqCst) {
                        return;
                    }
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|frame| {
                            frame.iter().map(|&s| s as f32 / i16::MAX as f32).sum::<f32>()
                                / channels as f32
                        })
                        .collect();
                    if recording.load(Ordering::SeqCst) {
                        for &sample in &mono {
                            let _ = producer.try_push(sample);
                        }
                    }
                },
                |err| error!("Audio capture error: {}", err),
                None,
            )?,
            _ => anyhow::bail!("Unsupported sample format"),
        };

        stream.play()?;
        info!("Audio capture started at {}Hz", mic_sample_rate);

        // Keep the stream alive
        std::mem::forget(stream);

        Ok(())
    }

    /// Start recording audio into the buffer
    pub fn start_recording(&mut self) {
        self.recording.store(true, Ordering::SeqCst);
        // Clear the ring buffer consumer
        if let Some(ref mut consumer) = self.consumer.as_mut() {
            let mut discard = vec![0.0f32; 4096];
            while consumer.pop_slice(&mut discard) > 0 {}
        }
        info!("Recording started");
    }

    /// Stop recording and return the captured audio as 16kHz mono f32 samples
    pub fn stop_recording(&mut self) -> Vec<Sample> {
        self.recording.store(false, Ordering::SeqCst);

        let mut audio = Vec::new();
        if let Some(ref mut consumer) = self.consumer {
            let mut chunk = vec![0.0f32; 4096];
            loop {
                let n = consumer.pop_slice(&mut chunk);
                if n == 0 {
                    break;
                }
                audio.extend_from_slice(&chunk[..n]);
            }
        }

        info!(
            "Recording stopped, captured {} samples ({:.2}s)",
            audio.len(),
            audio.len() as f64 / SAMPLE_RATE as f64
        );
        audio
    }

    /// Stop the capture system entirely
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::SeqCst)
    }
}

/// Save audio samples to a WAV file at 16kHz mono
pub fn save_wav(path: &std::path::Path, samples: &[Sample]) -> Result<()> {
    use std::io::Write;
    let num_samples = samples.len() as u32;
    let data_size = num_samples * 2; // 16-bit samples
    let file_size = 36 + data_size;

    let mut out = std::fs::File::create(path)?;
    // WAV header
    out.write_all(b"RIFF")?;
    out.write_all(&(file_size).to_le_bytes())?;
    out.write_all(b"WAVE")?;
    out.write_all(b"fmt ")?;
    out.write_all(&16u32.to_le_bytes())?; // chunk size
    out.write_all(&1u16.to_le_bytes())?; // PCM
    out.write_all(&1u16.to_le_bytes())?; // mono
    out.write_all(&SAMPLE_RATE.to_le_bytes())?;
    let byte_rate = SAMPLE_RATE * 2; // 16-bit mono
    out.write_all(&byte_rate.to_le_bytes())?;
    out.write_all(&2u16.to_le_bytes())?; // block align
    out.write_all(&16u16.to_le_bytes())?; // bits per sample
    out.write_all(b"data")?;
    out.write_all(&data_size.to_le_bytes())?;

    for &sample in samples {
        let s = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.write_all(&s.to_le_bytes())?;
    }

    Ok(())
}
