/// Global hotkey handling for Linux
///
/// On X11: Uses XGrabKey via x11 crate
/// On Wayland: Uses evdev to listen to keyboard events (requires root or udev rules)
///
/// For now, this module provides a hotkey listener that can detect key press/release.
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq)]
pub enum HotkeyEvent {
    Pressed,
    Released,
}

#[derive(Debug, Clone)]
pub struct HotkeyConfig {
    /// Key codes to watch (e.g., Key::Space)
    pub keys: Vec<String>,
    /// Modifier keys required (e.g., "Super", "Ctrl", "Alt")
    pub modifiers: Vec<String>,
    /// Minimum hold time before triggering (seconds)
    pub minimum_key_time: f64,
    /// Enable double-tap lock
    pub double_tap_lock: bool,
}

/// Simple hotkey listener using evdev
pub struct HotkeyListener {
    running: Arc<AtomicBool>,
}

impl HotkeyListener {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start listening for hotkey events
    /// Calls `on_event` with HotkeyEvent::Pressed / Released
    pub fn start<F>(&self, _config: HotkeyConfig, _on_event: F) -> Result<()>
    where
        F: Fn(HotkeyEvent) + Send + 'static,
    {
        self.running.store(true, Ordering::SeqCst);

        // For Wayland, we need a different approach
        // For X11, we can use XGrabKey
        // For a first version, let's use a simple approach:
        // Listen to /dev/input/event* via evdev

        info!("Hotkey listener starting...");
        info!("Note: For global hotkeys on Linux, you may need to run with appropriate permissions.");
        info!("On Wayland, global hotkeys are restricted. Consider using a D-Bus based approach.");

        // TODO: Implement actual hotkey listening
        // This will require:
        // 1. X11: XGrabKey on the root window
        // 2. Wayland: Protocol extension or portal-based hotkey
        // 3. Fallback: evdev device monitoring

        warn!("Hotkey listener not yet implemented. Use Ctrl+R in the GUI to test recording.");

        Ok(())
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}
