//! High-quality audio resampling via `rubato` (sinc interpolation).
//!
//! The recognizer expects 16 kHz mono f32; anything else (mic input at
//! 44.1/48 kHz, or a WAV file at a non-16kHz rate) goes through here.

use rubato::audioadapter_buffers::owned::InterleavedOwned;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

/// Input chunk size (frames) used internally by the resampler.
const CHUNK_SIZE: usize = 1024;

/// Resample mono f32 audio from `from_rate` to `to_rate` Hz.
///
/// Equal rates (or empty input) are a no-op fast path that returns the
/// samples unchanged.
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> anyhow::Result<Vec<f32>> {
    if from_rate == to_rate || samples.is_empty() {
        return Ok(samples.to_vec());
    }
    anyhow::ensure!(
        from_rate > 0 && to_rate > 0,
        "sample rates must be > 0 (got {} → {})",
        from_rate,
        to_rate
    );

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: None, // let rubato pick the highest alias-free cutoff
        oversampling_factor: 128,
        interpolation: SincInterpolationType::Linear,
        window: WindowFunction::BlackmanHarris2,
    };

    let mut resampler = Async::<f32>::new_sinc(
        to_rate as f64 / from_rate as f64,
        1.0, // fixed ratio — we never adjust it
        &params,
        CHUNK_SIZE,
        1, // mono
        FixedAsync::Input,
    )
    .map_err(|e| anyhow::anyhow!("Failed to create resampler: {}", e))?;

    // Single-channel buffer over the whole clip; `process_all` chunks it,
    // trims the resampler's startup delay, and returns exactly the
    // resampled frames.
    let input = InterleavedOwned::new_from(samples.to_vec(), 1, samples.len())
        .map_err(|e| anyhow::anyhow!("Invalid resampler input buffer: {}", e))?;
    let output = resampler
        .process_all(&input, samples.len(), None)
        .map_err(|e| anyhow::anyhow!("Resampling failed: {}", e))?;

    Ok(output.take_data())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_when_rates_match() {
        let samples = vec![0.0, 0.5, -0.5, 1.0];
        let out = resample(&samples, 16000, 16000).unwrap();
        assert_eq!(out, samples);
    }

    #[test]
    fn empty_input_does_not_panic() {
        assert!(resample(&[], 44100, 16000).unwrap().is_empty());
        assert!(resample(&[], 16000, 16000).unwrap().is_empty());
    }

    #[test]
    fn short_input_does_not_panic() {
        let out = resample(&[0.1, -0.1, 0.2], 44100, 16000).unwrap();
        assert!(!out.is_empty());
    }

    #[test]
    fn downsample_44100_to_16000_length_ratio() {
        let n = 44100; // one second
        let samples: Vec<f32> = (0..n)
            .map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / 44100.0).sin() * 0.5)
            .collect();
        let out = resample(&samples, 44100, 16000).unwrap();
        let expected = 16000;
        let diff = (out.len() as i64 - expected).abs();
        assert!(
            diff <= 2,
            "expected ~{} samples, got {}",
            expected,
            out.len()
        );
        // A 440 Hz sine survives a 44.1k → 16k resample with energy intact.
        let rms_in = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        let rms_out = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(
            (rms_in - rms_out).abs() < 0.05,
            "rms in={} out={}",
            rms_in,
            rms_out
        );
    }

    #[test]
    fn upsample_16000_to_48000_length_ratio() {
        let n = 16000;
        let samples: Vec<f32> = (0..n)
            .map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / 16000.0).sin() * 0.5)
            .collect();
        let out = resample(&samples, 16000, 48000).unwrap();
        let diff = (out.len() as i64 - 48000).abs();
        assert!(diff <= 2, "expected ~48000 samples, got {}", out.len());
    }

    #[test]
    fn rejects_zero_rates() {
        assert!(resample(&[0.1], 0, 16000).is_err());
        assert!(resample(&[0.1], 16000, 0).is_err());
    }
}
