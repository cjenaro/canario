//! Windows global hotkey backend.
//!
//! ## Design
//!
//! A dedicated pump thread owns a hidden message-only window
//! (`RegisterClassExW` + `CreateWindowExW` with `HWND_MESSAGE`) and runs a
//! blocking `GetMessageW` loop. Three message sources feed the shared
//! [`HotkeyProcessor`]:
//!
//! - **`WM_HOTKEY`** (`RegisterHotKey` with `MOD_NOREPEAT`) →
//!   `on_key_press`. Session-global and works regardless of which app is
//!   focused. `MOD_NOREPEAT` suppresses auto-repeat so a hold produces
//!   exactly one press, matching the Linux backends' repeat guarding.
//! - **`WM_INPUT`** (raw input, keyboard usage page, `RIDEV_INPUTSINK`) →
//!   key release (`on_key_release`), Escape (`on_escape`) and any other
//!   key (`on_other_key`, the modifier-cancellation path).
//!   `RIDEV_INPUTSINK` keeps the events flowing while our window is
//!   unfocused, which is always.
//! - **`WM_TIMER`** (~20 ms) → `on_tick` for the 200 ms minimum-hold /
//!   300 ms double-tap thresholds, plus the shutdown flag check and the
//!   release safety net below.
//!
//! ## Key-up detection: why raw input instead of pure polling
//!
//! `RegisterHotKey` has no release event, so key-up must come from
//! elsewhere. The two options:
//!
//! 1. **Raw input via `WM_INPUT`** (chosen, primary path). Event-driven,
//!   so key-up timestamps are exact — that matters for the 200 ms minimum
//!    hold and the 300 ms double-tap window. It also delivers *every*
//!   key, which is required for Escape-cancel and modifier "other key"
//!   cancellation parity with the evdev/X11 backends. Raw input is
//!   generated below the hotkey-matching layer, so presses intercepted
//!   by `RegisterHotKey` still arrive as `WM_INPUT`.
//! 2. **`GetAsyncKeyState` polling**. Simple and immune to UIPI (see
//!   limitations), but quantized to the poll interval and state-only:
//!   it cannot distinguish Escape presses or "other key" activity without
//!   diffing snapshots of the whole VK range.
//!
//! We use raw input as the event source and keep a cheap
//! `GetAsyncKeyState` check on each timer tick as a safety net: if a
//! release is ever missed (see UIPI below), the key is observed up within
//! one timer interval and the processor still sees `on_key_release`.
//!
//! ## Modifier-only hotkeys
//!
//! `RegisterHotKey` cannot express "the Win key alone" (its modifiers are
//! extra keys that must be held alongside the VK). When the hotkey itself
//! is a modifier (`ProcessorConfig::is_modifier`, e.g. `["Super"]`), press
//! detection comes from raw input makes of the modifier's VK instead —
//! the same single-source model the evdev backend uses.
//!
//! ## Known limitations (unverifiable from Linux — needs on-Windows QA)
//!
//! - **UIPI**: raw input to `RIDEV_INPUTSINK` windows stops while an
//!   *elevated* app is foreground. `RegisterHotKey` presses still fire;
//!   releases fall back to the `GetAsyncKeyState` net during that window.
//! - **Exclusivity**: a hotkey combo is owned by exactly one process per
//!   session; if another app registered it first, `start()` fails and the
//!   failure is surfaced via `HotkeyStatus.detail`.
//! - A bare Win-key hotkey still pops the Start menu on release (OS
//!   behavior; the OS swallows it only when another key was pressed in
//!   between).
//! - `stop()` latency is up to one timer interval (~20 ms): the pump
//!   thread blocks in `GetMessageW` and the timer is what wakes it to
//!   observe the shutdown flag (mirrors the poll cadence of the Linux
//!   backends).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use tracing::{debug, error, info};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, MOD_WIN, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL,
    VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SPACE,
};
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT, RIM_TYPEKEYBOARD,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    RegisterClassExW, SetTimer, TranslateMessage, HWND_MESSAGE, MSG, RI_KEY_BREAK, WM_HOTKEY,
    WM_INPUT, WM_TIMER, WNDCLASSEXW,
};

use super::processor::{HotkeyProcessor, ProcessorConfig};
use super::OnAction;

/// `RegisterHotKey` id — we register at most one hotkey.
const HOTKEY_ID: i32 = 1;
/// `SetTimer` id for the processor tick.
const TIMER_ID: usize = 1;
/// Pump tick interval. Must sit comfortably under the processor's 200 ms
/// minimum-hold and 300 ms double-tap thresholds; 20 ms matches the X11
/// backend's poll cadence.
const TIMER_INTERVAL_MS: u32 = 20;
/// Message-only window class name (process-lifetime registration).
const CLASS_NAME: &str = "CanarioHotkeyMessageWindow";
/// Raw keyboard reports are ~40 bytes; 64 leaves headroom and avoids a
/// heap allocation per `WM_INPUT`.
const RAW_BUF_SIZE: usize = 64;

/// Whether the window class has been registered in this process. Classes
/// live until process exit, so a stop/start cycle must not re-register
/// (that would fail with `ERROR_CLASS_ALREADY_EXISTS`).
static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Pump-thread state, borrowed by the window procedure. The wndproc
    /// only ever runs on the pump thread (we pump the queue there), so a
    /// thread-local avoids `SetWindowLongPtrW(GWLP_USERDATA)` lifetime
    /// gymnastics.
    static PUMP: RefCell<Option<PumpState>> = const { RefCell::new(None) };
}

/// Windows hotkey listener. Owns the pump thread.
pub struct WindowsHotkey {
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WindowsHotkey {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    /// Start listening for the hotkey.
    ///
    /// `key`/`modifiers` use the shared config names (e.g. key "space",
    /// modifiers ["Super", "Alt"] → Win+Alt+Space). Window creation,
    /// `RegisterHotKey` and raw-input registration happen on the pump
    /// thread but are reported back synchronously, so when `start()`
    /// returns the backend is either fully armed or the error is
    /// returned (same contract as the evdev backend's synchronous probe).
    pub fn start(
        &mut self,
        key: &str,
        modifiers: &[String],
        processor_config: ProcessorConfig,
        on_action: OnAction,
    ) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            bail!("Hotkey listener already running");
        }

        let target_vk = key_name_to_vk(key)?;
        let hotkey_mods = modifiers_to_win(modifiers)?;
        // RegisterHotKey can't express a bare modifier — raw input owns
        // press detection in that mode (see module docs).
        let register_hotkey = !processor_config.is_modifier;

        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);

        // Setup completion is reported back so start() can fail loudly.
        let (setup_tx, setup_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let handle = std::thread::Builder::new()
            .name("windows-hotkey".into())
            .spawn(move || {
                pump_thread(
                    running,
                    target_vk,
                    hotkey_mods,
                    register_hotkey,
                    processor_config,
                    on_action,
                    setup_tx,
                );
            })
            .context("Failed to spawn Windows hotkey thread")?;

        match setup_rx.recv() {
            Ok(Ok(())) => {
                self.thread = Some(handle);
                Ok(())
            }
            Ok(Err(e)) => {
                self.running.store(false, Ordering::SeqCst);
                let _ = handle.join();
                Err(anyhow!(e))
            }
            Err(_) => {
                self.running.store(false, Ordering::SeqCst);
                let _ = handle.join();
                Err(anyhow!("Windows hotkey thread died during setup"))
            }
        }
    }

    /// Stop listening. The pump thread observes the flag on its next
    /// timer wake (≤ `TIMER_INTERVAL_MS`) and tears down the window,
    /// hotkey registration and raw-input subscription.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Default for WindowsHotkey {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WindowsHotkey {
    fn drop(&mut self) {
        self.stop();
    }
}

/// State owned by the pump thread and driven from the window procedure.
struct PumpState {
    processor: HotkeyProcessor,
    on_action: OnAction,
    target_vk: u16,
    /// True when `RegisterHotKey` owns press detection (non-modifier
    /// hotkey): raw-input makes of the target key are then ignored, since
    /// they also fire for presses *without* the modifier combo.
    hotkey_registered: bool,
    /// Our view of the target key's physical state. Drives the
    /// `GetAsyncKeyState` safety net and dedupes releases.
    key_down: bool,
}

impl PumpState {
    fn emit(&mut self, action: Option<super::HotkeyAction>) {
        if let Some(action) = action {
            (self.on_action)(action);
        }
    }

    /// `WM_HOTKEY`: the full combo was pressed. With `MOD_NOREPEAT` this
    /// fires once per physical press.
    fn on_wm_hotkey(&mut self) {
        debug!("WM_HOTKEY press");
        self.key_down = true;
        let action = self.processor.on_key_press();
        self.emit(action);
    }

    /// `WM_TIMER`: hold-threshold tick + release safety net.
    fn on_timer(&mut self) {
        let action = self.processor.on_tick();
        self.emit(action);
        // Safety net: if raw input missed the release (UIPI while an
        // elevated app is foreground, or any other delivery gap), poll
        // the physical state. High bit set = key down.
        if self.key_down && unsafe { GetAsyncKeyState(self.target_vk as i32) } >= 0 {
            debug!("Release detected via GetAsyncKeyState fallback");
            self.key_down = false;
            let action = self.processor.on_key_release();
            self.emit(action);
        }
    }

    /// Raw input key event. `key_up` is the `RI_KEY_BREAK` flag.
    fn on_raw_key(&mut self, vk: u16, key_up: bool) {
        if vk == self.target_vk {
            if key_up {
                // Redundant releases are no-ops in the processor's state
                // machine, so no need to gate on key_down for correctness;
                // the flag just keeps the log quiet.
                if self.key_down {
                    self.key_down = false;
                    debug!("Raw input: hotkey release");
                    let action = self.processor.on_key_release();
                    self.emit(action);
                }
            } else if !self.hotkey_registered {
                // Modifier-only hotkey: raw input is the press source.
                // Auto-repeat makes are repeat-guarded by the processor.
                if !self.key_down {
                    debug!("Raw input: modifier hotkey press");
                    self.key_down = true;
                }
                let action = self.processor.on_key_press();
                self.emit(action);
            }
            // Non-modifier hotkey makes are handled via WM_HOTKEY — a raw
            // make alone doesn't prove the modifier combo was held.
        } else if key_up {
            // Releases of other keys are irrelevant.
        } else if vk == VK_ESCAPE {
            let action = self.processor.on_escape();
            self.emit(action);
        } else {
            // Modifier-cancellation path (bare-modifier hotkeys); a no-op
            // for non-modifier configs.
            let action = self.processor.on_other_key();
            self.emit(action);
        }
    }
}

/// Pump thread entry: set up the window/hotkey/raw-input, report back,
/// then run the message loop until the shutdown flag is observed.
#[allow(clippy::too_many_arguments)]
fn pump_thread(
    running: Arc<AtomicBool>,
    target_vk: u16,
    hotkey_mods: u32,
    register_hotkey: bool,
    processor_config: ProcessorConfig,
    on_action: OnAction,
    setup_tx: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let hwnd = match unsafe { setup(target_vk, hotkey_mods, register_hotkey) } {
        Ok(hwnd) => hwnd,
        Err(e) => {
            let _ = setup_tx.send(Err(e.to_string()));
            return;
        }
    };
    let _ = setup_tx.send(Ok(()));
    info!(
        "Windows hotkey active: vk=0x{:X} mods=0x{:X} register_hotkey={}",
        target_vk, hotkey_mods, register_hotkey
    );

    PUMP.with(|p| {
        *p.borrow_mut() = Some(PumpState {
            processor: HotkeyProcessor::new(processor_config),
            on_action,
            target_vk,
            hotkey_registered: register_hotkey,
            key_down: false,
        });
    });

    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let ret = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if ret == 0 {
                break; // WM_QUIT (not posted by us, but exit cleanly)
            }
            if ret == -1 {
                error!("GetMessageW failed: {}", std::io::Error::last_os_error());
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            // Checked after every dispatch; the 20 ms timer guarantees
            // the loop wakes at least that often while idle.
            if !running.load(Ordering::SeqCst) {
                break;
            }
        }

        teardown(hwnd, register_hotkey);
    }

    PUMP.with(|p| p.borrow_mut().take());
    info!("Windows hotkey listener stopped");
}

/// Create the message-only window, register the hotkey (if requested),
/// subscribe to raw keyboard input and start the tick timer.
///
/// # Safety
///
/// Calls Win32 windowing APIs. All handles created here are released by
/// [`teardown`] on the same thread.
unsafe fn setup(target_vk: u16, hotkey_mods: u32, register_hotkey: bool) -> Result<HWND> {
    let class_name: Vec<u16> = CLASS_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let window_name: Vec<u16> = "Canario Hotkey"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    if !CLASS_REGISTERED.load(Ordering::SeqCst) {
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            // A null instance is accepted for private window classes and
            // avoids a GetModuleHandleW dependency.
            hInstance: std::ptr::null_mut(),
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        if RegisterClassExW(&class) == 0 {
            let err = std::io::Error::last_os_error();
            // ERROR_CLASS_ALREADY_EXISTS (1410): another thread in this
            // process beat us to it — fine, the class is identical.
            if err.raw_os_error() != Some(1410) {
                return Err(anyhow!("RegisterClassExW failed: {err}"));
            }
        }
        CLASS_REGISTERED.store(true, Ordering::SeqCst);
    }

    // Message-only window: never visible, receives only targeted and
    // broadcast messages — exactly the WM_HOTKEY/WM_INPUT/WM_TIMER diet
    // this backend runs on.
    let hwnd = CreateWindowExW(
        0,
        class_name.as_ptr(),
        window_name.as_ptr(),
        0,
        0,
        0,
        0,
        0,
        HWND_MESSAGE,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null(),
    );
    if hwnd.is_null() {
        return Err(anyhow!(
            "CreateWindowExW failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    if register_hotkey {
        // MOD_NOREPEAT: one WM_HOTKEY per physical press, so hold-to-record
        // doesn't refire presses (auto-repeat is also repeat-guarded by the
        // processor, this just keeps the queue clean).
        if RegisterHotKey(
            hwnd,
            HOTKEY_ID,
            hotkey_mods | MOD_NOREPEAT,
            target_vk as u32,
        ) == 0
        {
            let err = std::io::Error::last_os_error();
            DestroyWindow(hwnd);
            return Err(anyhow!(
                "RegisterHotKey failed (combo already owned by another app?): {err}"
            ));
        }
    }

    // Keyboard raw input, delivered even while unfocused (INPUTSINK).
    let rid = RAWINPUTDEVICE {
        usUsagePage: 0x01, // HID_USAGE_PAGE_GENERIC
        usUsage: 0x06,     // HID_USAGE_GENERIC_KEYBOARD
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    if RegisterRawInputDevices(&rid, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32) == 0 {
        let err = std::io::Error::last_os_error();
        if register_hotkey {
            UnregisterHotKey(hwnd, HOTKEY_ID);
        }
        DestroyWindow(hwnd);
        return Err(anyhow!("RegisterRawInputDevices failed: {err}"));
    }

    if SetTimer(hwnd, TIMER_ID, TIMER_INTERVAL_MS, None) == 0 {
        let err = std::io::Error::last_os_error();
        teardown(hwnd, register_hotkey);
        return Err(anyhow!("SetTimer failed: {err}"));
    }

    Ok(hwnd)
}

/// Release everything [`setup`] created, in reverse order.
///
/// # Safety
///
/// `hwnd` must be a live window created by [`setup`] on this thread.
unsafe fn teardown(hwnd: HWND, hotkey_registered: bool) {
    KillTimer(hwnd, TIMER_ID);
    if hotkey_registered {
        UnregisterHotKey(hwnd, HOTKEY_ID);
    }
    let rid = RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x06,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: std::ptr::null_mut(),
    };
    RegisterRawInputDevices(&rid, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
    DestroyWindow(hwnd);
}

/// Window procedure for the hidden message window. Runs exclusively on
/// the pump thread; dispatches into the thread-local [`PumpState`].
///
/// # Safety
///
/// Standard wndproc contract; called by the OS during `DispatchMessageW`.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            if wparam as i32 == HOTKEY_ID {
                PUMP.with(|p| {
                    if let Some(pump) = &mut *p.borrow_mut() {
                        pump.on_wm_hotkey();
                    }
                });
            }
            0
        }
        WM_INPUT => {
            if let Some((vk, key_up)) = read_raw_keyboard(lparam) {
                PUMP.with(|p| {
                    if let Some(pump) = &mut *p.borrow_mut() {
                        pump.on_raw_key(vk, key_up);
                    }
                });
            }
            0
        }
        WM_TIMER => {
            PUMP.with(|p| {
                if let Some(pump) = &mut *p.borrow_mut() {
                    pump.on_timer();
                }
            });
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Extract `(virtual-key, is-release)` from a `WM_INPUT` message.
/// Returns `None` for non-keyboard input and fake keys (VKey 255, sent
/// for e.g. IME or elevated-desktop placeholders).
fn read_raw_keyboard(lparam: LPARAM) -> Option<(u16, bool)> {
    #[repr(align(8))]
    struct AlignedBuf([u8; RAW_BUF_SIZE]);

    unsafe {
        let hraw = lparam as HRAWINPUT;
        let header_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;

        let mut size: u32 = 0;
        // Query size; (UINT)-1 means error.
        if GetRawInputData(
            hraw,
            RID_INPUT,
            std::ptr::null_mut(),
            &mut size,
            header_size,
        ) == u32::MAX
        {
            return None;
        }
        if size as usize > RAW_BUF_SIZE {
            // Never happens for keyboard reports.
            return None;
        }

        let mut buf = AlignedBuf([0u8; RAW_BUF_SIZE]);
        if GetRawInputData(
            hraw,
            RID_INPUT,
            buf.0.as_mut_ptr() as *mut core::ffi::c_void,
            &mut size,
            header_size,
        ) == u32::MAX
        {
            return None;
        }

        let raw = &*(buf.0.as_ptr() as *const RAWINPUT);
        if raw.header.dwType != RIM_TYPEKEYBOARD {
            return None;
        }
        let kb = raw.data.keyboard;
        if kb.VKey == 0xFF {
            return None;
        }
        Some((kb.VKey, kb.Flags as u32 & RI_KEY_BREAK != 0))
    }
}

/// Map a shared-config key name to a Windows virtual-key code.
fn key_name_to_vk(name: &str) -> Result<u16> {
    let vk = match name {
        "Super" | "Super_L" => VK_LWIN,
        "Super_R" => VK_RWIN,
        "Alt" | "Alt_L" => VK_LMENU,
        "Alt_R" => VK_RMENU,
        "Control" | "Ctrl" | "Control_L" => VK_LCONTROL,
        "Control_R" => VK_RCONTROL,
        "Shift" | "Shift_L" => VK_LSHIFT,
        "Shift_R" => VK_RSHIFT,
        "space" | "Space" => VK_SPACE,
        other => {
            // VK codes for A-Z and 0-9 are their ASCII values.
            if other.len() == 1 {
                let c = other.chars().next().unwrap();
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase() as u16
                } else {
                    bail!("Unsupported key for Windows hotkey: {}", other);
                }
            } else {
                bail!("Unsupported key for Windows hotkey: {}", other);
            }
        }
    };
    Ok(vk)
}

/// Map shared-config modifier names to `RegisterHotKey` `MOD_*` flags.
fn modifiers_to_win(modifiers: &[String]) -> Result<u32> {
    let mut mask = 0u32;
    for m in modifiers {
        mask |= match m.as_str() {
            "Super" | "Super_L" | "Super_R" => MOD_WIN,
            "Alt" | "Alt_L" | "Alt_R" => MOD_ALT,
            "Control" | "Ctrl" | "Control_L" | "Control_R" => MOD_CONTROL,
            "Shift" | "Shift_L" | "Shift_R" => MOD_SHIFT,
            other => bail!("Unsupported modifier for Windows hotkey: {}", other),
        };
    }
    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hotkey_maps_to_win_alt_space() {
        let config = crate::HotkeyConfig::default();
        assert_eq!(key_name_to_vk(&config.key).unwrap(), VK_SPACE);
        assert_eq!(
            modifiers_to_win(&config.modifiers).unwrap(),
            MOD_WIN | MOD_ALT
        );
    }

    #[test]
    fn modifier_names_map_both_sides() {
        assert_eq!(key_name_to_vk("Super").unwrap(), VK_LWIN);
        assert_eq!(key_name_to_vk("Super_R").unwrap(), VK_RWIN);
        assert_eq!(key_name_to_vk("Alt").unwrap(), VK_LMENU);
        assert_eq!(key_name_to_vk("Control_R").unwrap(), VK_RCONTROL);
        assert_eq!(key_name_to_vk("Shift_L").unwrap(), VK_LSHIFT);
    }

    #[test]
    fn single_ascii_chars_map_to_their_vk() {
        assert_eq!(key_name_to_vk("a").unwrap(), 'A' as u16);
        assert_eq!(key_name_to_vk("7").unwrap(), '7' as u16);
        assert!(key_name_to_vk("ä").is_err());
        assert!(key_name_to_vk("F13").is_err());
    }

    #[test]
    fn unknown_modifiers_are_rejected() {
        assert!(modifiers_to_win(&["Hyper".to_string()]).is_err());
    }
}
