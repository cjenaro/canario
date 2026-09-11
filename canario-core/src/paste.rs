//! Paste text into the active application.
//!
//! Strategy (all platforms):
//!   1. Always copy text to the clipboard via `arboard` (pure Rust, no
//!      external deps)
//!   2. Inject the result into the focused app, best-effort:
//!      - Linux: auto-type via xdotool / wtype / ydotool, then a
//!        simulated Ctrl+V (xdotool / ydotool)
//!      - macOS: synthesized ⌘V via CoreGraphics `CGEvent`
//!      - Windows: synthesized Ctrl+V via Win32 `SendInput`
//!   3. If injection fails or is unavailable, the text is already on
//!      the clipboard — the user can paste manually.
//!
//! Contract: returns `Ok(true)` only if the keystroke injection backend
//! reported success; `Ok(false)` if the text is on the clipboard only.
//! Injection failure is never silently reported as a successful paste.

use anyhow::Result;
use tracing::{debug, warn};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Paste text: copy to clipboard + attempt injection into the focused app.
/// Returns Ok(true) if injected, Ok(false) if only in clipboard.
pub fn paste_text(text: &str) -> Result<bool> {
    paste_with(text, set_clipboard, inject)
}

/// The paste flow, with the clipboard and injection backends injected so
/// the event sequence and error behavior can be tested without a GUI.
///
/// Order matters: the clipboard is populated *before* any paste shortcut
/// is injected, so the focused app reads the fresh contents.
fn paste_with(
    text: &str,
    set_clipboard: impl FnOnce(&str) -> Result<()>,
    inject: impl FnOnce(&str) -> Result<&'static str>,
) -> Result<bool> {
    if text.is_empty() {
        return Ok(false);
    }

    // Step 1: Always put text in clipboard. Failure here only downgrades
    // the experience (manual Ctrl+V won't work), it must not abort the
    // injection attempt.
    if let Err(e) = set_clipboard(text) {
        warn!("Failed to set clipboard: {:#}", e);
    } else {
        debug!("Text copied to clipboard");
    }

    // Step 2: Best-effort injection into the focused app.
    match inject(text) {
        Ok(backend) => {
            tracing::info!("Pasted via {}", backend);
            Ok(true)
        }
        Err(e) => {
            tracing::info!("{:#} — text is on the clipboard only", e);
            Ok(false)
        }
    }
}

/// Copy text to the system clipboard via `arboard`.
fn set_clipboard(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text)?;
    Ok(())
}

/// Platform injection backend. Returns the backend name on success.
#[cfg(target_os = "macos")]
fn inject(_text: &str) -> Result<&'static str> {
    macos::inject_paste_shortcut()?;
    Ok("CoreGraphics CGEvent (Cmd+V)")
}

/// Platform injection backend. Returns the backend name on success.
#[cfg(target_os = "windows")]
fn inject(_text: &str) -> Result<&'static str> {
    windows::inject_paste_shortcut()?;
    Ok("Win32 SendInput (Ctrl+V)")
}

/// Platform injection backend. Returns the backend name on success.
#[cfg(target_os = "linux")]
fn inject(text: &str) -> Result<&'static str> {
    linux::inject(text)
}

/// Platforms without an injection backend: clipboard-only fallback.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn inject(_text: &str) -> Result<&'static str> {
    anyhow::bail!("paste injection is not supported on this platform")
}

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{anyhow, Result};
    use std::process::Command;
    use std::sync::OnceLock;
    use tracing::debug;

    /// Try to inject text into the focused app: auto-type first, then a
    /// simulated Ctrl+V. Returns the name of the tool that succeeded.
    pub(super) fn inject(text: &str) -> Result<&'static str> {
        if let Some(tool) = try_auto_type(text) {
            return Ok(match tool {
                "xdotool" => "auto-type (xdotool)",
                "wtype" => "auto-type (wtype)",
                "ydotool" => "auto-type (ydotool)",
                _ => "auto-type",
            });
        }
        if let Some(tool) = try_simulate_paste() {
            return Ok(match tool {
                "xdotool" => "simulated Ctrl+V (xdotool)",
                "ydotool" => "simulated Ctrl+V (ydotool)",
                _ => "simulated Ctrl+V",
            });
        }
        Err(anyhow!("no paste tool succeeded"))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::cell::RefCell;

    /// Records the sequence of backend calls so ordering can be asserted.
    struct Log {
        events: RefCell<Vec<String>>,
    }

    impl Log {
        fn new() -> Self {
            Self {
                events: RefCell::new(Vec::new()),
            }
        }
        fn clipboard(&self, text: &str) -> Result<()> {
            self.events.borrow_mut().push(format!("clipboard:{text}"));
            Ok(())
        }
        fn inject_ok(&self, _text: &str) -> Result<&'static str> {
            self.events.borrow_mut().push("inject:ok".into());
            Ok("mock")
        }
        fn events(&self) -> Vec<String> {
            self.events.borrow().clone()
        }
    }

    #[test]
    fn empty_text_does_nothing() {
        let log = Log::new();
        let result = paste_with("", |t| log.clipboard(t), |t| log.inject_ok(t));
        assert!(!result.unwrap());
        assert!(log.events().is_empty(), "no backend calls for empty text");
    }

    #[test]
    fn clipboard_is_populated_before_injection() {
        let log = Log::new();
        let result = paste_with("hello", |t| log.clipboard(t), |t| log.inject_ok(t));
        assert!(result.unwrap());
        assert_eq!(log.events(), vec!["clipboard:hello", "inject:ok"]);
    }

    #[test]
    fn injection_success_reports_true() {
        let result = paste_with("hi", |_| Ok(()), |_| Ok("mock-backend"));
        assert!(result.unwrap());
    }

    #[test]
    fn injection_failure_falls_back_to_clipboard_only() {
        let log = Log::new();
        let result = paste_with(
            "hello",
            |t| log.clipboard(t),
            |_| Err(anyhow!("injection unavailable")),
        );
        // Genuine failure must NOT be reported as a successful paste...
        assert!(!result.unwrap());
        // ...but the clipboard fallback contract still holds.
        assert_eq!(log.events(), vec!["clipboard:hello"]);
    }

    #[test]
    fn clipboard_failure_does_not_abort_injection() {
        let log = Log::new();
        let result = paste_with(
            "hello",
            |_| Err(anyhow!("no clipboard")),
            |t| log.inject_ok(t),
        );
        assert!(result.unwrap());
        assert_eq!(log.events(), vec!["inject:ok"]);
    }

    #[test]
    fn both_backends_failing_reports_false_not_error() {
        let result = paste_with(
            "hello",
            |_| Err(anyhow!("no clipboard")),
            |_| Err(anyhow!("no injector")),
        );
        // Clipboard-only path is a degraded success, not a hard error:
        // callers must be able to keep running.
        assert!(!result.unwrap());
    }

    #[test]
    fn injection_receives_the_text() {
        let seen = RefCell::new(String::new());
        let result = paste_with(
            "transcribed text",
            |_| Ok(()),
            |t| {
                *seen.borrow_mut() = t.to_string();
                Ok("mock")
            },
        );
        assert!(result.unwrap());
        assert_eq!(seen.into_inner(), "transcribed text");
    }
}
