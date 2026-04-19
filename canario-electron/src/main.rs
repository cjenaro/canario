//! Canario Electron Sidecar
//!
//! JSON stdin/stdout bridge over `canario-core`.
//!
//! Reads newline-delimited JSON commands from stdin, executes them via
//! `canario::Canario`, and writes newline-delimited JSON events + responses
//! to stdout.

use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};
use tracing::{error, info};

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
    IsModelDownloaded { id: String },
    #[serde(rename = "get_config")]
    GetConfig { id: String },
    #[serde(rename = "update_config")]
    UpdateConfig { id: String, config: serde_json::Value },
    #[serde(rename = "get_history")]
    GetHistory { id: String, limit: Option<usize> },
    #[serde(rename = "search_history")]
    SearchHistory { id: String, query: String },
    #[serde(rename = "delete_history")]
    DeleteHistory { id: String, target_id: String },
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
    OkResponse { id: id.into(), ok: true, data: None }
}

fn ok_data(id: impl Into<String>, data: serde_json::Value) -> OkResponse {
    OkResponse { id: id.into(), ok: true, data: Some(data) }
}

fn err(id: impl Into<String>, msg: impl Into<String>) -> ErrResponse {
    ErrResponse { id: id.into(), ok: false, error: msg.into() }
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
    // Send all tracing to stderr so stdout stays clean for JSON IPC
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

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
            if let canario_core::Event::TranscriptionReady { ref text, duration_secs } = event {
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
                // Can't send error response without an id
                continue;
            }
        };

        handle_command(&canario, cmd);
    }

    canario.shutdown();
    info!("Sidecar shutting down");
    Ok(())
}

fn handle_command(canario: &canario_core::Canario, cmd: Command) {
    match cmd {
        Command::StartRecording { id } => {
            match canario.start_recording() {
                Ok(()) => write_json(&ok(&id)),
                Err(e) => write_json(&err(&id, e.to_string())),
            }
        }
        Command::StopRecording { id } => {
            canario.stop_recording();
            write_json(&ok(&id));
        }
        Command::ToggleRecording { id } => {
            let recording = canario.toggle_recording();
            write_json(&ok_data(&id, serde_json::json!({ "recording": recording })));
        }
        Command::DownloadModel { id } => {
            match canario.download_model() {
                Ok(()) => write_json(&ok(&id)),
                Err(e) => write_json(&err(&id, e.to_string())),
            }
        }
        Command::DeleteModel { id } => {
            match canario.delete_model() {
                Ok(()) => write_json(&ok(&id)),
                Err(e) => write_json(&err(&id, e.to_string())),
            }
        }
        Command::IsModelDownloaded { id } => {
            let downloaded = canario.is_model_downloaded();
            write_json(&ok_data(&id, serde_json::json!(downloaded)));
        }
        Command::GetConfig { id } => {
            let config = canario.config();
            write_json(&ok_data(&id, serde_json::to_value(&config).unwrap()));
        }
        Command::UpdateConfig { id, config } => {
            let result = canario.update_config(|c| {
                // Merge the partial config
                if let Some(model) = config.get("model").and_then(|v| v.as_str()) {
                    if let Ok(variant) = serde_json::from_value::<canario_core::ModelVariant>(
                        serde_json::Value::String(model.to_string()),
                    ) {
                        c.model = variant;
                    }
                }
                if let Some(v) = config.get("auto_paste").and_then(|v| v.as_bool()) {
                    c.auto_paste = v;
                }
                if let Some(v) = config.get("sound_effects").and_then(|v| v.as_bool()) {
                    c.sound_effects = v;
                }
                if let Some(v) = config.get("autostart").and_then(|v| v.as_bool()) {
                    c.autostart = v;
                }
                if let Some(v) = config.get("minimum_key_time").and_then(|v| v.as_f64()) {
                    c.minimum_key_time = v;
                }
                if let Some(v) = config.get("double_tap_lock").and_then(|v| v.as_bool()) {
                    c.double_tap_lock = v;
                }
                if let Some(v) = config.get("double_tap_only").and_then(|v| v.as_bool()) {
                    c.double_tap_only = v;
                }
                if let Some(v) = config.get("hotkey").and_then(|v| v.as_array()) {
                    c.hotkey = v.iter().filter_map(|v| v.as_str().map(String::from)).collect();
                }
                if let Some(v) = config.get("num_threads").and_then(|v| v.as_u64()) {
                    c.num_threads = v as u32;
                }
                if let Some(v) = config.get("recording_audio_behavior") {
                    if let Ok(behavior) = serde_json::from_value(v.clone()) {
                        c.recording_audio_behavior = behavior;
                    }
                }
                if let Some(v) = config.get("post_processor") {
                    if let Ok(pp) = serde_json::from_value(v.clone()) {
                        c.post_processor = pp;
                    }
                }
            });
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
        Command::DeleteHistory { id, target_id } => {
            canario.delete_history(&target_id);
            write_json(&ok(&id));
        }
        Command::ClearHistory { id } => {
            canario.clear_history();
            write_json(&ok(&id));
        }
        Command::StartHotkey { id } => {
            match canario.start_hotkey() {
                Ok(()) => write_json(&ok(&id)),
                Err(e) => write_json(&err(&id, e.to_string())),
            }
        }
        Command::StopHotkey { id } => {
            canario.stop_hotkey();
            write_json(&ok(&id));
        }
        Command::RestartHotkey { id } => {
            match canario.restart_hotkey() {
                Ok(()) => write_json(&ok(&id)),
                Err(e) => write_json(&err(&id, e.to_string())),
            }
        }
        Command::Ping { id } => {
            write_json(&ok_data(&id, serde_json::json!({
                "pong": true,
                "version": env!("CARGO_PKG_VERSION"),
            })));
        }
        Command::Shutdown { id } => {
            write_json(&ok(&id));
            canario.shutdown();
            info!("Shutdown requested, exiting...");
            std::process::exit(0);
        }
    }
}
