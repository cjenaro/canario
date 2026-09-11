//! macOS global hotkey backend.
//!
//! ## Design
//!
//! A dedicated tap thread owns a CoreGraphics **event tap**
//! (`CGEventTapCreate` with `kCGSessionEventTap` +
//! `kCGHeadInsertEventTap` + `kCGEventTapOptionDefault`, watching
//! `keyDown`/`keyUp`/`flagsChanged`) serviced by that thread's
//! `CFRunLoop`, plus a ~20 ms repeating `CFRunLoopTimer`. Three event
//! sources feed the shared [`HotkeyProcessor`]:
//!
//! - **`keyDown`/`keyUp`** → `on_key_press` (target key down with the
//!   configured modifier flags held), `on_key_release` (target key up),
//!   `on_escape` (Escape down) and `on_other_key` (any other key down —
//!   the modifier-cancellation path). Auto-repeat key-downs are dropped
//!   so a hold produces exactly one press.
//! - **`flagsChanged`** → macOS delivers modifier keys as flag
//!   transitions, not key events. When the hotkey itself is a modifier
//!   (e.g. `["Super"]` → ⌘), these drive `on_key_press`/
//!   `on_key_release` for the target; other modifiers' transitions feed
//!   `on_other_key`. Non-modifier hotkeys read the event's flag state to
//!   check the required modifiers (⌘⌥Space-style combos — macOS has no
//!   `RegisterHotKey` equivalent, so the combo is matched manually).
//! - **timer** (~20 ms) → `on_tick` for the 200 ms minimum-hold /
//!   300 ms double-tap thresholds, plus the shutdown flag check.
//!
//! The tap is a *filter* (`kCGEventTapOptionDefault`) only because that
//! is the mode a tap needs to ever suppress events; the callback always
//! returns `Keep`, so user keystrokes reach the focused app unmodified
//! (nothing is consumed, unlike the Windows `RegisterHotKey` path).
//!
//! ## Accessibility permission
//!
//! `CGEventTapCreate` returns NULL unless the process is a trusted
//! Accessibility client. That is the expected first-run state, not an
//! error: `start()` reports it as [`super::HotkeyStatus`] guidance
//! (backend `"event-tap-denied"`, `permission_denied: true`, a fix
//! command that opens the Accessibility pane of System Settings) and
//! returns `Ok(Start::AccessDenied)` instead of failing — mirroring how
//! the evdev backend degrades to `socket-fallback` status when the
//! `input` group is missing. Electron frontends can raise the same
//! system prompt (the "grant access" / secured-event-check button) via
//! `systemPreferences.isTrustedAccessibilityClient(true)`.
//!
//! ## Known limitations (unverifiable from Linux — needs on-macOS QA)
//!
//! - The system disables the tap while a secure-input field (e.g. a
//!   password box) is focused; the callback re-arms it on the
//!   `tapDisabledByUserInput`/`tapDisabledByTimeout` notifications, so
//!   hotkeys resume afterwards, but events during secure input are
//!   lost.
//! - Events are never suppressed: the hotkey's keys still reach the
//!   focused app (⌘⌥Space and bare modifiers are inert by default, so
//!   the default config is unaffected).
//! - Both instances of the target modifier held at once (e.g. left+right
//!   ⌘) share one flag bit, so releasing one while holding the other is
//!   not seen as a release of the target.
//! - `stop()` latency is up to one timer interval (~20 ms): the tap
//!   thread blocks in `CFRunLoopRun` and the timer is what wakes it to
//!   observe the shutdown flag (mirrors the Windows backend's timer).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use core_foundation::base::TCFType;
use core_foundation::date::CFDate;
use core_foundation::runloop::{
    kCFRunLoopCommonModes, CFRunLoop, CFRunLoopTimer, CFRunLoopTimerContext, CFRunLoopTimerRef,
};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CGKeyCode, CallbackResult, EventField, KeyCode,
};
use tracing::{debug, info, warn};

use super::processor::{HotkeyProcessor, ProcessorConfig};
use super::{HotkeyAction, OnAction};

/// Runloop tick interval (seconds). Must sit comfortably under the
/// processor's 200 ms minimum-hold and 300 ms double-tap thresholds;
/// 20 ms matches the Windows/X11 backends' cadence.
const TIMER_INTERVAL_SECS: f64 = 0.02;

extern "C" {
    /// Re-arm an event tap the system disabled (timeout or secure
    /// input). core-graphics 0.25 declares this privately, so it is
    /// bound here with the same signature it uses there; the
    /// CoreGraphics framework is already linked by the crate itself.
    fn CGEventTapEnable(tap: core_graphics::sys::CGEventTapRef, enable: bool);
}

/// Why the backend could not arm itself, as reported by the tap thread.
enum SetupFailure {
    /// `CGEventTapCreate` returned NULL — the process lacks the
    /// Accessibility permission (the expected first-run state).
    AccessDenied,
    /// Any other setup failure.
    Other(&'static str),
}

/// Outcome of [`MacosHotkey::start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// The event tap is armed and the tap thread is servicing it.
    Active,
    /// The event tap could not be created (missing Accessibility
    /// permission). Nothing is listening — surface guidance via
    /// [`super::HotkeyStatus`] instead of failing `start()`.
    AccessDenied,
}

/// macOS hotkey listener. Owns the tap thread.
pub struct MacosHotkey {
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MacosHotkey {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    /// Start listening for the hotkey.
    ///
    /// `key`/`modifiers` use the shared config names (e.g. key "space",
    /// modifiers ["Super", "Alt"] → ⌘⌥Space). The event tap is created
    /// on the tap thread but the outcome is reported back synchronously,
    /// so when `start()` returns the backend is either fully armed, the
    /// Accessibility guidance is available, or the error is returned
    /// (same contract as the Windows backend's synchronous setup).
    pub fn start(
        &mut self,
        key: &str,
        modifiers: &[String],
        processor_config: ProcessorConfig,
        on_action: OnAction,
    ) -> Result<Start> {
        if self.running.load(Ordering::SeqCst) {
            bail!("Hotkey listener already running");
        }

        // Fail fast on unsupported names, before any thread is spawned.
        let target_keycode = key_name_to_keycode(key)?;
        let required_flags = modifiers_to_flags(modifiers)?;

        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);

        let (setup_tx, setup_rx) = std::sync::mpsc::channel::<Result<(), SetupFailure>>();

        let handle = std::thread::Builder::new()
            .name("macos-hotkey".into())
            .spawn(move || {
                tap_thread(
                    running,
                    target_keycode,
                    required_flags,
                    processor_config,
                    on_action,
                    setup_tx,
                )
            })
            .context("Failed to spawn macOS hotkey thread")?;

        match setup_rx.recv() {
            Ok(Ok(())) => {
                self.thread = Some(handle);
                Ok(Start::Active)
            }
            Ok(Err(SetupFailure::AccessDenied)) => {
                self.running.store(false, Ordering::SeqCst);
                let _ = handle.join();
                Ok(Start::AccessDenied)
            }
            Ok(Err(SetupFailure::Other(msg))) => {
                self.running.store(false, Ordering::SeqCst);
                let _ = handle.join();
                Err(anyhow!(msg))
            }
            Err(_) => {
                self.running.store(false, Ordering::SeqCst);
                let _ = handle.join();
                Err(anyhow!("macOS hotkey thread died during setup"))
            }
        }
    }

    /// Stop listening. The tap thread observes the flag on its next
    /// timer wake (≤ `TIMER_INTERVAL_SECS`), leaves `CFRunLoopRun` and
    /// tears down the timer, runloop source and event tap.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Default for MacosHotkey {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MacosHotkey {
    fn drop(&mut self) {
        self.stop();
    }
}

/// State shared by the tap callback and the timer callback (both run on
/// the tap thread's runloop; the mutex only reconciles the borrow
/// checker with C callbacks).
struct TapState {
    processor: HotkeyProcessor,
    on_action: OnAction,
    /// Configured hotkey key.
    target_keycode: CGKeyCode,
    /// Modifier flags that must accompany a target press (empty for
    /// single-key hotkeys — `contains` on the empty set is always
    /// true, so the gate is a no-op there).
    required_flags: CGEventFlags,
    /// The flag bit the target modifier key transitions, when the
    /// target is a modifier key (presses then arrive as `flagsChanged`
    /// instead of key events).
    target_flag: Option<CGEventFlags>,
    /// Our view of the target key's physical state; dedupes releases.
    key_down: bool,
}

impl TapState {
    fn emit(&mut self, action: Option<HotkeyAction>) {
        if let Some(action) = action {
            (self.on_action)(action);
        }
    }

    /// Non-modifier key event (`keyDown`/`keyUp`).
    fn on_key_event(
        &mut self,
        keycode: CGKeyCode,
        key_up: bool,
        autorepeat: bool,
        flags: CGEventFlags,
    ) {
        if keycode == self.target_keycode {
            if self.target_flag.is_some() {
                // Modifier target: presses/releases arrive as
                // flagsChanged, not key events. Synthetic key events
                // carrying a modifier keycode are ignored rather than
                // treated as "other key" activity (which would cancel a
                // legitimately held target).
            } else if key_up {
                // Redundant releases are no-ops in the processor's state
                // machine; the flag just keeps the log quiet.
                if self.key_down {
                    self.key_down = false;
                    debug!("Event tap: hotkey release");
                    let action = self.processor.on_key_release();
                    self.emit(action);
                }
            } else if !autorepeat && !self.key_down && flags.contains(self.required_flags) {
                // One press per physical hold: auto-repeat is dropped,
                // and the combo's modifiers must be held (the manual
                // equivalent of RegisterHotKey's implicit matching).
                self.key_down = true;
                debug!("Event tap: hotkey press");
                let action = self.processor.on_key_press();
                self.emit(action);
            }
        } else if key_up || autorepeat {
            // Releases and repeats of other keys are irrelevant.
        } else if keycode == KeyCode::ESCAPE {
            let action = self.processor.on_escape();
            self.emit(action);
        } else {
            // Modifier-cancellation path (bare-modifier hotkeys); a
            // no-op for non-modifier configs.
            let action = self.processor.on_other_key();
            self.emit(action);
        }
    }

    /// `flagsChanged`: a modifier key transitioned. `flags` is the new
    /// global modifier state the event carries.
    fn on_flags_changed(&mut self, keycode: CGKeyCode, flags: CGEventFlags) {
        match self.target_flag {
            Some(target_flag) if keycode == self.target_keycode => {
                if flags.contains(target_flag) {
                    // Pressed. Any other configured modifiers must be
                    // held too (a no-op check for single-modifier
                    // hotkeys, where `required_flags` is empty).
                    if !self.key_down && flags.contains(self.required_flags) {
                        self.key_down = true;
                        debug!("Event tap: modifier hotkey press");
                        let action = self.processor.on_key_press();
                        self.emit(action);
                    }
                } else if self.key_down {
                    self.key_down = false;
                    debug!("Event tap: modifier hotkey release");
                    let action = self.processor.on_key_release();
                    self.emit(action);
                }
            }
            _ => {
                // Another modifier transitioned — ordinary shortcut use
                // (⌘C etc.); feeds the cancellation path.
                let action = self.processor.on_other_key();
                self.emit(action);
            }
        }
    }

    /// Timer tick: hold-threshold check (200 ms minimum hold / 300 ms
    /// modifier threshold, enforced by the processor).
    fn tick(&mut self) {
        let action = self.processor.on_tick();
        self.emit(action);
    }
}

/// Stack context for the runloop timer callback. The timer's C context
/// `info` points here; the timer is removed and dropped before this
/// goes out of scope, so the raw pointer never dangles.
struct TimerContext {
    state: Arc<Mutex<TapState>>,
    running: Arc<AtomicBool>,
}

/// Runloop timer callback: hold-threshold tick + shutdown check.
extern "C" fn on_timer(_timer: CFRunLoopTimerRef, info: *mut c_void) {
    let ctx = unsafe { &*(info as *const TimerContext) };
    {
        let mut state = super::lock(&ctx.state);
        state.tick();
    }
    // stop() only flips the flag; the timer (≤ TIMER_INTERVAL_SECS) is
    // what wakes the runloop to observe it. Stopping the runloop from
    // within its own callback is the documented CFRunLoopStop pattern.
    if !ctx.running.load(Ordering::SeqCst) {
        CFRunLoop::get_current().stop();
    }
}

/// Dispatch one event tap callback into the shared state.
fn handle_tap_event(
    state: &Mutex<TapState>,
    port: &AtomicUsize,
    etype: CGEventType,
    event: &CGEvent,
) {
    match etype {
        // Out-of-band notifications: the system disabled our tap (slow
        // callback, or secure input engaged). Re-arm it; the runloop
        // source stays installed either way.
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            warn!(
                "Event tap disabled by the system ({:?}); re-enabling",
                etype
            );
            let raw = port.load(Ordering::SeqCst);
            if raw != 0 {
                // SAFETY: `raw` is the CFMachPortRef of our live tap,
                // stored immediately after creation; the tap outlives
                // every callback (it is dropped only after the runloop
                // stops, and `port` is zeroed first).
                unsafe { CGEventTapEnable(raw as core_graphics::sys::CGEventTapRef, true) };
            }
        }
        CGEventType::KeyDown | CGEventType::KeyUp => {
            // CGEventType doesn't implement PartialEq, hence the u32 cast.
            let key_up = etype as u32 == CGEventType::KeyUp as u32;
            let keycode =
                event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as CGKeyCode;
            let autorepeat =
                event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
            let mut state = super::lock(state);
            state.on_key_event(keycode, key_up, autorepeat, event.get_flags());
        }
        CGEventType::FlagsChanged => {
            let keycode =
                event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as CGKeyCode;
            let mut state = super::lock(state);
            state.on_flags_changed(keycode, event.get_flags());
        }
        _ => {}
    }
}

/// Tap thread entry: create the event tap, report back (creation
/// failure is almost always the missing Accessibility permission),
/// service the runloop until the shutdown flag is observed, then tear
/// down in reverse order.
fn tap_thread(
    running: Arc<AtomicBool>,
    target_keycode: CGKeyCode,
    required_flags: CGEventFlags,
    processor_config: ProcessorConfig,
    on_action: OnAction,
    setup_tx: std::sync::mpsc::Sender<Result<(), SetupFailure>>,
) {
    let target_flag = modifier_flag_for_keycode(target_keycode);
    let state = Arc::new(Mutex::new(TapState {
        processor: HotkeyProcessor::new(processor_config),
        on_action,
        target_keycode,
        required_flags,
        target_flag,
        key_down: false,
    }));

    // The mach port must be handed to the callback, but it only exists
    // after CGEventTap::new returns — and the callback closure is
    // created before that. Zeroed until then; also zeroed during
    // teardown so a late callback can't touch a dead port.
    let port = Arc::new(AtomicUsize::new(0));

    let tap_state = Arc::clone(&state);
    let tap_port = Arc::clone(&port);
    let tap = match CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
        ],
        move |_proxy, etype, event| {
            handle_tap_event(&tap_state, &tap_port, etype, event);
            // Observe only: user keystrokes must reach the focused app.
            CallbackResult::Keep
        },
    ) {
        Ok(tap) => tap,
        Err(()) => {
            // CGEventTapCreate returned NULL: the process is not a
            // trusted Accessibility client (or, rarely, another internal
            // failure). Reported as guided status, not a hard error —
            // see module docs.
            let _ = setup_tx.send(Err(SetupFailure::AccessDenied));
            return;
        }
    };
    port.store(
        tap.mach_port().as_concrete_TypeRef() as usize,
        Ordering::SeqCst,
    );

    let runloop = CFRunLoop::get_current();
    let source = match tap.mach_port().create_runloop_source(0) {
        Ok(source) => source,
        Err(()) => {
            let _ = setup_tx.send(Err(SetupFailure::Other(
                "CFMachPortCreateRunLoopSource failed",
            )));
            return;
        }
    };
    runloop.add_source(&source, unsafe { kCFRunLoopCommonModes });
    tap.enable();

    let timer_ctx = TimerContext {
        state: Arc::clone(&state),
        running: Arc::clone(&running),
    };
    let mut timer_context = CFRunLoopTimerContext {
        version: 0,
        info: &timer_ctx as *const TimerContext as *mut c_void,
        retain: None,
        release: None,
        copyDescription: None,
    };
    let timer = CFRunLoopTimer::new(
        CFDate::now().abs_time(),
        TIMER_INTERVAL_SECS,
        0,
        0,
        on_timer,
        &mut timer_context,
    );
    runloop.add_timer(&timer, unsafe { kCFRunLoopCommonModes });

    let _ = setup_tx.send(Ok(()));
    info!(
        "macOS hotkey active: keycode=0x{:X} required_flags={:?} modifier_target={}",
        target_keycode,
        required_flags,
        target_flag.is_some()
    );

    CFRunLoop::run_current();

    // Teardown, reverse order: timer first (its context points into
    // `timer_ctx`), then the runloop source, then the tap — dropping it
    // invalidates the mach port, which also removes the port from the
    // runloop. `state` and `timer_ctx` go last of all.
    runloop.remove_timer(&timer, unsafe { kCFRunLoopCommonModes });
    drop(timer);
    drop(source);
    port.store(0, Ordering::SeqCst);
    drop(tap);
    info!("macOS hotkey listener stopped");
}

/// Map a shared-config key name to a macOS virtual keycode.
fn key_name_to_keycode(name: &str) -> Result<CGKeyCode> {
    let keycode = match name {
        "Super" | "Super_L" => KeyCode::COMMAND,
        "Super_R" => KeyCode::RIGHT_COMMAND,
        "Alt" | "Alt_L" => KeyCode::OPTION,
        "Alt_R" => KeyCode::RIGHT_OPTION,
        "Control" | "Ctrl" | "Control_L" => KeyCode::CONTROL,
        "Control_R" => KeyCode::RIGHT_CONTROL,
        "Shift" | "Shift_L" => KeyCode::SHIFT,
        "Shift_R" => KeyCode::RIGHT_SHIFT,
        "space" | "Space" => KeyCode::SPACE,
        other => {
            if other.len() == 1 {
                let c = other.chars().next().unwrap();
                char_to_keycode(c)
                    .ok_or_else(|| anyhow!("Unsupported key for macOS hotkey: {}", other))?
            } else {
                bail!("Unsupported key for macOS hotkey: {}", other);
            }
        }
    };
    Ok(keycode)
}

/// Map a single ASCII character to its ANSI keycode (the letters and
/// digits the config accepts). ANSI keycodes are not contiguous, hence
/// the table.
fn char_to_keycode(c: char) -> Option<CGKeyCode> {
    let kc = match c.to_ascii_uppercase() {
        'A' => KeyCode::ANSI_A,
        'B' => KeyCode::ANSI_B,
        'C' => KeyCode::ANSI_C,
        'D' => KeyCode::ANSI_D,
        'E' => KeyCode::ANSI_E,
        'F' => KeyCode::ANSI_F,
        'G' => KeyCode::ANSI_G,
        'H' => KeyCode::ANSI_H,
        'Q' => KeyCode::ANSI_Q,
        'W' => KeyCode::ANSI_W,
        'R' => KeyCode::ANSI_R,
        'T' => KeyCode::ANSI_T,
        'Y' => KeyCode::ANSI_Y,
        'S' => KeyCode::ANSI_S,
        'Z' => KeyCode::ANSI_Z,
        'X' => KeyCode::ANSI_X,
        'V' => KeyCode::ANSI_V,
        'I' => KeyCode::ANSI_I,
        'P' => KeyCode::ANSI_P,
        'U' => KeyCode::ANSI_U,
        'O' => KeyCode::ANSI_O,
        'L' => KeyCode::ANSI_L,
        'K' => KeyCode::ANSI_K,
        'J' => KeyCode::ANSI_J,
        'M' => KeyCode::ANSI_M,
        'N' => KeyCode::ANSI_N,
        '1' => KeyCode::ANSI_1,
        '2' => KeyCode::ANSI_2,
        '3' => KeyCode::ANSI_3,
        '4' => KeyCode::ANSI_4,
        '5' => KeyCode::ANSI_5,
        '6' => KeyCode::ANSI_6,
        '7' => KeyCode::ANSI_7,
        '8' => KeyCode::ANSI_8,
        '9' => KeyCode::ANSI_9,
        '0' => KeyCode::ANSI_0,
        _ => return None,
    };
    Some(kc)
}

/// Map shared-config modifier names to the `CGEventFlags` bits that
/// must be held alongside the hotkey key.
fn modifiers_to_flags(modifiers: &[String]) -> Result<CGEventFlags> {
    let mut flags = CGEventFlags::empty();
    for m in modifiers {
        flags |= match m.as_str() {
            "Super" | "Super_L" | "Super_R" => CGEventFlags::CGEventFlagCommand,
            "Alt" | "Alt_L" | "Alt_R" => CGEventFlags::CGEventFlagAlternate,
            "Control" | "Ctrl" | "Control_L" | "Control_R" => CGEventFlags::CGEventFlagControl,
            "Shift" | "Shift_L" | "Shift_R" => CGEventFlags::CGEventFlagShift,
            other => bail!("Unsupported modifier for macOS hotkey: {}", other),
        };
    }
    Ok(flags)
}

/// The `CGEventFlags` bit a modifier keycode transitions; `None` for
/// non-modifier keys (whose presses arrive as key events instead).
fn modifier_flag_for_keycode(keycode: CGKeyCode) -> Option<CGEventFlags> {
    match keycode {
        KeyCode::COMMAND | KeyCode::RIGHT_COMMAND => Some(CGEventFlags::CGEventFlagCommand),
        KeyCode::OPTION | KeyCode::RIGHT_OPTION => Some(CGEventFlags::CGEventFlagAlternate),
        KeyCode::CONTROL | KeyCode::RIGHT_CONTROL => Some(CGEventFlags::CGEventFlagControl),
        KeyCode::SHIFT | KeyCode::RIGHT_SHIFT => Some(CGEventFlags::CGEventFlagShift),
        KeyCode::FUNCTION => Some(CGEventFlags::CGEventFlagSecondaryFn),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hotkey_maps_to_cmd_alt_space() {
        assert_eq!(key_name_to_keycode("space").unwrap(), KeyCode::SPACE);
        assert_eq!(
            modifiers_to_flags(&["Super".to_string(), "Alt".to_string()]).unwrap(),
            CGEventFlags::CGEventFlagCommand | CGEventFlags::CGEventFlagAlternate
        );
    }

    #[test]
    fn modifier_names_map_both_sides() {
        assert_eq!(key_name_to_keycode("Super").unwrap(), KeyCode::COMMAND);
        assert_eq!(
            key_name_to_keycode("Super_R").unwrap(),
            KeyCode::RIGHT_COMMAND
        );
        assert_eq!(key_name_to_keycode("Alt").unwrap(), KeyCode::OPTION);
        assert_eq!(
            key_name_to_keycode("Control_R").unwrap(),
            KeyCode::RIGHT_CONTROL
        );
        assert_eq!(key_name_to_keycode("Shift_L").unwrap(), KeyCode::SHIFT);
    }

    #[test]
    fn modifier_keycodes_have_flags() {
        assert_eq!(
            modifier_flag_for_keycode(KeyCode::COMMAND),
            Some(CGEventFlags::CGEventFlagCommand)
        );
        assert_eq!(
            modifier_flag_for_keycode(KeyCode::RIGHT_OPTION),
            Some(CGEventFlags::CGEventFlagAlternate)
        );
        assert_eq!(modifier_flag_for_keycode(KeyCode::SPACE), None);
    }

    #[test]
    fn single_ascii_chars_map_to_their_keycode() {
        assert_eq!(key_name_to_keycode("a").unwrap(), KeyCode::ANSI_A);
        assert_eq!(key_name_to_keycode("7").unwrap(), KeyCode::ANSI_7);
        assert!(key_name_to_keycode("ä").is_err());
        assert!(key_name_to_keycode("F13").is_err());
    }

    #[test]
    fn unknown_modifiers_are_rejected() {
        assert!(modifiers_to_flags(&["Hyper".to_string()]).is_err());
    }
}
