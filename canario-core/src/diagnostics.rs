//! Diagnostics collection for support / bug reports.
//!
//! [`collect`] gathers everything needed to debug a user report into one
//! JSON-serializable blob: versions, OS + display server, current config,
//! model download status, paste/audio tool availability, and the tail of
//! the most recent log file. Everything is local — nothing is redacted
//! and nothing leaves the machine (frontends decide what to do with it).
//!
//! Also home to the shared logging-path helpers ([`log_dir`],
//! [`DEFAULT_LOG_FILTER`], [`LOG_FILE_PREFIX`]) so every binary writes
//! its rolling log files to the same place with the same filter policy.
//! canario-core itself stays subscriber-free: the binaries install the
//! `tracing` subscriber.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Log file prefix used by the binaries' rolling file appenders.
/// `tracing_appender::rolling::daily(dir, LOG_FILE_PREFIX)` produces
/// files like `canario.log.2026-09-10`.
pub const LOG_FILE_PREFIX: &str = "canario.log";

/// Default `EnvFilter` directives: `info` for all canario crates,
/// `warn` for dependencies. Overridden entirely by `RUST_LOG`.
pub const DEFAULT_LOG_FILTER: &str =
    "warn,canario=info,canario_core=info,canario_cli=info,canario_gtk=info,canario_electron=info";

/// Number of log lines included in the diagnostics tail.
pub const LOG_TAIL_LINES: usize = 50;

/// Directory where rolling log files live:
/// `$XDG_STATE_HOME/canario/logs/` (or `~/.local/state/canario/logs/`).
pub fn log_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("~/.local/state"))
        .join("canario")
        .join("logs")
}

/// Which frontend produced this diagnostics blob.
#[derive(Debug, Serialize)]
pub struct FrontendInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct SystemInfo {
    /// `std::env::consts::OS` (e.g. "linux")
    pub os: String,
    /// `std::env::consts::ARCH` (e.g. "x86_64")
    pub arch: String,
    /// Kernel release (`uname -sr`), when available.
    pub kernel: Option<String>,
    /// "x11" | "wayland" | "unknown"
    pub display_server: String,
}

/// Per-model-file status.
#[derive(Debug, Serialize)]
pub struct ModelFileInfo {
    pub path: PathBuf,
    pub exists: bool,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ModelInfo {
    /// Selected variant, e.g. "ParakeetV3".
    pub variant: String,
    pub downloaded: bool,
    /// Why paths couldn't be resolved (custom variant misconfigured).
    pub paths_error: Option<String>,
    pub files: Vec<ModelFileInfo>,
}

/// Availability of external tools Canario shells out to.
#[derive(Debug, Serialize)]
pub struct ToolAvailability {
    pub xdotool: bool,
    pub wtype: bool,
    pub ydotool: bool,
    pub pactl: bool,
}

#[derive(Debug, Serialize)]
pub struct LogInfo {
    pub dir: PathBuf,
    /// Most recently modified log file, if any.
    pub latest_file: Option<PathBuf>,
    /// Last ~[`LOG_TAIL_LINES`] lines of `latest_file`.
    pub tail: Vec<String>,
}

/// The complete diagnostics blob returned by the sidecar's `diagnostics`
/// command and printed by `canario-cli --diagnostics`.
#[derive(Debug, Serialize)]
pub struct Diagnostics {
    /// canario-core library version.
    pub core_version: String,
    pub frontend: FrontendInfo,
    pub system: SystemInfo,
    pub config_path: PathBuf,
    pub config: serde_json::Value,
    pub model: ModelInfo,
    pub tools: ToolAvailability,
    pub logs: LogInfo,
}

/// Collect the full diagnostics blob. Best-effort and infallible:
/// individual probes degrade to `None`/`false` rather than failing.
pub fn collect(frontend_name: &str, frontend_version: &str) -> Diagnostics {
    let config = crate::config::AppConfig::load().unwrap_or_else(|e| {
        tracing::warn!("diagnostics: failed to load config, using defaults: {}", e);
        crate::config::AppConfig::default()
    });

    Diagnostics {
        core_version: env!("CARGO_PKG_VERSION").to_string(),
        frontend: FrontendInfo {
            name: frontend_name.to_string(),
            version: frontend_version.to_string(),
        },
        system: collect_system(),
        config_path: crate::config::AppConfig::config_file(),
        config: serde_json::to_value(&config).unwrap_or(serde_json::Value::Null),
        model: collect_model(&config),
        tools: collect_tools(),
        logs: collect_logs(),
    }
}

fn collect_system() -> SystemInfo {
    SystemInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        kernel: std::process::Command::new("uname")
            .arg("-sr")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()),
        display_server: display_server(),
    }
}

#[cfg(target_os = "linux")]
fn display_server() -> String {
    format!("{:?}", crate::hotkey::detect_display_server()).to_lowercase()
}

#[cfg(not(target_os = "linux"))]
fn display_server() -> String {
    "unsupported".to_string()
}

fn collect_model(config: &crate::config::AppConfig) -> ModelInfo {
    let (paths_error, files) = match config.model_paths() {
        Ok(paths) => {
            let files = [paths.encoder, paths.decoder, paths.joiner, paths.tokens]
                .into_iter()
                .map(|path| {
                    let meta = std::fs::metadata(&path).ok();
                    ModelFileInfo {
                        exists: meta.is_some(),
                        size_bytes: meta.map(|m| m.len()),
                        path,
                    }
                })
                .collect();
            (None, files)
        }
        Err(e) => (Some(e.to_string()), Vec::new()),
    };

    ModelInfo {
        variant: format!("{:?}", config.model),
        downloaded: config.is_model_downloaded(),
        paths_error,
        files,
    }
}

fn collect_tools() -> ToolAvailability {
    ToolAvailability {
        xdotool: probe_command("xdotool"),
        wtype: probe_command("wtype"),
        ydotool: probe_command("ydotool"),
        pactl: probe_command("pactl"),
    }
}

/// Probe for a command on PATH. Diagnostics re-probes every call (no
/// caching) so a bundle reflects the system *right now*.
fn probe_command(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn collect_logs() -> LogInfo {
    let dir = log_dir();
    let latest_file = latest_log_file(&dir);
    let tail = latest_file
        .as_deref()
        .map(|p| tail_lines(p, LOG_TAIL_LINES))
        .unwrap_or_default();
    LogInfo {
        dir,
        latest_file,
        tail,
    }
}

/// Most recently modified log file in `dir` (files named
/// `canario.log[.YYYY-MM-DD]`).
fn latest_log_file(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with(LOG_FILE_PREFIX))
                .unwrap_or(false)
        })
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

/// Last `n` lines of `path`. Reads at most the final 256 KiB so a huge
/// log file doesn't stall diagnostics.
fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(256 * 1024);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return Vec::new();
    }
    let lines: Vec<String> = buf.lines().map(|l| l.to_string()).collect();
    // If we seeked into the middle of a line, drop the partial first line.
    let start_idx = usize::from(start > 0 && !lines.is_empty());
    lines[start_idx..]
        .iter()
        .rev()
        .take(n)
        .rev()
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_lines_returns_last_n_lines() {
        let dir = std::env::temp_dir().join(format!("canario-diag-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("canario.log.2026-09-10");
        let content: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
        std::fs::write(&file, content).unwrap();

        let tail = tail_lines(&file, 50);
        assert_eq!(tail.len(), 50);
        assert_eq!(tail.first().unwrap(), "line 51");
        assert_eq!(tail.last().unwrap(), "line 100");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn latest_log_file_ignores_unrelated_files() {
        let dir = std::env::temp_dir().join(format!("canario-diag-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("unrelated.txt"), b"x").unwrap();
        let log = dir.join("canario.log.2026-09-10");
        std::fs::write(&log, b"x").unwrap();

        assert_eq!(latest_log_file(&dir), Some(log));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tail_lines_missing_file_is_empty() {
        assert!(tail_lines(Path::new("/nonexistent/canario.log"), 50).is_empty());
    }

    #[test]
    fn log_dir_is_under_state_home() {
        // With XDG_STATE_HOME set, the log dir must live under it.
        // (dirs::state_dir falls back to ~/.local/state when unset;
        // either way the path must end in canario/logs.)
        let dir = log_dir();
        assert!(dir.ends_with("canario/logs") || dir.ends_with("canario\\logs"));
    }
}
