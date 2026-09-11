//! macOS paste injection via CoreGraphics `CGEvent`.
//!
//! Posts a ⌘V (Cmd+V) keyboard event pair to the HID event tap so the
//! focused application pastes whatever is already on the pasteboard
//! (populated via `arboard` before this runs).
//!
//! Notes / limitations:
//! - `CGEventPost` is fire-and-forget (returns `void`), so a silently
//!   dropped event (e.g. blocked by security policy) cannot be detected
//!   here. Event creation failures *are* reported.
//! - Posting synthesized key events at the HID tap does not require
//!   Accessibility permission (unlike installing an event *tap*), but
//!   some hardened apps ignore synthetic events.

use anyhow::{anyhow, Result};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, KeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::thread;
use std::time::Duration;

/// Pause between the key-down and key-up events so the target app has
/// time to observe the pressed state before release.
const KEY_HOLD: Duration = Duration::from_millis(20);

/// Inject ⌘V into the focused application.
///
/// The shortcut is Cmd+V: a V key-down and key-up event, both carrying
/// the Command modifier flag, posted at the HID event tap.
pub(super) fn inject_paste_shortcut() -> Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("CGEventSource creation failed (HIDSystemState)"))?;

    let key_down = CGEvent::new_keyboard_event(source.clone(), KeyCode::ANSI_V, true)
        .map_err(|_| anyhow!("failed to create Cmd+V key-down event"))?;
    let key_up = CGEvent::new_keyboard_event(source, KeyCode::ANSI_V, false)
        .map_err(|_| anyhow!("failed to create Cmd+V key-up event"))?;

    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);

    key_down.post(CGEventTapLocation::HID);
    thread::sleep(KEY_HOLD);
    key_up.post(CGEventTapLocation::HID);

    Ok(())
}
