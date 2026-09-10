/// Paste text into the active application.
///
/// Strategy:
///   1. Always copy text to clipboard via `arboard` (pure Rust, no external deps)
///   2. Try to auto-type using xdotool / wtype / ydotool (best-effort)
///   3. If auto-type fails, text is already in clipboard — user can Ctrl+V
///
/// Returns Ok(true) if auto-typed, Ok(false) if only copied to clipboard.
use anyhow::Result;
use std::process::Command;
use std::sync::OnceLock;
use tracing::{debug, warn};

/// Paste text: copy to clipboard + attempt auto-type.
/// Returns Ok(true) if text was auto-typed, Ok(false) if only in clipboard.
pub fn paste_text(text: &str) -> Result<bool> {
    if text.is_empty() {
        return Ok(false);
    }

    // Step 1: Always put text in clipboard (no external deps needed)
    match arboard::Clipboard::new() {
        Ok(mut clipboard) => {
            if let Err(e) = clipboard.set_text(text) {
                warn!("Failed to set clipboard: {}", e);
            } else {
                debug!("Text copied to clipboard");
            }
        }
        Err(e) => {
            warn!("Failed to open clipboard: {}", e);
        }
    }

    // Step 2: Try auto-typing (best-effort)
    if let Some(tool) = try_auto_type(text) {
        tracing::info!("Pasted via auto-type ({})", tool);
        return Ok(true);
    }

    // Step 3: Try simulating Ctrl+V (best-effort)
    if let Some(tool) = try_simulate_paste() {
        tracing::info!("Pasted via simulated Ctrl+V ({})", tool);
        return Ok(true);
    }

    // Clipboard has the text — user can Ctrl+V manually
    tracing::info!("No paste tool succeeded — text is on the clipboard only");
    Ok(false)
}

/// Check if a command exists on PATH.
///
/// Result is cached per tool: `which` is only spawned once per process
/// instead of on every paste. (Tools are not expected to appear or
/// disappear mid-session.)
fn command_exists(name: &str) -> bool {
    static XDOTOOL: OnceLock<bool> = OnceLock::new();
    static WTYPE: OnceLock<bool> = OnceLock::new();
    static YDOTOOL: OnceLock<bool> = OnceLock::new();

    let cell = match name {
        "xdotool" => &XDOTOOL,
        "wtype" => &WTYPE,
        "ydotool" => &YDOTOOL,
        _ => return detect_command(name),
    };
    *cell.get_or_init(|| detect_command(name))
}

/// Actually probe for a command on PATH.
fn detect_command(name: &str) -> bool {
    let found = Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    debug!("paste tool detection: {} -> {}", name, found);
    found
}

/// Try to auto-type text using external tools.
/// Returns the name of the tool that succeeded.
fn try_auto_type(text: &str) -> Option<&'static str> {
    // xdotool (X11)
    if command_exists("xdotool") {
        if let Ok(status) = Command::new("xdotool")
            .args(["type", "--clearmodifiers", "--"])
            .arg(text)
            .stderr(std::process::Stdio::null())
            .status()
        {
            if status.success() {
                return Some("xdotool");
            }
        }
    }

    // wtype (Wayland)
    if command_exists("wtype") {
        if let Ok(status) = Command::new("wtype")
            .arg(text)
            .stderr(std::process::Stdio::null())
            .status()
        {
            if status.success() {
                return Some("wtype");
            }
        }
    }

    // ydotool (both X11 and Wayland)
    if command_exists("ydotool") {
        if let Ok(status) = Command::new("ydotool")
            .args(["type", "--"])
            .arg(text)
            .stderr(std::process::Stdio::null())
            .status()
        {
            if status.success() {
                return Some("ydotool");
            }
        }
    }

    None
}

/// Try to simulate Ctrl+V paste.
/// Returns the name of the tool that succeeded.
fn try_simulate_paste() -> Option<&'static str> {
    // xdotool (X11)
    if command_exists("xdotool") {
        if let Ok(status) = Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+v"])
            .stderr(std::process::Stdio::null())
            .status()
        {
            if status.success() {
                return Some("xdotool");
            }
        }
    }

    // ydotool (universal)
    // Ctrl+V: key 29 (left ctrl) down, key 47 (v) down, key 47 up, key 29 up
    if command_exists("ydotool") {
        if let Ok(status) = Command::new("ydotool")
            .args(["key", "29:1", "47:1", "47:0", "29:0"])
            .stderr(std::process::Stdio::null())
            .status()
        {
            if status.success() {
                return Some("ydotool");
            }
        }
    }

    None
}
