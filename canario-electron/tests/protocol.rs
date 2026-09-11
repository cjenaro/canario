//! Integration tests for the canario-electron sidecar's JSON stdin/stdout
//! protocol: spawn the compiled binary, drive newline-delimited JSON
//! commands, assert id-matched responses.
//!
//! Hermetic by construction:
//! - the child's HOME / XDG_CONFIG_HOME / XDG_DATA_HOME point at a temp
//!   dir, so no $HOME pollution;
//! - XDG_RUNTIME_DIR is overridden so the hotkey socket binds inside
//!   the temp dir instead of clobbering a real canario-hotkey.sock;
//! - no model download and no audio devices are touched (only
//!   config/history/ping/hotkey-status commands are exercised);
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
        // Keep the hotkey socket hermetic: without this, a start_hotkey
        // test would remove + rebind the developer's real
        // $XDG_RUNTIME_DIR/canario-hotkey.sock.
        let runtime_dir = tmp.path().join("runtime");
        std::fs::create_dir_all(&runtime_dir).unwrap();

        let mut child = Command::new(env!("CARGO_BIN_EXE_canario-electron"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("HOME", tmp.path())
            .env("XDG_CONFIG_HOME", tmp.path().join("config"))
            .env("XDG_DATA_HOME", tmp.path().join("data"))
            .env("XDG_RUNTIME_DIR", runtime_dir)
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

#[test]
fn get_config_reflects_external_edits_without_restart() {
    // canario-dmp.18: a config.json changed under a running sidecar
    // (other frontend, manual edit, CLI) must show up in get_config —
    // and a later update_config must not clobber the external change
    // with a stale boot snapshot (lost update).
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config/canario/config.json");
    let mut sidecar = Sidecar::spawn_with_home(tmp);

    sidecar.send(json!({ "cmd": "get_config", "id": "before" }));
    let before = sidecar.wait_for("before")["data"].clone();
    assert_eq!(before["auto_paste"], json!(true)); // default

    // External edit while the sidecar runs.
    let mut edited = before.clone();
    edited["auto_paste"] = json!(false);
    edited["sound_effects"] = json!(false);
    std::fs::write(&config_path, serde_json::to_string(&edited).unwrap()).unwrap();

    sidecar.send(json!({ "cmd": "get_config", "id": "after" }));
    let after = sidecar.wait_for("after")["data"].clone();
    assert_eq!(
        after["auto_paste"],
        json!(false),
        "external edit must be visible without restart"
    );
    assert_eq!(after["sound_effects"], json!(false));

    // update_config after an external edit keeps the external value:
    // its read-modify-write starts from the refreshed state.
    sidecar.send(json!({ "cmd": "update_config", "id": "upd", "config": { "auto_paste": true } }));
    assert_eq!(sidecar.wait_for("upd")["ok"], json!(true));
    sidecar.send(json!({ "cmd": "get_config", "id": "final" }));
    let final_config = sidecar.wait_for("final")["data"].clone();
    assert_eq!(final_config["auto_paste"], json!(true));
    assert_eq!(
        final_config["sound_effects"],
        json!(false),
        "update_config must not clobber the external edit"
    );
}

#[test]
fn pin_ping_shape_and_ts_protocol_constant() {
    // canario-dmp.4: the ping wire shape is the protocol handshake.
    // Pin every field so an accidental shape change fails here instead
    // of silently breaking the app's version check.
    let mut sidecar = Sidecar::spawn();
    sidecar.send(json!({ "cmd": "ping", "id": "pin-ping" }));
    let resp = sidecar.wait_for("pin-ping");
    assert_eq!(resp["ok"], json!(true));
    assert_eq!(resp["data"]["pong"], json!(true));
    assert!(
        resp["data"]["version"].as_str().is_some(),
        "ping must report the sidecar crate version: {resp}"
    );
    let wire_protocol = resp["data"]["protocol"]
        .as_u64()
        .unwrap_or_else(|| panic!("ping must report the protocol version: {resp}"))
        as u32;

    // The protocol constant lives in BOTH the sidecar (a binary crate —
    // not importable from an integration test, so parse the source)
    // and the Electron app (TypeScript; no codegen between the two
    // sides). Fail CI when any one of the three drifts from the others.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let declared = [
        (
            "canario-electron/src/main.rs",
            manifest.join("src/main.rs"),
            "pub const PROTOCOL_VERSION",
        ),
        (
            "canario-app/src/main/version.ts",
            manifest.join("../canario-app/src/main/version.ts"),
            "export const PROTOCOL_VERSION",
        ),
    ];
    for (label, path, needle) in declared {
        let source = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "cannot read {} to cross-check PROTOCOL_VERSION ({}); update this test if the file moved",
                path.display(),
                e
            )
        });
        let parsed: Option<u32> = source
            .lines()
            .find(|l| l.trim().starts_with(needle))
            .and_then(|l| l.split('=').nth(1))
            .and_then(|rhs| {
                rhs.trim()
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .ok()
            });
        assert_eq!(
            parsed,
            Some(wire_protocol),
            "PROTOCOL_VERSION drifted: wire={wire_protocol}, {label}={parsed:?} — bump all three together (sidecar const, TS const, this pin)"
        );
    }
}

/// Micro-benchmark: `get_config` round-trip latency (canario-dmp.18).
///
/// The Electron main process re-fetches config before each auto-paste
/// decision, so this round-trip sits on the transcript-to-paste
/// critical path — this bench is how a change to `get_config`'s cost
/// (e.g. the disk reload added in canario-dmp.18) proves it stays
/// negligible. Run explicitly:
///
/// ```text
/// cargo test -p canario-electron --test protocol bench_get_config -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn bench_get_config_roundtrip_latency() {
    let mut sidecar = Sidecar::spawn();

    // Warm-up: first round-trip includes process/pipe setup.
    for i in 0..20 {
        sidecar.send(json!({ "cmd": "get_config", "id": format!("warm-{i}") }));
        let _ = sidecar.wait_for(&format!("warm-{i}"));
    }

    const N: usize = 2000;
    let mut samples_us: Vec<f64> = Vec::with_capacity(N);
    for i in 0..N {
        let id = format!("bench-{i}");
        let start = std::time::Instant::now();
        sidecar.send(json!({ "cmd": "get_config", "id": id }));
        let resp = sidecar.wait_for(&id);
        samples_us.push(start.elapsed().as_secs_f64() * 1e6);
        assert_eq!(resp["ok"], json!(true));
    }

    samples_us.sort_by(|a, b| a.total_cmp(b));
    let mean = samples_us.iter().sum::<f64>() / N as f64;
    println!(
        "get_config round-trip over {N} calls: mean={mean:.1}µs p50={:.1}µs p95={:.1}µs p99={:.1}µs max={:.1}µs",
        samples_us[N / 2],
        samples_us[(N as f64 * 0.95) as usize],
        samples_us[(N as f64 * 0.99) as usize],
        samples_us[N - 1],
    );
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
fn status_and_lifecycle_commands_round_trip_when_idle() {
    // canario-dmp.5: the lifecycle must be queryable and cancellable
    // over the protocol. Idle-state round trip; the recording/download
    // halves are exercised by core's own tests (cancel semantics,
    // .part resume) since driving real audio/network from the protocol
    // harness is not feasible.
    let mut sidecar = Sidecar::spawn();

    sidecar.send(json!({ "cmd": "status", "id": "s1" }));
    let resp = sidecar.wait_for("s1");
    assert_eq!(resp["ok"], json!(true));
    assert_eq!(
        resp["data"],
        json!({ "recording": false, "transcribing": false, "downloading": false })
    );

    sidecar.send(json!({ "cmd": "is_downloading", "id": "d1" }));
    assert_eq!(sidecar.wait_for("d1")["data"], json!(false));

    // Cancels are safe no-ops when idle.
    sidecar.send(json!({ "cmd": "cancel_recording", "id": "c1" }));
    assert_eq!(sidecar.wait_for("c1")["ok"], json!(true));
    sidecar.send(json!({ "cmd": "cancel_download", "id": "c2" }));
    assert_eq!(sidecar.wait_for("c2")["ok"], json!(true));

    // Status is unchanged after the idle no-op cancels.
    sidecar.send(json!({ "cmd": "status", "id": "s2" }));
    assert_eq!(sidecar.wait_for("s2")["data"], resp["data"]);
}

#[test]
fn corrupted_config_is_quarantined_and_sidecar_boots_with_defaults() {
    // canario-dmp.16: a corrupt config used to abort Canario::new(),
    // exit the sidecar, and surface as a generic "sidecar not running"
    // toast after the 5s ping timeout. Now the file is quarantined and
    // the app boots from defaults.
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config/canario/config.json");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let corrupt = r#"{"model": "ParakeetV2", "auto_pas"#;
    std::fs::write(&config_path, corrupt).unwrap();

    let mut sidecar = Sidecar::spawn_with_home(tmp);

    // The sidecar boots despite the corrupt config and serves defaults.
    sidecar.send(json!({ "cmd": "get_config", "id": "boot" }));
    let resp = sidecar.wait_for("boot");
    assert_eq!(resp["ok"], json!(true));
    assert_eq!(resp["data"]["model"], json!("ParakeetV3"));

    // The corrupt bytes are preserved in exactly one .corrupt-* sibling...
    let config_dir = config_path.parent().unwrap();
    let mut quarantined: Vec<std::path::PathBuf> = std::fs::read_dir(config_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("config.json.corrupt-"))
                .unwrap_or(false)
        })
        .collect();
    quarantined.sort();
    assert_eq!(quarantined.len(), 1);
    assert_eq!(std::fs::read_to_string(&quarantined[0]).unwrap(), corrupt);

    // ...and the live config.json is valid defaults.
    let fresh: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(fresh["model"], json!("ParakeetV3"));

    // Diagnostics reports the quarantine path.
    sidecar.send(json!({ "cmd": "diagnostics", "id": "diag" }));
    let diag = sidecar.wait_for("diag")["data"].clone();
    let reported: Vec<String> = diag["quarantined_configs"]
        .as_array()
        .expect("quarantined_configs missing from diagnostics")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        reported,
        vec![quarantined[0].to_string_lossy().into_owned()]
    );
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
fn hotkey_status_before_start_reports_not_started() {
    let mut sidecar = Sidecar::spawn();

    sidecar.send(json!({ "cmd": "hotkey_status", "id": "hk-0" }));
    let resp = sidecar.wait_for("hk-0");

    assert_eq!(resp["ok"], json!(true));
    assert_eq!(resp["data"]["backend"], json!("not-started"));
    assert_eq!(resp["data"]["permission_denied"], json!(false));
    assert!(
        resp["data"]["fix_command"].is_null(),
        "unexpected fix_command: {resp}"
    );
}

/// The evdev permission probe runs synchronously inside start_hotkey,
/// so by the time the ok response arrives the status is settled —
/// this is what lets the renderer query on mount without racing
/// startup (the "not only in the logs" guarantee for the input-group
/// failure).
#[test]
fn hotkey_status_after_start_settles_and_carries_fix_command() {
    let mut sidecar = Sidecar::spawn();

    sidecar.send(json!({ "cmd": "start_hotkey", "id": "hk-start" }));
    assert_eq!(sidecar.wait_for("hk-start")["ok"], json!(true));

    sidecar.send(json!({ "cmd": "hotkey_status", "id": "hk-1" }));
    let resp = sidecar.wait_for("hk-1");

    assert_eq!(resp["ok"], json!(true));
    let backend = resp["data"]["backend"].as_str().unwrap();
    assert!(
        ["evdev", "x11", "socket-fallback"].contains(&backend),
        "unexpected backend {backend:?}: {resp}"
    );

    if resp["data"]["permission_denied"] == json!(true) {
        let fix = resp["data"]["fix_command"].as_str().unwrap();
        assert!(
            fix.contains("usermod") && fix.contains("input"),
            "fix_command should be a copy-pasteable usermod command: {fix:?}"
        );
    } else {
        assert!(
            resp["data"]["fix_command"].is_null(),
            "fix_command must only ride along with a permissions failure: {resp}"
        );
    }
}

/// A pre-unification Electron login entry (the contents the deleted
/// canario-app autostart.ts writer used to emit).
const LEGACY_AUTOSTART_ENTRY: &str = "\
[Desktop Entry]
Type=Application
Name=Canario
Comment=Voice-to-text
Exec=/opt/Canario/canario-electron
Icon=com.canario.Canario
Terminal=false
Categories=Utility;
X-GNOME-Autostart-enabled=true
Hidden=false
";

#[test]
fn set_autostart_with_exec_writes_regular_entry_and_syncs_config() {
    let tmp = tempfile::tempdir().unwrap();
    let entry = tmp
        .path()
        .join("config/autostart/com.canario.Canario.desktop");
    let mut sidecar = Sidecar::spawn_with_home(tmp);

    sidecar.send(json!({
        "cmd": "set_autostart",
        "id": "as-1",
        "enabled": true,
        "exec": "/usr/bin/fake-canario"
    }));
    let resp = sidecar.wait_for("as-1");
    assert_eq!(resp["ok"], json!(true));
    assert_eq!(resp["data"]["enabled"], json!(true));

    // A standalone REGULAR file (not a symlink) embedding the exec.
    let meta = std::fs::symlink_metadata(&entry).unwrap();
    assert!(meta.is_file(), "exec entry must be a regular file");
    let contents = std::fs::read_to_string(&entry).unwrap();
    assert!(
        contents.contains("Exec=/usr/bin/fake-canario"),
        "entry should embed the given exec: {contents}"
    );

    // config.autostart moved with the filesystem.
    sidecar.send(json!({ "cmd": "get_config", "id": "as-cfg-1" }));
    let resp = sidecar.wait_for("as-cfg-1");
    assert_eq!(resp["data"]["autostart"], json!(true));
}

#[test]
fn set_autostart_disable_removes_entry_and_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let entry = tmp
        .path()
        .join("config/autostart/com.canario.Canario.desktop");
    let mut sidecar = Sidecar::spawn_with_home(tmp);

    sidecar.send(json!({
        "cmd": "set_autostart",
        "id": "as-on",
        "enabled": true,
        "exec": "/usr/bin/fake-canario"
    }));
    assert_eq!(sidecar.wait_for("as-on")["ok"], json!(true));

    // Disable without an exec — the parameter is only meaningful for
    // enabling.
    sidecar.send(json!({ "cmd": "set_autostart", "id": "as-off", "enabled": false }));
    let resp = sidecar.wait_for("as-off");
    assert_eq!(resp["ok"], json!(true));
    assert_eq!(resp["data"]["enabled"], json!(false));
    assert!(!entry.exists(), "login entry must be gone");

    sidecar.send(json!({ "cmd": "get_config", "id": "as-cfg-2" }));
    let resp = sidecar.wait_for("as-cfg-2");
    assert_eq!(resp["data"]["autostart"], json!(false));
}

#[test]
fn legacy_autostart_entry_is_renamed_on_sidecar_boot() {
    let tmp = tempfile::tempdir().unwrap();
    let autostart_dir = tmp.path().join("config/autostart");
    std::fs::create_dir_all(&autostart_dir).unwrap();
    std::fs::write(
        autostart_dir.join("canario.desktop"),
        LEGACY_AUTOSTART_ENTRY,
    )
    .unwrap();

    let mut sidecar = Sidecar::spawn_with_home(tmp);

    // Wait until the sidecar is actually serving before inspecting the
    // filesystem — migration runs during startup, before the command
    // loop, and spawn() alone doesn't wait for it.
    sidecar.send(json!({ "cmd": "ping", "id": "booted" }));
    assert_eq!(sidecar.wait_for("booted")["ok"], json!(true));

    // Renamed to the unified identity, content preserved, no duplicate.
    let contents = std::fs::read_to_string(autostart_dir.join("com.canario.Canario.desktop"))
        .expect("legacy entry should have been renamed to the unified identity");
    assert_eq!(contents, LEGACY_AUTOSTART_ENTRY);
    assert!(!autostart_dir.join("canario.desktop").exists());
    assert_eq!(std::fs::read_dir(&autostart_dir).unwrap().count(), 1);
}

#[test]
fn legacy_autostart_entry_is_removed_when_new_identity_already_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let autostart_dir = tmp.path().join("config/autostart");
    std::fs::create_dir_all(&autostart_dir).unwrap();
    std::fs::write(
        autostart_dir.join("canario.desktop"),
        LEGACY_AUTOSTART_ENTRY,
    )
    .unwrap();
    std::fs::write(
        autostart_dir.join("com.canario.Canario.desktop"),
        "already unified",
    )
    .unwrap();

    let mut sidecar = Sidecar::spawn_with_home(tmp);

    sidecar.send(json!({ "cmd": "ping", "id": "booted" }));
    assert_eq!(sidecar.wait_for("booted")["ok"], json!(true));

    assert!(
        !autostart_dir.join("canario.desktop").exists(),
        "the duplicate legacy entry must be removed"
    );
    assert_eq!(
        std::fs::read_to_string(autostart_dir.join("com.canario.Canario.desktop")).unwrap(),
        "already unified",
        "the new-identity entry wins when both exist"
    );
    assert_eq!(std::fs::read_dir(&autostart_dir).unwrap().count(), 1);
}

/// Recursively collect every file under `dir` (best-effort, symlinks
/// not followed) — used by the credential-leak test to grep everything
/// the sidecar wrote under its temp HOME.
fn all_files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Serve exactly one HTTP request (headers + body captured over a
/// channel), replying with a canned OpenAI-style chat-completions
/// response. Returns the base URL (without version path) to configure.
fn spawn_one_shot_openai_server(
    response_body: &'static str,
) -> (String, std::sync::mpsc::Receiver<(String, String)>) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let n = stream.read(&mut chunk).unwrap();
            assert!(n > 0, "sidecar closed before sending headers");
            raw.extend_from_slice(&chunk[..n]);
            if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos;
            }
        };
        let headers = String::from_utf8_lossy(&raw[..header_end]).into_owned();
        let content_length: usize = headers
            .to_ascii_lowercase()
            .lines()
            .find(|l| l.starts_with("content-length:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        while raw.len() < header_end + 4 + content_length {
            let n = stream.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..n]);
        }
        let body = String::from_utf8_lossy(&raw[header_end + 4..]).into_owned();
        let _ = tx.send((headers, body));

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    (format!("http://{addr}"), rx)
}

/// canario-fgm.2 / fgm.1 D2: the transform credential is memory-only.
/// It arrives via `set_transform_credential`, is observable only as a
/// boolean, and must never reach config.json, the wire, or any file
/// the sidecar writes (logs included).
#[test]
fn set_transform_credential_is_memory_only_never_on_disk_or_logs() {
    const SECRET: &str = "sk-fgm2-leak-canary";
    let tmp = tempfile::tempdir().unwrap();
    // The Sidecar takes ownership of the TempDir (keeping it alive for
    // the child); capture the HOME path for the final on-disk sweep.
    let home = tmp.path().to_path_buf();
    let config_path = home.join("config/canario/config.json");
    let mut sidecar = Sidecar::spawn_with_home(tmp);

    // Default posture: disabled transform, no credential (D5a).
    sidecar.send(json!({ "cmd": "transform_status", "id": "t0" }));
    let status = sidecar.wait_for("t0");
    assert_eq!(status["ok"], json!(true));
    assert_eq!(status["data"]["enabled"], json!(false));
    assert_eq!(status["data"]["credential_present"], json!(false));
    assert_eq!(status["data"]["provider"]["base_url"], json!(""));
    assert_eq!(status["data"]["timeout_ms"], json!(4000));

    // A malformed credential-bearing line must not leak the key into
    // the log file either: the parse-failure path logs the raw input.
    sidecar.send_raw(&format!(
        "{{\"id\":\"bad-cred\",\"cmd\":\"set_transform_credential\",\"key\":\"{SECRET}\",\"oops\":"
    ));
    let resp = sidecar.wait_for("unknown"); // invalid JSON → id not recoverable
    assert_eq!(resp["ok"], json!(false));

    // An unknown command carrying a key field hits the same raw-line
    // log path with parseable JSON — also redacted.
    sidecar.send(json!({ "cmd": "explode", "id": "bad-cred-2", "key": SECRET }));
    assert_eq!(sidecar.wait_for("bad-cred-2")["ok"], json!(false));

    // Store the credential: the response reports presence, not value.
    sidecar.send(json!({ "cmd": "set_transform_credential", "id": "t1", "key": SECRET }));
    let resp = sidecar.wait_for("t1");
    assert_eq!(resp["ok"], json!(true));
    assert_eq!(resp["data"]["stored"], json!(true));
    assert!(!resp.to_string().contains(SECRET));

    // Persisted config carries provider metadata only — never the key.
    sidecar.send(json!({
        "cmd": "update_config",
        "id": "t2",
        "config": {
            "transform": {
                "enabled": true,
                "provider": { "base_url": "http://localhost:11434/v1", "model": "llama3" },
                "timeout_ms": 1500,
                "rules": []
            }
        }
    }));
    assert_eq!(sidecar.wait_for("t2")["ok"], json!(true));
    let on_disk = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        on_disk.contains("localhost:11434"),
        "provider metadata should persist: {on_disk}"
    );
    assert!(
        !on_disk.contains(SECRET),
        "config.json must never contain the key"
    );

    // get_config and transform_status echo the block minus any key.
    sidecar.send(json!({ "cmd": "get_config", "id": "t3" }));
    let config = sidecar.wait_for("t3");
    assert!(!config.to_string().contains(SECRET));
    assert_eq!(config["data"]["transform"]["enabled"], json!(true));

    sidecar.send(json!({ "cmd": "transform_status", "id": "t4" }));
    let status = sidecar.wait_for("t4");
    assert_eq!(status["data"]["enabled"], json!(true));
    assert_eq!(
        status["data"]["provider"]["base_url"],
        json!("http://localhost:11434/v1")
    );
    assert_eq!(status["data"]["provider"]["model"], json!("llama3"));
    assert_eq!(status["data"]["timeout_ms"], json!(1500));
    assert_eq!(status["data"]["credential_present"], json!(true));
    assert!(!status.to_string().contains(SECRET));

    // Clearing with key: null drops the memory-only copy.
    sidecar.send(json!({ "cmd": "set_transform_credential", "id": "t5", "key": null }));
    assert_eq!(sidecar.wait_for("t5")["data"]["stored"], json!(false));
    sidecar.send(json!({ "cmd": "transform_status", "id": "t6" }));
    assert_eq!(
        sidecar.wait_for("t6")["data"]["credential_present"],
        json!(false)
    );

    // D2 sweep: no file under the temp HOME (config, history, daily
    // logs, …) contains the key.
    for file in all_files_under(&home) {
        if let Ok(contents) = std::fs::read_to_string(&file) {
            assert!(
                !contents.contains(SECRET),
                "credential leaked into {}",
                file.display()
            );
        }
    }
}

/// canario-fgm.2: the settings "Test connection" button's backend —
/// one minimal chat-completions round trip through the configured
/// provider with the in-memory credential.
#[test]
fn transform_test_round_trips_against_a_local_openai_compatible_server() {
    const SECRET: &str = "sk-fgm2-test-key";
    let (base, captured) =
        spawn_one_shot_openai_server(r#"{"choices":[{"message":{"content":"pong"}}]}"#);
    let mut sidecar = Sidecar::spawn();

    sidecar.send(json!({
        "cmd": "update_config",
        "id": "cfg",
        "config": {
            "transform": {
                "enabled": true,
                "provider": { "base_url": format!("{base}/v1"), "model": "llama3" },
                "timeout_ms": 2000,
                "rules": []
            }
        }
    }));
    assert_eq!(sidecar.wait_for("cfg")["ok"], json!(true));
    sidecar.send(json!({ "cmd": "set_transform_credential", "id": "key", "key": SECRET }));
    assert_eq!(sidecar.wait_for("key")["data"]["stored"], json!(true));

    sidecar.send(json!({ "cmd": "transform_test", "id": "test" }));
    let resp = sidecar.wait_for("test");
    assert_eq!(resp["ok"], json!(true), "transform_test failed: {resp}");
    let latency = resp["data"]["latency_ms"].as_u64().unwrap();
    assert!(
        latency < 2000,
        "latency should be under the timeout: {latency}ms"
    );

    // The key travelled in the Authorization header only (D2)…
    let (headers, body) = captured.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        headers
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {SECRET}")),
        "missing bearer header: {headers}"
    );
    // …and the request body is the D5b minimal payload.
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["model"], json!("llama3"));
    assert_eq!(body["messages"][0]["role"], json!("system"));
    assert_eq!(body["messages"][1]["role"], json!("user"));
    assert_eq!(body["stream"], json!(false));
    assert!(body.to_string().contains("pong")); // the probe instruction/transcript
    assert!(!body.to_string().contains(SECRET));
}

#[test]
fn transform_test_fails_gracefully_without_a_server() {
    let mut sidecar = Sidecar::spawn();

    // No provider configured: fails before touching the network.
    sidecar.send(json!({ "cmd": "transform_test", "id": "t-none" }));
    let resp = sidecar.wait_for("t-none");
    assert_eq!(resp["ok"], json!(false));
    let err = resp["error"].as_str().unwrap();
    assert!(err.contains("base_url"), "unexpected error: {err}");

    // Nothing listening on the loopback endpoint + short timeout: a
    // fast, descriptive error (the D5d fallback contract).
    sidecar.send(json!({
        "cmd": "update_config",
        "id": "cfg",
        "config": {
            "transform": {
                "enabled": true,
                "provider": { "base_url": "http://127.0.0.1:9/v1", "model": "llama3" },
                "timeout_ms": 300,
                "rules": []
            }
        }
    }));
    assert_eq!(sidecar.wait_for("cfg")["ok"], json!(true));

    let started = Instant::now();
    sidecar.send(json!({ "cmd": "transform_test", "id": "t-refused" }));
    let resp = sidecar.wait_for("t-refused");
    assert_eq!(resp["ok"], json!(false));
    assert!(!resp["error"].as_str().unwrap().is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "transform_test should fail fast, not hang"
    );
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
