pub mod postprocess;
pub mod resample;
pub(crate) mod validate;

use anyhow::Result;
use sha2::{Digest, Sha256};
use sherpa_onnx::OfflineRecognizer;
use sherpa_onnx::OfflineRecognizerConfig;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, error, info};

use crate::config::ModelPaths;

/// Manages the ASR (Automatic Speech Recognition) model lifecycle
pub struct TranscriptionEngine {
    recognizer: Option<OfflineRecognizer>,
    model_paths: ModelPaths,
    num_threads: u32,
}

impl TranscriptionEngine {
    /// Create an engine for a built-in model directory (fixed file names).
    pub fn new(model_dir: PathBuf, num_threads: u32) -> Self {
        Self::from_paths(
            ModelPaths {
                encoder: model_dir.join("encoder.int8.onnx"),
                decoder: model_dir.join("decoder.int8.onnx"),
                joiner: model_dir.join("joiner.int8.onnx"),
                tokens: model_dir.join("tokens.txt"),
            },
            num_threads,
        )
    }

    /// Create an engine from explicit model file paths (supports
    /// `ModelVariant::Custom`, whose files may live anywhere).
    pub fn from_paths(model_paths: ModelPaths, num_threads: u32) -> Self {
        Self {
            recognizer: None,
            model_paths,
            num_threads,
        }
    }

    /// Check if model files are present
    pub fn is_model_available(&self) -> bool {
        self.model_paths.all_exist()
    }

    /// Load the model into memory
    pub fn load_model(&mut self) -> Result<()> {
        if !self.is_model_available() {
            anyhow::bail!(
                "Model files not found (encoder: {:?}). Run model download first.",
                self.model_paths.encoder
            );
        }

        info!(
            "Loading ASR model (encoder: {:?})",
            self.model_paths.encoder
        );

        // Validate BEFORE any sherpa FFI call: sherpa's C++ ReadTokens
        // exits the whole process on unparseable tokens, so corrupt
        // files must fail here as a regular error instead.
        validate::validate_model_files(&self.model_paths).map_err(|e| {
            anyhow::anyhow!(
                "ASR model files are corrupt or unreadable: {}. Re-download the model from Settings.",
                e
            )
        })?;

        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer.encoder =
            Some(self.model_paths.encoder.to_string_lossy().to_string());
        config.model_config.transducer.decoder =
            Some(self.model_paths.decoder.to_string_lossy().to_string());
        config.model_config.transducer.joiner =
            Some(self.model_paths.joiner.to_string_lossy().to_string());
        config.model_config.tokens = Some(self.model_paths.tokens.to_string_lossy().to_string());
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

        debug!(
            "Transcribing {} samples ({:.2}s)",
            samples.len(),
            samples.len() as f64 / 16000.0
        );

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

/// Simple WAV reader that returns 16kHz mono f32 samples.
///
/// Non-16kHz files are resampled to 16kHz (the recognizer's expected rate).
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
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| {
                let s = i16::from_le_bytes(*chunk);
                s as f32 / 32768.0
            })
            .collect(),
        32 => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
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

    // Resample to 16kHz if the file is at a different rate
    if sample_rate != 16000 {
        info!("Resampling {:?} from {}Hz to 16000Hz", path, sample_rate);
        let resampled = resample::resample(&mono, sample_rate, 16000)?;
        return Ok((resampled, 16000));
    }

    Ok((mono, sample_rate))
}

/// HuggingFace host used for both the metadata API and file downloads.
const HF_BASE_URL: &str = "https://huggingface.co";

/// Download model files from HuggingFace with progress reporting.
///
/// Files are streamed to `<name>.part` and renamed on completion, so an
/// interrupted download can resume (HTTP Range) where it left off.
/// Transient network errors are retried with backoff; `cancel` aborts
/// the download between chunks (checked per chunk and between files).
/// Every download is verified against the repo's published metadata:
/// the SHA-256 checksum for LFS-backed files, and the size for the rest.
pub async fn download_model_with_progress(
    model_dir: &std::path::Path,
    repo: &str,
    event_tx: &std::sync::mpsc::Sender<crate::event::Event>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    download_model_from(
        HF_BASE_URL,
        model_dir,
        repo,
        event_tx,
        cancel,
        RETRY_BACKOFF_BASE,
    )
    .await
}

/// Download model files from a HuggingFace-compatible `base_url`.
///
/// Checksums come from `{base_url}/api/models/{repo}?blobs=true`, where
/// LFS-backed files list `lfs.sha256` — a digest of the file's actual
/// content. Small git-tracked files (e.g. `tokens.txt`) only have a git
/// blob id (a SHA-1 over git's blob encoding, not the file content), so
/// they publish no usable checksum and are size-checked only. If the
/// metadata API cannot be reached, the download proceeds with size-only
/// verification (logged) rather than failing outright.
async fn download_model_from(
    base_url: &str,
    model_dir: &std::path::Path,
    repo: &str,
    event_tx: &std::sync::mpsc::Sender<crate::event::Event>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    retry_backoff: std::time::Duration,
) -> Result<()> {
    std::fs::create_dir_all(model_dir)?;

    let files = [
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "joiner.int8.onnx",
        "tokens.txt",
    ];

    let client = reqwest::Client::new();

    check_cancel(&cancel)?;

    // Best-effort checksum metadata; `None` means "verify sizes only".
    let metadata = fetch_repo_file_metadata(
        &client,
        &format!("{}/api/models/{}?blobs=true", base_url, repo),
    )
    .await;

    // Calculate total size estimate for progress
    let total_files = files.len() as f64;
    let mut completed = 0.0;

    for file in &files {
        check_cancel(&cancel)?;

        let url = format!("{}/{}/resolve/main/{}", base_url, repo, file);
        let dest = model_dir.join(file);
        let meta = metadata.as_ref().and_then(|m| m.get(*file));

        if dest.exists() {
            if verify_existing_file(file, &dest, meta, &cancel).await? {
                info!("{} already exists and passed verification, skipping", file);
                completed += 1.0;
                let _ = event_tx.send(crate::event::Event::ModelDownloadProgress {
                    progress: completed / total_files,
                });
                continue;
            }
            // Failed verification: discard and re-download from scratch.
            info!("{} failed verification, re-downloading", file);
            let _ = tokio::fs::remove_file(&dest).await;
        }

        info!("Downloading {}...", file);

        let tx = event_tx.clone();
        let mut on_progress = |downloaded: u64, total: Option<u64>| {
            if let Some(total) = total {
                if total > 0 {
                    let file_progress = downloaded as f64 / total as f64;
                    let overall = (completed + file_progress.min(1.0)) / total_files;
                    let _ = tx.send(crate::event::Event::ModelDownloadProgress {
                        progress: overall.min(0.99),
                    });
                }
            }
        };

        download_file(
            &client,
            &url,
            &dest,
            &cancel,
            meta,
            &mut on_progress,
            retry_backoff,
        )
        .await?;

        let size_mb =
            std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0) as f64 / (1024.0 * 1024.0);
        if meta.and_then(|m| m.sha256).is_some() {
            info!("Downloaded {} ({:.1} MB, SHA-256 verified)", file, size_mb);
        } else {
            info!(
                "Downloaded {} ({:.1} MB, size check only: no published SHA-256)",
                file, size_mb
            );
        }

        completed += 1.0;
        let _ = event_tx.send(crate::event::Event::ModelDownloadProgress {
            progress: completed / total_files,
        });
    }

    info!("Model download complete!");
    Ok(())
}

/// Verify an already-downloaded file against repo metadata.
///
/// `Ok(true)` = keep the file (verified, or nothing to verify against —
/// preserving the pre-checksum skip behaviour). `Ok(false)` = discard it
/// and re-download. Cancellation propagates as an error so the file is
/// left untouched for a future attempt.
async fn verify_existing_file(
    file: &str,
    dest: &std::path::Path,
    meta: Option<&RepoFileMeta>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<bool> {
    let meta = match meta {
        Some(meta) => meta,
        None => return Ok(true),
    };

    if let Some(expected) = meta.size {
        let actual = tokio::fs::metadata(dest).await.map(|m| m.len()).ok();
        if actual != Some(expected) {
            tracing::warn!(
                "{}: existing file has wrong size (expected {} bytes, got {:?}), re-downloading",
                file,
                expected,
                actual
            );
            return Ok(false);
        }
    }

    if let Some(expected) = meta.sha256 {
        info!("Verifying SHA-256 of existing {}", file);
        match sha256_file(dest, cancel).await {
            Ok(actual) => {
                if actual != expected {
                    tracing::warn!(
                        "{}: existing file failed SHA-256 verification, re-downloading",
                        file
                    );
                    return Ok(false);
                }
            }
            Err(e) => {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(e);
                }
                tracing::warn!(
                    "{}: could not hash existing file ({}), re-downloading",
                    file,
                    e
                );
                return Ok(false);
            }
        }
    }

    Ok(true)
}

/// Path of the in-progress partial file for a given destination.
fn part_path(dest: &std::path::Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// How to handle the server's response when a `.part` file already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeAction {
    /// Server honoured the Range request (206); append after this offset.
    Append(u64),
    /// Server ignored the Range (200) or there is no partial; restart cleanly.
    Restart,
}

fn resume_action(partial_len: u64, status: reqwest::StatusCode) -> ResumeAction {
    if partial_len > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
        ResumeAction::Append(partial_len)
    } else {
        ResumeAction::Restart
    }
}

/// Bail if the download has been cancelled.
fn check_cancel(cancel: &std::sync::atomic::AtomicBool) -> Result<()> {
    if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        anyhow::bail!("Download cancelled");
    }
    Ok(())
}

/// Sanity-check a finished file's size against the expected byte count.
fn verify_size(file: &str, expected: Option<u64>, actual: u64) -> Result<()> {
    if let Some(expected) = expected {
        if actual != expected {
            anyhow::bail!(
                "{}: size mismatch (expected {} bytes, got {})",
                file,
                expected,
                actual
            );
        }
    }
    Ok(())
}

/// Known-good integrity metadata for one file of a HuggingFace repo,
/// parsed from `/api/models/{repo}?blobs=true`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RepoFileMeta {
    /// SHA-256 of the file's content. Only LFS-backed files publish one
    /// (`siblings[].lfs.sha256`); plain git files (e.g. `tokens.txt`)
    /// leave this `None`.
    sha256: Option<[u8; 32]>,
    /// Size of the file in bytes, from `siblings[].size`.
    size: Option<u64>,
}

/// Parse the `siblings` array of a repo-info response into per-file
/// metadata keyed by repo-relative path. Malformed entries are skipped
/// rather than failing the whole download.
///
/// Note: `siblings[].blobId` is a *git blob* SHA-1 (a digest of
/// `"blob <size>\0<content>"`, not of the file content), so it is
/// deliberately ignored in favour of `lfs.sha256`.
fn parse_repo_metadata(json: &serde_json::Value) -> HashMap<String, RepoFileMeta> {
    let mut out = HashMap::new();
    let siblings = json.get("siblings").and_then(|s| s.as_array());
    for entry in siblings.into_iter().flatten() {
        let name = match entry.get("rfilename").and_then(|n| n.as_str()) {
            Some(name) => name,
            None => continue,
        };
        let size = entry.get("size").and_then(|s| s.as_u64());
        let sha256 = entry
            .pointer("/lfs/sha256")
            .and_then(|h| h.as_str())
            .and_then(parse_sha256_hex);
        if entry.get("lfs").is_some() && sha256.is_none() {
            tracing::warn!(
                "{}: LFS entry has no parsable sha256; size check only",
                name
            );
        }
        out.insert(name.to_string(), RepoFileMeta { sha256, size });
    }
    out
}

/// Parse a hex SHA-256 (64 digits, case-insensitive, tolerating
/// ETag-style quotes and surrounding whitespace) into bytes. Git blob
/// ids and anything else malformed yield `None`, so they can never be
/// mistaken for a content checksum.
fn parse_sha256_hex(s: &str) -> Option<[u8; 32]> {
    let s = s.trim().trim_matches('"');
    if s.len() != 64 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = (bytes[i * 2] as char).to_digit(16)?;
        let lo = (bytes[i * 2 + 1] as char).to_digit(16)?;
        *byte = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

/// Lowercase hex encoding (for logs and error messages).
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Fetch per-file checksum metadata from a HuggingFace repo-info API.
///
/// Best-effort by design: any failure returns `None`, so downloads
/// degrade to size-only verification instead of being blocked.
async fn fetch_repo_file_metadata(
    client: &reqwest::Client,
    api_url: &str,
) -> Option<HashMap<String, RepoFileMeta>> {
    match client.get(api_url).send().await {
        Err(e) => {
            tracing::warn!("Could not fetch repo metadata from {}: {}", api_url, e);
            None
        }
        Ok(response) if !response.status().is_success() => {
            tracing::warn!(
                "Could not fetch repo metadata from {}: HTTP {}",
                api_url,
                response.status()
            );
            None
        }
        Ok(response) => match response.json::<serde_json::Value>().await {
            Ok(json) => Some(parse_repo_metadata(&json)),
            Err(e) => {
                tracing::warn!("Could not parse repo metadata from {}: {}", api_url, e);
                None
            }
        },
    }
}

/// Compare a finished download's digest with the published checksum.
fn verify_checksum(file: &str, expected: &[u8; 32], actual: &[u8; 32]) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        anyhow::bail!(
            "{}: SHA-256 checksum mismatch (expected {}, got {})",
            file,
            hex(expected),
            hex(actual)
        )
    }
}

/// Stream a file through SHA-256 in bounded memory, checking `cancel`
/// between chunks so huge files stay cancellable.
async fn sha256_file(
    path: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<[u8; 32]> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; FILE_CHUNK];
    loop {
        check_cancel(cancel)?;
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(digest_bytes(&hasher))
}

/// Feed the first `len` bytes of an existing `.part` file into `hasher`,
/// so a resumed download verifies the *assembled* file — prefix already
/// on disk plus the newly streamed tail — not just the tail.
async fn seed_hasher_from_prefix(
    part: &std::path::Path,
    len: u64,
    hasher: &mut Sha256,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(part).await?;
    let mut buf = vec![0u8; FILE_CHUNK];
    let mut hashed = 0u64;
    while hashed < len {
        check_cancel(cancel)?;
        let want = std::cmp::min((len - hashed) as usize, buf.len());
        let n = file.read_exact(&mut buf[..want]).await?;
        hasher.update(&buf[..n]);
        hashed += n as u64;
    }
    Ok(())
}

/// Finalize a hasher into a plain 32-byte array.
fn digest_bytes(hasher: &Sha256) -> [u8; 32] {
    let finalized = hasher.clone().finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&finalized);
    out
}

/// Why a single download attempt failed — decides whether to retry.
#[derive(Debug)]
enum AttemptError {
    /// User cancelled; do not retry.
    Cancelled,
    /// Server refused the download (4xx etc.); retrying won't help.
    Fatal(anyhow::Error),
    /// Network hiccup or truncated body; worth retrying.
    Transient(anyhow::Error),
}

impl std::fmt::Display for AttemptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttemptError::Cancelled => write!(f, "Download cancelled"),
            AttemptError::Fatal(e) => write!(f, "{}", e),
            AttemptError::Transient(e) => write!(f, "{}", e),
        }
    }
}

const MAX_ATTEMPTS: u32 = 3;

/// Base delay between retry attempts, scaled by the attempt number.
/// Production waits seconds; tests pass milliseconds.
const RETRY_BACKOFF_BASE: std::time::Duration = std::time::Duration::from_secs(1);

/// Disk read granularity when hashing existing files.
const FILE_CHUNK: usize = 64 * 1024;

/// Download one file to `dest`, streaming via a resumable `.part` file,
/// with up to MAX_ATTEMPTS tries on transient errors. When the repo
/// publishes a checksum, the assembled `.part` file must match it before
/// it is renamed into place, so `dest` never contains unverified bytes.
async fn download_file(
    client: &reqwest::Client,
    url: &str,
    dest: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
    meta: Option<&RepoFileMeta>,
    on_progress: &mut impl FnMut(u64, Option<u64>),
    retry_backoff: std::time::Duration,
) -> Result<()> {
    let file_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "model file".to_string());
    let part = part_path(dest);

    let mut last_err: Option<AttemptError> = None;
    for attempt in 1..=MAX_ATTEMPTS {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            anyhow::bail!("Download cancelled");
        }

        match try_download_file(client, url, dest, &part, cancel, meta, on_progress).await {
            Ok(()) => {
                tokio::fs::rename(&part, dest).await?;
                return Ok(());
            }
            Err(e @ AttemptError::Cancelled) => {
                // Keep the .part file so a later download can resume.
                return Err(anyhow::anyhow!("{}", e));
            }
            Err(e @ AttemptError::Fatal(_)) => {
                let _ = tokio::fs::remove_file(&part).await;
                return Err(anyhow::anyhow!("{}", e));
            }
            Err(AttemptError::Transient(e)) => {
                tracing::warn!(
                    "Download of {} failed (attempt {}/{}): {}",
                    file_name,
                    attempt,
                    MAX_ATTEMPTS,
                    e
                );
                last_err = Some(AttemptError::Transient(e));
                if attempt < MAX_ATTEMPTS {
                    let backoff = retry_backoff * attempt;
                    // Sleep in small slices so cancellation stays responsive.
                    let slices = backoff.as_millis() / 100 + 1;
                    for _ in 0..slices {
                        check_cancel(cancel)?;
                        tokio::time::sleep(backoff / slices as u32).await;
                    }
                }
            }
        }
    }

    Err(match last_err {
        Some(e) => anyhow::anyhow!("{} (after {} attempts)", e, MAX_ATTEMPTS),
        None => anyhow::anyhow!("Download of {} failed", file_name),
    })
}

/// One attempt at downloading `url` into `part`, resuming from any
/// existing partial content, then verifying size and — when the repo
/// publishes one — the SHA-256 checksum of the assembled file.
async fn try_download_file(
    client: &reqwest::Client,
    url: &str,
    dest: &std::path::Path,
    part: &std::path::Path,
    cancel: &std::sync::atomic::AtomicBool,
    meta: Option<&RepoFileMeta>,
    on_progress: &mut impl FnMut(u64, Option<u64>),
) -> std::result::Result<(), AttemptError> {
    let file_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "model file".to_string());

    let expected_sha256 = meta.and_then(|m| m.sha256);
    let expected_size = meta.and_then(|m| m.size);

    let partial_len = tokio::fs::metadata(part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let mut request = client.get(url);
    if partial_len > 0 {
        info!("Resuming {} from {} bytes", file_name, partial_len);
        request = request.header(reqwest::header::RANGE, format!("bytes={}-", partial_len));
    }

    let response = request
        .send()
        .await
        .map_err(|e| AttemptError::Transient(e.into()))?;

    let status = response.status();
    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // Our partial is larger than the remote file — discard and restart.
        info!("{}: server rejected range, discarding partial", file_name);
        let _ = tokio::fs::remove_file(part).await;
        return Err(AttemptError::Transient(anyhow::anyhow!(
            "Server rejected resume range for {}",
            file_name
        )));
    }
    if status.is_client_error() {
        return Err(AttemptError::Fatal(anyhow::anyhow!(
            "Failed to download {}: HTTP {}",
            file_name,
            status
        )));
    }
    if !status.is_success() {
        return Err(AttemptError::Transient(anyhow::anyhow!(
            "Failed to download {}: HTTP {}",
            file_name,
            status
        )));
    }

    let action = resume_action(partial_len, status);
    let (offset, append) = match action {
        ResumeAction::Append(n) => (n, true),
        ResumeAction::Restart => (0, false),
    };
    if partial_len > 0 && !append {
        info!(
            "{}: server ignored Range header, restarting from scratch",
            file_name
        );
    }

    // Total expected size: resumed responses report only the remaining
    // bytes; fall back to the repo metadata size when no Content-Length
    // is present (e.g. chunked transfer encoding).
    let expected_total = response
        .content_length()
        .map(|rest| offset + rest)
        .or(expected_size);

    // Hash the file as it is assembled. On a resume, seed the hasher
    // with the bytes already on disk so the final digest covers the
    // whole file.
    let mut hasher = Sha256::new();
    if append {
        if let Err(e) = seed_hasher_from_prefix(part, offset, &mut hasher, cancel).await {
            // Unreadable partial: discard it so the retry starts clean.
            let _ = tokio::fs::remove_file(part).await;
            return Err(AttemptError::Transient(e));
        }
    }

    let stream = response.bytes_stream();
    let downloaded = stream_to_disk(stream, part, append, offset, cancel, &mut hasher, |n| {
        on_progress(n, expected_total)
    })
    .await
    .map_err(|e| {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            AttemptError::Cancelled
        } else {
            AttemptError::Transient(e)
        }
    })?;

    // Integrity check 1: streamed byte count must match the expected size.
    if let Err(e) = verify_size(&file_name, expected_total, downloaded) {
        // Corrupt/truncated partial — delete it so the retry starts clean.
        let _ = tokio::fs::remove_file(part).await;
        return Err(AttemptError::Transient(e));
    }

    // Integrity check 2: the assembled file's SHA-256 must match the
    // repo's published checksum. A mismatch discards the partial so the
    // retry cannot re-assemble the same corrupt file.
    match expected_sha256 {
        Some(expected) => {
            let actual = digest_bytes(&hasher);
            if let Err(e) = verify_checksum(&file_name, &expected, &actual) {
                let _ = tokio::fs::remove_file(part).await;
                return Err(AttemptError::Transient(e));
            }
            debug!("{}: SHA-256 verified", file_name);
        }
        None => {
            debug!(
                "{}: no SHA-256 published (not LFS-backed); size check only",
                file_name
            );
        }
    }

    Ok(())
}

/// Stream response chunks to disk as they arrive, updating `hasher` with
/// every byte written, and returning the total number of bytes now in
/// the file (including any resumed `offset`).
async fn stream_to_disk<S, B, E>(
    mut stream: S,
    path: &std::path::Path,
    append: bool,
    offset: u64,
    cancel: &std::sync::atomic::AtomicBool,
    hasher: &mut Sha256,
    mut on_progress: impl FnMut(u64),
) -> Result<u64>
where
    S: futures_util::Stream<Item = std::result::Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
    E: std::error::Error + Send + Sync + 'static,
{
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(path)
        .await?;

    let mut downloaded = offset;
    while let Some(chunk) = stream.next().await {
        check_cancel(cancel)?;
        let chunk = chunk.map_err(anyhow::Error::from)?;
        hasher.update(chunk.as_ref());
        file.write_all(chunk.as_ref()).await?;
        downloaded += chunk.as_ref().len() as u64;
        on_progress(downloaded);
    }
    file.flush().await?;

    Ok(downloaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    /// Regression (canario-2cc): the CLI file-transcription engine used
    /// to hand corrupt tokens straight to sherpa's `ReadTokens`, which
    /// exits the whole process. `load_model` must now fail validation
    /// first, as a plain `Err`. (If this test binary survives, the
    /// guard held; before the fix it would die here.)
    #[test]
    fn load_model_rejects_corrupt_tokens_instead_of_exiting() {
        let dir = tempfile::tempdir().unwrap();
        let paths = validate::test_support::model_paths_in(dir.path());
        validate::test_support::write_model_files(&paths, b"\xff\xfe\x00definitely not text");

        let mut engine = TranscriptionEngine::from_paths(paths, 2);
        let err = engine.load_model().unwrap_err();
        assert!(err.to_string().contains("tokens.txt"), "error: {}", err);
    }

    /// Same guarantee when the tokens are fine but an ONNX file is
    /// corrupt: the error names the offending file, before any FFI.
    #[test]
    fn load_model_rejects_corrupt_onnx_instead_of_exiting() {
        let dir = tempfile::tempdir().unwrap();
        let paths = validate::test_support::model_paths_in(dir.path());
        validate::test_support::write_garbage_onnx_model(&paths);

        let mut engine = TranscriptionEngine::from_paths(paths, 2);
        let err = engine.load_model().unwrap_err();
        assert!(err.to_string().contains("encoder"), "error: {}", err);
    }

    /// Missing files keep the pre-existing "not found" error — the
    /// validation guard must not change that path.
    #[test]
    fn load_model_missing_files_still_reports_not_found() {
        let mut engine = TranscriptionEngine::new(
            std::path::Path::new("/nonexistent-canario-test-model").to_path_buf(),
            2,
        );
        let err = engine.load_model().unwrap_err();
        assert!(err.to_string().contains("not found"), "error: {}", err);
    }

    /// Smoke test against a real downloaded model: compatible tokens
    /// and ONNX files must pass validation and load in sherpa.
    #[test]
    #[ignore = "requires CANARIO_TEST_MODEL_DIR pointing to a downloaded model"]
    fn load_model_accepts_downloaded_model() {
        let dir = std::env::var_os("CANARIO_TEST_MODEL_DIR").expect("set CANARIO_TEST_MODEL_DIR");
        let mut engine = TranscriptionEngine::new(std::path::Path::new(&dir).to_path_buf(), 2);
        engine
            .load_model()
            .expect("valid downloaded model should pass validation and load");
    }

    #[test]
    fn part_path_appends_suffix() {
        let dest = std::path::Path::new("/tmp/model/encoder.int8.onnx");
        assert_eq!(
            part_path(dest),
            std::path::Path::new("/tmp/model/encoder.int8.onnx.part")
        );
    }

    #[test]
    fn resume_appends_only_on_206_with_partial() {
        use reqwest::StatusCode;
        assert_eq!(
            resume_action(100, StatusCode::PARTIAL_CONTENT),
            ResumeAction::Append(100)
        );
        assert_eq!(resume_action(100, StatusCode::OK), ResumeAction::Restart);
        assert_eq!(
            resume_action(0, StatusCode::PARTIAL_CONTENT),
            ResumeAction::Restart
        );
        assert_eq!(resume_action(0, StatusCode::OK), ResumeAction::Restart);
    }

    #[test]
    fn verify_size_checks_content_length() {
        assert!(verify_size("f", Some(10), 10).is_ok());
        assert!(verify_size("f", None, 999).is_ok());
        assert!(verify_size("f", Some(10), 9).is_err());
        assert!(verify_size("f", Some(10), 11).is_err());
    }

    #[test]
    fn check_cancel_bails_only_when_set() {
        let flag = no_cancel();
        assert!(check_cancel(&flag).is_ok());
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        let err = check_cancel(&flag).unwrap_err();
        assert_eq!(err.to_string(), "Download cancelled");
    }

    fn chunk_stream(
        chunks: Vec<&'static [u8]>,
    ) -> impl futures_util::Stream<Item = std::result::Result<&'static [u8], std::io::Error>> {
        futures_util::stream::iter(chunks.into_iter().map(Ok))
    }

    #[tokio::test]
    async fn stream_to_disk_writes_chunks_incrementally() {
        let dir = std::env::temp_dir().join(format!("canario-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.part");
        let mut progress = Vec::new();
        let mut hasher = Sha256::new();

        let written = stream_to_disk(
            chunk_stream(vec![b"hello", b" ", b"world"]),
            &path,
            false,
            0,
            &no_cancel(),
            &mut hasher,
            |n| progress.push(n),
        )
        .await
        .unwrap();

        assert_eq!(written, 11);
        assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
        assert_eq!(progress, vec![5, 6, 11]);
        assert_eq!(digest_bytes(&hasher), sha256_of(b"hello world"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn stream_to_disk_appends_at_resume_offset() {
        let dir = std::env::temp_dir().join(format!("canario-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.part");
        std::fs::write(&path, b"partial-").unwrap();
        let mut hasher = Sha256::new();

        let written = stream_to_disk(
            chunk_stream(vec![b"rest"]),
            &path,
            true,
            8,
            &no_cancel(),
            &mut hasher,
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(written, 12);
        assert_eq!(std::fs::read(&path).unwrap(), b"partial-rest");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn stream_to_disk_aborts_when_cancelled() {
        let dir = std::env::temp_dir().join(format!("canario-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.part");
        let cancel = AtomicBool::new(true);

        let result = stream_to_disk(
            chunk_stream(vec![b"data"]),
            &path,
            false,
            0,
            &cancel,
            &mut Sha256::new(),
            |_| {},
        )
        .await;

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn stream_to_disk_truncates_on_restart() {
        let dir = std::env::temp_dir().join(format!("canario-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.part");
        std::fs::write(&path, b"stale-stale-stale").unwrap();

        let written = stream_to_disk(
            chunk_stream(vec![b"new"]),
            &path,
            false,
            0,
            &no_cancel(),
            &mut Sha256::new(),
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(written, 3);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── checksum / metadata helpers ────────────────────────────────

    fn sha256_of(data: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(data);
        digest_bytes(&hasher)
    }

    #[test]
    fn parse_sha256_hex_accepts_real_digests_only() {
        let abc = sha256_of(b"abc");
        let hex_abc = hex(&abc);
        assert_eq!(hex_abc.len(), 64);
        assert_eq!(parse_sha256_hex(&hex_abc), Some(abc));
        assert_eq!(parse_sha256_hex(&hex_abc.to_uppercase()), Some(abc));
        // ETag-style quoting and surrounding whitespace are tolerated.
        assert_eq!(parse_sha256_hex(&format!("\"{}\"  ", hex_abc)), Some(abc));
        // Git blob ids are SHA-1 (40 hex chars) — never content SHA-256s.
        assert_eq!(
            parse_sha256_hex("f0742785f6073e80b911964c455b05f3609bf23b"),
            None
        );
        assert_eq!(parse_sha256_hex(""), None);
        assert_eq!(parse_sha256_hex("nothex!"), None);
        assert_eq!(parse_sha256_hex(&hex_abc[..63]), None);
        assert_eq!(
            parse_sha256_hex(&format!("{}0", hex_abc)), // 65 chars
            None
        );
    }

    #[test]
    fn parse_repo_metadata_maps_lfs_and_git_files() {
        // Shape captured from
        // https://huggingface.co/api/models/<repo>?blobs=true
        let json: serde_json::Value = serde_json::json!({
            "siblings": [
                {"rfilename": "encoder.int8.onnx",
                 "blobId": "8db161c9bec4c729541a602b9c362f623347aafa",
                 "size": 652184296,
                 "lfs": {"sha256": "a32b12d17bbbc309d0686fbbcc2987b5e9b8333a7da83fa6b089f0a2acd651ab",
                         "size": 652184296, "pointerSize": 134}},
                {"rfilename": "tokens.txt",
                 "blobId": "f0742785f6073e80b911964c455b05f3609bf23b", "size": 9384},
                {"rfilename": "test_wavs/0.wav",
                 "blobId": "599bd43aa4a04788b6063616455b4a5efceb8e05",
                 "size": 237964,
                 "lfs": {"sha256": "5fceacff0315d49cb59fcc505bcecf1ed5f2f35c2897b1e65a59f30e5d922150",
                         "size": 237964, "pointerSize": 131}},
                {"size": 12}
            ]
        });

        let meta = parse_repo_metadata(&json);

        assert_eq!(meta.len(), 3);
        // LFS file: content SHA-256 + size.
        let encoder = meta.get("encoder.int8.onnx").unwrap();
        assert_eq!(
            encoder.sha256,
            Some(
                parse_sha256_hex(
                    "a32b12d17bbbc309d0686fbbcc2987b5e9b8333a7da83fa6b089f0a2acd651ab"
                )
                .unwrap()
            )
        );
        assert_eq!(encoder.size, Some(652184296));
        // Plain git-tracked file (tokens.txt): no content checksum, size only.
        let tokens = meta.get("tokens.txt").unwrap();
        assert_eq!(tokens.sha256, None);
        assert_eq!(tokens.size, Some(9384));
        // Subdirectory files are keyed by their repo-relative path.
        let wav = meta.get("test_wavs/0.wav").unwrap();
        assert_eq!(
            wav.sha256,
            Some(
                parse_sha256_hex(
                    "5fceacff0315d49cb59fcc505bcecf1ed5f2f35c2897b1e65a59f30e5d922150"
                )
                .unwrap()
            )
        );
    }

    #[test]
    fn parse_repo_metadata_tolerates_missing_or_malformed_entries() {
        let json: serde_json::Value = serde_json::json!({
            "siblings": [
                {"rfilename": "a.bin", "lfs": {"sha256": "not-hex", "size": 1}},
                {"rfilename": "b.bin", "size": 7},
                {"nonsense": true}
            ]
        });
        let meta = parse_repo_metadata(&json);
        // Unparsable sha256 degrades to size-only, it does not fail the file.
        assert_eq!(meta.get("a.bin").unwrap().sha256, None);
        assert_eq!(meta.get("b.bin").unwrap().size, Some(7));

        let no_siblings: serde_json::Value = serde_json::json!({});
        assert!(parse_repo_metadata(&no_siblings).is_empty());
        let wrong_type: serde_json::Value = serde_json::json!({"siblings": 3});
        assert!(parse_repo_metadata(&wrong_type).is_empty());
    }

    #[test]
    fn verify_checksum_reports_expected_and_actual() {
        let good = sha256_of(b"good");
        let bad = sha256_of(b"bad!");
        assert!(verify_checksum("f.bin", &good, &good).is_ok());
        let err = verify_checksum("f.bin", &good, &bad).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("f.bin"), "message: {}", msg);
        assert!(msg.contains(hex(&good).as_str()), "message: {}", msg);
        assert!(msg.contains(hex(&bad).as_str()), "message: {}", msg);
    }

    #[tokio::test]
    async fn sha256_file_hashes_content_and_respects_cancel() {
        let dir = std::env::temp_dir().join(format!("canario-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.bin");
        std::fs::write(&path, b"abc").unwrap();

        assert_eq!(
            sha256_file(&path, &no_cancel()).await.unwrap(),
            sha256_of(b"abc")
        );

        let cancelled = AtomicBool::new(true);
        let err = sha256_file(&path, &cancelled).await.unwrap_err();
        assert_eq!(err.to_string(), "Download cancelled");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn seeded_hasher_covers_prefix_plus_streamed_tail() {
        let dir = std::env::temp_dir().join(format!("canario-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.part");
        std::fs::write(&path, b"hello wor").unwrap();

        let mut hasher = Sha256::new();
        seed_hasher_from_prefix(&path, 9, &mut hasher, &no_cancel())
            .await
            .unwrap();

        let cancelled = AtomicBool::new(true);
        assert!(
            seed_hasher_from_prefix(&path, 9, &mut Sha256::new(), &cancelled)
                .await
                .is_err()
        );

        hasher.update(b"ld");
        assert_eq!(digest_bytes(&hasher), sha256_of(b"hello world"));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── offline end-to-end download tests (loopback HTTP) ──────────

    /// A parsed incoming request: path plus the start offset of a
    /// `bytes=N-` Range header, if any.
    struct TestRequest {
        path: String,
        range_start: Option<u64>,
    }

    struct TestResponse {
        status: u16,
        extra_headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl TestResponse {
        fn ok(body: Vec<u8>) -> Self {
            Self {
                status: 200,
                extra_headers: Vec::new(),
                body,
            }
        }

        fn json(body: String) -> Self {
            Self {
                status: 200,
                extra_headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                body: body.into_bytes(),
            }
        }

        fn not_found() -> Self {
            Self {
                status: 404,
                extra_headers: Vec::new(),
                body: b"gone".to_vec(),
            }
        }

        fn server_error() -> Self {
            Self {
                status: 500,
                extra_headers: Vec::new(),
                body: b"boom".to_vec(),
            }
        }

        fn range(bytes: &[u8], start: usize) -> Self {
            Self {
                status: 206,
                extra_headers: vec![(
                    "Content-Range".to_string(),
                    format!("bytes {}-{}/{}", start, bytes.len() - 1, bytes.len()),
                )],
                body: bytes[start..].to_vec(),
            }
        }

        fn range_unsatisfiable() -> Self {
            Self {
                status: 416,
                extra_headers: Vec::new(),
                body: Vec::new(),
            }
        }
    }

    fn reason(status: u16) -> &'static str {
        match status {
            200 => "OK",
            206 => "Partial Content",
            404 => "Not Found",
            416 => "Range Not Satisfiable",
            500 => "Internal Server Error",
            _ => "Unknown",
        }
    }

    /// Serve `bytes` honouring a `bytes=N-` Range request, like the
    /// HuggingFace CDN does.
    fn serve_bytes(bytes: &[u8], range_start: Option<u64>) -> TestResponse {
        match range_start {
            None => TestResponse::ok(bytes.to_vec()),
            Some(start) if (start as usize) < bytes.len() => {
                TestResponse::range(bytes, start as usize)
            }
            Some(_) => TestResponse::range_unsatisfiable(),
        }
    }

    /// The four model files the downloader fetches, as tiny fakes.
    /// The third tuple element: does the repo publish an LFS sha256?
    fn fake_repo_files() -> Vec<(String, Vec<u8>, bool)> {
        vec![
            (
                "encoder.int8.onnx".to_string(),
                b"fake encoder payload".to_vec(),
                true,
            ),
            ("decoder.int8.onnx".to_string(), b"decoder!".to_vec(), true),
            ("joiner.int8.onnx".to_string(), b"jj".to_vec(), true),
            ("tokens.txt".to_string(), b"a b c\n".to_vec(), false),
        ]
    }

    /// Build a repo-info response body. With `publish_bad_hash`, LFS
    /// files advertise the checksum of *different* content.
    fn siblings_json(files: &[(String, Vec<u8>, bool)], publish_bad_hash: bool) -> String {
        let siblings: Vec<serde_json::Value> = files
            .iter()
            .map(|(name, data, lfs)| {
                if *lfs {
                    let digest = if publish_bad_hash {
                        sha256_of(format!("{}-corrupt", name).as_bytes())
                    } else {
                        sha256_of(data)
                    };
                    serde_json::json!({
                        "rfilename": name,
                        "blobId": "0000000000000000000000000000000000000000",
                        "size": data.len(),
                        "lfs": {"sha256": hex(&digest), "size": data.len()},
                    })
                } else {
                    serde_json::json!({
                        "rfilename": name,
                        "blobId": "0000000000000000000000000000000000000000",
                        "size": data.len(),
                    })
                }
            })
            .collect();
        serde_json::json!({ "siblings": siblings }).to_string()
    }

    /// Request log: (path, range_start) in arrival order.
    type RequestLog = std::sync::Arc<Mutex<Vec<(String, Option<u64>)>>>;

    /// Minimal HTTP/1.1 server bound to loopback; one thread, one
    /// connection per request (`Connection: close`).
    struct TestServer {
        base: String,
        shutdown: std::sync::Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl TestServer {
        fn start<F>(handler: F) -> Self
        where
            F: Fn(&TestRequest) -> TestResponse + Send + Sync + 'static,
        {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let shutdown = std::sync::Arc::new(AtomicBool::new(false));
            let flag = shutdown.clone();
            let handle = std::thread::spawn(move || {
                let handler = std::sync::Arc::new(handler);
                while !flag.load(std::sync::atomic::Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => handle_connection(stream, handler.as_ref()),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                base: format!("http://{}", addr),
                shutdown,
                handle: Some(handle),
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.shutdown
                .store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn handle_connection<F>(stream: TcpStream, handler: &F)
    where
        F: Fn(&TestRequest) -> TestResponse,
    {
        let stream_clone = match stream.try_clone() {
            Ok(clone) => clone,
            Err(_) => return,
        };
        let mut reader = BufReader::new(stream_clone);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            return;
        }
        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("/")
            .to_string();

        let mut range_start = None;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("range") {
                    let spec = value.trim().strip_prefix("bytes=").unwrap_or("");
                    if let Some(start) = spec.split('-').next() {
                        range_start = start.parse().ok();
                    }
                }
            }
        }

        let response = handler(&TestRequest { path, range_start });

        let mut out = format!(
            "HTTP/1.1 {} {}\r\n",
            response.status,
            reason(response.status)
        );
        out.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
        out.push_str("Connection: close\r\n");
        for (name, value) in &response.extra_headers {
            out.push_str(&format!("{}: {}\r\n", name, value));
        }
        out.push_str("\r\n");

        let mut stream = stream;
        let _ = stream.write_all(out.as_bytes());
        let _ = stream.write_all(&response.body);
        let _ = stream.flush();
    }

    fn temp_model_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("canario-dl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn event_channel() -> (
        std::sync::mpsc::Sender<crate::event::Event>,
        std::sync::mpsc::Receiver<crate::event::Event>,
    ) {
        std::sync::mpsc::channel()
    }

    fn final_progress(rx: &std::sync::mpsc::Receiver<crate::event::Event>) -> Option<f64> {
        let mut last = None;
        while let Ok(crate::event::Event::ModelDownloadProgress { progress }) = rx.try_recv() {
            last = Some(progress);
        }
        last
    }

    fn start_repo_server(
        files: Vec<(String, Vec<u8>, bool)>,
        metadata: String,
        log: RequestLog,
    ) -> TestServer {
        TestServer::start(move |req| {
            log.lock()
                .unwrap()
                .push((req.path.clone(), req.range_start));
            if req.path.starts_with("/api/models/") {
                return TestResponse::json(metadata.clone());
            }
            let name = req.path.rsplit('/').next().unwrap_or("");
            match files.iter().find(|(n, _, _)| n == name) {
                Some((_, data, _)) => serve_bytes(data, req.range_start),
                None => TestResponse::not_found(),
            }
        })
    }

    async fn run_download(server: &TestServer, dir: &std::path::Path) -> Result<()> {
        let (tx, _rx) = event_channel();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        download_model_from(
            &server.base,
            dir,
            "test/repo",
            &tx,
            cancel,
            std::time::Duration::from_millis(1),
        )
        .await
    }

    #[tokio::test]
    async fn download_verifies_checksums_and_completes() {
        let files = fake_repo_files();
        let server = start_repo_server(
            files.clone(),
            siblings_json(&files, false),
            std::sync::Arc::new(Mutex::new(Vec::new())),
        );
        let (tx, rx) = event_channel();
        let dir = temp_model_dir();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));

        download_model_from(
            &server.base,
            &dir,
            "test/repo",
            &tx,
            cancel,
            std::time::Duration::from_millis(1),
        )
        .await
        .unwrap();

        for (name, data, _) in &files {
            assert_eq!(&std::fs::read(dir.join(name)).unwrap(), data, "{}", name);
            assert!(!dir.join(format!("{}.part", name)).exists(), "{}", name);
        }
        assert_eq!(final_progress(&rx), Some(1.0));
    }

    #[tokio::test]
    async fn download_fails_when_checksum_mismatches() {
        let files = fake_repo_files();
        let log: RequestLog = std::sync::Arc::new(Mutex::new(Vec::new()));
        let server = start_repo_server(files.clone(), siblings_json(&files, true), log.clone());
        let dir = temp_model_dir();

        let err = run_download(&server, &dir).await.unwrap_err().to_string();

        assert!(err.contains("SHA-256 checksum mismatch"), "error: {}", err);
        assert!(err.contains("encoder.int8.onnx"), "error: {}", err);
        assert!(err.contains("after 3 attempts"), "error: {}", err);
        // No corrupt file may be left behind, completed or partial.
        assert!(!dir.join("encoder.int8.onnx").exists());
        assert!(!dir.join("encoder.int8.onnx.part").exists());
        // The first file exhausts its retries; later files are untouched.
        let resolve_requests = log
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.contains("/resolve/"))
            .count();
        assert_eq!(resolve_requests, 3);
    }

    #[tokio::test]
    async fn resumed_download_passes_checksum_over_whole_file() {
        let files = fake_repo_files();
        let log: RequestLog = std::sync::Arc::new(Mutex::new(Vec::new()));
        let server = start_repo_server(files.clone(), siblings_json(&files, false), log.clone());
        let dir = temp_model_dir();

        // Half-finished encoder from a previous, cancelled session.
        let encoder = &files[0].1;
        std::fs::write(dir.join("encoder.int8.onnx.part"), &encoder[..5]).unwrap();

        run_download(&server, &dir).await.unwrap();

        assert_eq!(
            std::fs::read(dir.join("encoder.int8.onnx")).unwrap(),
            *encoder
        );
        assert!(!dir.join("encoder.int8.onnx.part").exists());

        // Exactly one encoder request, resumed at byte 5: the assembled
        // file (prefix + streamed tail) passed verification. If the
        // hasher only covered the tail, a full re-download would show up
        // here as a second, Range-less request.
        let encoder_requests: Vec<Option<u64>> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.ends_with("/encoder.int8.onnx"))
            .map(|(_, r)| *r)
            .collect();
        assert_eq!(encoder_requests, vec![Some(5)]);
    }

    #[tokio::test]
    async fn corrupt_resumed_partial_is_detected_and_redownloaded() {
        let files = fake_repo_files();
        let log: RequestLog = std::sync::Arc::new(Mutex::new(Vec::new()));
        let server = start_repo_server(files.clone(), siblings_json(&files, false), log.clone());
        let dir = temp_model_dir();

        // A partial whose length looks plausible but whose bytes are not
        // the encoder's: sizes line up after the 206, only the checksum
        // can catch it.
        std::fs::write(dir.join("encoder.int8.onnx.part"), b"XXXX").unwrap();

        run_download(&server, &dir).await.unwrap();

        let encoder = &files[0].1;
        assert_eq!(
            std::fs::read(dir.join("encoder.int8.onnx")).unwrap(),
            *encoder
        );
        assert!(!dir.join("encoder.int8.onnx.part").exists());

        let encoder_requests: Vec<Option<u64>> = log
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.ends_with("/encoder.int8.onnx"))
            .map(|(_, r)| *r)
            .collect();
        // First attempt resumed onto the corrupt prefix; the mismatch
        // discarded it, and the retry fetched the whole file cleanly.
        assert_eq!(encoder_requests, vec![Some(4), None]);
    }

    #[tokio::test]
    async fn existing_files_are_verified_before_being_skipped() {
        let files = fake_repo_files();
        let log: RequestLog = std::sync::Arc::new(Mutex::new(Vec::new()));
        let server = start_repo_server(files.clone(), siblings_json(&files, false), log.clone());
        let dir = temp_model_dir();

        // encoder: same size, different bytes → checksum fails, re-download.
        std::fs::write(dir.join("encoder.int8.onnx"), b"Fake encoder payload").unwrap();
        // decoder: correct content → verified and skipped.
        std::fs::write(dir.join("decoder.int8.onnx"), &files[1].1).unwrap();
        // joiner: wrong size → size check fails, re-download.
        std::fs::write(dir.join("joiner.int8.onnx"), b"too long").unwrap();
        // tokens: no published hash, and the size still matches → kept
        // as-is (a same-size content change is undetectable without a
        // checksum; a wrong size would still trigger a re-download).
        std::fs::write(dir.join("tokens.txt"), b"ZZZZZZ").unwrap();

        run_download(&server, &dir).await.unwrap();

        assert_eq!(
            std::fs::read(dir.join("encoder.int8.onnx")).unwrap(),
            files[0].1
        );
        assert_eq!(
            std::fs::read(dir.join("decoder.int8.onnx")).unwrap(),
            files[1].1
        );
        assert_eq!(
            std::fs::read(dir.join("joiner.int8.onnx")).unwrap(),
            files[2].1
        );
        assert_eq!(std::fs::read(dir.join("tokens.txt")).unwrap(), b"ZZZZZZ");

        let log = log.lock().unwrap();
        let count = |suffix: &str| log.iter().filter(|(p, _)| p.ends_with(suffix)).count();
        assert_eq!(count("/encoder.int8.onnx"), 1); // re-downloaded
        assert_eq!(count("/decoder.int8.onnx"), 0); // verified + skipped
        assert_eq!(count("/joiner.int8.onnx"), 1); // re-downloaded
        assert_eq!(count("/tokens.txt"), 0); // unverifiable → skipped
    }

    #[tokio::test]
    async fn unavailable_metadata_api_still_downloads() {
        let files = fake_repo_files();
        let log: RequestLog = std::sync::Arc::new(Mutex::new(Vec::new()));
        let served_files = files.clone();
        let server = TestServer::start(move |req| {
            log.lock()
                .unwrap()
                .push((req.path.clone(), req.range_start));
            if req.path.starts_with("/api/models/") {
                return TestResponse::server_error();
            }
            let name = req.path.rsplit('/').next().unwrap_or("");
            match served_files.iter().find(|(n, _, _)| n == name) {
                Some((_, data, _)) => serve_bytes(data, req.range_start),
                None => TestResponse::not_found(),
            }
        });
        let dir = temp_model_dir();

        // The metadata API 500s: downloads must degrade to size-only
        // verification instead of failing.
        run_download(&server, &dir).await.unwrap();

        for (name, data, _) in &files {
            assert_eq!(&std::fs::read(dir.join(name)).unwrap(), data, "{}", name);
        }
    }

    #[tokio::test]
    async fn missing_repo_file_fails_with_useful_error() {
        let files = fake_repo_files();
        let log: RequestLog = std::sync::Arc::new(Mutex::new(Vec::new()));
        let metadata = siblings_json(&files, false);
        let served_files = files.clone();
        let server_log = log.clone();
        let server = TestServer::start(move |req| {
            server_log
                .lock()
                .unwrap()
                .push((req.path.clone(), req.range_start));
            if req.path.starts_with("/api/models/") {
                return TestResponse::json(metadata.clone());
            }
            if req.path.ends_with("/encoder.int8.onnx") {
                return TestResponse::not_found();
            }
            let name = req.path.rsplit('/').next().unwrap_or("");
            match served_files.iter().find(|(n, _, _)| n == name) {
                Some((_, data, _)) => serve_bytes(data, req.range_start),
                None => TestResponse::not_found(),
            }
        });
        let dir = temp_model_dir();

        let err = run_download(&server, &dir).await.unwrap_err().to_string();

        assert!(err.contains("HTTP 404"), "error: {}", err);
        assert!(err.contains("encoder.int8.onnx"), "error: {}", err);
        assert!(!dir.join("encoder.int8.onnx").exists());
        assert!(!dir.join("encoder.int8.onnx.part").exists());
        // 404 is fatal: no retries burned on it.
        let encoder_requests = log
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.ends_with("/encoder.int8.onnx"))
            .count();
        assert_eq!(encoder_requests, 1);
    }

    #[tokio::test]
    async fn cancelled_download_bails_and_preserves_partial() {
        let files = fake_repo_files();
        let log: RequestLog = std::sync::Arc::new(Mutex::new(Vec::new()));
        let server =
            start_repo_server(files, siblings_json(&fake_repo_files(), false), log.clone());
        let dir = temp_model_dir();
        std::fs::write(dir.join("encoder.int8.onnx.part"), b"par").unwrap();

        let (tx, _rx) = event_channel();
        let cancel = std::sync::Arc::new(AtomicBool::new(true));
        let err = download_model_from(
            &server.base,
            &dir,
            "test/repo",
            &tx,
            cancel,
            std::time::Duration::from_millis(1),
        )
        .await
        .unwrap_err();

        assert_eq!(err.to_string(), "Download cancelled");
        // The partial is kept for a future resumable attempt, and no
        // requests were made at all.
        assert!(dir.join("encoder.int8.onnx.part").exists());
        assert!(log.lock().unwrap().is_empty());
    }
}
