//! Canario Electron Sidecar
//!
//! JSON stdin/stdout bridge over `canario-core`.
//!
//! Reads newline-delimited JSON commands from stdin, executes them via
//! `canario::Canario`, and writes newline-delimited JSON events + responses
//! to stdout.

use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

// ── Command types ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd")]
enum Command {
    #[serde(rename = "start_recording")]
    StartRecording { id: String },
    #[serde(rename = "stop_recording")]
    StopRecording { id: String },
    #[serde(rename = "toggle_recording")]
    ToggleRecording { id: String },
    #[serde(rename = "download_model")]
    DownloadModel { id: String },
    #[serde(rename = "delete_model")]
    DeleteModel { id: String },
    #[serde(rename = "is_model_downloaded")]
    IsModelDownloaded {
        id: String,
        model: Option<canario_core::ModelVariant>,
    },
    #[serde(rename = "get_config")]
    GetConfig { id: String },
    #[serde(rename = "update_config")]
    UpdateConfig {
        id: String,
        config: serde_json::Value,
    },
    #[serde(rename = "get_history")]
    GetHistory { id: String, limit: Option<usize> },
    #[serde(rename = "search_history")]
    SearchHistory { id: String, query: String },
    #[serde(rename = "delete_history")]
    DeleteHistory {
        id: String,
        /// Entry to delete. The Electron frontend sends `target_id`
        /// (canario-app/src/renderer/primitives/createCanario.ts), while
        /// PRD-ELECTRON.md Appendix A documents `id`. Accept both:
        /// `target_id`/`entry_id` wins if present, otherwise fall back
        /// to `id` (which then doubles as request id and entry id).
        #[serde(default, alias = "target_id")]
        entry_id: Option<String>,
    },
    #[serde(rename = "clear_history")]
    ClearHistory { id: String },
    #[serde(rename = "start_hotkey")]
    StartHotkey { id: String },
    #[serde(rename = "stop_hotkey")]
    StopHotkey { id: String },
    #[serde(rename = "restart_hotkey")]
    RestartHotkey { id: String },
    #[serde(rename = "ping")]
    Ping { id: String },
    #[serde(rename = "diagnostics")]
    Diagnostics { id: String },
    #[serde(rename = "shutdown")]
    Shutdown { id: String },
}

// ── Response types ───────────────────────────────────────────────────────────

#[derive(Serialize)]
struct OkResponse {
    id: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct ErrResponse {
    id: String,
    ok: bool,
    error: String,
}

fn ok(id: impl Into<String>) -> OkResponse {
    OkResponse {
        id: id.into(),
        ok: true,
        data: None,
    }
}

fn ok_data(id: impl Into<String>, data: serde_json::Value) -> OkResponse {
    OkResponse {
        id: id.into(),
        ok: true,
        data: Some(data),
    }
}

fn err(id: impl Into<String>, msg: impl Into<String>) -> ErrResponse {
    ErrResponse {
        id: id.into(),
        ok: false,
        error: msg.into(),
    }
}

// ── Event forwarding ────────────────────────────────────────────────────────

/// Serialize a canario-core Event to JSON manually.
/// This works because Event has #[derive(serde::Serialize)].
fn serialize_event(event: &canario_core::Event) -> String {
    serde_json::to_string(event).unwrap()
}

fn write_json<T: Serialize>(val: &T) {
    let mut stdout = std::io::stdout().lock();
    match serde_json::to_string(val) {
        Ok(json) => {
            let _ = writeln!(stdout, "{}", json);
        }
        Err(e) => {
            error!("Failed to serialize: {}", e);
        }
    }
    let _ = stdout.flush();
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() -> anyhow::Result<()> {
    // Send tracing to stderr (stdout stays clean for JSON IPC) only in
    // debug builds / when RUST_LOG is set; logs always go to a daily-
    // rotated file under the XDG state dir. The guard must stay alive
    // until exit — dropping it flushes buffered log lines.
    let mut log_guard = init_logging();

    info!("Canario Electron sidecar starting...");

    let (canario, rx) = canario_core::Canario::new()?;

    // Spawn event forwarder thread: reads from canario-core channel,
    // writes JSON events to stdout.
    let event_tx_canario = canario.clone();
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            // Special handling: auto-add transcription to history
            // Note: auto-paste is handled by the Electron main process, not the sidecar.
            // The sidecar only adds to history here.
            if let canario_core::Event::TranscriptionReady {
                ref text,
                duration_secs,
            } = event
            {
                event_tx_canario.add_history(text.clone(), duration_secs, None);
            }
            let json = serialize_event(&event);
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{}", json);
            let _ = stdout.flush();
        }
        info!("Event channel closed, sidecar exiting");
        std::process::exit(0);
    });

    // Read commands from stdin
    let stdin = std::io::stdin();
    let reader = stdin.lock();

    info!("Sidecar ready, reading commands from stdin...");

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                info!("Stdin closed: {}", e);
                break;
            }
        };

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let cmd: Command = match serde_json::from_str(line) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to parse command: {} — input: {}", e, line);
                // Best-effort: recover the `id` from the raw JSON so the
                // frontend's id-matched promise resolves instead of
                // hitting its 10s timeout.
                let id = serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .and_then(|v| v.get("id").and_then(|i| i.as_str().map(String::from)))
                    .unwrap_or_else(|| "unknown".to_string());
                write_json(&err(id, format!("invalid command: {}", e)));
                continue;
            }
        };

        handle_command(&canario, cmd, &mut log_guard);
    }

    canario.shutdown();
    info!("Sidecar shutting down");
    Ok(())
}

/// Install the tracing subscriber: daily-rotated, non-blocking log file
/// under the XDG state dir (always) + stderr in debug builds or when
/// `RUST_LOG` is set (stdout is reserved for JSON IPC).
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::prelude::*;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(canario_core::diagnostics::DEFAULT_LOG_FILTER)
    });

    let stderr_enabled = cfg!(debug_assertions) || std::env::var_os("RUST_LOG").is_some();

    let log_dir = canario_core::diagnostics::log_dir();
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "⚠  Cannot create log dir {:?}: {} — logging to stderr only",
            log_dir, e
        );
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .init();
        return None;
    }

    let file_appender =
        tracing_appender::rolling::daily(&log_dir, canario_core::diagnostics::LOG_FILE_PREFIX);
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .with(stderr_enabled.then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr)))
        .init();

    Some(guard)
}

fn handle_command(
    canario: &canario_core::Canario,
    cmd: Command,
    log_guard: &mut Option<tracing_appender::non_blocking::WorkerGuard>,
) {
    match cmd {
        Command::StartRecording { id } => match canario.start_recording() {
            Ok(()) => write_json(&ok(&id)),
            Err(e) => write_json(&err(&id, e.to_string())),
        },
        Command::StopRecording { id } => {
            canario.stop_recording();
            write_json(&ok(&id));
        }
        Command::ToggleRecording { id } => {
            let recording = canario.toggle_recording();
            write_json(&ok_data(&id, serde_json::json!({ "recording": recording })));
        }
        Command::DownloadModel { id } => match canario.download_model() {
            Ok(()) => write_json(&ok(&id)),
            Err(e) => write_json(&err(&id, e.to_string())),
        },
        Command::DeleteModel { id } => match canario.delete_model() {
            Ok(()) => write_json(&ok(&id)),
            Err(e) => write_json(&err(&id, e.to_string())),
        },
        Command::IsModelDownloaded { id, model } => {
            let mut config = canario.config();
            if let Some(model) = model {
                config.model = model;
            }
            let downloaded = config.is_model_downloaded();
            write_json(&ok_data(&id, serde_json::json!(downloaded)));
        }
        Command::GetConfig { id } => {
            let config = canario.config();
            write_json(&ok_data(&id, serde_json::to_value(&config).unwrap()));
        }
        Command::UpdateConfig { id, config } => {
            let result = canario.update_config(|c| merge_config(c, &config));
            match result {
                Ok(()) => write_json(&ok(&id)),
                Err(e) => write_json(&err(&id, e.to_string())),
            }
        }
        Command::GetHistory { id, limit } => {
            let entries = canario.recent_history(limit.unwrap_or(50));
            write_json(&ok_data(&id, serde_json::to_value(&entries).unwrap()));
        }
        Command::SearchHistory { id, query } => {
            let entries = canario.search_history(&query);
            write_json(&ok_data(&id, serde_json::to_value(&entries).unwrap()));
        }
        Command::DeleteHistory { id, entry_id } => {
            let target = entry_id.as_deref().unwrap_or(&id);
            canario.delete_history(target);
            write_json(&ok(&id));
        }
        Command::ClearHistory { id } => {
            canario.clear_history();
            write_json(&ok(&id));
        }
        Command::StartHotkey { id } => match canario.start_hotkey() {
            Ok(()) => write_json(&ok(&id)),
            Err(e) => write_json(&err(&id, e.to_string())),
        },
        Command::StopHotkey { id } => {
            canario.stop_hotkey();
            write_json(&ok(&id));
        }
        Command::RestartHotkey { id } => match canario.restart_hotkey() {
            Ok(()) => write_json(&ok(&id)),
            Err(e) => write_json(&err(&id, e.to_string())),
        },
        Command::Ping { id } => {
            write_json(&ok_data(
                &id,
                serde_json::json!({
                    "pong": true,
                    "version": env!("CARGO_PKG_VERSION"),
                }),
            ));
        }
        Command::Diagnostics { id } => {
            let diag =
                canario_core::diagnostics::collect("canario-electron", env!("CARGO_PKG_VERSION"));
            write_json(&ok_data(&id, serde_json::to_value(&diag).unwrap()));
        }
        Command::Shutdown { id } => {
            write_json(&ok(&id));
            canario.shutdown();
            info!("Shutdown requested, exiting...");
            // Drop the log guard explicitly: `std::process::exit` skips
            // destructors, and dropping it flushes buffered log lines.
            drop(log_guard.take());
            std::process::exit(0);
        }
    }
}

// ── Config merge ─────────────────────────────────────────────────────────────

/// Merge a partial config JSON object onto `current`.
///
/// The current config is serialized to a `serde_json::Value`, each key of
/// the partial is applied on top, and the result is deserialized back into
/// `AppConfig`. This means every `AppConfig` field — present and future —
/// is configurable without hand-maintaining a whitelist.
///
/// Validation policy:
/// - **Unknown keys are ignored** (logged), consistent with `AppConfig`'s
///   serde behavior on disk ("Unknown fields are ignored by serde_json").
/// - A key whose value fails to deserialize (wrong type, bad enum
///   variant) is skipped; the field keeps its previous value. Other keys
///   in the same update still apply.
fn merge_config(current: &mut canario_core::AppConfig, partial: &serde_json::Value) {
    let Some(obj) = partial.as_object() else {
        warn!("update_config: payload is not a JSON object, ignoring");
        return;
    };

    let mut merged = match serde_json::to_value(&*current) {
        Ok(v) => v,
        Err(e) => {
            error!("update_config: failed to serialize current config: {}", e);
            return;
        }
    };

    for (key, val) in obj {
        if merged.get(key).is_none() {
            warn!("update_config: ignoring unknown key {:?}", key);
            continue;
        }
        merged[key] = val.clone();
        match serde_json::from_value::<canario_core::AppConfig>(merged.clone()) {
            Ok(new_config) => *current = new_config,
            Err(e) => {
                warn!("update_config: ignoring invalid value for {:?}: {}", key, e);
                // Roll back this key only; keep any earlier applied keys.
                if let Ok(v) = serde_json::to_value(&*current) {
                    merged = v;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_applies_known_fields() {
        let mut cfg = canario_core::AppConfig::default();
        merge_config(
            &mut cfg,
            &serde_json::json!({
                "auto_paste": false,
                "show_tray_icon": false,
                "num_threads": 8,
                "minimum_key_time": 0.5,
            }),
        );
        assert!(!cfg.auto_paste);
        assert!(!cfg.show_tray_icon);
        assert_eq!(cfg.num_threads, 8);
        assert!((cfg.minimum_key_time - 0.5).abs() < f64::EPSILON);
        // Untouched field keeps default
        assert!(cfg.sound_effects);
    }

    #[test]
    fn merge_applies_custom_model_paths() {
        let mut cfg = canario_core::AppConfig::default();
        merge_config(
            &mut cfg,
            &serde_json::json!({
                "custom_encoder_path": "/tmp/enc.onnx",
                "custom_tokens_path": null,
            }),
        );
        assert_eq!(
            cfg.custom_encoder_path,
            Some(std::path::PathBuf::from("/tmp/enc.onnx"))
        );
        assert_eq!(cfg.custom_tokens_path, None);
    }

    #[test]
    fn merge_ignores_unknown_keys() {
        let mut cfg = canario_core::AppConfig::default();
        let before = cfg.clone();
        merge_config(&mut cfg, &serde_json::json!({ "not_a_field": 42 }));
        assert_eq!(
            serde_json::to_value(&cfg).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
    }

    #[test]
    fn merge_skips_invalid_values_but_applies_valid_ones() {
        let mut cfg = canario_core::AppConfig::default();
        let original_threads = cfg.num_threads;
        merge_config(
            &mut cfg,
            &serde_json::json!({
                "num_threads": "not-a-number",
                "autostart": true,
            }),
        );
        // Invalid value skipped
        assert_eq!(cfg.num_threads, original_threads);
        // Valid sibling key still applied
        assert!(cfg.autostart);
    }

    #[test]
    fn merge_ignores_non_object_payload() {
        let mut cfg = canario_core::AppConfig::default();
        let before = cfg.clone();
        merge_config(&mut cfg, &serde_json::json!([1, 2, 3]));
        assert_eq!(
            serde_json::to_value(&cfg).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
    }

    #[test]
    fn merge_applies_enum_and_nested_fields() {
        let mut cfg = canario_core::AppConfig::default();
        merge_config(
            &mut cfg,
            &serde_json::json!({
                "model": "ParakeetV2",
                "recording_audio_behavior": "Mute",
                "hotkey": ["Ctrl", "Space"],
            }),
        );
        assert_eq!(cfg.model, canario_core::ModelVariant::ParakeetV2);
        assert_eq!(
            cfg.recording_audio_behavior,
            canario_core::AudioBehavior::Mute
        );
        assert_eq!(cfg.hotkey, vec!["Ctrl".to_string(), "Space".to_string()]);
    }
}
