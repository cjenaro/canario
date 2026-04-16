/// Paste text into the active application.
///
/// Strategy:
///   1. Always copy text to clipboard via `arboard` (pure Rust, no external deps)
///   2. Try to auto-type using xdotool / wtype / ydotool (best-effort)
///   3. If auto-type fails, text is already in clipboard — user can Ctrl+V
use anyhow::Result;
use std::process::Command;
use tracing::{debug, info, warn};

/// Paste text: copy to clipboard + attempt auto-type.
pub fn paste_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
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

    // Step 2: Try auto-typing (best-effort, may not be available)
    if let Some(text) = try_auto_type(text) {
        debug!("Auto-typed: {} chars", text.len());
        return Ok(());
    }

    // Step 3: Try simulating Ctrl+V (best-effort)
    if try_simulate_paste() {
        debug!("Simulated Ctrl+V");
        return Ok(());
    }

    // Clipboard has the text — user can Ctrl+V manually
    info!("Text copied to clipboard (auto-type unavailable — install xdotool, wtype, or ydotool)");
    Ok(())
}

/// Try to auto-type text using external tools. Returns the text if successful.
fn try_auto_type(text: &str) -> Option<String> {
    // xdotool (X11)
    if let Ok(output) = Command::new("xdotool").arg("--version").output() {
        if output.status.success() {
            let status = Command::new("xdotool")
                .args(["type", "--clearmodifiers", "--"])
                .arg(text)
                .status()
                .ok()?;
            if status.success() {
                return Some(text.to_string());
            }
        }
    }

    // wtype (Wayland)
    if let Ok(output) = Command::new("wtype").arg("--version").output() {
        if output.status.success() {
            let status = Command::new("wtype").arg(text).status().ok()?;
            if status.success() {
                return Some(text.to_string());
            }
        }
    }

    // ydotool (both X11 and Wayland)
    if let Ok(output) = Command::new("ydotool").arg("--version").output() {
        if output.status.success() {
            let status = Command::new("ydotool")
                .args(["type", "--"])
                .arg(text)
                .status()
                .ok()?;
            if status.success() {
                return Some(text.to_string());
            }
        }
    }

    None
}

/// Try to simulate Ctrl+V paste
fn try_simulate_paste() -> bool {
    // xdotool (X11)
    if let Ok(status) = Command::new("xdotool")
        .args(["key", "--clearmodifiers", "ctrl+v"])
        .status()
    {
        if status.success() {
            return true;
        }
    }

    // ydotool (universal)
    // Ctrl+V: key 29 (left ctrl) down, key 47 (v) down, key 47 up, key 29 up
    if let Ok(status) = Command::new("ydotool")
        .args(["key", "29:1", "47:1", "47:0", "29:0"])
        .status()
    {
        if status.success() {
            return true;
        }
    }

    false
}
