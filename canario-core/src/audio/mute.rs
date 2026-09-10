//! Mute system audio output while recording (`AudioBehavior::Mute`).
//!
//! Uses `pactl` (PulseAudio/PipeWire) to mute the default sink when a
//! recording starts and restores the prior state when it stops or is
//! cancelled. If `pactl` is not installed this is a graceful no-op
//! (a warning is logged once).

use std::process::Command;
use std::sync::OnceLock;
use tracing::{debug, warn};

/// Restores the pre-recording mute state when `restore()` is called.
///
/// Created by [`mute_for_recording`]; `Canario` holds it for the
/// duration of a recording.
pub struct MuteGuard {
    /// Sink mute state before we muted it — what to restore to.
    was_muted: bool,
}

/// Mute the default sink for the duration of a recording.
///
/// Returns a [`MuteGuard`] that restores the previous state, or `None`
/// if `pactl` is unavailable (already logged).
pub fn mute_for_recording() -> Option<MuteGuard> {
    if !pactl_available() {
        warn!(
            "AudioBehavior::Mute is set but `pactl` was not found — leaving system audio unmuted"
        );
        return None;
    }

    let was_muted = match get_sink_mute() {
        Some(m) => m,
        None => {
            warn!("Could not read sink mute state — leaving system audio unmuted");
            return None;
        }
    };

    if was_muted {
        debug!("Sink already muted before recording; will leave it muted on stop");
    } else {
        set_sink_mute(true);
    }

    Some(MuteGuard { was_muted })
}

impl MuteGuard {
    /// The mute state to restore when recording ends.
    ///
    /// Pure decision logic, split out so it can be tested without
    /// invoking `pactl`.
    fn restore_value(&self) -> bool {
        self.was_muted
    }

    /// Restore the sink to its pre-recording mute state.
    pub fn restore(self) {
        let target = self.restore_value();
        // If the sink was already muted before recording we never
        // touched it, so there is nothing to restore.
        if !self.was_muted {
            set_sink_mute(target);
        }
    }

    /// Construct a guard with a known prior state (tests only — avoids pactl).
    #[cfg(test)]
    fn for_test(was_muted: bool) -> Self {
        Self { was_muted }
    }
}

/// Parse the output of `pactl get-sink-mute @DEFAULT_SINK@`
/// ("Mute: yes" / "Mute: no"). Returns `None` on unexpected output.
fn parse_sink_muted(output: &str) -> Option<bool> {
    match output.trim().to_ascii_lowercase().as_str() {
        "mute: yes" => Some(true),
        "mute: no" => Some(false),
        _ => None,
    }
}

/// Probe for `pactl` on PATH (cached — not expected to appear mid-session).
fn pactl_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let found = Command::new("which")
            .arg("pactl")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        debug!("pactl detection: {}", found);
        found
    })
}

/// Current mute state of the default sink, if it could be read.
fn get_sink_mute() -> Option<bool> {
    let output = Command::new("pactl")
        .args(["get-sink-mute", "@DEFAULT_SINK@"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_sink_muted(&text)
}

/// Set the default sink's mute state (best-effort).
fn set_sink_mute(muted: bool) {
    let arg = if muted { "1" } else { "0" };
    match Command::new("pactl")
        .args(["set-sink-mute", "@DEFAULT_SINK@", arg])
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => {
            debug!("Sink mute set to {}", muted);
        }
        Ok(status) => warn!("pactl set-sink-mute exited with {}", status),
        Err(e) => warn!("Failed to run pactl set-sink-mute: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pactl_get_sink_mute_output() {
        assert_eq!(parse_sink_muted("Mute: yes\n"), Some(true));
        assert_eq!(parse_sink_muted("Mute: no\n"), Some(false));
        assert_eq!(parse_sink_muted("Mute: YES"), Some(true));
        assert_eq!(parse_sink_muted("garbage"), None);
        assert_eq!(parse_sink_muted(""), None);
    }

    /// The guard restores exactly the pre-recording state: unmute only
    /// if we were the ones who muted.
    #[test]
    fn guard_restores_prior_state() {
        assert!(!MuteGuard::for_test(false).restore_value());
        assert!(MuteGuard::for_test(true).restore_value());
    }
}
