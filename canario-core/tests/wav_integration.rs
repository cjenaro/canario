//! Integration tests for the recording/transcription input pipeline:
//! `read_wav` (public API) + resampling, driven by synthetic WAV files
//! written to a temp dir. No audio hardware and no model download
//! required.
//!
//! The ignored model-backed smoke test can be run against a cached model:
//! `CANARIO_TEST_MODEL_DIR=/path/to/model cargo test -p canario-core --test
//! wav_integration transcribes_synthetic_wav_with_cached_model -- --ignored`

use std::path::Path;

use canario_core::read_wav;

/// Write a minimal 44-byte-header PCM/float WAV file, matching what
/// `read_wav` parses (it reads a fixed 44-byte header, then treats the
/// rest of the file as interleaved sample data).
fn write_wav(path: &Path, sample_rate: u32, channels: u16, bits_per_sample: u16, data: &[u8]) {
    let format_tag: u16 = if bits_per_sample == 32 { 3 } else { 1 }; // IEEE float vs PCM
    let block_align = channels * bits_per_sample / 8;
    let byte_rate = sample_rate * u32::from(block_align);

    let mut w = Vec::with_capacity(44 + data.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36u32 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&format_tag.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&sample_rate.to_le_bytes());
    w.extend_from_slice(&byte_rate.to_le_bytes());
    w.extend_from_slice(&block_align.to_le_bytes());
    w.extend_from_slice(&bits_per_sample.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(data);
    std::fs::write(path, w).unwrap();
}

/// One second of a 16-bit PCM sine sweep (chirp) from `f_start` to `f_end`.
fn sine_sweep_i16(sample_rate: u32, f_start: f32, f_end: f32) -> Vec<u8> {
    let n = sample_rate as usize;
    let mut data = Vec::with_capacity(n * 2);
    let mut phase = 0f32;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let freq = f_start + (f_end - f_start) * t;
        phase += 2.0 * std::f32::consts::PI * freq / sample_rate as f32;
        let sample = (phase.sin() * 0.5 * i16::MAX as f32) as i16;
        data.extend_from_slice(&sample.to_le_bytes());
    }
    data
}

#[test]
fn reads_16khz_mono_wav_without_resampling() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sweep16k.wav");
    let data = sine_sweep_i16(16000, 200.0, 4000.0);
    write_wav(&path, 16000, 1, 16, &data);

    let (samples, rate) = read_wav(&path).unwrap();

    assert_eq!(rate, 16000);
    assert_eq!(samples.len(), 16000, "one second at 16kHz");
    assert!(
        samples.iter().all(|s| (-1.0..=1.0).contains(s)),
        "i16 PCM normalized to [-1, 1]"
    );
    // A sine sweep has real energy (not silence, not clipping to DC).
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    assert!(rms > 0.2, "expected audible signal, rms={rms}");
}

#[test]
fn converts_stereo_wav_to_mono() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("stereo.wav");
    let mono = sine_sweep_i16(16000, 440.0, 440.0);
    // Interleave L/R with identical samples.
    let mut stereo = Vec::with_capacity(mono.len() * 2);
    for frame in mono.chunks_exact(2) {
        stereo.extend_from_slice(frame);
        stereo.extend_from_slice(frame);
    }
    write_wav(&path, 16000, 2, 16, &stereo);

    let (samples, rate) = read_wav(&path).unwrap();

    assert_eq!(rate, 16000);
    assert_eq!(samples.len(), 16000, "stereo frames averaged to mono");
}

#[test]
fn resamples_44100hz_wav_to_16000hz() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sweep44k.wav");
    let data = sine_sweep_i16(44100, 200.0, 4000.0);
    write_wav(&path, 44100, 1, 16, &data);

    let (samples, rate) = read_wav(&path).unwrap();

    assert_eq!(rate, 16000, "read_wav reports the resampled rate");
    let diff = (samples.len() as i64 - 16000).abs();
    assert!(
        diff <= 8,
        "one second at 44.1kHz should resample to ~16000 samples, got {}",
        samples.len()
    );
    // Signal survives the resample with energy intact.
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    assert!(
        rms > 0.2,
        "resampled signal lost too much energy, rms={rms}"
    );
}

#[test]
fn reads_32bit_float_wav() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("float.wav");
    let n = 16000usize;
    let mut data = Vec::with_capacity(n * 4);
    for i in 0..n {
        let s = (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / 16000.0).sin() * 0.25;
        data.extend_from_slice(&s.to_le_bytes());
    }
    write_wav(&path, 16000, 1, 32, &data);

    let (samples, rate) = read_wav(&path).unwrap();

    assert_eq!(rate, 16000);
    assert_eq!(samples.len(), n);
    assert!(samples.iter().all(|s| s.abs() <= 0.25 + f32::EPSILON));
}

#[test]
fn rejects_non_wav_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("not-a-wav.bin");
    std::fs::write(
        &path,
        b"this is definitely not a RIFF/WAVE file, long enough to fill the header".repeat(4),
    )
    .unwrap();

    assert!(read_wav(&path).is_err());
}

#[test]
fn rejects_missing_file() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(read_wav(&tmp.path().join("does-not-exist.wav")).is_err());
}

#[test]
fn rejects_unsupported_bit_depth() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("8bit.wav");
    write_wav(&path, 16000, 1, 8, &[128u8; 1600]);

    assert!(read_wav(&path).is_err());
}

/// Exercise the actual ONNX runtime through both public transcription paths.
/// Synthetic audio has no known transcript, so this checks loading/decoding
/// and agreement between file and sample input, rather than speech accuracy.
#[test]
#[ignore = "requires CANARIO_TEST_MODEL_DIR pointing to a cached Parakeet model"]
fn transcribes_synthetic_wav_with_cached_model() {
    let model_dir = std::env::var_os("CANARIO_TEST_MODEL_DIR")
        .filter(|value| !value.is_empty())
        .expect("set CANARIO_TEST_MODEL_DIR to the directory containing encoder.int8.onnx, decoder.int8.onnx, joiner.int8.onnx and tokens.txt");
    let mut engine = canario_core::TranscriptionEngine::new(model_dir.into(), 2);
    assert!(
        engine.is_model_available(),
        "cached model files are missing"
    );

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("model-smoke.wav");
    write_wav(&path, 16000, 1, 16, &sine_sweep_i16(16000, 200.0, 4000.0));
    assert!(
        engine.transcribe_file(&path).is_err(),
        "model is not loaded yet"
    );

    engine
        .load_model()
        .expect("cached model must load successfully");
    let from_file = engine.transcribe_file(&path).expect("WAV decoding failed");
    let (samples, _) = read_wav(&path).unwrap();
    let from_samples = engine.transcribe(&samples).expect("sample decoding failed");
    assert_eq!(from_file, from_samples, "file and sample input must agree");

    engine.unload();
    assert!(
        engine.transcribe_file(&path).is_err(),
        "unload must release the recognizer"
    );
}
