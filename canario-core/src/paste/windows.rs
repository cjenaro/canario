//! Windows paste injection via Win32 `SendInput`.
//!
//! Synthesizes a Ctrl+V key sequence so the focused application pastes
//! whatever is already on the clipboard (populated via `arboard` before
//! this runs).
//!
//! Failure contract: `SendInput` returns the number of events actually
//! inserted; anything less than the full sequence is reported as an
//! error so the caller can fall back to "clipboard only".

use anyhow::{anyhow, Result};
use std::mem::size_of;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL, VK_V,
};

/// The Ctrl+V sequence, in order: Ctrl down, V down, V up, Ctrl up.
///
/// Extracted as a pure function so the exact event sequence is
/// inspectable/testable without calling `SendInput`.
fn paste_shortcut_events() -> [(u16, bool); 4] {
    [
        (VK_CONTROL, false),
        (VK_V, false),
        (VK_V, true),
        (VK_CONTROL, true),
    ]
}

/// Build a keyboard `INPUT` record for a virtual-key press/release.
fn key_input(vk: u16, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Inject Ctrl+V into the focused application.
pub(super) fn inject_paste_shortcut() -> Result<()> {
    let events = paste_shortcut_events();
    let inputs: Vec<INPUT> = events
        .iter()
        .map(|&(vk, key_up)| key_input(vk, key_up))
        .collect();

    // SAFETY: `inputs` is a valid, properly aligned slice of `INPUT`
    // records that outlives the call; `cbSize` matches the struct.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };

    if sent as usize != inputs.len() {
        return Err(anyhow!(
            "SendInput injected {sent}/{} events (input blocked)",
            inputs.len()
        ));
    }
    Ok(())
}
