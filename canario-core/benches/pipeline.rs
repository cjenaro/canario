//! Pipeline stage benchmarks — the pure-Rust cost centers identified in
//! bead canario-oxi's lead list:
//!
//! - **resample**: mic rate → 16 kHz on stop (`recording.rs` final decode
//!   path; the same conversion the live-captions worker pays per partial)
//! - **whole-buffer clone**: `audio_buf.lock().clone()` taken on every
//!   stop before the mic is released
//! - **decode**: final whole-buffer recognizer decode, warm cache —
//!   `recognizer.decode(stream)` over typical dictation lengths
//!
//! Run with `cargo bench -p canario-core`. Decode benches require a real
//! model and are gated behind `CANARIO_TEST_MODEL_DIR` (same convention
//! as the ignored model tests) — without it they register nothing, so
//! the default bench run stays model-free.
//!
//! Everything else (mic open, poll loops, beep, paste) is timing-mark
//! territory, not criterion: those stages need real devices. Use
//! `scripts/bench-pipeline` for the end-to-end numbers.

use std::hint::black_box;
use std::time::Duration;

use canario_core::resample::resample;
use canario_core::TranscriptionEngine;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

/// Dictation lengths worth benching: a quick phrase, a typical utterance,
/// a long ramble.
const CLIP_SECS: &[f64] = &[2.0, 5.0, 10.0];

/// The mic rate desktop hosts most often report (PipeWire/Pulse default).
const MIC_RATE: u32 = 48_000;

/// Deterministic speech-like test signal: syllable-rate amplitude
/// envelope over a few formant-ish harmonics plus a noise floor.
///
/// Real speech would be better; this is the reproducible stand-in that
/// still gives the decoder non-trivial work (unlike pure silence or a
/// pure sine, which TDT models short-circuit).
fn speech_like(seconds: f64, rate: u32) -> Vec<f32> {
    let n = (seconds * rate as f64) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            // ~4 syllables/s envelope with 20 ms "consonant" gaps
            let syll = (t * 4.0 * std::f64::consts::PI).sin().max(0.0).powi(2);
            let gap = ((t * 4.0) % 1.0) > 0.94;
            let env = if gap { 0.0 } else { 0.25 + 0.7 * syll };
            let mut s = 0.0;
            for (k, f) in [220.0, 660.0, 1100.0, 2400.0].iter().enumerate() {
                s += (t * f * 2.0 * std::f64::consts::PI).sin() / (k as f64 + 1.0);
            }
            // cheap deterministic "noise"
            let noise = ((i as f64 * 12.9898).sin() * 43758.5453).fract() - 0.5;
            (env * (0.5 * s) + 0.02 * noise) as f32
        })
        .collect()
}

/// Resample mic-rate mono audio to the recognizer's 16 kHz — the exact
/// call the recording thread makes on stop (and live captions per
/// partial). A fresh resampler is constructed each time, matching
/// `resample()`'s no-state-across-calls design.
fn bench_resample(c: &mut Criterion) {
    let mut group = c.benchmark_group("resample");
    for &secs in CLIP_SECS {
        let samples = speech_like(secs, MIC_RATE);
        group.throughput(Throughput::Bytes(
            (samples.len() * std::mem::size_of::<f32>()) as u64,
        ));
        group.bench_with_input(
            BenchmarkId::new(format!("{}hz_to_16khz", MIC_RATE), format!("{}s", secs)),
            &samples,
            |b, samples| {
                b.iter(|| {
                    let out = resample(black_box(samples), MIC_RATE, 16_000).unwrap();
                    black_box(out);
                })
            },
        );
    }
    group.finish();
}

/// Whole-buffer clone on stop (`recording.rs`): `Vec<f32>` clone of the
/// mono capture buffer, from a quick phrase up to a long ramble.
fn bench_whole_buffer_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("whole_buffer_clone");
    for &secs in &[2.0f64, 10.0, 60.0] {
        let samples = speech_like(secs, MIC_RATE);
        group.throughput(Throughput::Bytes(
            (samples.len() * std::mem::size_of::<f32>()) as u64,
        ));
        group.bench_with_input(
            BenchmarkId::new("mic_buffer", format!("{}s", secs)),
            &samples,
            |b, samples| {
                b.iter(|| {
                    let cloned = black_box(samples).clone();
                    black_box(cloned);
                })
            },
        );
    }
    group.finish();
}

/// Warm-cache recognizer decode of a finished dictation. The model is
/// loaded once per bench run (warm by construction — the thing being
/// measured is `create_stream + accept_waveform + decode`, the same
/// sequence the recording thread's final decode performs).
///
/// Gated behind `CANARIO_TEST_MODEL_DIR`, pointing at a downloaded model
/// directory (encoder.int8.onnx & friends), exactly like the ignored
/// model tests. Not set → this bench registers nothing.
fn bench_decode(c: &mut Criterion) {
    let model_dir = match std::env::var_os("CANARIO_TEST_MODEL_DIR") {
        Some(dir) => std::path::PathBuf::from(dir),
        None => {
            // Not an error: the model-free bench run must stay green.
            eprintln!(
                "decode benches skipped: set CANARIO_TEST_MODEL_DIR to a downloaded model \
                 directory to include them"
            );
            return;
        }
    };

    let threads: u32 = std::env::var("CANARIO_BENCH_DECODE_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);

    let mut engine = TranscriptionEngine::new(model_dir.clone(), threads);
    engine
        .load_model()
        .expect("CANARIO_TEST_MODEL_DIR must hold a loadable model");

    let mut group = c.benchmark_group("decode");
    // Decodes are 100 ms–1 s each; the default 100 samples would take
    // minutes and add nothing statistically.
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(20));
    for &secs in CLIP_SECS {
        let samples = speech_like(secs, 16_000);
        group.throughput(Throughput::Elements(samples.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("warm", format!("{}s", secs)),
            &samples,
            |b, samples| {
                b.iter(|| {
                    let text = engine.transcribe(black_box(samples)).unwrap();
                    black_box(text);
                })
            },
        );
    }
    group.finish();
}

/// Native paste stage (`canario-core` `paste_text`): clipboard write +
/// injection into the focused window (wtype/xdotool/ydotool spawn).
///
/// **Side effects**: this really types into whatever window is focused.
/// Gated behind `CANARIO_BENCH_PASTE=1` so a normal `cargo bench` run
/// never injects text; point the focus at a scratch editor when enabled.
fn bench_paste(c: &mut Criterion) {
    if !std::env::var_os("CANARIO_BENCH_PASTE").is_some_and(|v| {
        let v = v.to_string_lossy().to_ascii_lowercase();
        !v.is_empty() && v != "0" && v != "false" && v != "no" && v != "off"
    }) {
        return;
    }
    eprintln!("paste bench enabled by CANARIO_BENCH_PASTE: it WILL type into the focused window");
    c.bench_function("paste/native", |b| {
        b.iter(|| {
            let ok = canario_core::paste_text(black_box("benchmark paste sentence.")).unwrap();
            black_box(ok);
        })
    });
}

criterion_group!(
    benches,
    bench_resample,
    bench_whole_buffer_clone,
    bench_decode,
    bench_paste
);
criterion_main!(benches);
