/// HotKeyProcessor — press-and-hold / double-tap detection logic.
///
/// Ported from Hex's HotKeyProcessor behavior:
///
/// **Press-and-hold**: Key down → start recording. Key up → stop recording, transcribe, paste.
///   - If held less than `minimum_key_time`, the action is cancelled (accidental tap).
///
/// **Double-tap to lock**: Two quick taps within `double_tap_timeout` → toggle recording on
///   (stays on until next explicit tap or toggle). This is useful for longer dictation.
///
/// **Modifier-only handling**: If the hotkey is a modifier (Super, Alt, Ctrl), we use a
///   0.3s threshold to distinguish "hotkey press" from "normal modifier use" — if any other
///   key is pressed while the modifier is held, we cancel the hotkey.
///
/// **Cancel**: Escape key or mouse click cancels an active recording.
use std::time::{Duration, Instant};

/// Duration within which two key presses count as a double-tap.
const DOUBLE_TAP_TIMEOUT: Duration = Duration::from_millis(300);

/// Time to wait after a modifier press before deciding it's a hotkey activation.
/// If any other key is pressed during this window, the hotkey is cancelled.
const MODIFIER_THRESHOLD: Duration = Duration::from_millis(300);

/// Actions the processor can emit.
#[derive(Debug, Clone, PartialEq)]
pub enum HotkeyAction {
    /// Start recording (press-and-hold detected, or double-tap lock engaged)
    StartRecording,
    /// Stop recording and transcribe (key released after hold, or toggle off)
    StopRecording,
    /// Cancel recording immediately (Escape pressed)
    CancelRecording,
}

/// Internal state machine.
#[derive(Debug, Clone, PartialEq)]
enum ProcessorState {
    /// Idle — no key activity
    Idle,
    /// Key is physically held down
    KeyDown {
        pressed_at: Instant,
    },
    /// Key was released; waiting to see if it's a double-tap
    WaitingForSecondTap {
        released_at: Instant,
    },
    /// Recording is locked ON (double-tap) — stays on until explicit stop
    LockedRecording,
    /// Recording via press-and-hold (key still held)
    HoldRecording,
}

/// Configuration for the processor.
#[derive(Debug, Clone)]
pub struct ProcessorConfig {
    /// Minimum time the key must be held to trigger recording (seconds).
    /// Presses shorter than this are ignored (accidental taps).
    pub minimum_key_time: Duration,
    /// Enable double-tap to lock recording on.
    pub double_tap_lock: bool,
    /// Is the hotkey a modifier key? (affects cancellation logic)
    pub is_modifier: bool,
}

impl Default for ProcessorConfig {
    fn default() -> Self {
        Self {
            minimum_key_time: Duration::from_millis(200),
            double_tap_lock: true,
            is_modifier: true,
        }
    }
}

/// The hotkey state processor. Feed it raw key events; it emits `HotkeyAction`s.
pub struct HotkeyProcessor {
    state: ProcessorState,
    config: ProcessorConfig,
    /// Whether we're actively recording (used to know if a cancel is relevant)
    recording: bool,
}

impl HotkeyProcessor {
    pub fn new(config: ProcessorConfig) -> Self {
        Self {
            state: ProcessorState::Idle,
            config,
            recording: false,
        }
    }

    /// Call when the hotkey is pressed (key down).
    /// Returns any action that should be taken.
    pub fn on_key_press(&mut self) -> Option<HotkeyAction> {
        match self.state {
            ProcessorState::Idle => {
                self.state = ProcessorState::KeyDown {
                    pressed_at: Instant::now(),
                };
                None
            }
            ProcessorState::WaitingForSecondTap { released_at } => {
                if self.config.double_tap_lock
                    && released_at.elapsed() < DOUBLE_TAP_TIMEOUT
                {
                    // Double-tap detected — lock recording ON
                    self.state = ProcessorState::LockedRecording;
                    self.recording = true;
                    Some(HotkeyAction::StartRecording)
                } else {
                    // Too slow — treat as a new single press
                    self.state = ProcessorState::KeyDown {
                        pressed_at: Instant::now(),
                    };
                    None
                }
            }
            ProcessorState::LockedRecording => {
                // Tap while locked → toggle off
                self.state = ProcessorState::Idle;
                self.recording = false;
                Some(HotkeyAction::StopRecording)
            }
            ProcessorState::KeyDown { .. } | ProcessorState::HoldRecording { .. } => {
                // Already in a key-down state, ignore repeat
                None
            }
        }
    }

    /// Call when the hotkey is released (key up).
    /// Returns any action that should be taken.
    pub fn on_key_release(&mut self) -> Option<HotkeyAction> {
        match self.state {
            ProcessorState::KeyDown { pressed_at } => {
                let held = pressed_at.elapsed();

                if held < self.config.minimum_key_time {
                    // Too short — if double-tap is enabled, wait for second tap
                    if self.config.double_tap_lock {
                        self.state = ProcessorState::WaitingForSecondTap {
                            released_at: Instant::now(),
                        };
                    } else {
                        self.state = ProcessorState::Idle;
                    }
                    return None;
                }

                // Held long enough — this is a press-and-hold recording
                if self.config.is_modifier {
                    // For modifier keys: the recording was started after MODIFIER_THRESHOLD,
                    // now released → stop
                    self.state = ProcessorState::Idle;
                    if self.recording {
                        self.recording = false;
                        Some(HotkeyAction::StopRecording)
                    } else {
                        None
                    }
                } else {
                    // Non-modifier: press-and-hold → record while held
                    self.state = ProcessorState::Idle;
                    if self.recording {
                        self.recording = false;
                        Some(HotkeyAction::StopRecording)
                    } else {
                        None
                    }
                }
            }
            ProcessorState::HoldRecording { .. } => {
                // Key released after hold recording → stop
                self.state = ProcessorState::Idle;
                self.recording = false;
                Some(HotkeyAction::StopRecording)
            }
            ProcessorState::LockedRecording | ProcessorState::WaitingForSecondTap { .. } => {
                // Key release in these states is irrelevant
                None
            }
            ProcessorState::Idle => None,
        }
    }

    /// Called periodically while the key is held to check if the minimum
    /// hold time has elapsed. For modifier keys, waits MODIFIER_THRESHOLD
    /// before starting recording.
    ///
    /// Returns `StartRecording` when the hold threshold is first exceeded.
    pub fn on_tick(&mut self) -> Option<HotkeyAction> {
        match &self.state {
            ProcessorState::KeyDown { pressed_at } => {
                let held = pressed_at.elapsed();

                let threshold = if self.config.is_modifier {
                    MODIFIER_THRESHOLD
                } else {
                    self.config.minimum_key_time
                };

                if held >= threshold {
                    self.state = ProcessorState::HoldRecording;
                    self.recording = true;
                    Some(HotkeyAction::StartRecording)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Call when any other (non-hotkey) key is pressed.
    /// For modifier-only hotkeys, this cancels the hotkey to avoid interfering
    /// with normal modifier use (e.g., Super+C for copy).
    pub fn on_other_key(&mut self) -> Option<HotkeyAction> {
        if self.config.is_modifier {
            match self.state {
                ProcessorState::KeyDown { .. } => {
                    // Cancel — this was a normal modifier use
                    self.state = ProcessorState::Idle;
                    None
                }
                ProcessorState::HoldRecording { .. } => {
                    // Already recording — other keys typed during recording are fine
                    // (user might be typing while dictating, but that's OK)
                    None
                }
                _ => None,
            }
        } else {
            None
        }
    }

    /// Call when Escape is pressed — cancels any active recording.
    pub fn on_escape(&mut self) -> Option<HotkeyAction> {
        match self.state {
            ProcessorState::HoldRecording { .. }
            | ProcessorState::LockedRecording => {
                self.state = ProcessorState::Idle;
                self.recording = false;
                Some(HotkeyAction::CancelRecording)
            }
            ProcessorState::KeyDown { .. } => {
                self.state = ProcessorState::Idle;
                None
            }
            _ => None,
        }
    }

    /// Reset the processor to idle state.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.state = ProcessorState::Idle;
        self.recording = false;
    }

    /// Is the processor currently in a recording state?
    pub fn is_recording(&self) -> bool {
        self.recording
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_press_and_hold() {
        let mut proc = HotkeyProcessor::new(ProcessorConfig {
            minimum_key_time: Duration::from_millis(50),
            double_tap_lock: false,
            is_modifier: false,
        });

        // Press key
        assert_eq!(proc.on_key_press(), None);
        // Hold for enough time — tick should trigger start
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(proc.on_tick(), Some(HotkeyAction::StartRecording));
        assert!(proc.is_recording());
        // Release — should stop
        assert_eq!(proc.on_key_release(), Some(HotkeyAction::StopRecording));
        assert!(!proc.is_recording());
    }

    #[test]
    fn test_short_tap_ignored() {
        let mut proc = HotkeyProcessor::new(ProcessorConfig {
            minimum_key_time: Duration::from_millis(200),
            double_tap_lock: false,
            is_modifier: false,
        });

        assert_eq!(proc.on_key_press(), None);
        // Release immediately
        assert_eq!(proc.on_key_release(), None);
        assert!(!proc.is_recording());
    }

    #[test]
    fn test_double_tap_lock() {
        let mut proc = HotkeyProcessor::new(ProcessorConfig {
            minimum_key_time: Duration::from_millis(50),
            double_tap_lock: true,
            is_modifier: false,
        });

        // First tap
        assert_eq!(proc.on_key_press(), None);
        assert_eq!(proc.on_key_release(), None); // too short, enters WaitingForSecondTap

        // Second tap quickly
        assert_eq!(proc.on_key_press(), Some(HotkeyAction::StartRecording));
        assert!(proc.is_recording());

        // Tap again to stop
        assert_eq!(proc.on_key_press(), Some(HotkeyAction::StopRecording));
        assert!(!proc.is_recording());
    }

    #[test]
    fn test_escape_cancels() {
        let mut proc = HotkeyProcessor::new(ProcessorConfig {
            minimum_key_time: Duration::from_millis(10),
            double_tap_lock: false,
            is_modifier: false,
        });

        assert_eq!(proc.on_key_press(), None);
        std::thread::sleep(Duration::from_millis(15));
        assert_eq!(proc.on_tick(), Some(HotkeyAction::StartRecording));

        // Press Escape
        assert_eq!(proc.on_escape(), Some(HotkeyAction::CancelRecording));
        assert!(!proc.is_recording());
    }

    #[test]
    fn test_modifier_other_key_cancels() {
        let mut proc = HotkeyProcessor::new(ProcessorConfig {
            minimum_key_time: Duration::from_millis(50),
            double_tap_lock: false,
            is_modifier: true,
        });

        assert_eq!(proc.on_key_press(), None);
        // Press another key while modifier is held
        assert_eq!(proc.on_other_key(), None);
        assert!(!proc.is_recording());
        // Now tick should not start recording
        assert_eq!(proc.on_tick(), None);
    }
}
