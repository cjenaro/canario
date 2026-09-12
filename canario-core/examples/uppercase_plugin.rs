//! Rust twin of `examples/plugins/uppercase/upper.py` — proof that the
//! plugin runtime is language-agnostic (canario-11h.1 P2: any
//! executable speaking NDJSON over stdio).
//!
//! Run it as a plugin by pointing a manifest's `entry` at the built
//! binary (`cargo build --example uppercase_plugin`, then use
//! `target/debug/examples/uppercase_plugin` as `entry` — absolute
//! paths are fine; `entry` resolves relative to the plugin dir).
//!
//! The whole contract: one JSON request per stdin line, one JSON reply
//! per request, matched by `id`, flushed per line.

use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue; // non-JSON chatter is ignored, not fatal
        };
        let Some(id) = request.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let text = request
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_uppercase();
        let reply = serde_json::json!({ "id": id, "text": text });
        if writeln!(stdout, "{reply}")
            .and_then(|()| stdout.flush())
            .is_err()
        {
            break; // sidecar closed the pipe: exit quietly
        }
    }
}
