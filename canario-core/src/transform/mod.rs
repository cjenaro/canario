//! LLM transformation provider client (canario-fgm.2).
//!
//! The binding decisions from canario-fgm.1:
//!
//! - **D1** — LLM calls live in the SIDECAR (Rust, reqwest), never the
//!   renderer. This module is the only provider client; the wire is
//!   OpenAI chat-completions against any OpenAI-compatible
//!   [`base_url`](TransformSettings::provider) (OpenAI, Ollama, llama.cpp
//!   server, …). Non-OpenAI providers (Anthropic) will be adapted here
//!   when they land.
//! - **D2** — the API key NEVER persists here. It arrives as a function
//!   argument (the Electron main process pushes it into the sidecar's
//!   memory via `set_transform_credential`) and is used only as a
//!   `Authorization: Bearer` header value. Nothing in this module
//!   writes config, files, or logs.
//! - **D5** — the request body carries ONLY the transcript plus a short
//!   style instruction (see [`chat_request_body`]) — never audio, never
//!   history, never config, never other transcripts. Local loopback
//!   endpoints are first-class ([`is_loopback_base_url`]). The timeout
//!   comes from [`TransformSettings::effective_timeout`]; on timeout or
//!   ANY error this module reports the error and the CALLER falls back
//!   to the raw transcript (the pipeline itself lands in
//!   canario-fgm.3/4 — this module is plumbing only).
//!
//! No function here touches the recording pipeline. The sidecar wires
//! them into `transform_test` (connection probe) today; fgm.3/4 wire
//! them into the transcription path.

use std::time::Duration;

use anyhow::{anyhow, bail};
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Serve exactly one HTTP request on a std TCP listener and hand
    /// the captured request (headers + body) back over a channel.
    /// Returns (listener address, receiver).
    fn spawn_one_shot_server(
        response_body: &'static str,
    ) -> (
        std::net::SocketAddr,
        std::sync::mpsc::Receiver<(String, String)>,
    ) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Read the full request: headers, then Content-Length body.
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
            let _ = tx.send((headers, body));

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });

        (addr, rx)
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
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
}
