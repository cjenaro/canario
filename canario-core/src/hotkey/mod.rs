/// Global hotkey handling for Linux.
///
/// Automatically detects the display server (X11 vs Wayland) and uses
/// the appropriate backend:
///
/// - **X11**: `XGrabKey` via x11rb — full press-and-hold and double-tap support
/// - **Wayland**: `evdev` raw keyboard input, with socket-based fallback
///
/// Usage:
/// ```no_run
/// use canario::hotkey::{HotkeyListener, HotkeyConfig};
///
/// let config = HotkeyConfig::default();
/// let listener = HotkeyListener::new();
/// listener.start(config, |action| {
///     println!("Hotkey action: {:?}", action);
/// }).unwrap();
/// ```no_run
mod processor;
mod x11;
mod wayland;

pub use processor::{HotkeyAction, ProcessorConfig};

use anyhow::{bail, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::info;

/// Configuration for the global hotkey.
#[derive(Debug, Clone)]
pub struct HotkeyConfig {
    /// Key to watch (e.g., "Super_L", "space", "Alt_L")
    pub key: String,
    /// Modifier keys required (e.g., ["Super"])
    pub modifiers: Vec<String>,
    /// Processor configuration (hold time, double-tap, etc.)
    pub processor: ProcessorConfig,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            key: "space".into(),
            modifiers: vec!["Super".into()],
            processor: ProcessorConfig::default(),
        }
    }
}

impl HotkeyConfig {
    /// Create config from the app config's hotkey field.
    /// The hotkey vector format is like ["Super", "Space"] — last element is the key,
    /// everything else is a modifier.
    pub fn from_app_config(
        hotkey: &[String],
        minimum_key_time: f64,
        double_tap_lock: bool,
        _double_tap_only: bool,
    ) -> Self {
        if hotkey.is_empty() {
            return Self::default();
        }

        // Last element is the key, rest are modifiers
        let (modifiers, key) = if hotkey.len() == 1 {
            (vec![], hotkey[0].clone())
        } else {
            let mods = hotkey[..hotkey.len() - 1].to_vec();
            let key = hotkey[hotkey.len() - 1].clone();
            (mods, key)
        };

        // Detect if the key itself is a modifier (e.g., ["Super"])
        let is_modifier = is_modifier_key(&key);

        Self {
            key,
            modifiers,
            processor: ProcessorConfig {
                minimum_key_time: std::time::Duration::from_secs_f64(minimum_key_time),
                double_tap_lock,
                is_modifier,
            },
        }
    }
}

/// Check if a key name is a modifier key.
fn is_modifier_key(key: &str) -> bool {
    matches!(
        key,
        "Super"
            | "Super_L"
            | "Super_R"
            | "Alt"
            | "Alt_L"
            | "Alt_R"
            | "Control"
            | "Ctrl"
            | "Control_L"
            | "Control_R"
            | "Shift"
            | "Shift_L"
            | "Shift_R"
            | "Hyper"
            | "Meta"
    )
}

/// Detect the current display server.
fn detect_display_server() -> DisplayServer {
    // Check XDG_SESSION_TYPE first
    if let Ok(session_type) = std::env::var("XDG_SESSION_TYPE") {
        match session_type.as_str() {
            "x11" => return DisplayServer::X11,
            "wayland" => return DisplayServer::Wayland,
            _ => {}
        }
    }

    // Check for WAYLAND_DISPLAY
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        return DisplayServer::Wayland;
    }

    // Check for DISPLAY (X11)
    if std::env::var("DISPLAY").is_ok() {
        return DisplayServer::X11;
    }

    DisplayServer::Unknown
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DisplayServer {
    X11,
    Wayland,
    Unknown,
}

/// Global hotkey listener.
///
/// Wraps the platform-specific backend and the shared `HotKeyProcessor`.
pub struct HotkeyListener {
    running: Arc<AtomicBool>,
    x11: x11::X11Hotkey,
    wayland: wayland::WaylandHotkey,
}

impl HotkeyListener {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            x11: x11::X11Hotkey::new(),
            wayland: wayland::WaylandHotkey::new(),
        }
    }

    /// Start listening for the configured hotkey.
    pub fn start<F>(&mut self, config: HotkeyConfig, on_action: F) -> Result<()>
    where
        F: Fn(HotkeyAction) + Send + Sync + 'static,
    {
        if self.running.load(Ordering::SeqCst) {
            bail!("Hotkey listener already running");
        }

        self.running.store(true, Ordering::SeqCst);

        let display_server = detect_display_server();
        info!("Detected display server: {:?}", display_server);

        match display_server {
            DisplayServer::X11 => {
                // On X11 (or XWayland), use XGrabKey
                // Even on Wayland sessions, DISPLAY might be set for XWayland,
                // but XGrabKey only works for X11 apps. If XDG_SESSION_TYPE is
                // "wayland", prefer the Wayland backend.
                self.x11.start(
                    &config.key,
                    &config.modifiers,
                    config.processor,
                    Arc::new(on_action),
                )?;
            }
            DisplayServer::Wayland | DisplayServer::Unknown => {
                // On Wayland, try evdev with socket fallback
                // The key name format differs: evdev uses KEY_LEFTMETA etc.
                let evdev_key = to_evdev_key_name(&config.key);
                self.wayland.start(
                    &evdev_key,
                    &config.modifiers,
                    config.processor,
                    Arc::new(on_action),
                )?;
            }
        }

        Ok(())
    }

    /// Stop listening.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.x11.stop();
        self.wayland.stop();
    }
}

/// Convert a human key name to evdev-style name.
fn to_evdev_key_name(key: &str) -> String {
    match key {
        "Super" | "Super_L" => "KEY_LEFTMETA".into(),
        "Super_R" => "KEY_RIGHTMETA".into(),
        "Alt" | "Alt_L" => "KEY_LEFTALT".into(),
        "Alt_R" => "KEY_RIGHTALT".into(),
        "Control" | "Ctrl" | "Control_L" => "KEY_LEFTCTRL".into(),
        "Control_R" => "KEY_RIGHTCTRL".into(),
        "Shift" | "Shift_L" => "KEY_LEFTSHIFT".into(),
        "Shift_R" => "KEY_RIGHTSHIFT".into(),
        "space" | "Space" => "KEY_SPACE".into(),
        other => other.into(),
    }
}
