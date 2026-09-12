//! Plugin system: manifest discovery, resident child processes, and
//! the transform-stage dispatch (canario-11h.1 decisions, canario-11h.2
//! prototype).
//!
//! The binding decisions from canario-11h.1 (owner-confirmed):
//!
//! - **P1 surface** — v1 is a single `transform` hook at the same stage
//!   the fgm transform occupies (recording thread, post local rules,
//!   pre-LLM per OQ-1, pre-`TranscriptionReady` so history stores the
//!   final text). Overlay widgets, sound sets, rule packs and command
//!   access are deferred.
//! - **P2 runtime** — external processes speaking NDJSON over stdio
//!   ("a sidecar of the sidecar": the codebase's own established
//!   child-process pattern). Any executable; spawn lazily on first
//!   dispatch, keep resident, unload = kill (plugins are stateless
//!   filters, so no drain handshake in v1).
//! - **P3 distribution** — local install folder only:
//!   `~/.config/canario/plugins/<id>/{plugin.json, entry…}`. The
//!   manifest carries `api_version` + `source_url` so a future
//!   registry is additive. Hello-world ships in-repo as an example
//!   (`examples/plugins/uppercase/`), not in the release package.
//! - **P4 security** — manifest-declared capabilities enforced at the
//!   dispatch point: the permission IS the data we send. A plugin
//!   without the `transcripts` grant never receives text — it cannot
//!   exfiltrate what it never sees. Everything defaults OFF
//!   (`plugins.enabled_master` master kill-switch per OQ-9 + per-plugin
//!   allow-list). Honest limit: a process GRANTED transcripts can
//!   still use the network; OS-level sandboxing is a named future
//!   hardening bead, prerequisite for any third-party distribution.
//! - **Budgets (OQ-2)** — per-plugin deadline
//!   [`DEFAULT_PLUGIN_TIMEOUT_MS`] (1 s), whole-chain budget
//!   [`crate::config::PLUGIN_CHAIN_BUDGET_MS`] (2 s). Timeout, crash,
//!   malformed reply, or exhausted budget → the raw text flows on
//!   unchanged, the plugin is marked degraded (skipped until restart)
//!   and one log line is written — mirroring fgm D5d: dictation never
//!   blocks, no text is ever lost.
//!
//! Provenance (OQ-4) is log-only in v1: plugin rewrites are logged by
//! character count; no wire field was added to `TranscriptionReady`.
//!
//! The manager lives behind a process-wide store (the credential-store
//! precedent from `crate::transform`): the recording thread reaches it
//! without threading it through the recording API, and every frontend
//! (Electron sidecar, GTK, CLI) gets plugins for free (OQ-8). An
//! uninitialized store is a passthrough — tests and tooling that never
//! install one pay a single cheap lock read. Enabling a plugin takes
//! effect on the next `update_config` (the store is re-initialized when
//! the `plugins` block changes) or restart; manual `config.json` edits
//! need a restart.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::PluginsSettings;

/// The one capability v1 plugins can be granted (P4). Enforcement is
/// literal: no grant → the dispatch point never sends the transcript.
pub const PERMISSION_TRANSCRIPTS: &str = "transcripts";

/// The one hook v1 dispatches (P1).
pub const HOOK_TRANSFORM: &str = "transform";

/// Manifest schema version this build speaks (`api_version` in
/// plugin.json). A plugin declaring a different version is discovered
/// but never run — reported as incompatible by [`PluginManager::list`].
pub const PLUGIN_API_VERSION: u32 = 0;

// ── Manifest (plugin.json schema v0) ───────────────────────────────────────

/// `plugin.json` — one plugin's declaration (P1/P3/P4 wire shape).
///
/// Unknown fields are ignored and everything but `id`/`entry` defaults,
/// so hand-written and future manifests keep loading. `entry` is
/// resolved relative to the manifest's directory; `args` are passed
/// through verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PluginManifest {
    /// Plugin id. MUST equal its directory name — the allow-list and
    /// status reports key on it.
    pub id: String,
    /// Human-readable name for status surfaces.
    pub name: String,
    /// Plugin version (semver recommended, not enforced).
    pub version: String,
    /// Manifest schema version this plugin targets ([`PLUGIN_API_VERSION`]).
    pub api_version: u32,
    /// Executable to run, relative to the plugin directory.
    pub entry: String,
    /// Extra argv for the entry.
    pub args: Vec<String>,
    /// Hooks the plugin wants to receive. Only [`HOOK_TRANSFORM`]
    /// exists in v1; a plugin not listing it is never dispatched.
    pub hooks: Vec<String>,
    /// Requested capabilities. Only [`PERMISSION_TRANSCRIPTS`] exists
    /// in v1; without it the plugin receives no text (P4).
    pub permissions: Vec<String>,
    /// Where this plugin came from (URL or path) — future-registry
    /// provenance (P3), purely informational in v1.
    pub source_url: Option<String>,
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: String::new(),
            api_version: PLUGIN_API_VERSION,
            entry: String::new(),
            args: Vec::new(),
            hooks: Vec::new(),
            permissions: Vec::new(),
            source_url: None,
        }
    }
}

impl PluginManifest {
    /// Parse one manifest from its directory (`<dir>/plugin.json`),
    /// validating the load-bearing invariants: non-empty id matching
    /// the directory name, and a non-empty `entry`.
    fn load(dir: &Path) -> anyhow::Result<PluginManifest> {
        let path = dir.join("plugin.json");
        let raw = std::fs::read_to_string(&path)?;
        let manifest: PluginManifest = serde_json::from_str(&raw)?;
        let dirname = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if manifest.id.trim().is_empty() {
            anyhow::bail!("manifest id is empty");
        }
        if manifest.id != dirname {
            anyhow::bail!(
                "manifest id {:?} does not match directory name {:?}",
                manifest.id,
                dirname
            );
        }
        if manifest.entry.trim().is_empty() {
            anyhow::bail!("manifest entry is empty");
        }
        Ok(manifest)
    }

    /// The plugin directory this manifest was loaded from.
    fn dir(&self, root: &Path) -> PathBuf {
        root.join(&self.id)
    }

    /// Absolute entry path (manifest dir + `entry`).
    fn entry_path(&self, root: &Path) -> PathBuf {
        self.dir(root).join(&self.entry)
    }

    fn wants_transform(&self) -> bool {
        self.hooks.iter().any(|h| h == HOOK_TRANSFORM)
    }

    fn grants_transcripts(&self) -> bool {
        self.permissions.iter().any(|p| p == PERMISSION_TRANSCRIPTS)
    }
}

// ── Per-plugin runtime state ───────────────────────────────────────────────

/// Why a plugin is or is not currently transforming.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    /// Discovered and enabled, not yet spawned (spawn is lazy).
    Idle,
    /// Resident child running, accepting dispatches.
    Running,
    /// Failed (timeout/crash/malformed/budget) and skipped until the
    /// store is re-initialized — the raw text flows on in its place.
    Degraded,
    /// Incompatible `api_version` — never dispatched.
    Incompatible,
    /// Missing the `transform` hook or the `transcripts` grant — never
    /// dispatched (P4: the permission IS the data we send).
    NotGranted,
    /// Not in the allow-list, or the master switch is off — discovered
    /// but inert.
    Disabled,
}

/// One plugin's entry for the `list_plugins` / `plugin_status` commands.
#[derive(Debug, Clone, Serialize)]
pub struct PluginStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub state: PluginState,
    /// Last dispatch latency, when known (status surfaces only).
    pub last_latency_ms: Option<u64>,
    /// Total transform dispatches answered.
    pub calls: u64,
    /// Human-readable reason when `state` is `Degraded`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// A resident plugin child plus its reply pump.
struct PluginProcess {
    child: Child,
    stdin: ChildStdin,
    /// Reply pump channel: `(request id, Some(text))` for a well-formed
    /// reply, `(request id, None)` for a JSON reply without a string
    /// `text`. The pump thread owns stdout and exits when the child
    /// does (the channel then disconnects — seen by waiters as a crash).
    replies: Receiver<(String, Option<String>)>,
}

impl PluginProcess {
    /// Spawn the entry with piped stdio (stderr null: a plugin's own
    /// chatter must not leak into the sidecar's stderr) and a reader
    /// thread demultiplexing replies by id.
    fn spawn(manifest: &PluginManifest, root: &Path) -> anyhow::Result<Self> {
        let mut child = Command::new(manifest.entry_path(root))
            .args(&manifest.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("plugin stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("plugin stdout unavailable"))?;

        let (tx, replies) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue; // non-JSON chatter is ignored, not fatal
                };
                // A reply without an id is unroutable; without a string
                // text it is malformed (delivered as None → raw
                // fallback at the dispatch site).
                if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
                    let text = value.get("text").and_then(|v| v.as_str());
                    if tx.send((id.to_owned(), text.map(str::to_owned))).is_err() {
                        break;
                    }
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            replies,
        })
    }

    /// Send one transform request (NDJSON, id-matched).
    fn request_transform(&mut self, request_id: &str, text: &str) -> std::io::Result<()> {
        let payload = json!({ "id": request_id, "hook": HOOK_TRANSFORM, "text": text });
        writeln!(self.stdin, "{payload}")?;
        self.stdin.flush()
    }

    /// Await the reply for `request_id` until `deadline`, discarding
    /// stragglers from earlier timed-out requests (their dispatch
    /// already fell back).
    fn await_reply(&self, request_id: &str, deadline: Instant) -> Result<String, &'static str> {
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err("timeout");
            };
            match self.replies.recv_timeout(remaining) {
                Ok((id, Some(text))) if id == request_id => return Ok(text),
                Ok((id, None)) if id == request_id => return Err("malformed reply"),
                Ok(_) => continue, // stale id from a previous dispatch
                Err(RecvTimeoutError::Timeout) => return Err("timeout"),
                Err(RecvTimeoutError::Disconnected) => return Err("plugin exited"),
            }
        }
    }

    /// Best-effort kill (the pump thread exits when stdout closes).
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One discovered plugin's full runtime entry.
struct PluginEntry {
    manifest: PluginManifest,
    process: Option<PluginProcess>,
    state: PluginState,
    last_latency_ms: Option<u64>,
    calls: u64,
    last_error: Option<String>,
    /// Monotonic request counter for id matching (per plugin, so ids
    /// stay short and unique on the wire).
    next_request: u64,
}

// ── Manager ────────────────────────────────────────────────────────────────

/// Discovers, runs, and supervises the enabled plugin chain.
///
/// All state sits behind one mutex: the recording thread calls
/// [`PluginManager::transform`] synchronously, and `list`/`status`
/// read the same snapshot. Spawning is lazy (first dispatch), so an
/// enabled-but-never-used plugin costs nothing until dictation.
pub struct PluginManager {
    inner: Mutex<ManagerInner>,
}

struct ManagerInner {
    root: PathBuf,
    settings: PluginsSettings,
    entries: Vec<PluginEntry>,
}

impl PluginManager {
    /// Discover plugins under `root` (sorted by id for a deterministic
    /// chain order) and classify each against `settings`. Unreadable
    /// or invalid manifests are logged and skipped — one broken folder
    /// must not take the subsystem (or dictation) down.
    pub fn new(root: PathBuf, settings: PluginsSettings) -> Self {
        let mut dirs: Vec<PathBuf> = match std::fs::read_dir(&root) {
            Ok(read_dir) => read_dir
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect(),
            // A missing plugins dir is the normal fresh-install case.
            Err(e) => {
                tracing::debug!("plugins: cannot read {}: {e}", root.display());
                Vec::new()
            }
        };
        dirs.sort();

        let mut entries = Vec::new();
        for dir in dirs {
            match PluginManifest::load(&dir) {
                Ok(manifest) => {
                    tracing::debug!("plugins: discovered {} v{}", manifest.id, manifest.version);
                    entries.push(PluginEntry {
                        state: classify(&manifest, &settings),
                        manifest,
                        process: None,
                        last_latency_ms: None,
                        calls: 0,
                        last_error: None,
                        next_request: 0,
                    });
                }
                Err(e) => {
                    let id = dir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?")
                        .to_owned();
                    tracing::warn!("plugins: skipping {id}: {e}");
                }
            }
        }

        Self {
            inner: Mutex::new(ManagerInner {
                root,
                settings,
                entries,
            }),
        }
    }

    /// Discovery view for the `list_plugins` command: every manifest
    /// found under the root with its classification. Never spawns.
    pub fn list(&self) -> Vec<PluginStatus> {
        let inner = self.inner.lock().unwrap();
        inner.entries.iter().map(status_of).collect()
    }

    /// The `plugin_status` command: master-switch state plus runtime
    /// counters per plugin.
    pub fn status(&self) -> (bool, Vec<PluginStatus>) {
        let inner = self.inner.lock().unwrap();
        (
            inner.settings.enabled_master,
            inner.entries.iter().map(status_of).collect(),
        )
    }

    /// Run the enabled plugin chain over `text` (the transform hook,
    /// P1). Chain order is the sorted discovery order; in the pipeline
    /// the stage runs local rules → plugins → LLM (OQ-1, see
    /// `recording::emit_transcription_ready`).
    ///
    /// D5d posture, verbatim: on timeout, crash, malformed reply, or
    /// exhausted chain budget the text flows on unchanged and the
    /// plugin is marked degraded (skipped until the store is
    /// re-initialized). The call never blocks longer than
    /// [`crate::config::PLUGIN_CHAIN_BUDGET_MS`] overall.
    pub fn transform(&self, text: &str) -> String {
        let mut inner = self.inner.lock().unwrap();
        let root = inner.root.clone();
        let mut current = text.to_owned();
        let chain_deadline =
            Instant::now() + Duration::from_millis(crate::config::PLUGIN_CHAIN_BUDGET_MS);
        let per_plugin = inner.settings.effective_timeout();

        for entry in inner.entries.iter_mut() {
            if !matches!(entry.state, PluginState::Running | PluginState::Idle) {
                continue; // disabled / not-granted / incompatible / degraded
            }
            if Instant::now() >= chain_deadline {
                degrade(entry, "plugin chain budget exhausted");
                continue;
            }

            // Lazy spawn on first dispatch.
            if entry.process.is_none() {
                match PluginProcess::spawn(&entry.manifest, &root) {
                    Ok(process) => {
                        entry.process = Some(process);
                        entry.state = PluginState::Running;
                    }
                    Err(e) => {
                        degrade(entry, &format!("spawn failed: {e}"));
                        continue;
                    }
                }
            }

            let request_id = format!("req-{}", entry.next_request);
            entry.next_request += 1;
            let deadline = Instant::now() + per_plugin;
            let started = Instant::now();

            let outcome: Result<String, String> = entry
                .process
                .as_mut()
                .map(|process| {
                    process
                        .request_transform(&request_id, &current)
                        .map_err(|e| format!("write failed: {e}"))
                        .and_then(|()| {
                            process
                                .await_reply(&request_id, deadline)
                                .map_err(str::to_owned)
                        })
                })
                .unwrap_or_else(|| Err("no process".to_owned()));

            match outcome {
                Ok(transformed) => {
                    entry.last_latency_ms = Some(started.elapsed().as_millis() as u64);
                    entry.calls += 1;
                    if transformed != current {
                        // OQ-4: log-only provenance in v1 (counts, not
                        // the text itself — transcripts stay out of
                        // logs, mirroring the transform posture).
                        tracing::info!(
                            "plugin {} rewrote transcript ({} → {} chars)",
                            entry.manifest.id,
                            current.chars().count(),
                            transformed.chars().count()
                        );
                    }
                    current = transformed;
                }
                Err(reason) => {
                    // D5d: kill the misbehaving child, degrade the
                    // plugin, raw text flows on.
                    if let Some(process) = entry.process.as_mut() {
                        process.kill();
                    }
                    entry.process = None;
                    degrade(entry, &reason);
                }
            }
        }
        current
    }

    /// Kill every resident child (v1 unload: plugins are stateless
    /// filters, so close-and-kill is the whole lifecycle).
    pub fn shutdown(&self) {
        let mut inner = self.inner.lock().unwrap();
        for entry in inner.entries.iter_mut() {
            if let Some(mut process) = entry.process.take() {
                process.kill();
            }
        }
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Initial classification of a discovered manifest against settings.
fn classify(manifest: &PluginManifest, settings: &PluginsSettings) -> PluginState {
    if manifest.api_version != PLUGIN_API_VERSION {
        PluginState::Incompatible
    } else if !manifest.grants_transcripts() || !manifest.wants_transform() {
        PluginState::NotGranted
    } else if !settings.enabled_master || !settings.enabled.contains(&manifest.id) {
        PluginState::Disabled
    } else {
        PluginState::Idle
    }
}

/// Mark a plugin degraded with a reason (and log it — D5d).
fn degrade(entry: &mut PluginEntry, reason: &str) {
    tracing::warn!("plugin {} degraded: {reason}", entry.manifest.id);
    entry.state = PluginState::Degraded;
    entry.last_error = Some(reason.to_owned());
}

/// Snapshot one entry for the status commands.
fn status_of(entry: &PluginEntry) -> PluginStatus {
    PluginStatus {
        id: entry.manifest.id.clone(),
        name: entry.manifest.name.clone(),
        version: entry.manifest.version.clone(),
        api_version: entry.manifest.api_version,
        state: entry.state.clone(),
        last_latency_ms: entry.last_latency_ms,
        calls: entry.calls,
        last_error: entry.last_error.clone(),
    }
}

// ── Process-wide store (the credential-store precedent) ────────────────────

static MANAGER: OnceLock<Mutex<Option<Arc<PluginManager>>>> = OnceLock::new();

fn store() -> &'static Mutex<Option<Arc<PluginManager>>> {
    MANAGER.get_or_init(|| Mutex::new(None))
}

/// Install the process-wide manager (called by `Canario::new` from real
/// binaries and on `plugins` config changes; hermetic tests install
/// their own under a lock). Installing over an existing manager
/// replaces it — the old manager's children are killed by
/// [`PluginManager::shutdown`] on drop.
pub fn init(plugins_dir: PathBuf, settings: PluginsSettings) {
    let manager = Arc::new(PluginManager::new(plugins_dir, settings));
    *store().lock().unwrap() = Some(manager);
}

/// Remove the process-wide manager (kills children). The next transform
/// is a passthrough.
pub fn reset() {
    *store().lock().unwrap() = None;
}

/// Run the transform hook through the process-wide manager. A missing
/// manager (never installed / `cargo test` without one) is a
/// passthrough — one cheap lock read, no allocation.
pub fn transform(text: &str) -> String {
    let manager = store().lock().unwrap().clone();
    match manager {
        Some(manager) => manager.transform(text),
        None => text.to_owned(),
    }
}

/// `list_plugins` through the process-wide manager (empty when not
/// installed).
pub fn list() -> Vec<PluginStatus> {
    let manager = store().lock().unwrap().clone();
    manager.map(|m| m.list()).unwrap_or_default()
}

/// `plugin_status` through the process-wide manager (`(false, [])`
/// when not installed).
pub fn status() -> (bool, Vec<PluginStatus>) {
    let manager = store().lock().unwrap().clone();
    manager.map(|m| m.status()).unwrap_or((false, Vec::new()))
}

// ── Tests ──────────────────────────────────────────────────────────────────
//
// The fixtures are real python3 plugins — the same NDJSON-over-stdio
// dialect any third-party executable speaks (P2: "any executable").
// python3 is the one interpreter present everywhere this suite runs
// (CI ubuntu-latest, the maintainer's Linux desktop); the Rust-bin
// variant proving language-agnosticism is
// `canario-core/examples/uppercase_plugin.rs`.

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize every test that installs the process-wide store (the
    /// credential-store precedent).
    static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn write_plugin(root: &Path, id: &str, script: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.py"), script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                dir.join("plugin.py"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let manifest = json!({
            "id": id,
            "name": id,
            "version": "0.1.0",
            "api_version": PLUGIN_API_VERSION,
            "entry": "plugin.py",
            "hooks": [HOOK_TRANSFORM],
            "permissions": [PERMISSION_TRANSCRIPTS],
        });
        std::fs::write(
            dir.join("plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    /// The reference uppercase plugin (same shape as
    /// examples/plugins/uppercase).
    const UPPER_PY: &str = r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    req = json.loads(line)
    print(json.dumps({"id": req["id"], "text": req["text"].upper()}), flush=True)
"#;

    fn settings(ids: &[&str]) -> PluginsSettings {
        PluginsSettings {
            enabled_master: true,
            enabled: ids.iter().map(|s| s.to_string()).collect(),
            timeout_ms: 0, // default (clamped to 1 s)
        }
    }

    #[test]
    fn hello_world_transform_uppercases() {
        let _guard = STORE_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        write_plugin(root.path(), "upper", UPPER_PY);
        init(root.path().to_path_buf(), settings(&["upper"]));

        assert_eq!(transform("hello canario"), "HELLO CANARIO");

        // Status saw the dispatch: running, one call, sane latency.
        let (master, statuses) = status();
        assert!(master);
        let s = statuses.iter().find(|s| s.id == "upper").unwrap();
        assert_eq!(s.state, PluginState::Running);
        assert_eq!(s.calls, 1);
        assert!(s.last_latency_ms.is_some());
        assert!(s.last_error.is_none());

        reset();
    }

    #[test]
    fn timeout_falls_back_to_raw_and_degrades() {
        let _guard = STORE_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let slow = r#"#!/usr/bin/env python3
import json, sys, time
for line in sys.stdin:
    req = json.loads(line)
    time.sleep(5)
    print(json.dumps({"id": req["id"], "text": req["text"]}), flush=True)
"#;
        write_plugin(root.path(), "slow", slow);
        // Tight per-plugin deadline so the test stays fast (the clamp
        // floor is exactly 100 ms).
        init(
            root.path().to_path_buf(),
            PluginsSettings {
                timeout_ms: 100,
                ..settings(&["slow"])
            },
        );

        let started = Instant::now();
        assert_eq!(transform("unchanged please"), "unchanged please");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the deadline bounded the dispatch"
        );

        let (_, statuses) = status();
        let s = statuses.iter().find(|s| s.id == "slow").unwrap();
        assert_eq!(s.state, PluginState::Degraded);
        assert_eq!(s.last_error.as_deref(), Some("timeout"));
        assert_eq!(s.calls, 0);

        // Degraded stays skipped — a second transform doesn't re-wait.
        assert_eq!(transform("still raw"), "still raw");
        reset();
    }

    #[test]
    fn crash_falls_back_to_raw_and_degrades() {
        let _guard = STORE_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let crashy = r#"#!/usr/bin/env python3
import sys
sys.stdin.readline()  # wait for the first request…
sys.exit(3)           # …then die on it
"#;
        write_plugin(root.path(), "crash", crashy);
        init(root.path().to_path_buf(), settings(&["crash"]));

        assert_eq!(transform("raw survives"), "raw survives");

        let (_, statuses) = status();
        let s = statuses.iter().find(|s| s.id == "crash").unwrap();
        assert_eq!(s.state, PluginState::Degraded);
        assert!(s.last_error.is_some());
        reset();
    }

    #[test]
    fn chain_runs_in_sorted_order_and_composes() {
        let _guard = STORE_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        // Order probe: "ß" uppercases to "SS". bang ("b" < "u") appends
        // "ß" FIRST, then upper makes "HISS"; the reverse order would
        // end in a literal "ß". Sorted discovery order is observable.
        let bang = r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    req = json.loads(line)
    print(json.dumps({"id": req["id"], "text": req["text"] + "ß"}), flush=True)
"#;
        write_plugin(root.path(), "bang", bang);
        write_plugin(root.path(), "upper", UPPER_PY);
        init(root.path().to_path_buf(), settings(&["upper", "bang"]));

        assert_eq!(transform("hi"), "HISS");
        reset();
    }

    #[test]
    fn disabled_master_and_allowlist_are_inert() {
        let _guard = STORE_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        write_plugin(root.path(), "upper", UPPER_PY);

        // Master off: discovered but disabled, transform passthrough.
        init(
            root.path().to_path_buf(),
            PluginsSettings {
                enabled_master: false,
                enabled: vec!["upper".into()],
                timeout_ms: 0,
            },
        );
        assert_eq!(transform("quiet"), "quiet");
        let (master, statuses) = status();
        assert!(!master);
        assert_eq!(statuses[0].state, PluginState::Disabled);

        // Master on but plugin not allow-listed: still inert.
        init(
            root.path().to_path_buf(),
            PluginsSettings {
                enabled_master: true,
                enabled: vec![],
                timeout_ms: 0,
            },
        );
        assert_eq!(transform("quiet"), "quiet");
        let (_, statuses) = status();
        assert_eq!(statuses[0].state, PluginState::Disabled);
        reset();
    }

    #[test]
    fn missing_transcripts_grant_never_receives_text() {
        let _guard = STORE_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("mum");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.json"),
            json!({
                "id": "mum",
                "entry": "plugin.py",
                "hooks": [HOOK_TRANSFORM],
                // permissions deliberately ABSENT — P4: no grant, no
                // text. Classification happens before any spawn, so no
                // executable is needed either.
            })
            .to_string(),
        )
        .unwrap();

        init(root.path().to_path_buf(), settings(&["mum"]));
        assert_eq!(transform("private"), "private");
        let (_, statuses) = status();
        assert_eq!(statuses[0].state, PluginState::NotGranted);
        reset();
    }

    #[test]
    fn incompatible_api_version_is_never_run() {
        let _guard = STORE_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        write_plugin(root.path(), "future", UPPER_PY);
        let manifest_path = root.path().join("future/plugin.json");
        let mut manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        manifest["api_version"] = json!(99);
        std::fs::write(&manifest_path, manifest.to_string()).unwrap();

        init(root.path().to_path_buf(), settings(&["future"]));
        assert_eq!(transform("raw"), "raw");
        let (_, statuses) = status();
        assert_eq!(statuses[0].state, PluginState::Incompatible);
        reset();
    }

    #[test]
    fn malformed_reply_falls_back_to_raw() {
        let _guard = STORE_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        // JSON with the right id but no `text` — the pump delivers
        // `(id, None)`, the dispatch treats it as malformed.
        let half_reply = r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    req = json.loads(line)
    print(json.dumps({"id": req["id"], "got": req["text"]}), flush=True)
"#;
        write_plugin(root.path(), "half", half_reply);
        init(
            root.path().to_path_buf(),
            PluginsSettings {
                timeout_ms: 300,
                ..settings(&["half"])
            },
        );

        assert_eq!(transform("keep me"), "keep me");
        let (_, statuses) = status();
        let s = statuses.iter().find(|s| s.id == "half").unwrap();
        assert_eq!(s.state, PluginState::Degraded);
        assert_eq!(s.last_error.as_deref(), Some("malformed reply"));
        reset();
    }

    #[test]
    fn manifest_id_must_match_directory() {
        let root = tempfile::tempdir().unwrap();
        write_plugin(root.path(), "real-name", UPPER_PY);
        // Corrupt the id so it no longer matches the directory.
        let path = root.path().join("real-name/plugin.json");
        let mut manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        manifest["id"] = json!("other-name");
        std::fs::write(&path, manifest.to_string()).unwrap();

        let manager = PluginManager::new(root.path().to_path_buf(), settings(&["real-name"]));
        assert!(
            manager.list().is_empty(),
            "the mismatched manifest is skipped"
        );
    }

    #[test]
    fn missing_plugins_dir_is_an_empty_chain() {
        let root = tempfile::tempdir().unwrap();
        let manager =
            PluginManager::new(root.path().join("does-not-exist"), settings(&["anything"]));
        assert!(manager.list().is_empty());
        assert_eq!(manager.transform("passthrough"), "passthrough");
    }

    #[test]
    fn uninitialized_store_is_passthrough() {
        let _guard = STORE_LOCK.lock().unwrap();
        // reset() first in case another test left a manager installed.
        reset();
        assert_eq!(transform("as-is"), "as-is");
        assert!(list().is_empty());
        assert!(!status().0);
        assert!(status().1.is_empty());
    }

    /// OQ-2 measurement harness (run on demand, not in CI):
    /// `cargo test -p canario-core --release plugins -- --ignored --nocapture`
    ///
    /// Measures exactly what the plugin stage adds to
    /// release-to-transcript: (a) the passthrough cost with the
    /// subsystem disabled — the case every default install pays — and
    /// (b) the first-dispatch spawn + steady-state roundtrip with a
    /// real python plugin enabled. The rest of the pipeline is
    /// unchanged; end-to-end numbers for it live in the bench-pipeline
    /// runs (see .beads notes / canario-2z0-after.json baseline).
    #[test]
    #[ignore]
    fn bench_plugin_seam_costs() {
        let _guard = STORE_LOCK.lock().unwrap();

        // (a) Disabled: no manager installed at all.
        reset();
        const N: u32 = 100_000;
        let started = Instant::now();
        for i in 0..N {
            let _ = transform("benchmark transcript text");
            std::hint::black_box(i);
        }
        let per_call_off = started.elapsed().as_nanos() as f64 / f64::from(N);
        println!("passthrough (no manager): {per_call_off:.0} ns/call");

        // (b) Enabled: real python plugin, first call pays the spawn.
        let root = tempfile::tempdir().unwrap();
        write_plugin(root.path(), "upper", UPPER_PY);
        init(root.path().to_path_buf(), settings(&["upper"]));
        let first = Instant::now();
        assert_eq!(transform("cold"), "COLD");
        let first_ms = first.elapsed().as_millis();
        let started = Instant::now();
        const M: u32 = 200;
        for i in 0..M {
            assert_eq!(
                transform("benchmark transcript text"),
                "BENCHMARK TRANSCRIPT TEXT"
            );
            std::hint::black_box(i);
        }
        let per_call_on = started.elapsed().as_secs_f64() / f64::from(M);
        println!("enabled first dispatch (spawn): {first_ms} ms");
        println!(
            "enabled steady-state roundtrip: {:.3} ms/call",
            per_call_on * 1000.0
        );
        reset();
    }
}
