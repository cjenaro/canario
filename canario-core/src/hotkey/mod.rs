/// Global hotkey handling for Linux.
///
/// Automatically detects the display server (X11 vs Wayland) and uses
/// the appropriate backend:
///
/// - **X11**: `XGrabKey` via x11rb — full press-and-hold and double-tap support
/// - **Wayland**: `evdev` raw keyboard input (requires `input` group), with socket-based fallback
///
/// Default hotkey: **Super+Alt+Space** (avoids conflicts with most desktop environments
/// which already bind Super+Space to the app launcher).
///
/// Usage:
/// ```no_run
/// use canario_core::{HotkeyConfig, HotkeyListener};
///
/// let config = HotkeyConfig::default();
/// let mut listener = HotkeyListener::new();
/// listener.start(config, |action| {
///     println!("Hotkey action: {:?}", action);
/// }).unwrap();
/// ```
mod processor;
#[cfg(all(target_os = "linux", feature = "linux-input"))]
mod wayland;
#[cfg(all(target_os = "linux", feature = "x11"))]
mod x11;

pub use processor::{HotkeyAction, ProcessorConfig};

use anyhow::{bail, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(target_os = "linux")]
use tracing::info;

/// Callback type: fired when the processor emits an action.
pub(crate) type OnAction = Arc<dyn Fn(HotkeyAction) + Send + Sync>;

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
            modifiers: vec!["Super".into(), "Alt".into()],
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
        double_tap_only: bool,
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
                double_tap_only,
                is_modifier,
            },
        }
    }
}

/// Path of the Unix datagram socket used for external hotkey control
/// (e.g. `canario-cli --toggle-external`).
///
/// Prefers `$XDG_RUNTIME_DIR` (per-user tmpfs, not world-writable like
/// `/tmp`, so no symlink/squatting risk) and falls back to the system
/// temp dir when it is unset. Both the listener and any client MUST use
/// this function so the two sides can't drift apart.
pub fn hotkey_socket_path() -> std::path::PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("canario-hotkey.sock")
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
#[cfg(target_os = "linux")]
pub(crate) fn detect_display_server() -> DisplayServer {
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

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DisplayServer {
    X11,
    Wayland,
    Unknown,
}

/// Global hotkey listener.
///
pub struct HotkeyListener {
    running: Arc<AtomicBool>,
    #[cfg(all(target_os = "linux", feature = "x11"))]
    x11: x11::X11Hotkey,
    #[cfg(all(target_os = "linux", feature = "linux-input"))]
    wayland: wayland::WaylandHotkey,
}

impl Default for HotkeyListener {
    fn default() -> Self {
        Self::new()
    }
}

impl HotkeyListener {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            #[cfg(all(target_os = "linux", feature = "x11"))]
            x11: x11::X11Hotkey::new(),
            #[cfg(all(target_os = "linux", feature = "linux-input"))]
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

        let on_action: OnAction = Arc::new(on_action);

        #[cfg(target_os = "linux")]
        {
            let display_server = detect_display_server();
            info!("Detected display server: {:?}", display_server);

            match display_server {
                DisplayServer::X11 => {
                    // On X11 (or XWayland), use XGrabKey
                    #[cfg(feature = "x11")]
                    {
                        info!("Using X11 hotkey backend (XGrabKey)");
                        self.x11
                            .start(
                                &config.key,
                                &config.modifiers,
                                config.processor.clone(),
                                on_action.clone(),
                            )
                            .map_err(|e| {
                                tracing::error!("X11 hotkey backend failed to start: {}", e);
                                e
                            })?;
                    }
                    #[cfg(not(feature = "x11"))]
                    bail!("X11 hotkey support not compiled in (enable the `x11` feature)");
                }
                DisplayServer::Wayland | DisplayServer::Unknown => {
                    // On Wayland, try evdev with socket fallback.
                    // The key name format differs: evdev uses KEY_LEFTMETA etc.
                    #[cfg(feature = "linux-input")]
                    {
                        info!("Using evdev hotkey backend (Wayland)");
                        let evdev_key = to_evdev_key_name(&config.key);
                        self.wayland
                            .start(
                                &evdev_key,
                                &config.modifiers,
                                config.processor.clone(),
                                on_action.clone(),
                            )
                            .map_err(|e| {
                                tracing::error!("evdev hotkey backend failed to start: {}", e);
                                e
                            })?;
                    }
                    #[cfg(not(feature = "linux-input"))]
                    bail!(
                        "Wayland hotkey support not compiled in (enable the `linux-input` feature)"
                    );
                }
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            // TODO: macOS/Windows hotkey backends (separate epic)
            let _ = &on_action;
            let _ = &config;
            bail!("Global hotkey is not yet supported on this platform");
        }

        Ok(())
    }

    /// Stop listening.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        #[cfg(all(target_os = "linux", feature = "x11"))]
        self.x11.stop();
        #[cfg(all(target_os = "linux", feature = "linux-input"))]
        self.wayland.stop();
    }
}

/// Convert a human key name to evdev-style name.
#[cfg(all(target_os = "linux", feature = "linux-input"))]
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
