/// Minimal CLI prototype for testing the transcription pipeline
///
/// Usage:
///   canario-cli --download [--model-dir <path>]
///   canario-cli --wav <file> [--model-dir <path>]
///   canario-cli --mic [--model-dir <path>]
use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();

    let encoder = get_arg(&args, "--encoder");
    let decoder = get_arg(&args, "--decoder");
    let tokens = get_arg(&args, "--tokens");
    let wav_file = get_arg(&args, "--wav");
    let use_mic = args.contains(&"--mic".to_string());
    let model_dir = get_arg(&args, "--model-dir");
    let download = args.contains(&"--download".to_string());

    // Default model directory
    let default_model_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("canario")
        .join("models")
        .join("parakeet-tdt-0.6b-v3");

    let model_path = model_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or(default_model_dir);

    // Download model if requested
    if download {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(crate_lib_download(model_path))?;
        return Ok(());
    }

    // Resolve model paths - try sherpa-onnx format first
    let is_sherpa = model_path.join("encoder.int8.onnx").exists();

    let (encoder_path, decoder_path, joiner_path, tokens_path) = if is_sherpa {
        (
            model_path.join("encoder.int8.onnx"),
            model_path.join("decoder.int8.onnx"),
            model_path.join("joiner.int8.onnx"),
            model_path.join("tokens.txt"),
        )
    } else {
        (
            encoder.map(PathBuf::from).unwrap_or_else(|| model_path.join("encoder-model.int8.onnx")),
            decoder.map(PathBuf::from).unwrap_or_else(|| model_path.join("decoder_joint-model.int8.onnx")),
            // For istupakov format, decoder and joiner are in one file
            PathBuf::from(""),
            tokens.map(PathBuf::from).unwrap_or_else(|| model_path.join("vocab.txt")),
        )
    };

    for (name, path) in [
        ("encoder", &encoder_path),
        ("decoder", &decoder_path),
        ("tokens", &tokens_path),
    ] {
        if !path.exists() && !path.as_os_str().is_empty() {
            eprintln!("{} not found at {:?}", name, path);
            eprintln!("Run with --download to download the model.");
            std::process::exit(1);
        }
    }
    if !is_sherpa && !joiner_path.exists() && !joiner_path.as_os_str().is_empty() {
        eprintln!("joiner not found at {:?}", joiner_path);
        std::process::exit(1);
    }

    // Create recognizer
    println!("Loading model from {:?}...", model_path);
    let mut config = sherpa_onnx::OfflineRecognizerConfig::default();

    if is_sherpa {
        // sherpa-onnx format (separate decoder + joiner)
        config.model_config.transducer.encoder = Some(encoder_path.to_string_lossy().to_string());
        config.model_config.transducer.decoder = Some(decoder_path.to_string_lossy().to_string());
        config.model_config.transducer.joiner = Some(joiner_path.to_string_lossy().to_string());
    } else {
        // istupakov format (combined decoder_joint)
        config.model_config.transducer.encoder = Some(encoder_path.to_string_lossy().to_string());
        config.model_config.transducer.decoder = Some(decoder_path.to_string_lossy().to_string());
        config.model_config.transducer.joiner = Some(decoder_path.to_string_lossy().to_string());
    }
    config.model_config.tokens = Some(tokens_path.to_string_lossy().to_string());
    config.model_config.model_type = Some("nemo_transducer".to_string());
    config.model_config.num_threads = 4;
    config.model_config.debug = false;

    let recognizer = sherpa_onnx::OfflineRecognizer::create(&config)
        .ok_or_else(|| anyhow::anyhow!("Failed to create recognizer"))?;

    println!("Model loaded!");

    if let Some(wav) = &wav_file {
        // Read WAV file manually and feed samples
        let (samples, sr) = read_wav_file(wav)?;
        let stream = recognizer.create_stream();
        stream.accept_waveform(sr as i32, &samples);
        recognizer.decode(&stream);

        if let Some(result) = stream.get_result() {
            println!("{}", result.text);
        }
    } else if use_mic {
        record_and_transcribe(&recognizer)?;
    } else {
        eprintln!("Usage:");
        eprintln!("  canario-cli --download [--model-dir <path>]");
        eprintln!("  canario-cli --mic [--model-dir <path>]");
        eprintln!("  canario-cli --wav <file> [--model-dir <path>]");
        eprintln!("  canario-cli --encoder <e> --decoder <d> --tokens <t> --wav <file>");
    }

    Ok(())
}

fn record_and_transcribe(recognizer: &sherpa_onnx::OfflineRecognizer) -> Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No input device"))?;
    let supported = device.default_input_config()?;

    let mic_sr = supported.sample_rate().0;
    let channels = supported.channels() as usize;

    println!(
        "🎤 Recording from '{}' at {}Hz...",
        device.name().unwrap_or_default(),
        mic_sr
    );
    println!("   Press Ctrl+C to stop recording and transcribe.\n");

    let recording = Arc::new(AtomicBool::new(true));
    let recording_clone = recording.clone();
    let samples: Arc<parking_lot::Mutex<Vec<f32>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let samples_clone = samples.clone();

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
        cpal::SampleFormat::I16 => device.build_input_stream(
            &supported.into(),
            move |data: &[i16], _| {
                if !recording_clone.load(Ordering::SeqCst) {
                    return;
                }
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|frame| {
                        frame.iter().map(|&s| s as f32 / i16::MAX as f32).sum::<f32>()
                            / channels as f32
                    })
                    .collect();
                samples_clone.lock().extend_from_slice(&mono);
            },
            |err| eprintln!("Audio error: {}", err),
            None,
        )?,
        _ => anyhow::bail!("Unsupported sample format"),
    };

    stream.play()?;

    // Handle Ctrl+C
    let recording_ctrlc = recording.clone();
    ctrlc::set_handler(move || {
        eprintln!("\n⏹  Stopping recording...");
        recording_ctrlc.store(false, Ordering::SeqCst);
    })?;

    // Wait for recording to stop
    while recording.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    drop(stream);

    let audio = samples.lock().clone();
    let duration = audio.len() as f64 / mic_sr as f64;
    println!("Captured {} samples ({:.2}s)", audio.len(), duration);

    if audio.is_empty() || duration < 0.1 {
        println!("Recording too short, nothing to transcribe.");
        return Ok(());
    }

    // Resample to 16kHz if needed
    let audio_16k = if mic_sr != 16000 {
        println!("Resampling from {}Hz to 16000Hz...", mic_sr);
        simple_resample(&audio, mic_sr, 16000)
    } else {
        audio
    };

    // Transcribe
    println!("🔄 Transcribing...");
    let stream = recognizer.create_stream();
    stream.accept_waveform(16000, &audio_16k);
    recognizer.decode(&stream);

    if let Some(result) = stream.get_result() {
        println!("\n✅ Result: {}", result.text);
    } else {
        println!("No transcription result.");
    }

    Ok(())
}

/// Read a WAV file and return (mono f32 samples, sample_rate)
fn read_wav_file(path: &str) -> Result<(Vec<f32>, u32)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut all_data = Vec::new();
    file.read_to_end(&mut all_data)?;

    // Parse RIFF header
    if all_data.len() < 12 || &all_data[0..4] != b"RIFF" || &all_data[8..12] != b"WAVE" {
        anyhow::bail!("Not a valid WAV file");
    }

    // Parse chunks to find 'fmt ' and 'data'
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
        // Align to even boundary
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
                let val =
                    i32::from_le_bytes([c[0], c[1], c[2], 0]);
                val as f32 / 8388608.0
            })
            .collect(),
        _ => anyhow::bail!("Unsupported bits per sample: {}", bits_per_sample),
    };

    // Convert to mono
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

/// Simple linear interpolation resampling
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

/// Download model files from HuggingFace
async fn crate_lib_download(model_dir: PathBuf) -> Result<()> {
    std::fs::create_dir_all(&model_dir)?;

    let repo = "istupakov/parakeet-tdt-0.6b-v3-onnx";
    let files = [
        "encoder-model.int8.onnx",
        "decoder_joint-model.int8.onnx",
        "vocab.txt",
    ];

    for file in &files {
        let dest = model_dir.join(file);
        if dest.exists() {
            println!("✓ {} already exists, skipping", file);
            continue;
        }

        let url = format!("https://huggingface.co/{}/resolve/main/{}", repo, file);
        println!("⬇ Downloading {}...", file);

        let response = reqwest::get(&url).await?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to download {}: HTTP {}", file, response.status());
        }

        let bytes = response.bytes().await?;
        std::fs::write(&dest, &bytes)?;

        let size_mb = bytes.len() as f64 / (1024.0 * 1024.0);
        println!("  ✓ Downloaded {} ({:.1} MB)", file, size_mb);
    }

    println!("\n✅ Model download complete! Files in: {:?}", model_dir);
    Ok(())
}

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}
