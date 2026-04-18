pub mod postprocess;

use anyhow::Result;
use sherpa_onnx::OfflineRecognizer;
use sherpa_onnx::OfflineRecognizerConfig;
use std::path::PathBuf;
use tracing::{info, error, debug};

/// Manages the ASR (Automatic Speech Recognition) model lifecycle
pub struct TranscriptionEngine {
    recognizer: Option<OfflineRecognizer>,
    model_dir: PathBuf,
    num_threads: u32,
}

impl TranscriptionEngine {
    pub fn new(model_dir: PathBuf, num_threads: u32) -> Self {
        Self {
            recognizer: None,
            model_dir,
            num_threads,
        }
    }

    /// Check if model files are present
    pub fn is_model_available(&self) -> bool {
        self.encoder_path().exists() && self.decoder_path().exists() && self.tokens_path().exists()
    }

    fn encoder_path(&self) -> PathBuf {
        self.model_dir.join("encoder.int8.onnx")
    }

    fn decoder_path(&self) -> PathBuf {
        self.model_dir.join("decoder.int8.onnx")
    }

    fn joiner_path(&self) -> PathBuf {
        self.model_dir.join("joiner.int8.onnx")
    }

    fn tokens_path(&self) -> PathBuf {
        self.model_dir.join("tokens.txt")
    }

    /// Load the model into memory
    pub fn load_model(&mut self) -> Result<()> {
        if !self.is_model_available() {
            anyhow::bail!(
                "Model files not found in {:?}. Run model download first.",
                self.model_dir
            );
        }

        info!("Loading ASR model from {:?}", self.model_dir);

        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer.encoder = Some(self.encoder_path().to_string_lossy().to_string());
        config.model_config.transducer.decoder = Some(self.decoder_path().to_string_lossy().to_string());
        config.model_config.transducer.joiner = Some(self.joiner_path().to_string_lossy().to_string());
        config.model_config.tokens = Some(self.tokens_path().to_string_lossy().to_string());
        config.model_config.model_type = Some("nemo_transducer".to_string());
        config.model_config.num_threads = self.num_threads as i32;
        config.model_config.debug = false;

        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| anyhow::anyhow!("Failed to create recognizer"))?;

        self.recognizer = Some(recognizer);
        info!("ASR model loaded successfully");

        Ok(())
    }

    /// Transcribe raw audio samples (16kHz mono f32)
    pub fn transcribe(&self, samples: &[f32]) -> Result<String> {
        let recognizer = self
            .recognizer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Model not loaded"))?;

        debug!("Transcribing {} samples ({:.2}s)", samples.len(), samples.len() as f64 / 16000.0);

        let stream = recognizer.create_stream();
        stream.accept_waveform(16000, samples);
        recognizer.decode(&stream);

        match stream.get_result() {
            Some(result) => {
                let text = result.text.trim().to_string();
                info!("Transcription result: '{}'", text);
                Ok(text)
            }
            None => {
                error!("Transcription returned no result");
                Ok(String::new())
            }
        }
    }

    /// Transcribe a WAV file
    pub fn transcribe_file(&self, path: &std::path::Path) -> Result<String> {
        let recognizer = self
            .recognizer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Model not loaded"))?;

        let stream = recognizer.create_stream();

        // Read WAV file and accept waveform
        let (samples, _sample_rate) = read_wav(path)?;
        stream.accept_waveform(16000, &samples);
        recognizer.decode(&stream);

        match stream.get_result() {
            Some(result) => {
                let text = result.text.trim().to_string();
                info!("Transcription result: '{}'", text);
                Ok(text)
            }
            None => Ok(String::new()),
        }
    }

    /// Release the model from memory
    pub fn unload(&mut self) {
        self.recognizer = None;
        info!("Model unloaded");
    }
}

/// Simple WAV reader that returns 16kHz mono f32 samples
pub fn read_wav(path: &std::path::Path) -> Result<(Vec<f32>, u32)> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut header = [0u8; 44];
    file.read_exact(&mut header)?;

    // Verify RIFF header
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        anyhow::bail!("Not a valid WAV file");
    }

    let num_channels = u16::from_le_bytes([header[22], header[23]]);
    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let bits_per_sample = u16::from_le_bytes([header[34], header[35]]);

    let mut data = Vec::new();
    file.read_to_end(&mut data)?;

    let samples: Vec<f32> = match bits_per_sample {
        16 => data
            .chunks_exact(2)
            .map(|chunk| {
                let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                s as f32 / 32768.0
            })
            .collect(),
        32 => data
            .chunks_exact(4)
            .map(|chunk| {
                let s = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                s
            })
            .collect(),
        _ => anyhow::bail!("Unsupported bits per sample: {}", bits_per_sample),
    };

    // Convert to mono if stereo
    let mono = if num_channels > 1 {
        samples
            .chunks(num_channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / num_channels as f32)
            .collect()
    } else {
        samples
    };

    // TODO: resample if sample_rate != 16000

    Ok((mono, sample_rate))
}

/// Download model files from HuggingFace with progress reporting.
pub async fn download_model_with_progress(
    model_dir: &std::path::Path,
    repo: &str,
    event_tx: &std::sync::mpsc::Sender<crate::event::Event>,
) -> Result<()> {
    std::fs::create_dir_all(model_dir)?;

    let files = [
        ("encoder.int8.onnx", true),
        ("decoder.int8.onnx", true),
        ("joiner.int8.onnx", true),
        ("tokens.txt", false),
    ];

    // Calculate total size estimate for progress
    let total_files = files.len() as f64;
    let mut completed = 0.0;

    for (file, _large) in &files {
        let url = format!("https://huggingface.co/{}/resolve/main/{}", repo, file);
        let dest = model_dir.join(file);

        if dest.exists() {
            info!("{} already exists, skipping", file);
            completed += 1.0;
            let _ = event_tx.send(crate::event::Event::ModelDownloadProgress(completed / total_files));
            continue;
        }

        info!("Downloading {}...", file);
        let response = reqwest::get(&url).await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to download {}: HTTP {}", file, response.status());
        }

        let total_size = response.content_length().unwrap_or(0) as f64;

        // Stream the response to track progress
        let response = response;
        let mut downloaded: f64 = 0.0;
        let mut buf = Vec::new();

        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            downloaded += chunk.len() as f64;
            buf.extend_from_slice(&chunk);

            // Report progress: (files_completed + current_file_progress) / total_files
            if total_size > 0.0 {
                let file_progress = downloaded / total_size;
                let overall = (completed + file_progress) / total_files;
                let _ = event_tx.send(crate::event::Event::ModelDownloadProgress(overall.min(0.99)));
            }
        }

        std::fs::write(&dest, &buf)?;

        let size_mb = buf.len() as f64 / (1024.0 * 1024.0);
        info!("Downloaded {} ({:.1} MB)", file, size_mb);

        completed += 1.0;
        let _ = event_tx.send(crate::event::Event::ModelDownloadProgress(completed / total_files));
    }

    info!("Model download complete!");
    Ok(())
}

/// Download model files from HuggingFace (no progress reporting)
pub async fn download_model(model_dir: &std::path::Path, repo: &str) -> Result<()> {
    std::fs::create_dir_all(model_dir)?;

    let files = [
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "joiner.int8.onnx",
        "tokens.txt",
    ];

    for file in &files {
        let url = format!("https://huggingface.co/{}/resolve/main/{}", repo, file);
        let dest = model_dir.join(file);

        if dest.exists() {
            info!("{} already exists, skipping", file);
            continue;
        }

        info!("Downloading {}...", file);
        let response = reqwest::get(&url).await?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to download {}: HTTP {}", file, response.status());
        }

        let _total_size = response.content_length();
        let bytes = response.bytes().await?;

        std::fs::write(&dest, &bytes)?;

        let size_mb = bytes.len() as f64 / (1024.0 * 1024.0);
        info!("Downloaded {} ({:.1} MB)", file, size_mb);
    }

    info!("Model download complete!");
    Ok(())
}
