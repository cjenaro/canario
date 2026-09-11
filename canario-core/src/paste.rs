//! Paste text into the active application.
//!
//! Strategy (all platforms):
//!   1. Always copy text to the clipboard via `arboard` (pure Rust, no
//!      external deps), then read the clipboard back. `set_text`
//!      returning is not proof the rest of the session (compositor,
//!      clipboard managers, the focused app) can see the new content
//!      yet — a paste keystroke sent too early pastes whatever was
//!      there before (the stale-clipboard race, canario-fhm) — so the
//!      verification is a short, bounded read-retry.
//!   2. Inject the result into the focused app, best-effort:
//!      - Linux: one synthesized Ctrl+V (ydotool anywhere; xdotool
//!        under X11 — under Wayland it can only reach XWayland and
//!        would "succeed" while the keystroke goes nowhere). The
//!        clipboard is already populated and verified, so this is a
//!        single round trip instead of one keystroke per character.
//!        Char-by-char typing (wtype under Wayland, xdotool under X11,
//!        ydotool anywhere) stays the fallback for apps that swallow
//!        synthetic pastes and for the unverified-clipboard case.
//!      - macOS: synthesized ⌘V via CoreGraphics `CGEvent`
//!      - Windows: synthesized Ctrl+V via Win32 `SendInput`
//!   3. If injection fails or is unavailable, the text is already on
//!      the clipboard — the user can paste manually.
//!
//! Contract: returns `Ok(true)` only if the keystroke injection backend
//! reported success; `Ok(false)` if the text is on the clipboard only.
//! Injection failure is never silently reported as a successful paste.

use anyhow::Result;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tracing::{debug, warn};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Paste text: copy to clipboard + attempt injection into the focused app.
/// Returns Ok(true) if injected, Ok(false) if only in clipboard.
pub fn paste_text(text: &str) -> Result<bool> {
    crate::timing::mark("paste_start");
    let out = paste_with(text, set_clipboard, verify_clipboard, inject);
    crate::timing::mark("paste_done");
    out
}

/// The paste flow, with the clipboard, verification and injection
/// backends injected so the event sequence and error behavior can be
/// tested without a GUI.
///
/// Order matters: the clipboard is populated and *verified readable*
/// before any paste shortcut is injected, so the focused app reads the
/// fresh contents. The verification result is passed to the injector,
/// which skips the paste shortcut (falling back to typing) when the
/// read-back could not confirm the fresh text.
fn paste_with(
    text: &str,
    set_clipboard: impl FnOnce(&str) -> Result<()>,
    verify_clipboard: impl FnOnce(&str) -> bool,
    inject: impl FnOnce(&str, bool) -> Result<&'static str>,
) -> Result<bool> {
    if text.is_empty() {
        return Ok(false);
    }

    // Step 1: Always put text in clipboard. Failure here only downgrades
    // the experience (manual Ctrl+V won't work), it must not abort the
    // injection attempt.
    let mut clipboard_verified = false;
    match set_clipboard(text) {
        Err(e) => warn!("Failed to set clipboard: {:#}", e),
        Ok(()) => {
            debug!("Text copied to clipboard");
            // Step 2: read the clipboard back before trusting it with a
            // paste keystroke. A mismatch after a short propagation wait
            // means the write has not landed everywhere it must: a Ctrl+V
            // now would deliver stale content (canario-fhm), so the
            // injectors fall back to typing instead.
            clipboard_verified = verify_clipboard(text);
            if !clipboard_verified {
                warn!("Clipboard read-back did not match — paste keystroke skipped (stale-clipboard guard)");
            }
        }
    }

    // Step 3: Best-effort injection into the focused app.
    match inject(text, clipboard_verified) {
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

/// Process-lifetime clipboard handle.
///
/// On Linux the clipboard contents are *served*: on X11 by a thread and
/// selection window owned by the `arboard::Clipboard` that published
/// them (dropping the last handle hands the data to a clipboard manager
/// if one runs — and unpublishes it otherwise), on Wayland by a
/// detached serve process. Holding one handle for the life of the
/// process keeps the just-copied text alive for the synthetic Ctrl+V
/// that follows (canario-cy0) instead of racing its publication against
/// the handle's drop, and makes the set → read-back verification a real
/// round trip through the session rather than a reconnect.
static CLIPBOARD: OnceLock<Option<Mutex<arboard::Clipboard>>> = OnceLock::new();

/// The shared clipboard handle, initialized on first use.
///
/// Initialization failure (headless session, no display) is cached: the
/// clipboard stays unusable and callers fall back to typing — the same
/// outcome as the per-call `Clipboard::new()` failures this replaces.
fn clipboard_handle() -> Option<&'static Mutex<arboard::Clipboard>> {
    CLIPBOARD
        .get_or_init(|| arboard::Clipboard::new().ok().map(Mutex::new))
        .as_ref()
}

/// Copy text to the system clipboard via `arboard`.
fn set_clipboard(text: &str) -> Result<()> {
    let handle = clipboard_handle().ok_or_else(|| anyhow::anyhow!("no clipboard available"))?;
    // A panic while holding the lock does not invalidate the clipboard
    // itself; recover the live handle.
    let mut clipboard = handle.lock().unwrap_or_else(|p| p.into_inner());
    clipboard.set_text(text)?;
    Ok(())
}

/// Read the clipboard back. On Wayland this is a compositor round trip
/// (exactly the path a real Ctrl+V takes); on X11 arboard short-circuits
/// when this process owns the selection, which is fine there — selection
/// ownership is server-synchronous once `set_text` has flushed.
fn read_clipboard() -> Result<String> {
    let handle = clipboard_handle().ok_or_else(|| anyhow::anyhow!("no clipboard available"))?;
    let mut clipboard = handle.lock().unwrap_or_else(|p| p.into_inner());
    Ok(clipboard.get_text()?)
}

/// Waits (ms) before each clipboard read-back attempt: the first check
/// is immediate, the rest give a lagging write room to propagate. The
/// total is deliberately small — the fallback it guards (typing) costs
/// far more than the worst case here.
const READBACK_WAITS_MS: [u64; 4] = [0, 10, 25, 50];

/// Verify the clipboard actually holds `text` (canario-fhm guard).
///
/// Returns true as soon as a read-back matches; false after the bounded
/// retry window, in which case the caller must not send a paste
/// keystroke (it would deliver stale content) and falls back to typing.
fn verify_clipboard(text: &str) -> bool {
    for wait_ms in READBACK_WAITS_MS {
        if wait_ms > 0 {
            std::thread::sleep(Duration::from_millis(wait_ms));
        }
        match read_clipboard() {
            Ok(current) if current == text => {
                debug!("Clipboard read-back matched after {wait_ms}ms wait");
                return true;
            }
            Ok(current) => debug!(
                "Clipboard read-back differs ({} vs {} chars) after {wait_ms}ms wait",
                current.chars().count(),
                text.chars().count()
            ),
            Err(e) => debug!("Clipboard read-back failed after {wait_ms}ms wait: {:#}", e),
        }
    }
    false
}

/// Platform injection backend. Returns the backend name on success.
/// `clipboard_verified` reports whether the read-back confirmed the
/// fresh text is visible — Linux uses it to skip the paste shortcut.
#[cfg(target_os = "macos")]
fn inject(_text: &str, _clipboard_verified: bool) -> Result<&'static str> {
    macos::inject_paste_shortcut()?;
    Ok("CoreGraphics CGEvent (Cmd+V)")
}

/// Platform injection backend. Returns the backend name on success.
/// `clipboard_verified` reports whether the read-back confirmed the
/// fresh text is visible — Windows has no typing fallback to gate.
#[cfg(target_os = "windows")]
fn inject(_text: &str, _clipboard_verified: bool) -> Result<&'static str> {
    windows::inject_paste_shortcut()?;
    Ok("Win32 SendInput (Ctrl+V)")
}

/// Platform injection backend. Returns the backend name on success.
#[cfg(target_os = "linux")]
fn inject(text: &str, clipboard_verified: bool) -> Result<&'static str> {
    linux::inject(text, clipboard_verified)
}

/// Platforms without an injection backend: clipboard-only fallback.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn inject(_text: &str, _clipboard_verified: bool) -> Result<&'static str> {
    anyhow::bail!("paste injection is not supported on this platform")
}

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{anyhow, Result};
    use std::process::Command;
    use std::sync::OnceLock;
    use tracing::debug;

    /// One injection approach, tried in [`strategies`] order.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Strategy {
        /// One synthesized Ctrl+V reading the just-verified clipboard.
        PasteShortcut,
        /// Char-by-char typing via an external tool.
        AutoType,
    }

    /// Injection strategies in preference order (canario-cy0).
    ///
    /// The paste shortcut — a single Ctrl+V round trip reading the
    /// clipboard `paste_with` just populated — is dramatically cheaper
    /// than one keystroke per character, so it comes first; but only
    /// when the clipboard read-back verified the fresh text, otherwise
    /// the keystroke would paste stale content (canario-fhm). Auto-typing
    /// is the always-available fallback: slower, yet immune to the
    /// clipboard race and to apps that swallow synthetic pastes.
    fn strategies(clipboard_verified: bool) -> &'static [Strategy] {
        if clipboard_verified {
            &[Strategy::PasteShortcut, Strategy::AutoType]
        } else {
            &[Strategy::AutoType]
        }
    }

    /// Try to inject text into the focused app: verified-clipboard
    /// Ctrl+V first, char-by-char typing as fallback. Returns the name
    /// of the tool that succeeded.
    pub(super) fn inject(text: &str, clipboard_verified: bool) -> Result<&'static str> {
        for strategy in strategies(clipboard_verified) {
            let tool = match strategy {
                Strategy::PasteShortcut => try_simulate_paste(),
                Strategy::AutoType => try_auto_type(text),
            };
            if let Some(tool) = tool {
                return Ok(match (strategy, tool) {
                    (Strategy::PasteShortcut, "xdotool") => "simulated Ctrl+V (xdotool)",
                    (Strategy::PasteShortcut, "ydotool") => "simulated Ctrl+V (ydotool)",
                    (Strategy::PasteShortcut, _) => "simulated Ctrl+V",
                    (Strategy::AutoType, "xdotool") => "auto-type (xdotool)",
                    (Strategy::AutoType, "wtype") => "auto-type (wtype)",
                    (Strategy::AutoType, "ydotool") => "auto-type (ydotool)",
                    (Strategy::AutoType, _) => "auto-type",
                });
            }
        }
        Err(anyhow!("no paste tool succeeded"))
    }

    /// Whether the session is Wayland (xdotool can only reach XWayland
    /// here, so it is skipped in favor of wtype/ydotool).
    fn is_wayland() -> bool {
        std::env::var_os("WAYLAND_DISPLAY").is_some()
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

    /// Run a paste tool, reporting whether it exited successfully.
    fn run_tool(name: &str, args: &[&str]) -> bool {
        match Command::new(name)
            .args(args)
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(status) => status.success(),
            Err(_) => false,
        }
    }

    /// Try to auto-type text using external tools.
    /// Returns the name of the tool that succeeded.
    fn try_auto_type(text: &str) -> Option<&'static str> {
        if is_wayland() {
            // wtype (native Wayland typing via the virtual-keyboard
            // protocol)
            if command_exists("wtype") && run_tool("wtype", &["--", text]) {
                return Some("wtype");
            }
        } else {
            // xdotool (X11)
            if command_exists("xdotool")
                && run_tool("xdotool", &["type", "--clearmodifiers", "--", text])
            {
                return Some("xdotool");
            }
        }

        // ydotool (uinput — works on both X11 and Wayland)
        // --delay 0 skips ydotool's default 100ms pre-press sleep; a 2ms
        // gap between the four events keeps realistic chord spacing.
        if command_exists("ydotool") && run_tool("ydotool", &["type", "--delay", "0", "--", text]) {
            return Some("ydotool");
        }

        None
    }

    /// Try to simulate Ctrl+V paste.
    /// Returns the name of the tool that succeeded.
    fn try_simulate_paste() -> Option<&'static str> {
        if !is_wayland() {
            // xdotool (X11 only: under Wayland it talks to XWayland and
            // "succeeds" while the keystroke reaches nothing)
            if command_exists("xdotool")
                && run_tool("xdotool", &["key", "--clearmodifiers", "ctrl+v"])
            {
                return Some("xdotool");
            }
        }

        // ydotool (universal; wtype has no key command)
        // Ctrl+V: key 29 (left ctrl) down, key 47 (v) down, key 47 up, key 29 up
        // --delay 0 skips ydotool's default 100ms pre-press sleep (the
        // dominant cost of the whole paste at defaults); --key-delay 2 keeps
        // a small gap between the four events.
        if command_exists("ydotool")
            && run_tool(
                "ydotool",
                &[
                    "key",
                    "--delay",
                    "0",
                    "--key-delay",
                    "2",
                    "29:1",
                    "47:1",
                    "47:0",
                    "29:0",
                ],
            )
        {
            return Some("ydotool");
        }

        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn verified_clipboard_plans_paste_shortcut_before_typing() {
            assert_eq!(
                strategies(true),
                &[Strategy::PasteShortcut, Strategy::AutoType]
            );
        }

        #[test]
        fn unverified_clipboard_skips_the_paste_shortcut() {
            // A Ctrl+V into an unverified clipboard would paste stale
            // content (canario-fhm) — typing is the only safe strategy.
            assert_eq!(strategies(false), &[Strategy::AutoType]);
        }
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
        verify_result: bool,
    }

    impl Log {
        fn new() -> Self {
            Self {
                events: RefCell::new(Vec::new()),
                verify_result: true,
            }
        }
        fn clipboard(&self, text: &str) -> Result<()> {
            self.events.borrow_mut().push(format!("clipboard:{text}"));
            Ok(())
        }
        fn verify(&self, text: &str) -> bool {
            self.events.borrow_mut().push(format!("verify:{text}"));
            self.verify_result
        }
        fn inject_ok(&self, _text: &str, verified: bool) -> Result<&'static str> {
            self.events
                .borrow_mut()
                .push(format!("inject:ok:verified={verified}"));
            Ok("mock")
        }
        fn events(&self) -> Vec<String> {
            self.events.borrow().clone()
        }
    }

    #[test]
    fn empty_text_does_nothing() {
        let log = Log::new();
        let result = paste_with(
            "",
            |t| log.clipboard(t),
            |t| log.verify(t),
            |t, v| log.inject_ok(t, v),
        );
        assert!(!result.unwrap());
        assert!(log.events().is_empty(), "no backend calls for empty text");
    }

    #[test]
    fn clipboard_is_populated_and_verified_before_injection() {
        let log = Log::new();
        let result = paste_with(
            "hello",
            |t| log.clipboard(t),
            |t| log.verify(t),
            |t, v| log.inject_ok(t, v),
        );
        assert!(result.unwrap());
        assert_eq!(
            log.events(),
            vec!["clipboard:hello", "verify:hello", "inject:ok:verified=true"]
        );
    }

    #[test]
    fn read_back_failure_downgrades_to_unverified_injection() {
        // The stale-clipboard guard: a read-back mismatch must reach the
        // injector as `false` so it can skip the paste keystroke, but it
        // must not block injection outright — typing still runs.
        let mut log = Log::new();
        log.verify_result = false;
        let result = paste_with(
            "hello",
            |t| log.clipboard(t),
            |t| log.verify(t),
            |t, v| log.inject_ok(t, v),
        );
        assert!(result.unwrap());
        assert_eq!(
            log.events(),
            vec![
                "clipboard:hello",
                "verify:hello",
                "inject:ok:verified=false"
            ]
        );
    }

    #[test]
    fn clipboard_failure_skips_verification_and_injects_unverified() {
        let log = Log::new();
        let result = paste_with(
            "hello",
            |_| Err(anyhow!("no clipboard")),
            |t| log.verify(t),
            |t, v| log.inject_ok(t, v),
        );
        assert!(result.unwrap());
        // No clipboard write -> nothing to verify; injection still runs.
        assert_eq!(log.events(), vec!["inject:ok:verified=false"]);
    }

    #[test]
    fn injection_success_reports_true() {
        let result = paste_with("hi", |_| Ok(()), |_| true, |_, _| Ok("mock-backend"));
        assert!(result.unwrap());
    }

    #[test]
    fn injection_failure_falls_back_to_clipboard_only() {
        let log = Log::new();
        let result = paste_with(
            "hello",
            |t| log.clipboard(t),
            |t| log.verify(t),
            |_, _| Err(anyhow!("injection unavailable")),
        );
        // Genuine failure must NOT be reported as a successful paste...
        assert!(!result.unwrap());
        // ...but the clipboard fallback contract still holds.
        assert_eq!(log.events(), vec!["clipboard:hello", "verify:hello"]);
    }

    #[test]
    fn both_backends_failing_reports_false_not_error() {
        let result = paste_with(
            "hello",
            |_| Err(anyhow!("no clipboard")),
            |_| false,
            |_, _| Err(anyhow!("no injector")),
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
            |_| true,
            |t, _| {
                *seen.borrow_mut() = t.to_string();
                Ok("mock")
            },
        );
        assert!(result.unwrap());
        assert_eq!(seen.into_inner(), "transcribed text");
    }
}
