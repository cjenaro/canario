/// Canario CLI — Native Linux voice-to-text
///
/// Modes:
///   canario-cli --download              Download ASR + VAD models
///   canario-cli --wav <file>            Transcribe a WAV file
///   canario-cli --mic                   Stream from mic with VAD (auto-detect speech)
///   canario-cli --mic --paste           Auto-paste transcription into focused app
///   canario-cli --mic --toggle          Toggle recording on/off with Enter key
///   canario-cli --mic --paste --toggle  Toggle mode with auto-paste
use anyhow::Result;
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, SileroVadModelConfig, VadModelConfig,
    VoiceActivityDetector,
};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ── Paths ──────────────────────────────────────────────────────────────────

fn default_model_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("canario")
        .join("models")
        .join("sherpa-parakeet-tdt-v3")
}

fn vad_model_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("canario")
        .join("models")
        .join("silero_vad.onnx")
}

// ── CLI arg parsing ────────────────────────────────────────────────────────

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

struct CliArgs {
    download: bool,
    wav_file: Option<String>,
    use_mic: bool,
    paste: bool,
    toggle: bool,
    model_dir: PathBuf,
    encoder: Option<String>,
    decoder: Option<String>,
    tokens: Option<String>,
}

fn parse_args() -> CliArgs {
    let args: Vec<String> = std::env::args().collect();
    CliArgs {
        download: args.contains(&"--download".to_string()),
        wav_file: get_arg(&args, "--wav"),
        use_mic: args.contains(&"--mic".to_string()),
        paste: args.contains(&"--paste".to_string()),
        toggle: args.contains(&"--toggle".to_string()),
        model_dir: get_arg(&args, "--model-dir")
            .map(PathBuf::from)
            .unwrap_or_else(default_model_dir),
        encoder: get_arg(&args, "--encoder"),
        decoder: get_arg(&args, "--decoder"),
        tokens: get_arg(&args, "--tokens"),
    }
}

// ── Main ───────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = parse_args();

    // ── Download mode ──────────────────────────────────────────────────
    if cli.download {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            download_asr_model(&cli.model_dir).await?;
            download_vad_model().await
        })?;
        return Ok(());
    }

    // ── Resolve model paths ────────────────────────────────────────────
    let (encoder_path, decoder_path, joiner_path, tokens_path) =
        resolve_model_paths(&cli);

    for (name, path) in [
        ("encoder", &encoder_path),
        ("decoder", &decoder_path),
        ("tokens", &tokens_path),
    ] {
        if !path.exists() && !path.as_os_str().is_empty() {
            eprintln!("❌ {} not found at {:?}", name, path);
            eprintln!("   Run with --download to download the model.");
            std::process::exit(1);
        }
    }

    let is_sherpa = cli.model_dir.join("encoder.int8.onnx").exists();

    // ── Create ASR recognizer ──────────────────────────────────────────
    eprintln!("Loading model from {:?}...", cli.model_dir);
    let recognizer = create_recognizer(
        &encoder_path,
        &decoder_path,
        &joiner_path,
        &tokens_path,
        is_sherpa,
    )
    .ok_or_else(|| anyhow::anyhow!("Failed to create recognizer"))?;
    eprintln!("✅ Model loaded!");

    // ── Dispatch ───────────────────────────────────────────────────────
    if let Some(wav) = &cli.wav_file {
        transcribe_wav(&recognizer, wav)?;
    } else if cli.use_mic {
        // Ensure VAD model exists
        if !vad_model_path().exists() {
            eprintln!("⚠️  VAD model not found. Downloading...");
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(download_vad_model())?;
        }

        if cli.toggle {
            run_toggle_mode(&recognizer, cli.paste)?;
        } else {
            run_vad_streaming(&recognizer, cli.paste)?;
        }
    } else {
        eprintln!("Canario — Native Linux voice-to-text\n");
        eprintln!("Usage:");
        eprintln!("  canario --download              Download models");
        eprintln!("  canario --wav <file>            Transcribe a WAV file");
        eprintln!("  canario --mic                   Stream from mic (VAD auto-detect)");
        eprintln!("  canario --mic --paste           Stream + auto-paste results");
        eprintln!("  canario --mic --toggle          Press Enter to start/stop recording");
        eprintln!("  canario --mic --paste --toggle  Toggle mode + auto-paste");
    }

    Ok(())
}

// ── Model path resolution ──────────────────────────────────────────────────

fn resolve_model_paths(cli: &CliArgs) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let is_sherpa = cli.model_dir.join("encoder.int8.onnx").exists();

    if is_sherpa {
        (
            cli.model_dir.join("encoder.int8.onnx"),
            cli.model_dir.join("decoder.int8.onnx"),
            cli.model_dir.join("joiner.int8.onnx"),
            cli.model_dir.join("tokens.txt"),
        )
    } else {
        (
            cli.encoder
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| cli.model_dir.join("encoder-model.int8.onnx")),
            cli.decoder
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| cli.model_dir.join("decoder_joint-model.int8.onnx")),
            PathBuf::from(""), // joiner = decoder for istupakov format
            cli.tokens
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| cli.model_dir.join("vocab.txt")),
        )
    }
}

fn create_recognizer(
    encoder: &PathBuf,
    decoder: &PathBuf,
    joiner: &PathBuf,
    tokens: &PathBuf,
    is_sherpa: bool,
) -> Option<OfflineRecognizer> {
    let mut config = OfflineRecognizerConfig::default();

    if is_sherpa {
        config.model_config.transducer.encoder = Some(encoder.to_string_lossy().to_string());
        config.model_config.transducer.decoder = Some(decoder.to_string_lossy().to_string());
        config.model_config.transducer.joiner = Some(joiner.to_string_lossy().to_string());
    } else {
        config.model_config.transducer.encoder = Some(encoder.to_string_lossy().to_string());
        config.model_config.transducer.decoder = Some(decoder.to_string_lossy().to_string());
        // For istupakov format, decoder and joiner are the same file
        config.model_config.transducer.joiner = Some(decoder.to_string_lossy().to_string());
    }
    config.model_config.tokens = Some(tokens.to_string_lossy().to_string());
    config.model_config.model_type = Some("nemo_transducer".to_string());
    config.model_config.num_threads = 4;
    config.model_config.debug = false;

    OfflineRecognizer::create(&config)
}

// ── WAV file transcription ─────────────────────────────────────────────────

fn transcribe_wav(recognizer: &OfflineRecognizer, wav_path: &str) -> Result<()> {
    let (samples, sr) = read_wav_file(wav_path)?;
    let stream = recognizer.create_stream();
    stream.accept_waveform(sr as i32, &samples);
    recognizer.decode(&stream);

    if let Some(result) = stream.get_result() {
        println!("{}", result.text);
    }
    Ok(())
}

// ── VAD-based streaming mic ────────────────────────────────────────────────

fn create_vad() -> Result<VoiceActivityDetector> {
    let vad_path = vad_model_path();
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

fn run_vad_streaming(recognizer: &OfflineRecognizer, auto_paste: bool) -> Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let vad = create_vad()?;
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No input device"))?;
    let supported = device.default_input_config()?;
    let mic_sr = supported.sample_rate().0;
    let channels = supported.channels() as usize;

    eprintln!(
        "🎤 Streaming from '{}' at {}Hz (VAD auto-detect)",
        device.name().unwrap_or_default(),
        mic_sr
    );
    eprintln!("   Speak naturally. Press Ctrl+C to quit.\n");

    // Shared audio buffer — VAD reads from here
    let audio_buf: Arc<parking_lot::Mutex<Vec<f32>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let audio_buf_clone = audio_buf.clone();
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    // Set up Ctrl+C handler
    ctrlc::set_handler(move || {
        eprintln!("\n⏹  Stopping...");
        running_clone.store(false, Ordering::SeqCst);
    })?;

    // Audio capture stream
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
            |err| eprintln!("Audio error: {}", err),
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
                |err| eprintln!("Audio error: {}", err),
                None,
            )?
        }
        _ => anyhow::bail!("Unsupported sample format"),
    };

    stream.play()?;

    // Main VAD loop
    let window_size = 512; // Silero VAD window at 16kHz = 32ms
    while running.load(Ordering::SeqCst) {
        // Drain captured audio
        let new_audio: Vec<f32> = {
            let mut buf = audio_buf.lock();
            std::mem::take(&mut *buf)
        };

        if new_audio.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }

        // Resample to 16kHz if needed
        let audio_16k = if mic_sr != 16000 {
            simple_resample(&new_audio, mic_sr, 16000)
        } else {
            new_audio
        };

        // Feed to VAD in window_size chunks
        for chunk in audio_16k.chunks(window_size) {
            // Pad last chunk if needed
            if chunk.len() == window_size {
                vad.accept_waveform(chunk);
            } else {
                let mut padded = chunk.to_vec();
                padded.resize(window_size, 0.0f32);
                vad.accept_waveform(&padded);
            }

            // Check if VAD has a complete speech segment
            while !vad.is_empty() {
                if let Some(segment) = vad.front() {
                    let samples = segment.samples().to_vec();
                    let duration = samples.len() as f64 / 16000.0;
                    vad.pop();

                    if duration < 0.1 {
                        continue;
                    }

                    eprint!("🔄 Transcribing ({:.1}s)... ", duration);

                    let stream = recognizer.create_stream();
                    stream.accept_waveform(16000, &samples);
                    recognizer.decode(&stream);

                    if let Some(result) = stream.get_result() {
                        let text = result.text.trim().to_string();
                        if !text.is_empty() {
                            println!("{}", text);
                            if auto_paste {
                                match crate_paste::paste_text(&text) {
                                    Ok(()) => eprintln!("📋 Pasted!"),
                                    Err(e) => eprintln!("⚠️  Paste failed: {}", e),
                                }
                            }
                        } else {
                            eprintln!("(empty)");
                        }
                    } else {
                        eprintln!("(no result)");
                    }
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Flush any remaining speech in VAD
    vad.flush();
    while !vad.is_empty() {
        if let Some(segment) = vad.front() {
            let samples = segment.samples().to_vec();
            let duration = samples.len() as f64 / 16000.0;
            vad.pop();

            if duration < 0.1 {
                continue;
            }

            eprint!("🔄 Transcribing ({:.1}s)... ", duration);
            let stream = recognizer.create_stream();
            stream.accept_waveform(16000, &samples);
            recognizer.decode(&stream);

            if let Some(result) = stream.get_result() {
                let text = result.text.trim().to_string();
                if !text.is_empty() {
                    println!("{}", text);
                    if auto_paste {
                        match crate_paste::paste_text(&text) {
                            Ok(()) => eprintln!("📋 Pasted!"),
                            Err(e) => eprintln!("⚠️  Paste failed: {}", e),
                        }
                    }
                }
            }
        }
    }

    drop(stream);
    Ok(())
}

// ── Toggle mode (press Enter to start/stop) ────────────────────────────────

fn run_toggle_mode(recognizer: &OfflineRecognizer, auto_paste: bool) -> Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No input device"))?;
    let supported = device.default_input_config()?;
    let mic_sr = supported.sample_rate().0;
    let channels = supported.channels() as usize;

    let samples: Arc<parking_lot::Mutex<Vec<f32>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let recording = Arc::new(AtomicBool::new(false));
    let samples_clone = samples.clone();
    let recording_clone = recording.clone();

    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &supported.into(),
            move |data: &[f32], _| {
                if !recording_clone.load(Ordering::SeqCst) {
                    return;
                }
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                    .collect();
                samples_clone.lock().extend_from_slice(&mono);
            },
            |err| eprintln!("Audio error: {}", err),
            None,
        )?,
        cpal::SampleFormat::I16 => {
            let s = samples.clone();
            let r = recording.clone();
            device.build_input_stream(
                &supported.into(),
                move |data: &[i16], _| {
                    if !r.load(Ordering::SeqCst) {
                        return;
                    }
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|frame| {
                            frame.iter().map(|&s| s as f32 / i16::MAX as f32).sum::<f32>()
                                / channels as f32
                        })
                        .collect();
                    s.lock().extend_from_slice(&mono);
                },
                |err| eprintln!("Audio error: {}", err),
                None,
            )?
        }
        _ => anyhow::bail!("Unsupported sample format"),
    };

    stream.play()?;

    eprintln!("🎤 Toggle mode — press Enter to start/stop recording");
    eprintln!("   Type 'q' + Enter to quit\n");

    let stdin = io::stdin();
    loop {
        eprint!("⏸  Ready. Press Enter to start recording... ");
        io::stderr().flush()?;

        let mut input = String::new();
        let n = stdin.lock().read_line(&mut input)?;
        if n == 0 {
            break; // EOF
        }
        let cmd = input.trim();

        if cmd == "q" || cmd == "quit" {
            break;
        }

        // Start recording
        recording.store(true, Ordering::SeqCst);
        samples.lock().clear();
        eprintln!("🔴 Recording... Press Enter to stop.");

        let mut input2 = String::new();
        let n2 = stdin.lock().read_line(&mut input2)?;
        if n2 == 0 {
            // EOF — stop recording and transcribe
            recording.store(false, Ordering::SeqCst);
        } else {
            let cmd2 = input2.trim();
            recording.store(false, Ordering::SeqCst);
            if cmd2 == "q" || cmd2 == "quit" {
                break;
            }
        }

        let audio = samples.lock().clone();
        let duration = audio.len() as f64 / mic_sr as f64;

        if duration < 0.1 {
            eprintln!("⚠️  Recording too short, nothing to transcribe.\n");
            continue;
        }

        // Resample to 16kHz if needed
        let audio_16k = if mic_sr != 16000 {
            simple_resample(&audio, mic_sr, 16000)
        } else {
            audio
        };

        eprint!("🔄 Transcribing ({:.1}s)... ", duration);
        let rec_stream = recognizer.create_stream();
        rec_stream.accept_waveform(16000, &audio_16k);
        recognizer.decode(&rec_stream);

        if let Some(result) = rec_stream.get_result() {
            let text = result.text.trim().to_string();
            if !text.is_empty() {
                println!("{}", text);
                if auto_paste {
                    match crate_paste::paste_text(&text) {
                        Ok(()) => eprintln!("📋 Pasted!"),
                        Err(e) => eprintln!("⚠️  Paste failed: {}", e),
                    }
                }
            } else {
                eprintln!("(no speech detected)");
            }
        } else {
            eprintln!("(no result)");
        }
        eprintln!();
    }

    drop(stream);
    Ok(())
}

// ── WAV file reader ────────────────────────────────────────────────────────

fn read_wav_file(path: &str) -> Result<(Vec<f32>, u32)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut all_data = Vec::new();
    file.read_to_end(&mut all_data)?;

    if all_data.len() < 12 || &all_data[0..4] != b"RIFF" || &all_data[8..12] != b"WAVE" {
        anyhow::bail!("Not a valid WAV file");
    }

    let mut sample_rate = 16000u32;
    let mut num_channels = 1u16;
    let mut bits_per_sample = 16u16;
    let mut audio_data = Vec::new();

    let mut pos = 12usize;
    while pos + 8 <= all_data.len() {
        let chunk_id = &all_data[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            all_data[pos + 4],
            all_data[pos + 5],
            all_data[pos + 6],
            all_data[pos + 7],
        ]) as usize;

        if chunk_id == b"fmt " {
            if chunk_size < 16 {
                anyhow::bail!("Invalid fmt chunk");
            }
            let fmt = &all_data[pos + 8..];
            let _audio_format = u16::from_le_bytes([fmt[0], fmt[1]]);
            num_channels = u16::from_le_bytes([fmt[2], fmt[3]]);
            sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
            bits_per_sample = u16::from_le_bytes([fmt[14], fmt[15]]);
        } else if chunk_id == b"data" {
            let end = (pos + 8 + chunk_size).min(all_data.len());
            audio_data = all_data[pos + 8..end].to_vec();
            break;
        }

        pos += 8 + chunk_size;
        if chunk_size % 2 != 0 {
            pos += 1;
        }
    }

    if audio_data.is_empty() {
        anyhow::bail!("No audio data found in WAV file");
    }

    let samples: Vec<f32> = match bits_per_sample {
        16 => audio_data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        32 => audio_data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        24 => audio_data
            .chunks_exact(3)
            .map(|c| {
                let val = i32::from_le_bytes([c[0], c[1], c[2], 0]);
                val as f32 / 8388608.0
            })
            .collect(),
        _ => anyhow::bail!("Unsupported bits per sample: {}", bits_per_sample),
    };

    let mono = if num_channels > 1 {
        samples
            .chunks(num_channels as usize)
            .map(|f| f.iter().sum::<f32>() / num_channels as f32)
            .collect()
    } else {
        samples
    };

    Ok((mono, sample_rate))
}

// ── Resampling ─────────────────────────────────────────────────────────────

fn simple_resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    let ratio = to_rate as f64 / from_rate as f64;
    let new_len = (samples.len() as f64 * ratio) as usize;
    (0..new_len)
        .map(|i| {
            let src_idx = i as f64 / ratio;
            let lo = src_idx.floor() as usize;
            let hi = (lo + 1).min(samples.len() - 1);
            let frac = src_idx - lo as f64;
            samples[lo] * (1.0 - frac as f32) + samples[hi] * frac as f32
        })
        .collect()
}

// ── Model download ─────────────────────────────────────────────────────────

async fn download_asr_model(model_dir: &PathBuf) -> Result<()> {
    std::fs::create_dir_all(model_dir)?;

    let repo = "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8";
    let files = [
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "joiner.int8.onnx",
        "tokens.txt",
    ];

    for file in &files {
        let dest = model_dir.join(file);
        if dest.exists() {
            eprintln!("✓ {} already exists, skipping", file);
            continue;
        }

        let url = format!("https://huggingface.co/{}/resolve/main/{}", repo, file);
        eprintln!("⬇ Downloading {}...", file);

        let response = reqwest::get(&url).await?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to download {}: HTTP {}", file, response.status());
        }

        let bytes = response.bytes().await?;
        std::fs::write(&dest, &bytes)?;

        let size_mb = bytes.len() as f64 / (1024.0 * 1024.0);
        eprintln!("  ✓ Downloaded {} ({:.1} MB)", file, size_mb);
    }

    eprintln!("✅ ASR model download complete!");
    Ok(())
}

async fn download_vad_model() -> Result<()> {
    let dest = vad_model_path();
    if dest.exists() {
        eprintln!("✓ Silero VAD model already exists, skipping");
        return Ok(());
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let url = "https://raw.githubusercontent.com/snakers4/silero-vad/master/src/silero_vad/data/silero_vad.onnx";
    eprintln!("⬇ Downloading Silero VAD model...");

    let response = reqwest::get(url).await?;
    if !response.status().is_success() {
        anyhow::bail!(
            "Failed to download VAD model: HTTP {}",
            response.status()
        );
    }

    let bytes = response.bytes().await?;
    std::fs::write(&dest, &bytes)?;

    let size_kb = bytes.len() as f64 / 1024.0;
    eprintln!("  ✓ Downloaded Silero VAD ({:.0} KB)", size_kb);
    Ok(())
}

// ── Paste module inline ────────────────────────────────────────────────────

mod crate_paste {
    use std::io::Write;
    use std::process::Command;

    /// Paste text into the active application
    pub fn paste_text(text: &str) -> anyhow::Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        // Try xdotool first (X11)
        if xdotool_available() {
            let status = Command::new("xdotool")
                .args(["type", "--clearmodifiers", "--"])
                .arg(text)
                .status()?;
            if status.success() {
                return Ok(());
            }
        }

        // Fallback to wtype (Wayland)
        if wtype_available() {
            let status = Command::new("wtype").arg(text).status()?;
            if status.success() {
                return Ok(());
            }
        }

        // Last resort: clipboard + Ctrl+V
        clipboard_copy(text)?;
        simulate_paste()?;

        Ok(())
    }

    fn xdotool_available() -> bool {
        Command::new("xdotool")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn wtype_available() -> bool {
        Command::new("wtype")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn clipboard_copy(text: &str) -> anyhow::Result<()> {
        // Try wl-copy (Wayland)
        if let Ok(mut child) = Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()?;
            return Ok(());
        }

        // Try xclip (X11)
        if let Ok(mut child) = Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()?;
            return Ok(());
        }

        // Try xsel (X11)
        let mut child = Command::new("xsel")
            .args(["--clipboard", "--input"])
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;

        Ok(())
    }

    fn simulate_paste() -> anyhow::Result<()> {
        let status = Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+shift+v"])
            .status();

        if status.map(|s| s.success()).unwrap_or(false) {
            return Ok(());
        }

        let status = Command::new("ydotool")
            .args(["key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"])
            .status();

        if status.map(|s| s.success()).unwrap_or(false) {
            return Ok(());
        }

        anyhow::bail!(
            "No paste method available. Install xdotool, wtype, or ydotool."
        )
    }
}
