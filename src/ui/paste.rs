use anyhow::Result;
use std::process::Command;
use tracing::{debug, info};

/// Paste text into the active application
pub fn paste_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }

    // Try xdotool first (X11)
    if let Ok(output) = Command::new("xdotool").arg("--version").output() {
        if output.status.success() {
            debug!("Using xdotool for paste");
            // Use xdotool to type the text
            let status = Command::new("xdotool")
                .args(["type", "--clearmodifiers", "--"])
                .arg(text)
                .status()?;

            if status.success() {
                return Ok(());
            }
        }
    }

    // Fallback to wtype (Wayland)
    if let Ok(output) = Command::new("wtype").arg("--version").output() {
        if output.status.success() {
            debug!("Using wtype for paste");
            let status = Command::new("wtype").arg(text).status()?;
            if status.success() {
                return Ok(());
            }
        }
    }

    // Last resort: copy to clipboard and simulate Ctrl+V
    info!("Falling back to clipboard paste");
    copy_to_clipboard(text)?;
    simulate_paste()?;

    Ok(())
}

fn copy_to_clipboard(text: &str) -> Result<()> {
    // Try wl-copy (Wayland)
    if let Ok(mut child) = Command::new("wl-copy").stdin(std::process::Stdio::piped()).spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        return Ok(());
    }

    // Try xclip (X11)
    if let Ok(mut child) = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        return Ok(());
    }

    // Try xsel (X11)
    let mut child = Command::new("xsel")
        .args(["--clipboard", "--input"])
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(text.as_bytes())?;
    }
    child.wait()?;

    Ok(())
}

fn simulate_paste() -> Result<()> {
    // Try xdotool (X11)
    let status = Command::new("xdotool")
        .args(["key", "--clearmodifiers", "ctrl+shift+v"])
        .status();

    if status.map(|s| s.success()).unwrap_or(false) {
        return Ok(());
    }

    // Try wtype (Wayland) - wtype doesn't support key simulation easily
    // Try ydotool (works on both X11 and Wayland)
    let status = Command::new("ydotool")
        .args(["key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"]) // Ctrl+Shift+V
        .status();

    if status.map(|s| s.success()).unwrap_or(false) {
        return Ok(());
    }

    anyhow::bail!("No clipboard paste method available. Install xdotool, wtype, or ydotool.");
}
