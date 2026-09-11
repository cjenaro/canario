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

// ── Pipeline timing marks ────────────────────────────────────────────────────

/// Timing stage for a received command, for the latency-relevant
/// commands only (recording control). `None` = not worth a mark.
///
/// Marks fire the moment the command is parsed, i.e. after stdin read +
/// JSON parse — the entry edge of the sidecar hop. See
/// `canario_core::timing` for the full stage model.
fn cmd_timing_stage(cmd: &Command) -> Option<&'static str> {
    match cmd {
        Command::StartRecording { .. } => Some("sidecar_cmd_start_recording"),
        Command::StopRecording { .. } => Some("sidecar_cmd_stop_recording"),
        Command::ToggleRecording { .. } => Some("sidecar_cmd_toggle_recording"),
        _ => None,
    }
}

/// Timing stage for a forwarded event, for the pipeline milestones only
/// (frequent chatter like `AudioLevel` is deliberately unmarked).
fn event_timing_stage(event: &canario_core::Event) -> Option<&'static str> {
    use canario_core::Event;
    match event {
        Event::RecordingStarted => Some("sidecar_event_recording_started"),
        Event::RecordingStopped => Some("sidecar_event_recording_stopped"),
        Event::RecordingCancelled => Some("sidecar_event_recording_cancelled"),
        Event::TranscriptionReady { .. } => Some("sidecar_event_transcription_ready"),
        Event::HotkeyTriggered => Some("sidecar_event_hotkey_triggered"),
        Event::Error { .. } => Some("sidecar_event_error"),
        _ => None,
    }
}

// ── Command types ────────────────────────────────────────────────────────────

/// Wire-protocol compatibility version between this sidecar and the
/// Electron app (canario-dmp.4).
///
/// Bump when a command, event, or response shape changes in a way an
/// old peer must not silently misinterpret. The Electron app carries
/// its own copy (canario-app/src/main/version.ts) — there is no
/// codegen; the pair is pinned together by
/// `pin_ping_shape_and_ts_protocol_constant` in tests/protocol.rs and
/// compared at runtime by checkVersion().
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd")]
enum Command {
    #[serde(rename = "start_recording")]
    StartRecording { id: String },
    #[serde(rename = "stop_recording")]
    StopRecording { id: String },
    #[serde(rename = "toggle_recording")]
    ToggleRecording { id: String },
    #[serde(rename = "cancel_recording")]
    CancelRecording { id: String },
    #[serde(rename = "download_model")]
    DownloadModel { id: String },
    #[serde(rename = "cancel_download")]
    CancelDownload { id: String },
    #[serde(rename = "is_downloading")]
    IsDownloading { id: String },
    #[serde(rename = "status")]
    Status { id: String },
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
        /// Entry to delete. `entry_id` is canonical; the Electron
        /// renderer's spelling `target_id` is accepted as a serde
        /// alias (canario-app/src/renderer/primitives/createCanario.ts).
        /// When NEITHER is present the command errors — the request
        /// `id` no longer doubles as the entry id (canario-dmp.8): a
        /// typo'd command must fail loudly, not delete whatever entry
        /// happens to share the request id.
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
    #[serde(rename = "hotkey_status")]
    HotkeyStatus { id: String },
    #[serde(rename = "set_transform_credential")]
    SetTransformCredential {
        id: String,
        /// The provider API key (D2): held in memory only — never
        /// written to config.json or logs. `None`/empty drops the
        /// stored credential.
        key: Option<String>,
    },
    #[serde(rename = "transform_status")]
    TransformStatus { id: String },
    #[serde(rename = "transform_test")]
    TransformTest { id: String },
    #[serde(rename = "list_audio_devices")]
    ListAudioDevices { id: String },
    #[serde(rename = "set_autostart")]
    SetAutostart {
        id: String,
        enabled: bool,
        /// Launch command for the standalone login entry. `None` links
        /// the installed menu entry (`Exec=canario`) instead — Electron
        /// callers pass their own exe path here.
        exec: Option<String>,
    },
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

/// Serialize `val` as a single JSON line on stdout. Used for both responses
/// and forwarded canario-core events — the wire format is identical either
/// way (`Event` derives `serde::Serialize`).
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

    // Absorb the legacy Electron-written login entry (canario-dmp.17).
    // Non-fatal: the sidecar must still boot if migration fails — the
    // entry is absorbed on a later successful run or by the next
    // explicit set_autostart.
    if let Err(e) = canario_core::autostart::migrate_legacy_autostart() {
        warn!("Legacy autostart migration failed: {}", e);
    }

    // Spawn event forwarder thread: reads from canario-core channel,
    // writes JSON events to stdout.
    let event_tx_canario = canario.clone();
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            // Special handling: auto-add transcription to history
            // Note: auto-paste is handled by the Electron main process, not the sidecar.
            // The sidecar only adds to history here.
            // fgm.3 D3: `text` is the canonical (transformed) transcript;
            // `raw_text` rides along only when a transformation changed it.
            if let canario_core::Event::TranscriptionReady {
                ref text,
                duration_secs,
                ref raw_text,
                ..
            } = event
            {
                event_tx_canario.add_history(text.clone(), duration_secs, None, raw_text.clone());
            }
            if let Some(stage) = event_timing_stage(&event) {
                canario_core::timing::mark(stage);
            }
            write_json(&event);
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
                // The raw line may carry a credential (e.g. a malformed
                // set_transform_credential) — redact before logging so
                // the key never reaches the log file (D2).
                error!(
                    "Failed to parse command: {} — input: {}",
                    e,
                    redact_for_log(line)
                );
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

        if let Some(stage) = cmd_timing_stage(&cmd) {
            canario_core::timing::mark(stage);
        }
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
        Command::CancelRecording { id } => {
            // Discard the in-flight recording without transcribing.
            // Safe no-op when idle — core owns that guarantee.
            canario.cancel_recording();
            write_json(&ok(&id));
        }
        Command::CancelDownload { id } => {
            // Requests cancellation; ModelDownloadFailed arrives as an
            // event once the download loop observes it (.part files are
            // kept for resume). Safe no-op when nothing is running.
            canario.cancel_download();
            write_json(&ok(&id));
        }
        Command::IsDownloading { id } => {
            write_json(&ok_data(&id, serde_json::json!(canario.is_downloading())));
        }
        Command::Status { id } => {
            let status = canario.lifecycle_status();
            write_json(&ok_data(&id, serde_json::to_value(status).unwrap()));
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
            // Serve the on-disk truth, not the boot snapshot: the file
            // may have changed under a running instance (other
            // frontend, manual edit, CLI) — canario-dmp.18. Falls back
            // to the in-memory snapshot if the reload fails.
            let config = canario.refresh_config().unwrap_or_else(|e| {
                warn!(
                    "get_config: reloading config.json failed ({}), serving snapshot",
                    e
                );
                canario.config()
            });
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
        Command::DeleteHistory { id, entry_id } => match entry_id {
            // canario-dmp.8: the entry id must be named explicitly
            // (`entry_id`, or the renderer's `target_id` alias). The
            // old fallback — reusing the request `id` as the entry id —
            // is removed: it turned a malformed command into a
            // silent, wrong deletion.
            Some(entry_id) => {
                canario.delete_history(&entry_id);
                write_json(&ok(&id));
            }
            None => write_json(&err(&id, "delete_history requires entry_id (or target_id)")),
        },
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
        // Pull-based health query. `start_hotkey` settles the evdev
        // permission probe synchronously, so the renderer can fetch
        // this on mount (or after a restart) without racing startup.
        Command::HotkeyStatus { id } => {
            let status = canario.hotkey_status();
            write_json(&ok_data(&id, serde_json::to_value(&status).unwrap()));
        }
        // Memory-only credential handoff (fgm.1 D2): the Electron main
        // process persists the key via safeStorage and pushes it here.
        // The response reports whether a key is now held — never the
        // key itself — and nothing is written to config.json or logs.
        Command::SetTransformCredential { id, key } => {
            let stored = set_transform_credential(key.as_deref());
            write_json(&ok_data(&id, serde_json::json!({ "stored": stored })));
        }
        // Sidecar truth about the transform feature: the provider
        // block from config (minus any key material — it never lives
        // there) plus whether the in-memory credential is present.
        // Reads the on-disk config like get_config so external edits
        // are visible without a restart.
        Command::TransformStatus { id } => {
            let config = canario.refresh_config().unwrap_or_else(|e| {
                warn!(
                    "transform_status: reloading config.json failed ({}), serving snapshot",
                    e
                );
                canario.config()
            });
            let transform = &config.transform;
            write_json(&ok_data(
                &id,
                serde_json::json!({
                    "enabled": transform.enabled,
                    "provider": {
                        "base_url": transform.provider.base_url,
                        "model": transform.provider.model,
                    },
                    "timeout_ms": transform.timeout_ms,
                    "credential_present": transform_credential().is_some(),
                }),
            ));
        }
        // Connection probe for the settings UI ("Test connection"):
        // one minimal chat-completions round trip through the
        // configured provider with the in-memory credential. Blocks
        // the command loop for at most the configured timeout (same
        // posture as start_hotkey's synchronous probe).
        Command::TransformTest { id } => {
            let config = canario.config();
            let key = transform_credential();
            match canario_core::transform::test_connection_blocking(
                &config.transform,
                key.as_deref(),
            ) {
                Ok(latency_ms) => write_json(&ok_data(
                    &id,
                    serde_json::json!({ "latency_ms": latency_ms }),
                )),
                // Error strings carry no key material: the credential
                // only ever travels inside the Authorization header.
                Err(e) => write_json(&err(&id, e.to_string())),
            }
        }
        // Audio input device enumeration for the settings device
        // picker (canario-1hq.2). Never errors the response: a
        // missing/failed audio subsystem yields an empty list (the
        // picker then offers only "System default"), and nothing in
        // the enumeration→serialization path can fail — `err` stays
        // reserved for IO-class trouble.
        Command::ListAudioDevices { id } => {
            let devices = canario.list_input_devices();
            write_json(&ok_data(&id, serde_json::to_value(&devices).unwrap()));
        }
        // One shared login-entry implementation for every frontend
        // (canario-dmp.17): the entry and config.autostart move
        // together, so the frontends can never double-launch at login.
        Command::SetAutostart { id, enabled, exec } => {
            match canario.set_autostart(enabled, exec.as_deref()) {
                Ok(()) => {
                    let now_enabled =
                        canario_core::autostart::is_autostart_enabled().unwrap_or(enabled);
                    write_json(&ok_data(&id, serde_json::json!({ "enabled": now_enabled })));
                }
                Err(e) => write_json(&err(&id, e.to_string())),
            }
        }
        Command::Ping { id } => {
            write_json(&ok_data(
                &id,
                serde_json::json!({
                    "pong": true,
                    "version": env!("CARGO_PKG_VERSION"),
                    "protocol": PROTOCOL_VERSION,
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

// ── Transform provider plumbing (canario-fgm.1 D1/D2, canario-fgm.2) ─────────
//
// Hook points for the LLM transform feature. fgm.3 moved the in-memory
// credential store INTO canario-core (`canario_core::transform`), so
// the recording pipeline — which runs in this same process — reads the
// key without threading it through the recording API. The thin
// wrappers below keep the sidecar's command loop and redaction reading
// one source of truth.

/// Clone of the currently held key, if any (D2). Callers must treat
/// the value as a secret: use it in request headers, never in logs or
/// serialized output.
fn transform_credential() -> Option<String> {
    canario_core::transform::credential()
}

/// Store (`Some` non-empty) or drop (`None`/blank) the credential.
/// Returns whether a key is held afterwards.
fn set_transform_credential(key: Option<&str>) -> bool {
    canario_core::transform::set_credential(key)
}

/// Redact credential-bearing fields from a raw command line before it
/// is logged (D2: the key must never reach the log file). Structurally
/// blanks `key`/`api_key`/`credential` values, then scrubs any
/// occurrence of the currently held key as defense in depth. Non-JSON
/// or credential-free lines pass through unchanged.
fn redact_for_log(line: &str) -> String {
    let redacted = match serde_json::from_str::<serde_json::Value>(line) {
        Ok(mut v) if v.is_object() => {
            let mut changed = false;
            for field in ["key", "api_key", "credential"] {
                if let Some(val) = v.get(field) {
                    if !val.is_null() {
                        v[field] = serde_json::json!("[redacted]");
                        changed = true;
                    }
                }
            }
            // Parsed: return the (possibly redacted) serialization —
            // `changed == false` still means "safe, log as-is".
            Some(if changed {
                v.to_string()
            } else {
                line.to_string()
            })
        }
        _ => None,
    };
    match redacted {
        Some(line) => canario_core::transform::redact(&line, transform_credential().as_deref()),
        // An UNPARSEABLE line can't be structurally redacted, and no
        // credential may be held yet to scrub with (the malformed
        // set_transform_credential may be the FIRST time we ever see
        // the key) — so a raw line that merely looks credential-bearing
        // must not be logged at all. Marker, not value.
        None if looks_credential_bearing(line) => {
            "[input withheld: unparseable line carries a credential field]".to_string()
        }
        None => canario_core::transform::redact(line, transform_credential().as_deref()),
    }
}

/// Cheap scan for credential-bearing content in a line we could not
/// parse as JSON: any of the known field names followed by a quote or
/// digit (i.e. a non-null value) anywhere in the text.
fn looks_credential_bearing(line: &str) -> bool {
    ["key", "api_key", "credential"].iter().any(|field| {
        ["\"", "'", ":"].iter().any(|sep| {
            line.contains(&format!("{field}{sep}")) || line.contains(&format!("{field} {sep}"))
        })
    })
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
/// - Unknown keys are preserved in `AppConfig::extra` (serde flatten),
///   consistent with `AppConfig`'s on-disk behavior — a newer frontend's
///   update must survive an older sidecar, exactly like a newer
///   config.json must survive an older binary (canario-dmp.22).
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
        merged[key] = val.clone();
        match serde_json::from_value::<canario_core::AppConfig>(merged.clone()) {
            Ok(new_config) => *current = new_config,
            Err(e) => {
                // A serde error can quote the offending value — scrub
                // the in-memory credential out of it (D2).
                warn!(
                    "update_config: ignoring invalid value for {:?}: {}",
                    key,
                    canario_core::transform::redact(
                        &e.to_string(),
                        transform_credential().as_deref()
                    )
                );
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

    /// The credential static is process-global and Rust runs unit
    /// tests on parallel threads — serialize every test that touches it.
    static CREDENTIAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn transform_credential_store_round_trip_and_clearing() {
        let _guard = CREDENTIAL_TEST_LOCK.lock().unwrap();
        // D2 semantics: Some(non-empty) stores, None/blank drops.
        assert!(!set_transform_credential(None));
        assert!(transform_credential().is_none());

        assert!(set_transform_credential(Some("sk-test-key")));
        assert_eq!(transform_credential().as_deref(), Some("sk-test-key"));

        // Blank counts as absent, not as a stored empty string.
        assert!(!set_transform_credential(Some("   ")));
        assert!(transform_credential().is_none());

        assert!(set_transform_credential(Some("sk-test-key")));
        assert!(!set_transform_credential(None));
        assert!(transform_credential().is_none());
    }

    #[test]
    fn redact_for_log_blanks_credential_fields() {
        let _guard = CREDENTIAL_TEST_LOCK.lock().unwrap();
        // Credential-bearing command line: the key field is blanked…
        let line = r#"{"id":"x","cmd":"set_transform_credential","key":"sk-super-secret"}"#;
        let redacted = redact_for_log(line);
        assert!(!redacted.contains("sk-super-secret"), "{redacted}");
        assert!(redacted.contains("\"key\":\"[redacted]\""), "{redacted}");
        // …while the rest of the line stays useful for debugging.
        assert!(redacted.contains("set_transform_credential"), "{redacted}");
    }

    #[test]
    fn redact_for_log_passes_credential_free_lines_through() {
        for line in [
            r#"{"id":"1","cmd":"get_config"}"#,
            r#"{"id":"2","cmd":"update_config","config":{"auto_paste":false}}"#,
            // key: null carries no secret — left untouched.
            r#"{"id":"3","cmd":"set_transform_credential","key":null}"#,
            "not json at all",
        ] {
            assert_eq!(redact_for_log(line), line.to_string());
        }
    }

    #[test]
    fn redact_for_log_withholds_unparseable_credential_bearing_lines() {
        // A malformed credential line (the raw input IS logged on the
        // parse-failure path, and no key may be held yet to scrub
        // with) must be withheld entirely — marker, not value.
        let malformed = r#"{"id":"bad","cmd":"set_transform_credential","key":"sk-leaky","oops":"#;
        assert!(!redact_for_log(malformed).contains("sk-leaky"));
        assert!(redact_for_log(malformed).contains("withheld"));

        // Credential-free garbage still logs verbatim.
        let benign = r#"{"id":"bad","cmd":"get_config","oops":"#;
        assert_eq!(redact_for_log(benign), benign.to_string());
    }

    #[test]
    fn redact_for_log_scrubs_held_secret_from_arbitrary_text() {
        let _guard = CREDENTIAL_TEST_LOCK.lock().unwrap();
        // Defense in depth: a non-JSON line (or any field) that happens
        // to contain the currently held key is scrubbed too.
        set_transform_credential(Some("sk-live-secret"));
        let scrubbed = redact_for_log("garbage line mentioning sk-live-secret");
        assert_eq!(scrubbed, "garbage line mentioning [redacted]");
        set_transform_credential(None);
    }

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
    fn merge_applies_onboarding_completed() {
        // The Electron main process flips the first-launch wizard flag
        // through update_config (canario-xv9) — the generic merge path
        // must accept it like any other known AppConfig key.
        let mut cfg = canario_core::AppConfig::default();
        assert!(!cfg.onboarding_completed);
        merge_config(
            &mut cfg,
            &serde_json::json!({ "onboarding_completed": true }),
        );
        assert!(cfg.onboarding_completed);
        // And back off (Settings → About "Re-run onboarding")
        merge_config(
            &mut cfg,
            &serde_json::json!({ "onboarding_completed": false }),
        );
        assert!(!cfg.onboarding_completed);
        // Untouched field keeps default
        assert!(cfg.auto_paste);
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
    fn merge_preserves_unknown_keys_in_extra() {
        // canario-dmp.22: an update payload key this build doesn't know
        // rides along in `extra` instead of being dropped — a newer
        // frontend's update survives an older sidecar, and the value
        // reaches config.json (and get_config) verbatim.
        let mut cfg = canario_core::AppConfig::default();
        merge_config(
            &mut cfg,
            &serde_json::json!({ "not_a_field": 42, "future_block": { "on": true } }),
        );
        assert_eq!(cfg.extra.get("not_a_field"), Some(&serde_json::json!(42)));
        assert_eq!(
            cfg.extra.get("future_block"),
            Some(&serde_json::json!({ "on": true }))
        );
        // Flattened onto the wire like any other top-level key…
        let wire = serde_json::to_value(&cfg).unwrap();
        assert_eq!(wire["not_a_field"], serde_json::json!(42));
        assert_eq!(wire["future_block"], serde_json::json!({ "on": true }));
        // …and known keys in the same payload still apply.
        merge_config(
            &mut cfg,
            &serde_json::json!({ "auto_paste": false, "another_unknown": "x" }),
        );
        assert!(!cfg.auto_paste);
        assert_eq!(
            cfg.extra.get("another_unknown"),
            Some(&serde_json::json!("x"))
        );
        // The earlier extras survived the second merge.
        assert_eq!(cfg.extra.get("not_a_field"), Some(&serde_json::json!(42)));
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

    #[test]
    fn merge_applies_transform_block_and_skips_invalid_values() {
        // canario-fgm.2: the renderer writes the provider block through
        // the same generic merge path as every other AppConfig key.
        let mut cfg = canario_core::AppConfig::default();
        assert!(!cfg.transform.enabled); // default OFF (D5a)
        merge_config(
            &mut cfg,
            &serde_json::json!({
                "transform": {
                    "enabled": true,
                    "provider": { "base_url": "http://localhost:11434/v1", "model": "llama3" },
                    "timeout_ms": 1500,
                    "rules": []
                }
            }),
        );
        assert!(cfg.transform.enabled);
        assert_eq!(cfg.transform.provider.base_url, "http://localhost:11434/v1");
        assert_eq!(cfg.transform.provider.model, "llama3");
        assert_eq!(cfg.transform.timeout_ms, 1500);

        // A wrong-typed subfield skips the whole key, keeping the
        // previous block (same policy as every other key).
        merge_config(
            &mut cfg,
            &serde_json::json!({ "transform": { "timeout_ms": "forever" } }),
        );
        assert_eq!(cfg.transform.timeout_ms, 1500);
        assert!(cfg.transform.enabled);
    }
}
