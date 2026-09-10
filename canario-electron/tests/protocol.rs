//! Integration tests for the canario-electron sidecar's JSON stdin/stdout
//! protocol: spawn the compiled binary, drive newline-delimited JSON
//! commands, assert id-matched responses.
//!
//! Hermetic by construction:
//! - the child's HOME / XDG_CONFIG_HOME / XDG_DATA_HOME point at a temp
//!   dir, so no $HOME pollution;
//! - no model download and no audio devices are touched (only
//!   config/history/ping commands are exercised);
//! - every read has a timeout and the child is killed in a Drop guard.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

struct Sidecar {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<Value>,
    // Keep the temp HOME alive for the lifetime of the child.
    _tmp: tempfile::TempDir,
}

impl Sidecar {
    fn spawn() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        Self::spawn_with_home(tmp)
    }

    fn spawn_with_home(tmp: tempfile::TempDir) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_canario-electron"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("HOME", tmp.path())
            .env("XDG_CONFIG_HOME", tmp.path().join("config"))
            .env("XDG_DATA_HOME", tmp.path().join("data"))
            // Keep tracing quiet even if the developer has RUST_LOG set.
            .env("RUST_LOG", "error")
            .spawn()
            .expect("failed to spawn canario-electron sidecar");

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        // Pump stdout lines into a channel so tests can read with timeouts.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(val) = serde_json::from_str::<Value>(&line) {
                    if tx.send(val).is_err() {
                        break;
                    }
                }
            }
        });

        Sidecar {
            child,
            stdin,
            lines: rx,
            _tmp: tmp,
        }
    }

    fn send(&mut self, cmd: Value) {
        let mut line = serde_json::to_string(&cmd).unwrap();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn send_raw(&mut self, raw: &str) {
        self.stdin.write_all(raw.as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Wait for a response with the given `id`, skipping any interleaved
    /// events. Panics on timeout so a stuck sidecar fails fast instead of
    /// hanging the test run.
    fn wait_for(&self, id: &str) -> Value {
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("timed out waiting for response id={id:?}"));
            match self.lines.recv_timeout(remaining) {
                Ok(msg) => {
                    if msg.get("id").and_then(Value::as_str) == Some(id) {
                        return msg;
                    }
                    // Interleaved event or response to another id; keep waiting.
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    panic!("timed out waiting for response id={id:?}")
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("sidecar stdout closed while waiting for id={id:?}")
                }
            }
        }
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Seed a history.json with two entries before the sidecar starts.
fn seed_history(tmp: &tempfile::TempDir) {
    let dir = tmp.path().join("data").join("canario");
    std::fs::create_dir_all(&dir).unwrap();
    let history = json!({
        "entries": [
            {
                "id": "entry-1",
                "timestamp": "2026-01-01T00:00:00Z",
                "text": "first entry",
                "duration_secs": 1.0,
                "source_app": null
            },
            {
                "id": "req-delete-fallback",
                "timestamp": "2026-01-01T00:01:00Z",
                "text": "second entry",
                "duration_secs": 2.0,
                "source_app": "tests"
            }
        ]
    });
    std::fs::write(
        dir.join("history.json"),
        serde_json::to_string(&history).unwrap(),
    )
    .unwrap();
}

#[test]
fn ping_responds_with_pong_and_version() {
    let mut sidecar = Sidecar::spawn();

    sidecar.send(json!({ "cmd": "ping", "id": "ping-1" }));
    let resp = sidecar.wait_for("ping-1");

    assert_eq!(resp["ok"], json!(true));
    assert_eq!(resp["data"]["pong"], json!(true));
    assert!(
        resp["data"]["version"].as_str().is_some(),
        "ping should report the sidecar version: {resp}"
    );
}

#[test]
fn get_config_returns_default_config_with_version() {
    let mut sidecar = Sidecar::spawn();

    sidecar.send(json!({ "cmd": "get_config", "id": "cfg-1" }));
    let resp = sidecar.wait_for("cfg-1");

    assert_eq!(resp["ok"], json!(true));
    let config = &resp["data"];
    assert!(
        config["config_version"].as_u64().unwrap_or(0) >= 1,
        "config_version missing from get_config response: {config}"
    );
    assert!(config.get("model").is_some());
    assert!(config.get("hotkey").is_some());
}

#[test]
fn update_config_applies_known_keys_and_ignores_unknown() {
    let mut sidecar = Sidecar::spawn();

    sidecar.send(json!({
        "cmd": "update_config",
        "id": "upd-1",
        "config": {
            "num_threads": 8,
            "auto_paste": false,
            "definitely_not_a_real_key": 12345
        }
    }));
    let resp = sidecar.wait_for("upd-1");
    assert_eq!(resp["ok"], json!(true));

    // Known keys applied; unknown key didn't break anything.
    sidecar.send(json!({ "cmd": "get_config", "id": "cfg-2" }));
    let resp = sidecar.wait_for("cfg-2");
    assert_eq!(resp["data"]["num_threads"], json!(8));
    assert_eq!(resp["data"]["auto_paste"], json!(false));

    // The update persisted to the temp config dir.
    sidecar.send(json!({ "cmd": "update_config", "id": "upd-2", "config": { "num_threads": 2 } }));
    assert_eq!(sidecar.wait_for("upd-2")["ok"], json!(true));
    sidecar.send(json!({ "cmd": "get_config", "id": "cfg-3" }));
    assert_eq!(sidecar.wait_for("cfg-3")["data"]["num_threads"], json!(2));
}

#[test]
fn malformed_json_line_returns_id_matched_error() {
    let mut sidecar = Sidecar::spawn();

    // Valid JSON with an id but an unknown command → id-matched error.
    sidecar.send_raw(r#"{"id": "bad-cmd-1", "cmd": "explode"}"#);
    let resp = sidecar.wait_for("bad-cmd-1");
    assert_eq!(resp["ok"], json!(false));
    assert!(resp["error"].as_str().unwrap().contains("invalid command"));

    // Missing required field (ping without id) → error recovered as "unknown".
    sidecar.send_raw(r#"{"cmd": "ping"}"#);
    let resp = sidecar.wait_for("unknown");
    assert_eq!(resp["ok"], json!(false));

    // Not JSON at all → error recovered as "unknown" again.
    sidecar.send_raw("this is not json");
    let resp = sidecar.wait_for("unknown");
    assert_eq!(resp["ok"], json!(false));

    // The sidecar is still alive and well after all that.
    sidecar.send(json!({ "cmd": "ping", "id": "ping-after" }));
    assert_eq!(sidecar.wait_for("ping-after")["ok"], json!(true));
}

#[test]
fn delete_history_accepts_target_id_and_id_fallback_spellings() {
    let tmp = tempfile::tempdir().unwrap();
    seed_history(&tmp);
    let mut sidecar = Sidecar::spawn_with_home(tmp);

    // Both seeded entries are visible.
    sidecar.send(json!({ "cmd": "get_history", "id": "h-1" }));
    let resp = sidecar.wait_for("h-1");
    assert_eq!(resp["ok"], json!(true));
    let entries = resp["data"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "seeded history should load: {entries:?}");

    // Spelling 1: Electron frontend's `target_id` names the entry.
    sidecar.send(json!({ "cmd": "delete_history", "id": "del-1", "target_id": "entry-1" }));
    assert_eq!(sidecar.wait_for("del-1")["ok"], json!(true));

    sidecar.send(json!({ "cmd": "get_history", "id": "h-2" }));
    let resp = sidecar.wait_for("h-2");
    let entries = resp["data"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], json!("req-delete-fallback"));

    // Spelling 2: documented `id` doubles as request id AND entry id.
    sidecar.send(json!({ "cmd": "delete_history", "id": "req-delete-fallback" }));
    assert_eq!(sidecar.wait_for("req-delete-fallback")["ok"], json!(true));

    sidecar.send(json!({ "cmd": "get_history", "id": "h-3" }));
    let resp = sidecar.wait_for("h-3");
    assert_eq!(resp["data"].as_array().unwrap().len(), 0);
}

#[test]
fn clear_history_empties_the_store() {
    let tmp = tempfile::tempdir().unwrap();
    seed_history(&tmp);
    let mut sidecar = Sidecar::spawn_with_home(tmp);

    sidecar.send(json!({ "cmd": "clear_history", "id": "clr-1" }));
    assert_eq!(sidecar.wait_for("clr-1")["ok"], json!(true));

    sidecar.send(json!({ "cmd": "get_history", "id": "h-1" }));
    let resp = sidecar.wait_for("h-1");
    assert_eq!(resp["data"].as_array().unwrap().len(), 0);
}

#[test]
fn is_model_downloaded_is_false_in_fresh_home() {
    let mut sidecar = Sidecar::spawn();

    sidecar.send(json!({ "cmd": "is_model_downloaded", "id": "mdl-1" }));
    let resp = sidecar.wait_for("mdl-1");

    assert_eq!(resp["ok"], json!(true));
    assert_eq!(resp["data"], json!(false));
}

#[test]
fn model_inventory_query_preserves_selected_model_and_saved_config() {
    let tmp = tempfile::tempdir().unwrap();
    let model_dir = tmp
        .path()
        .join("data/canario/models/sherpa-parakeet-tdt-v2");
    std::fs::create_dir_all(&model_dir).unwrap();
    // Readiness checks only file presence. Never select these synthetic model
    // files, so the recognizer must not try loading them during this query.
    for file in [
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "joiner.int8.onnx",
        "tokens.txt",
    ] {
        std::fs::write(model_dir.join(file), b"fixture").unwrap();
    }
    let config_path = tmp.path().join("config/canario/config.json");
    let mut sidecar = Sidecar::spawn_with_home(tmp);
    sidecar.send(json!({ "cmd": "get_config", "id": "before" }));
    let before = sidecar.wait_for("before")["data"].clone();
    let saved_before = std::fs::read(&config_path).unwrap();

    sidecar.send(json!({ "cmd": "is_model_downloaded", "id": "v2", "model": "ParakeetV2" }));
    let v2 = sidecar.wait_for("v2");
    assert_eq!(v2["ok"], json!(true));
    assert_eq!(v2["data"], json!(true));
    sidecar.send(json!({ "cmd": "is_model_downloaded", "id": "selected" }));
    assert_eq!(sidecar.wait_for("selected")["data"], json!(false));

    sidecar.send(json!({ "cmd": "get_config", "id": "after" }));
    assert_eq!(sidecar.wait_for("after")["data"], before);
    assert_eq!(std::fs::read(&config_path).unwrap(), saved_before);
}

#[test]
fn diagnostics_returns_versions_system_config_tools_and_logs() {
    let mut sidecar = Sidecar::spawn();

    sidecar.send(json!({ "cmd": "diagnostics", "id": "diag-1" }));
    let resp = sidecar.wait_for("diag-1");

    assert_eq!(resp["ok"], json!(true));
    let data = &resp["data"];
    assert_eq!(data["frontend"]["name"], json!("canario-electron"));
    assert!(data["frontend"]["version"].as_str().is_some());
    assert!(data["core_version"].as_str().is_some());
    assert_eq!(data["system"]["os"], json!(std::env::consts::OS));
    assert!(data["system"]["display_server"].as_str().is_some());
    assert!(data["config"].get("config_version").is_some());
    assert_eq!(data["model"]["downloaded"], json!(false));
    for tool in ["xdotool", "wtype", "ydotool", "pactl"] {
        assert!(
            data["tools"][tool].as_bool().is_some(),
            "tools.{tool} missing from diagnostics: {data}"
        );
    }
    // Log dir is hermetic: under the temp HOME's state dir.
    let log_dir = data["logs"]["dir"].as_str().unwrap();
    assert!(
        log_dir.contains("canario/logs"),
        "unexpected log dir {log_dir:?}"
    );
    assert!(data["logs"]["tail"].as_array().is_some());
}

#[test]
fn shutdown_responds_ok_and_exits() {
    let mut sidecar = Sidecar::spawn();

    sidecar.send(json!({ "cmd": "shutdown", "id": "bye-1" }));
    assert_eq!(sidecar.wait_for("bye-1")["ok"], json!(true));

    // The process should exit on its own; poll briefly, then the Drop
    // guard cleans up if it didn't.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match sidecar.child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                assert!(
                    Instant::now() < deadline,
                    "sidecar did not exit within 5s of shutdown command"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("failed to poll sidecar exit status: {e}"),
        }
    }
}
