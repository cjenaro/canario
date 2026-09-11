//! LLM transformation: provider client, rule engine, and pipeline
//! orchestrator (canario-fgm.2 plumbing, canario-fgm.3 pipeline).
//!
//! The binding decisions from canario-fgm.1:
//!
//! - **D1** — LLM calls live in the SIDECAR (Rust, reqwest), never the
//!   renderer. This module is the only provider client; the wire is
//!   OpenAI chat-completions against any OpenAI-compatible
//!   [`base_url`](TransformSettings::provider) (OpenAI, Ollama, llama.cpp
//!   server, …). Non-OpenAI providers (Anthropic) will be adapted here
//!   when they land.
//! - **D2** — the API key NEVER persists here. It lives in the
//!   process-wide in-memory store ([`set_credential`] /
//!   [`credential`]) — the Electron main process pushes it into the
//!   sidecar's memory via the `set_transform_credential` command — and
//!   is used only as a `Authorization: Bearer` header value. Nothing in
//!   this module writes config, files, or logs.
//! - **D3** — [`apply_transformation`] runs in the recording thread
//!   BETWEEN transcription and the `TranscriptionReady` event (see
//!   `recording::emit_transcription_ready`): the event's `text` is the
//!   transformed transcript (what gets pasted and stored), with the raw
//!   transcript riding along as `raw_text` only when it differs.
//! - **D4** — [`focused_app`] is the best-effort focused-app seam for
//!   rule matching; [`match_rule`] implements the semantics
//!   (case-insensitive substring, default rule on empty `app_match`,
//!   `None` focused matches the default rule only, first match wins).
//! - **D5** — the request body carries ONLY the transcript plus a short
//!   style instruction (see [`chat_request_body`]) — never audio, never
//!   history, never config, never other transcripts. Local loopback
//!   endpoints are first-class ([`is_loopback_base_url`]). The timeout
//!   comes from [`TransformSettings::effective_timeout`]; on timeout or
//!   ANY error [`apply_transformation`] returns the raw transcript as
//!   [`TransformOutcome::Raw`] with the failure reason (D5d) — dictation
//!   never blocks and no audio is ever lost.

use std::time::Duration;

use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::TransformSettings;

/// Body snippet cap for provider error messages — enough to diagnose a
/// 401/404 without dumping a server's whole error page into a toast.
const ERROR_SNIPPET_MAX_CHARS: usize = 200;

/// Placeholder used by the settings connection test. The response text
/// is parsed but not matched — any 2xx with a choices[0].message.content
/// proves the endpoint speaks chat-completions with the configured
/// model.
const TEST_INSTRUCTION: &str = "You are a connectivity probe. Reply with the single word: pong.";
const TEST_TRANSCRIPT: &str = "ping";

/// Is `base_url` a local loopback endpoint (fgm.1 D5c)?
///
/// Loopback = the URL scheme is http/https (fgm.1: "no scheme
/// restriction beyond http(s)" — anything else isn't an endpoint) and
/// the host is `localhost` (case-insensitive), an IPv4 address in
/// `127.0.0.0/8`, `::1`, or an IPv4-mapped `::ffff:127.x.x.x`
/// (parsed with std's `Ipv6Addr`, so any equivalent serialization —
/// `::ffff:7f00:1` — counts too). Unparseable input is not local
/// (callers show the non-local warning + validation error instead of
/// silently treating a typo as safe).
pub fn is_loopback_base_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    // `host_str` keeps brackets on IPv6 literals (`[::1]`).
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" {
        return true;
    }
    if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() {
        return v4.is_loopback();
    }
    if let Ok(v6) = host.parse::<std::net::Ipv6Addr>() {
        return v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback());
    }
    false
}

/// The chat-completions endpoint for an OpenAI-compatible `base_url`.
///
/// Convention (matching OpenAI SDK usage): `base_url` includes any
/// version path — `https://api.openai.com/v1`,
/// `http://localhost:11434/v1` (Ollama), `http://localhost:8080/v1`
/// (llama.cpp server). A trailing slash is tolerated.
pub fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

/// Build the chat-completions request body (D5b: the payload that
/// leaves the machine contains ONLY the transcript + the short style
/// instruction — plus the wire-required model name and the
/// non-streaming flag; never audio, history, config, or other
/// transcripts).
pub fn chat_request_body(model: &str, instruction: &str, transcript: &str) -> Value {
    json!({
        "model": model,
        "messages": [
            { "role": "system", "content": instruction },
            { "role": "user", "content": transcript },
        ],
        "stream": false,
    })
}

/// One chat-completions call. `api_key` is the in-memory credential
/// (D2) — sent as a Bearer token when present and omitted otherwise
/// (local servers like Ollama need no key).
pub struct ChatCompletionRequest<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub api_key: Option<&'a str>,
    /// Short style instruction (the system message).
    pub instruction: &'a str,
    /// The dictation transcript (the only user content sent, D5b).
    pub transcript: &'a str,
    /// From [`TransformSettings::effective_timeout`].
    pub timeout: Duration,
}

/// Send one non-streaming chat-completions request and return the
/// assistant message text.
///
/// Errors are for the caller's fallback path (D5d) and diagnostics:
/// they describe the failure without ever containing the API key (the
/// key only ever travels inside the `Authorization` header, which no
/// reqwest error includes — and callers run [`redact`] on strings that
/// surface to logs as defense in depth).
pub async fn chat_completion(request: &ChatCompletionRequest<'_>) -> anyhow::Result<String> {
    let base_url = request.base_url.trim();
    if base_url.is_empty() {
        bail!("no transform provider base_url configured");
    }
    let model = request.model.trim();
    if model.is_empty() {
        bail!("no transform provider model configured");
    }

    let client = reqwest::Client::builder()
        .timeout(request.timeout)
        .build()?;
    let mut http = client
        .post(chat_completions_url(base_url))
        .json(&chat_request_body(
            model,
            request.instruction,
            request.transcript,
        ));
    if let Some(key) = request.api_key.map(str::trim).filter(|k| !k.is_empty()) {
        http = http.bearer_auth(key);
    }

    let response = http.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!(
            "provider returned HTTP {}: {}",
            status,
            truncate_for_error(&body)
        );
    }
    let body: Value = response.json().await?;
    body.pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("provider response missing choices[0].message.content"))
}

/// Connection probe for the settings UI ("Test connection"). Sends a
/// minimal chat-completions round trip through the configured provider
/// using the in-memory credential, returning the wall-clock latency.
///
/// The probe transcript is a fixed `"ping"` (D5b applies to it too:
/// nothing of the user's leaves the machine).
pub async fn test_connection(
    settings: &TransformSettings,
    api_key: Option<&str>,
) -> anyhow::Result<u64> {
    let started = std::time::Instant::now();
    chat_completion(&ChatCompletionRequest {
        base_url: &settings.provider.base_url,
        model: &settings.provider.model,
        api_key,
        instruction: TEST_INSTRUCTION,
        transcript: TEST_TRANSCRIPT,
        timeout: settings.effective_timeout(),
    })
    .await?;
    Ok(started.elapsed().as_millis() as u64)
}

/// Blocking wrapper over [`test_connection`] for the sidecar's
/// synchronous command loop. The current-thread runtime mirrors how
/// core bridges its async model downloads; the call blocks for at most
/// the configured timeout (same posture as the hotkey permission
/// probe).
pub fn test_connection_blocking(
    settings: &TransformSettings,
    api_key: Option<&str>,
) -> anyhow::Result<u64> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(test_connection(settings, api_key))
}

/// Cap a provider-returned error body at a readable length.
fn truncate_for_error(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= ERROR_SNIPPET_MAX_CHARS {
        text.to_owned()
    } else {
        let truncated: String = text.chars().take(ERROR_SNIPPET_MAX_CHARS).collect();
        format!("{truncated}…")
    }
}

// ── Process-wide credential store (fgm.3) ────────────────────────────────

/// The in-memory transform credential (D2), process-wide.
///
/// fgm.2 kept this in the sidecar's own static; fgm.3 moves it here so
/// the recording thread can read it without threading it through the
/// recording API (core and sidecar share one process). The Electron
/// main process pushes the key in via the sidecar's
/// `set_transform_credential` command, which delegates to
/// [`set_credential`]. NEVER persisted — not to config.json, not to
/// logs (the sidecar scrubs payload-derived log lines), not to
/// diagnostics.
static CREDENTIAL: std::sync::OnceLock<parking_lot::Mutex<Option<String>>> =
    std::sync::OnceLock::new();

/// Store (`Some` non-empty) or drop (`None`/blank) the transform
/// credential. Returns whether a key is held afterwards.
pub fn set_credential(key: Option<&str>) -> bool {
    let lock = CREDENTIAL.get_or_init(|| parking_lot::Mutex::new(None));
    let mut held = lock.lock();
    *held = key
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned);
    held.is_some()
}

/// Clone of the held credential, if any. Callers must treat the value
/// as a secret (D2): use it in request headers, never in logs or
/// serialized output.
pub fn credential() -> Option<String> {
    CREDENTIAL.get().and_then(|lock| lock.lock().clone())
}

// ── Focused-app detection (fgm.1 D4) ────────────────────────────────────

/// Best-effort identity of the focused application, for per-app rule
/// matching (fgm.1 D4). `None` means "unknown" — [`match_rule`] then
/// matches the default rule only.
///
/// Platform posture (binding decision D4):
///
/// - **X11** — `xdotool getactivewindow getwindowclassname` (the WM
///   class), guarded by a PATH probe like the paste backends so a
///   missing tool degrades to `None` instead of an error.
/// - **Wayland** — `None` for now (documented limitation): there is no
///   portable compositor-independent window-identity protocol, so
///   per-app rules fall back to the global default.
///   Compositor-specific detection is a future follow-up, not a
///   blocker.
/// - **Windows** — `GetForegroundWindow` + `GetWindowTextW` via the
///   existing `windows-sys` dependency.
/// - **macOS** — deferred until canario-7x5.1 (NSWorkspace): this stub
///   panics so a future macOS pipeline is forced to wire a real
///   implementation instead of silently matching nothing.
pub fn focused_app() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        focused_app_with_session(std::env::var_os("WAYLAND_DISPLAY").is_some())
    }
    #[cfg(target_os = "windows")]
    {
        windows_focused_app()
    }
    #[cfg(target_os = "macos")]
    {
        // macOS detection is deferred until canario-7x5.1 (NSWorkspace).
        // D4 says "deferred detection", not "crash the dictation": return
        // None so rules fall back to the global default — a transform
        // must never take the recording thread down on any platform
        // (same posture as D5d for provider failures).
        None
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

/// Linux detection with the session kind injected, so the Wayland
/// `None` contract is unit-testable without mutating process env.
#[cfg(target_os = "linux")]
fn focused_app_with_session(is_wayland: bool) -> Option<String> {
    if is_wayland {
        // Wayland limitation (D4): no portable focused-window identity;
        // rules fall back to the global default.
        return None;
    }
    x11_focused_app()
}

/// X11 path: WM class of the active window via xdotool. Only attempted
/// when xdotool is on PATH (probe cached per process, like paste.rs).
#[cfg(target_os = "linux")]
fn x11_focused_app() -> Option<String> {
    use std::process::{Command, Stdio};

    if !xdotool_available() {
        return None;
    }
    let output = Command::new("xdotool")
        .args(["getactivewindow", "getwindowclassname"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    normalize_class(&String::from_utf8_lossy(&output.stdout))
}

/// Is xdotool on PATH? Cached per process — the probe spawns `which`
/// once, like the paste tool detection.
#[cfg(target_os = "linux")]
fn xdotool_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::process::Command::new("which")
            .arg("xdotool")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Trim xdotool's stdout into a matchable identifier ("" → `None`).
#[cfg(target_os = "linux")]
fn normalize_class(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Windows path: title of the foreground window.
#[cfg(target_os = "windows")]
fn windows_focused_app() -> Option<String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};

    // SAFETY: both calls take/return plain window handles and buffers;
    // no COM, no callbacks, no reentrancy.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let mut buf = [0u16; 256];
    // SAFETY: `buf` is a valid UTF-16 buffer of `buf.len()` elements,
    // matching the count passed in (room for the NUL included).
    let len = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

// ── Rule engine (fgm.3) ──────────────────────────────────────────────────

/// One per-app transformation rule (`transform.rules[]`, fgm.3).
///
/// `app_match` is matched case-insensitively as a substring of the
/// [`focused_app`] identifier (D4); an EMPTY `app_match` is the default
/// rule — it matches any focused app (and is the only rule that can
/// match when detection returned `None`). Where it sits in `rules`
/// decides its precedence: [`match_rule`] takes the first match.
///
/// fgm.2 shipped `rules` as raw JSON placeholders; those entries load
/// without breaking the config (unknown fields ignored, missing fields
/// defaulted) — a placeholder that used the `instruction` key even
/// keeps it, becoming a working default rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct TransformRule {
    /// Case-insensitive substring of the focused-app identifier.
    /// Empty = the default (catch-all) rule.
    pub app_match: String,
    /// The style instruction for the provider, e.g. "make it formal".
    pub instruction: String,
}

/// Pick the rule for `focused` from `rules` (fgm.1 D4):
///
/// - matching is a case-insensitive substring test of `app_match`
///   against the focused-app identifier;
/// - the default rule (empty `app_match`) matches any focused app;
/// - `focused == None` (Wayland, missing xdotool, …) matches the
///   default rule ONLY — a specific app can never be confirmed;
/// - **first match wins**, so ordering is user-controlled precedence;
/// - `None` when nothing matches → the pipeline leaves the transcript
///   raw.
pub fn match_rule<'a>(
    rules: &'a [TransformRule],
    focused: Option<&str>,
) -> Option<&'a TransformRule> {
    rules.iter().find(|rule| match rule.app_match.trim() {
        "" => true,
        needle => focused.is_some_and(|id| id.to_lowercase().contains(&needle.to_lowercase())),
    })
}

// ── Pipeline orchestration (fgm.3, D3/D5d) ──────────────────────────────

/// Why [`TransformOutcome::Raw`] flowed on unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawReason {
    /// `transform.enabled` is false (D5a) — not a failure.
    Disabled,
    /// No rule matched the focused app (or the matched rule carries no
    /// instruction) — not a provider failure.
    NoRuleMatched,
    /// The provider call failed or timed out (D5d) — dictation must
    /// never block or lose audio, so the raw transcript flows on. The
    /// string is the error, for the caller's debug log.
    Provider(String),
}

impl RawReason {
    /// D5d: did a *failure* force the fallback? Drives the
    /// `transform_failed` flag on `Event::TranscriptionReady` — the
    /// benign reasons (disabled / no rule) are configuration states,
    /// not failures the UI should report.
    pub fn is_failure(&self) -> bool {
        matches!(self, RawReason::Provider(_))
    }
}

impl std::fmt::Display for RawReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RawReason::Disabled => write!(f, "transform disabled"),
            RawReason::NoRuleMatched => write!(f, "no transformation rule matched"),
            RawReason::Provider(err) => write!(f, "transform failed: {err}"),
        }
    }
}

/// The result of running the transformation pipeline over one
/// transcript (fgm.3). This enum IS the result type of
/// [`apply_transformation`]: per D5d every failure becomes `Raw` with a
/// reason instead of an `Err` — the caller always has text to emit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransformOutcome {
    /// The provider returned a transformed transcript.
    Transformed(String),
    /// The transcript flows on unchanged, with why.
    Raw { text: String, reason: RawReason },
}

/// Wrap a rule's style instruction into the system message actually
/// sent (the "instruction rendering" step): a short, fixed rewrite
/// contract around the user's style, so the provider returns only the
/// rewritten text (no preambles or quotes to strip downstream).
///
/// Kept deliberately short — it is part of every request payload (D5b).
pub fn render_instruction(style: &str) -> String {
    format!(
        "You rewrite voice-dictation transcripts. Apply this style instruction to the user's \
         text, keeping the original language: {}. Reply with ONLY the rewritten text — no \
         preamble, no quotes, no commentary.",
        style.trim()
    )
}

/// Run the transformation pipeline over one transcript (fgm.3):
///
/// 1. `settings.enabled` gate (D5a);
/// 2. [`match_rule`] against `focused` (D4);
/// 3. [`render_instruction`] the matched rule's style;
/// 4. one [`chat_completion`] call with the configured provider, the
///    in-memory credential (D2) and [`TransformSettings::effective_timeout`]
///    (D5d) — the reqwest client enforces the timeout.
///
/// Returns the outcome; on ANY error or timeout this is
/// [`TransformOutcome::Raw`] carrying the raw transcript plus the
/// failure reason (D5d — never block dictation, never lose audio). The
/// outer `Err` is reserved for the one thing that is not a provider
/// failure: failing to build the throwaway tokio runtime the blocking
/// call runs on (callers map it to a `Raw` fallback, same as fgm.2's
/// `test_connection_blocking` posture).
pub fn apply_transformation(
    settings: &TransformSettings,
    rules: &[TransformRule],
    credential: Option<&str>,
    transcript: &str,
    focused: Option<&str>,
) -> anyhow::Result<TransformOutcome> {
    let raw_for = |reason: RawReason| TransformOutcome::Raw {
        text: transcript.to_owned(),
        reason,
    };
    if !settings.enabled {
        return Ok(raw_for(RawReason::Disabled));
    }
    let Some(rule) = match_rule(rules, focused) else {
        return Ok(raw_for(RawReason::NoRuleMatched));
    };
    if rule.instruction.trim().is_empty() {
        return Ok(raw_for(RawReason::NoRuleMatched));
    }

    let instruction = render_instruction(&rule.instruction);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(chat_completion(&ChatCompletionRequest {
        base_url: &settings.provider.base_url,
        model: &settings.provider.model,
        api_key: credential,
        instruction: &instruction,
        transcript,
        timeout: settings.effective_timeout(),
    }));
    Ok(match result {
        Ok(text) => TransformOutcome::Transformed(text),
        // D5d: any provider failure (connection refused, HTTP error,
        // timeout, malformed response) falls back to the raw transcript.
        Err(e) => raw_for(RawReason::Provider(e.to_string())),
    })
}

/// Replace any occurrence of `secret` in `text` with `[redacted]`.
///
/// Defense in depth for D2: the sidecar runs this over every string it
/// logs that was derived from a raw command line, so even a malformed
/// payload or an error echoing input can never write the key to the
/// log file. Returns the input unchanged when there is no secret.
pub fn redact(text: &str, secret: Option<&str>) -> String {
    match secret.map(str::trim).filter(|s| !s.is_empty()) {
        Some(secret) if text.contains(secret) => text.replace(secret, "[redacted]"),
        _ => text.to_owned(),
    }
}

/// Shared fixtures for transform-related tests across the crate
/// (`recording` drives the pipeline's emit path through the same
/// servers).
#[cfg(test)]
pub(crate) mod test_support {
    use std::io::{Read, Write};

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    /// Read one full HTTP request (headers + Content-Length body) from
    /// `stream`, returning `(headers, body)`.
    fn read_one_request(stream: &mut std::net::TcpStream) -> (String, String) {
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let n = stream.read(&mut chunk).unwrap();
            assert!(n > 0, "client closed before sending headers");
            raw.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_subslice(&raw, b"\r\n\r\n") {
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
        (headers, body)
    }

    /// Serve exactly one HTTP request on a std TCP listener and hand
    /// the captured request (headers + body) back over a channel.
    /// Replies with `response_line` + `response_body`. Returns
    /// (listener address, receiver).
    fn spawn_one_shot_raw(
        response_line: &str,
        response_body: &'static str,
    ) -> (
        std::net::SocketAddr,
        std::sync::mpsc::Receiver<(String, String)>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let response_line = response_line.to_owned();

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (headers, body) = read_one_request(&mut stream);
            let _ = tx.send((headers, body));

            let response = format!(
                "{response_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        (addr, rx)
    }

    /// One-shot server answering `200 OK` with an OpenAI-style
    /// chat-completions body (see fgm.2's tests).
    pub fn spawn_one_shot_server(
        response_body: &'static str,
    ) -> (
        std::net::SocketAddr,
        std::sync::mpsc::Receiver<(String, String)>,
    ) {
        spawn_one_shot_raw("HTTP/1.1 200 OK", response_body)
    }

    /// One-shot server answering with an HTTP error status — the
    /// misbehaving provider (bad key, wrong model, …).
    pub fn spawn_one_shot_http_error(
        status_line: &str,
        response_body: &'static str,
    ) -> (
        std::net::SocketAddr,
        std::sync::mpsc::Receiver<(String, String)>,
    ) {
        spawn_one_shot_raw(status_line, response_body)
    }

    /// Accept one connection and hold it open without ever responding —
    /// the provider that never answers, for timeout enforcement (D5d).
    /// The socket closes when the thread ends (well after any test
    /// timeout window).
    pub fn spawn_black_hole_server() -> std::net::SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(30));
        });
        addr
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TransformProvider;
    use test_support::{spawn_one_shot_http_error, spawn_one_shot_server};

    #[test]
    fn loopback_detection() {
        // Loopback hosts (any scheme, any port, optional path).
        for url in [
            "http://localhost:11434/v1",
            "https://localhost/v1",
            "http://LOCALHOST:8080/v1",
            "http://127.0.0.1:8080/v1",
            "http://127.0.0.1/v1",
            "http://127.1.2.3/v1", // whole 127/8 is loopback
            "http://[::1]:8080/v1",
            "http://[::ffff:127.0.0.1]:8080/v1",
            "http://[::ffff:7f00:1]:8080/v1", // equivalent serialization
        ] {
            assert!(is_loopback_base_url(url), "expected loopback: {url}");
        }
        // Non-loopback hosts — cloud providers and LAN addresses are
        // NOT local even though a LAN box may be "the user's".
        for url in [
            "https://api.openai.com/v1",
            "http://api.anthropic.com/v1",
            "http://192.168.1.10:11434/v1",
            "http://10.0.0.2/v1",
            "http://172.17.0.1/v1",
            "http://0.0.0.0/v1",
        ] {
            assert!(!is_loopback_base_url(url), "expected non-loopback: {url}");
        }
        // Unparseable / empty input is conservatively non-local.
        for url in [
            "",
            "   ",
            "not a url",
            "localhost:11434",
            "ftp://localhost/v1",
        ] {
            assert!(!is_loopback_base_url(url), "expected non-loopback: {url}");
        }
    }

    #[test]
    fn chat_completions_url_joins_with_version_path() {
        assert_eq!(
            chat_completions_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("http://localhost:11434/v1/"),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn request_body_carries_only_transcript_and_instruction() {
        // D5b: the payload contains ONLY the transcript + the short
        // style instruction (plus the wire-required model + stream
        // flag). Exact-equality pins that no other user data — audio,
        // history, config, other transcripts — can ride along.
        let body = chat_request_body("llama3", "Fix typos.", "hello wrld");
        assert_eq!(
            body,
            json!({
                "model": "llama3",
                "messages": [
                    { "role": "system", "content": "Fix typos." },
                    { "role": "user", "content": "hello wrld" }
                ],
                "stream": false
            })
        );
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        // And no credential-shaped field ever appears in the body.
        assert!(body.get("api_key").is_none());
        assert!(body.get("key").is_none());
        assert!(body.get("authorization").is_none());
    }

    #[test]
    fn redact_scrubs_the_secret() {
        assert_eq!(
            redact("bearer sk-secret-123", Some("sk-secret-123")),
            "bearer [redacted]"
        );
        assert_eq!(
            redact("no secret here", Some("sk-secret-123")),
            "no secret here"
        );
        assert_eq!(redact("anything", None), "anything");
        assert_eq!(redact("empty", Some("   ")), "empty");
    }

    #[test]
    fn error_snippets_are_truncated() {
        let long = "x".repeat(500);
        let truncated = truncate_for_error(&long);
        assert_eq!(truncated.chars().count(), ERROR_SNIPPET_MAX_CHARS + 1); // + the ellipsis
        assert!(truncated.ends_with('…'));
        assert_eq!(truncate_for_error("short"), "short");
        assert_eq!(truncate_for_error("  padded  "), "padded");
    }

    #[tokio::test]
    async fn chat_completion_sends_credential_header_and_minimal_payload() {
        // End-to-end against a loopback server: proves (a) the key
        // arrives as a function argument and travels ONLY in the
        // Authorization header (D2), and (b) the request body carries
        // only the instruction + transcript (D5b).
        const SECRET: &str = "sk-fgm2-test-secret";
        let (addr, rx) = spawn_one_shot_server(r#"{"choices":[{"message":{"content":"pong"}}]}"#);

        let text = chat_completion(&ChatCompletionRequest {
            base_url: &format!("http://{addr}/v1"),
            model: "test-model",
            api_key: Some(SECRET),
            instruction: "Reply with pong.",
            transcript: "ping",
            timeout: Duration::from_secs(5),
        })
        .await
        .unwrap();

        assert_eq!(text, "pong");
        let (headers, body) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // Key in the Authorization header only…
        let lowercase = headers.to_ascii_lowercase();
        assert!(
            lowercase.contains(&format!("authorization: bearer {SECRET}")),
            "missing bearer header in {headers}"
        );
        // …and never in the JSON body.
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "test-model",
                "messages": [
                    { "role": "system", "content": "Reply with pong." },
                    { "role": "user", "content": "ping" }
                ],
                "stream": false
            })
        );
        assert!(!body.to_string().contains(SECRET));
        // The request hit the versioned chat-completions path.
        assert!(
            lowercase.starts_with("post /v1/chat/completions"),
            "unexpected request line in {headers}"
        );
    }

    #[tokio::test]
    async fn chat_completion_without_key_omits_authorization_header() {
        // Local servers (Ollama, llama.cpp) work keyless — no header.
        let (addr, rx) = spawn_one_shot_server(r#"{"choices":[{"message":{"content":"pong"}}]}"#);

        chat_completion(&ChatCompletionRequest {
            base_url: &format!("http://{addr}"),
            model: "llama3",
            api_key: None,
            instruction: "Reply with pong.",
            transcript: "ping",
            timeout: Duration::from_secs(5),
        })
        .await
        .unwrap();

        let (headers, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!headers.to_ascii_lowercase().contains("authorization"));
    }

    #[tokio::test]
    async fn chat_completion_reports_connection_errors() {
        // Nothing listens on port 1 → a fast, descriptive error (the
        // caller's fallback path, D5d).
        let err = chat_completion(&ChatCompletionRequest {
            base_url: "http://127.0.0.1:1/v1",
            model: "m",
            api_key: Some("sk-x"),
            instruction: "i",
            transcript: "t",
            timeout: Duration::from_millis(500),
        })
        .await
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn missing_configuration_errors_before_any_network() {
        for (base_url, model) in [("", "m"), ("http://localhost:11434/v1", "")] {
            let err = chat_completion(&ChatCompletionRequest {
                base_url,
                model,
                api_key: None,
                instruction: "i",
                transcript: "t",
                timeout: Duration::from_millis(50),
            })
            .await
            .unwrap_err();
            assert!(
                err.to_string().contains("no transform provider"),
                "unexpected error for base_url={base_url:?}: {err}"
            );
        }
    }

    // ── fgm.3: rule matching (D4 semantics) ─────────────────────────

    fn rule(app_match: &str, instruction: &str) -> TransformRule {
        TransformRule {
            app_match: app_match.into(),
            instruction: instruction.into(),
        }
    }

    /// Substring matching is case-insensitive in BOTH directions, and
    /// the first matching rule wins even when a later one also matches
    /// (ordering = user-controlled precedence).
    #[test]
    fn match_rule_substring_is_case_insensitive_and_first_wins() {
        let rules = vec![
            rule("whatsapp", "be informal"),
            rule("WhatsApp Web", "extra emojis"),
            rule("mail", "be formal"),
        ];
        // Case-insensitive both ways: the identifier's case and the
        // needle's case never matter.
        for focused in [
            "whatsapp",
            "Whatsapp",
            "WHATSAPP-desktop",
            "org.whatsapp.Whatsapp",
        ] {
            assert_eq!(
                match_rule(&rules, Some(focused)).map(|r| r.instruction.as_str()),
                Some("be informal"),
                "focused={focused}"
            );
        }
        // Needle uppercase matches lowercase identifier.
        let upper = vec![rule("FIREFOX", "be terse")];
        assert_eq!(
            match_rule(&upper, Some("firefox-esr")).map(|r| r.instruction.as_str()),
            Some("be terse")
        );
        // Substring, not equality: a bare suffix of the identifier.
        assert_eq!(
            match_rule(&rules, Some("thunderbird-mail-wizard")).map(|r| r.instruction.as_str()),
            Some("be formal")
        );
        // First match wins across overlapping needles.
        let overlap = vec![rule("fire", "first"), rule("firefox", "second")];
        assert_eq!(
            match_rule(&overlap, Some("firefox")).map(|r| r.instruction.as_str()),
            Some("first")
        );
    }

    /// The default rule (empty app_match) is a catch-all for a known
    /// focused app — and the ONLY thing that can match when detection
    /// failed (Wayland, missing xdotool: D4).
    #[test]
    fn match_rule_none_focused_matches_default_only() {
        let specific = vec![rule("whatsapp", "informal")];
        assert!(
            match_rule(&specific, None).is_none(),
            "an unknown focused app must never match a specific rule"
        );

        let with_default = vec![rule("whatsapp", "informal"), rule("", "tidy everything")];
        assert_eq!(
            match_rule(&with_default, None).map(|r| r.instruction.as_str()),
            Some("tidy everything")
        );
        // A known app that matches nothing specific also lands on the
        // default rule…
        assert_eq!(
            match_rule(&with_default, Some("krita")).map(|r| r.instruction.as_str()),
            Some("tidy everything")
        );
        // …but a matching specific rule earlier in the list wins over a
        // default that appears before it? No — first match wins: the
        // default placed FIRST swallows everything after it.
        let default_first = vec![rule("", "default"), rule("whatsapp", "informal")];
        assert_eq!(
            match_rule(&default_first, Some("whatsapp")).map(|r| r.instruction.as_str()),
            Some("default")
        );
        // Whitespace-only app_match is the default rule too.
        let padded = vec![rule("  ", "padded default")];
        assert_eq!(
            match_rule(&padded, None).map(|r| r.instruction.as_str()),
            Some("padded default")
        );
    }

    /// No rules (or nothing matching) → `None` → the pipeline leaves
    /// the transcript raw.
    #[test]
    fn match_rule_no_match_returns_none() {
        assert!(match_rule(&[], Some("firefox")).is_none());
        assert!(match_rule(&[rule("vim", "code style")], Some("firefox")).is_none());
    }

    // ── fgm.3: instruction rendering ─────────────────────────────────

    #[test]
    fn render_instruction_wraps_the_style_in_a_rewrite_contract() {
        let rendered = render_instruction("make it formal");
        assert!(
            rendered.contains("make it formal"),
            "the rule's style must ride along: {rendered}"
        );
        // The contract: output ONLY the rewritten text, same language.
        assert!(rendered.contains("ONLY the rewritten text"), "{rendered}");
        assert!(rendered.contains("original language"), "{rendered}");
        // …and nothing user-identifying beyond the style itself.
        assert!(!rendered.contains("transcript history"));
        // Leading/trailing style whitespace never leaks in.
        assert!(
            render_instruction("  tidy  ").contains("tidy. Reply"),
            "{rendered:?}"
        );
    }

    // ── fgm.3: pipeline orchestration (D3/D5d) ────────────────────────

    /// Everything `apply_transformation` needs, with the default rule
    /// FIRST so the focused app (whatever the test machine detects —
    /// Wayland `None`, X11 `Some(class)`) always matches it: the tests
    /// exercise the provider path, not the matcher.
    fn pipeline_settings(base_url: String, timeout_ms: u64) -> TransformSettings {
        TransformSettings {
            enabled: true,
            provider: TransformProvider {
                base_url,
                model: "llama3".into(),
            },
            timeout_ms,
            rules: vec![rule("", "be terse"), rule("never-matches-xyzzy", "unused")],
        }
    }

    /// Success path: the matched rule's instruction is RENDERED into
    /// the system message (D5b payload: instruction + transcript only)
    /// and the provider's text comes back as `Transformed`.
    #[test]
    fn apply_transformation_sends_rendered_instruction_and_transforms() {
        let (addr, rx) =
            spawn_one_shot_server(r#"{"choices":[{"message":{"content":"Tidy text."}}]}"#);
        let settings = pipeline_settings(format!("http://{addr}/v1"), 2000);

        let outcome = apply_transformation(
            &settings,
            &settings.rules,
            Some("sk-fgm3-pipeline"),
            "hello wrld",
            None,
        )
        .unwrap();

        assert_eq!(outcome, TransformOutcome::Transformed("Tidy text.".into()));
        let (headers, body) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("authorization: bearer sk-fgm3-pipeline"),
            "credential travels in the header only: {headers}"
        );
        let body: Value = serde_json::from_str(&body).unwrap();
        // The system message is the RENDERED instruction (the matched
        // rule's style wrapped in the rewrite contract), the user
        // message is the transcript — and that is the whole payload.
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("be terse"), "system message: {system}");
        assert!(
            system.contains("ONLY the rewritten text"),
            "system message: {system}"
        );
        assert_eq!(body["messages"][1]["content"], json!("hello wrld"));
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    }

    /// D5d, connection error: transform enabled but nothing listens
    /// (unroutable 127.0.0.1:9, short timeout) → `Raw` with the
    /// ORIGINAL transcript and a failure reason.
    #[test]
    fn apply_transformation_connection_error_falls_back_to_raw() {
        let settings = pipeline_settings("http://127.0.0.1:9/v1".into(), 250);

        let started = std::time::Instant::now();
        let outcome =
            apply_transformation(&settings, &settings.rules, None, "hello wrld", None).unwrap();

        let TransformOutcome::Raw { text, reason } = outcome else {
            panic!("a connection error must fall back to Raw");
        };
        assert_eq!(text, "hello wrld");
        let RawReason::Provider(err) = reason else {
            panic!("a connection error is a provider failure: {reason:?}");
        };
        assert!(!err.is_empty(), "the reason must carry the failure");
        // Failed fast — a refused connection must not hang the pipeline.
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// D5d, HTTP error: a provider answering 401 is still just a
    /// fallback-to-raw, with the status in the reason.
    #[test]
    fn apply_transformation_http_error_falls_back_to_raw() {
        let (addr, _rx) = spawn_one_shot_http_error(
            "HTTP/1.1 401 Unauthorized",
            r#"{"error":{"message":"bad key"}}"#,
        );
        let settings = pipeline_settings(format!("http://{addr}/v1"), 2000);

        let outcome =
            apply_transformation(&settings, &settings.rules, None, "hello wrld", None).unwrap();

        let TransformOutcome::Raw { text, reason } = outcome else {
            panic!("an HTTP error must fall back to Raw");
        };
        assert_eq!(text, "hello wrld");
        let RawReason::Provider(err) = reason else {
            panic!("an HTTP error is a provider failure: {reason:?}");
        };
        assert!(err.contains("401"), "reason should name the status: {err}");
    }

    /// D5d, timeout: a provider that accepts but never answers gets cut
    /// off at `effective_timeout` (clamped ≥ 250 ms) — the pipeline
    /// returns `Raw` around the timeout, not early, not never.
    #[test]
    fn apply_transformation_timeout_falls_back_to_raw() {
        let addr = test_support::spawn_black_hole_server();
        let settings = pipeline_settings(format!("http://{addr}/v1"), 250);

        let started = std::time::Instant::now();
        let outcome =
            apply_transformation(&settings, &settings.rules, None, "hello wrld", None).unwrap();

        let TransformOutcome::Raw { text, reason } = outcome else {
            panic!("a timeout must fall back to Raw");
        };
        assert_eq!(text, "hello wrld");
        assert!(
            matches!(reason, RawReason::Provider(_)),
            "a timeout is a provider failure: {reason:?}"
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(200),
            "returned after {elapsed:?} — before the 250 ms timeout could fire"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "returned after {elapsed:?} — the timeout did not bound the call"
        );
    }

    /// The benign fallbacks are NOT failures (D5d flag semantics):
    /// disabled transform, no matching rule, and a rule without an
    /// instruction all leave the transcript raw without flagging.
    #[test]
    fn apply_transformation_benign_skips_are_not_failures() {
        let mut settings = pipeline_settings("http://127.0.0.1:9/v1".into(), 250);

        // Disabled (D5a): raw, reason Disabled.
        settings.enabled = false;
        let outcome =
            apply_transformation(&settings, &settings.rules, None, "hello wrld", None).unwrap();
        assert_eq!(
            outcome,
            TransformOutcome::Raw {
                text: "hello wrld".into(),
                reason: RawReason::Disabled,
            }
        );

        // No rule matches and no default exists: raw, reason NoRuleMatched.
        let mut specific = settings.clone();
        specific.enabled = true;
        specific.rules = vec![rule("whatsapp", "informal")];
        let outcome =
            apply_transformation(&specific, &specific.rules, None, "hello wrld", None).unwrap();
        assert_eq!(
            outcome,
            TransformOutcome::Raw {
                text: "hello wrld".into(),
                reason: RawReason::NoRuleMatched,
            }
        );

        // Matched rule with an empty instruction: same benign reason.
        let mut blank = specific;
        blank.rules = vec![rule("", "   ")];
        let outcome = apply_transformation(&blank, &blank.rules, None, "hello wrld", None).unwrap();
        assert_eq!(
            outcome,
            TransformOutcome::Raw {
                text: "hello wrld".into(),
                reason: RawReason::NoRuleMatched,
            }
        );

        // …while a provider failure IS one (is_failure drives the
        // transform_failed flag on the event).
        assert!(!RawReason::Disabled.is_failure());
        assert!(!RawReason::NoRuleMatched.is_failure());
        assert!(RawReason::Provider("boom".into()).is_failure());
    }

    /// fgm.2's placeholder rule shapes (raw JSON) deserialize without
    /// failing the whole config — old config files keep loading. The
    /// placeholder `instruction` field matches the real shape so it is
    /// kept; unknown keys (`app`) are ignored, missing fields default.
    #[test]
    fn placeholder_rules_deserialize_as_defaults() {
        let parsed: Vec<TransformRule> = serde_json::from_str(
            r#"[{ "app": "firefox", "instruction": "be terse" }, { "placeholder": true }]"#,
        )
        .unwrap();
        assert_eq!(
            parsed,
            vec![
                TransformRule {
                    app_match: String::new(),
                    instruction: "be terse".into()
                },
                TransformRule::default(),
            ]
        );
        // Missing fields on a real-shaped rule also default.
        let parsed: Vec<TransformRule> =
            serde_json::from_str(r#"[{ "app_match": "vim" }]"#).unwrap();
        assert_eq!(parsed, vec![rule("vim", "")]);
    }

    // ── fgm.3: process-wide credential store (D2) ─────────────────────

    /// The store is process-global; serialize tests that touch it (the
    /// same posture as the sidecar's fgm.2 credential tests).
    static CREDENTIAL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn credential_store_round_trip_and_clearing() {
        let _guard = CREDENTIAL_TEST_LOCK.lock().unwrap();
        // D2 semantics: Some(non-empty) stores, None/blank drops.
        assert!(!set_credential(None));
        assert!(credential().is_none());

        assert!(set_credential(Some("sk-core-test")));
        assert_eq!(credential().as_deref(), Some("sk-core-test"));

        assert!(!set_credential(Some("   ")));
        assert!(credential().is_none());

        assert!(set_credential(Some("sk-core-test")));
        assert!(!set_credential(None));
        assert!(credential().is_none());
    }

    // ── fgm.3: focused-app detection (D4) ─────────────────────────────

    #[cfg(target_os = "linux")]
    #[test]
    fn focused_app_wayland_session_returns_none() {
        // The documented Wayland limitation: no portable focused-window
        // identity, so rules fall back to the global default. (The X11
        // branch spawns xdotool against the real session — not unit
        // testable, same posture as the paste tools.)
        assert_eq!(focused_app_with_session(true), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn xdotool_output_normalizes_to_a_matchable_identifier() {
        assert_eq!(normalize_class("firefox\n"), Some("firefox".into()));
        assert_eq!(
            normalize_class("  org.gnome.TextEditor  "),
            Some("org.gnome.TextEditor".into())
        );
        assert_eq!(normalize_class(""), None);
        assert_eq!(normalize_class("   \n"), None);
    }
}
