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
    if try_auto_type(text) {
        return Ok(true);
    }

    // Step 3: Try simulating Ctrl+V (best-effort)
    if try_simulate_paste() {
        return Ok(true);
    }

    // Clipboard has the text — user can Ctrl+V manually
    Ok(false)
}

/// Check if a command exists on PATH
fn command_exists(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Try to auto-type text using external tools.
fn try_auto_type(text: &str) -> bool {
    // xdotool (X11)
    if command_exists("xdotool") {
        if let Ok(status) = Command::new("xdotool")
            .args(["type", "--clearmodifiers", "--"])
            .arg(text)
            .status()
        {
            if status.success() {
                return true;
            }
        }
    }

    // wtype (Wayland)
    if command_exists("wtype") {
        if let Ok(status) = Command::new("wtype")
            .arg(text)
            .status()
        {
            if status.success() {
                return true;
            }
        }
    }

    // ydotool (both X11 and Wayland)
    if command_exists("ydotool") {
        if let Ok(status) = Command::new("ydotool")
            .args(["type", "--"])
            .arg(text)
            .status()
        {
            if status.success() {
                return true;
            }
        }
    }

    false
}

/// Try to simulate Ctrl+V paste
fn try_simulate_paste() -> bool {
    // xdotool (X11)
    if command_exists("xdotool") {
        if let Ok(status) = Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+v"])
            .status()
        {
            if status.success() {
                return true;
            }
        }
    }

    // ydotool (universal)
    // Ctrl+V: key 29 (left ctrl) down, key 47 (v) down, key 47 up, key 29 up
    if command_exists("ydotool") {
        if let Ok(status) = Command::new("ydotool")
            .args(["key", "29:1", "47:1", "47:0", "29:0"])
            .status()
        {
            if status.success() {
                return true;
            }
        }
    }

    false
}
