/// Sound effects for recording start/stop/confirmation.
///
/// Generates simple beep tones programmatically and plays them
/// using the `rodio` audio output library. No external sound files needed.
use std::io::Cursor;

/// A single-beep tone at ~800 Hz, 120 ms — played when recording starts.
pub fn beep_start() {
    play_tone(800.0, 0.12);
}

/// A double-beep tone at ~600 Hz, 80 ms × 2 — played when recording stops.
pub fn beep_stop() {
    play_tone(600.0, 0.08);
    std::thread::sleep(std::time::Duration::from_millis(60));
    play_tone(600.0, 0.08);
}

/// A confirmation chime at ~1000 Hz, 150 ms — played after transcription is pasted.
pub fn beep_confirm() {
    play_tone(1000.0, 0.15);
}

/// Generate a sine-wave WAV in memory and play it via rodio.
fn play_tone(freq: f32, duration_secs: f32) {
    let sample_rate = 44100u32;
    let num_samples = (sample_rate as f32 * duration_secs) as usize;

    // Generate sine wave samples with fade-in/fade-out envelope
    let samples: Vec<f32> = (0..num_samples)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let envelope = {
                let attack = (i as f32 / (sample_rate as f32 * 0.01)).min(1.0);
                let release = ((num_samples - i) as f32 / (sample_rate as f32 * 0.01)).min(1.0);
                attack * release
            };
            (2.0 * std::f32::consts::PI * freq * t).sin() * envelope * 0.3
        })
        .collect();

    // Encode as 16-bit PCM WAV in memory
    let wav_data = encode_wav(&samples, sample_rate);

    // Play in a background thread so we don't block the UI
    std::thread::spawn(move || {
        if let Err(e) = play_wav_bytes(&wav_data) {
            tracing::debug!("Sound effect playback failed: {}", e);
        }
    });
}

/// Encode f32 samples as a 16-bit PCM WAV file in memory.
fn encode_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let num_channels = 1u16;
    let bits_per_sample = 16u16;
    let data_size = (samples.len() * 2) as u32; // 16-bit = 2 bytes per sample
    let byte_rate = sample_rate * num_channels as u32 * bits_per_sample as u32 / 8;
    let block_align = num_channels * bits_per_sample / 8;

    let mut out = Vec::with_capacity(44 + data_size as usize);

    // RIFF header
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_size).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    // fmt chunk
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    out.extend_from_slice(&num_channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());

    // data chunk
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_size.to_le_bytes());

    for &sample in samples {
        let s = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&s.to_le_bytes());
    }

    out
}

/// Play WAV bytes through the default audio output using rodio.
fn play_wav_bytes(wav_data: &[u8]) -> anyhow::Result<()> {
    use rodio::{Decoder, OutputStream, Sink};
    use std::io::BufReader;

    let (_stream, stream_handle) = OutputStream::try_default()?;
    let sink = Sink::try_new(&stream_handle)?;

    let cursor = Cursor::new(wav_data.to_vec());
    let source = Decoder::new(BufReader::new(cursor))?;

    sink.append(source);
    sink.sleep_until_end();

    Ok(())
}
