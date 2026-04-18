/// Wayland global hotkey support.
///
/// Wayland doesn't allow global key grabbing like X11. We use multiple strategies:
///
/// 1. **evdev** — Read raw keyboard events from `/dev/input/event*`. Requires the user
///    to be in the `input` group. Full press-and-hold and double-tap support.
///
/// 2. **Socket-based activation** — If no raw input is available, the user can set a system
///    keyboard shortcut that sends a command to our Unix socket.
///
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tracing::{debug, error, info, warn};

use super::processor::{HotkeyAction, HotkeyProcessor, ProcessorConfig};

/// Callback type: fired when the processor emits an action.
type OnAction = Arc<dyn Fn(HotkeyAction) + Send + Sync>;

/// Wayland hotkey listener. Tries multiple strategies.
pub struct WaylandHotkey {
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WaylandHotkey {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    /// Start listening for hotkey events.
    ///
    /// On Wayland, we try in order:
    /// 1. evdev (if the user has permissions)
    /// 2. Socket-based activation (D-Bus or Unix socket)
    ///
    /// For evdev, `key_name` should be an evdev key code name (e.g., "KEY_LEFTMETA" for Super).
    pub fn start(
        &mut self,
        key_name: &str,
        modifiers: &[String],
        processor_config: ProcessorConfig,
        on_action: OnAction,
    ) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            bail!("Hotkey listener already running");
        }

        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);

        let key_name = key_name.to_string();
        let modifiers = modifiers.to_vec();

        let on_action: Arc<dyn Fn(HotkeyAction) + Send + Sync> = Arc::from(on_action);

        let handle = std::thread::Builder::new()
            .name("wayland-hotkey".into())
            .spawn(move || {
                // Always start socket listener for external triggers (--toggle-external)
                let socket_running = running.clone();
                let socket_on_action = on_action.clone();
                std::thread::Builder::new()
                    .name("canario-hotkey-socket".into())
                    .spawn(move || {
                        if let Err(e) = socket_loop(&socket_running, &socket_on_action) {
                            debug!("Socket listener error: {}", e);
                        }
                    })
                    .ok();

                // Try evdev for real key listening
                if try_evdev(&running, &key_name, &modifiers, &processor_config, &on_action) {
                    return;
                }

                // evdev not available — socket is already running as fallback
                info!("evdev not available. Socket listener is running for external triggers.");
                // Keep this thread alive until stopped
                while running.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .context("Failed to spawn Wayland hotkey thread")?;

        self.thread = Some(handle);
        Ok(())
    }

    /// Stop the hotkey listener.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        // Send a dummy signal to the socket to unblock the read
        let socket_path = socket_path();
        if socket_path.exists() {
            if let Ok(sock) = UnixDatagram::unbound() {
                let _ = sock.send_to(b"x", &socket_path);
            }
        }
    }

}

impl Drop for WaylandHotkey {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Try to use evdev for raw keyboard input.
/// Returns true if successfully started and completed.
fn try_evdev(
    running: &Arc<AtomicBool>,
    key_name: &str,
    modifiers: &[String],
    processor_config: &ProcessorConfig,
    on_action: &Arc<dyn Fn(HotkeyAction) + Send + Sync>,
) -> bool {
    use evdev::KeyCode;

    // Map key name to evdev KeyCode
    let target_key = match map_key_name(key_name) {
        Some(k) => k,
        None => {
            warn!("Could not map key name '{}' to evdev key", key_name);
            return false;
        }
    };

    // Map required modifiers to evdev KeyCodes for tracking
    let required_mods: Vec<KeyCode> = modifiers.iter().filter_map(|m| map_key_name(m)).collect();
    let esc_key = KeyCode::KEY_ESC;

    // Find ALL keyboard devices that support our key, filtering out virtual devices
    let mut devices: Vec<evdev::Device> = evdev::enumerate()
        .filter_map(|(_, device)| {
            let name = device.name().unwrap_or("").to_lowercase();
            // Skip virtual / software devices that won't see real key presses
            if name.contains("virtual")
                || name.contains("ydotool")
                || name.contains("uinput")
                || name.contains("synthetic")
            {
                return None;
            }
            if let Some(keys) = device.supported_keys() {
                if keys.contains(target_key) {
                    return Some(device);
                }
            }
            None
        })
        .collect();

    if devices.is_empty() {
        // Fallback: try without filtering (maybe all keyboards are "virtual")
        devices = evdev::enumerate()
            .filter_map(|(_, device)| {
                if let Some(keys) = device.supported_keys() {
                    if keys.contains(target_key) {
                        return Some(device);
                    }
                }
                None
            })
            .collect();
    }

    if devices.is_empty() {
        warn!(
            "No keyboard devices found with key '{}'. \
             You may need to add yourself to the 'input' group: \
             sudo usermod -aG input $USER",
            key_name
        );
        return false;
    }

    // Set all devices to non-blocking
    for device in &mut devices {
        if let Err(e) = device.set_nonblocking(true) {
            warn!("Failed to set device to non-blocking: {}", e);
        }
    }

    let device_names: Vec<&str> = devices.iter().map(|d| d.name().unwrap_or("?")).collect();
    info!(
        "Monitoring {} evdev device(s): {:?} (key={:?}, mods={:?})",
        devices.len(),
        device_names,
        key_name,
        modifiers,
    );

    let mut processor = HotkeyProcessor::new(processor_config.clone());
    let mut key_down = false;
    let mut held_mods: std::collections::HashSet<KeyCode> = std::collections::HashSet::new();

    loop {
        if !running.load(Ordering::SeqCst) {
            break;
        }

        // Poll ALL devices
        for device in &mut devices {
            match device.fetch_events() {
                Ok(events) => {
                    for event in events {
                        if event.event_type() == evdev::EventType::KEY {
                            let code = KeyCode(event.code());
                            let value = event.value();

                            // Track modifier state
                            if required_mods.contains(&code) {
                                match value {
                                    1 => {
                                        held_mods.insert(code);
                                    }
                                    0 => {
                                        held_mods.remove(&code);
                                    }
                                    _ => {}
                                }
                            }

                            if code == target_key {
                                // Check if all required modifiers are held
                                let mods_satisfied =
                                    required_mods.iter().all(|m| held_mods.contains(m));
                                let is_our_hotkey = required_mods.is_empty() || mods_satisfied;

                                match value {
                                    1 if is_our_hotkey => {
                                        debug!("evdev: hotkey press (mods: {:?})", held_mods);
                                        key_down = true;
                                        if let Some(action) = processor.on_key_press() {
                                            on_action(action);
                                        }
                                        if let Some(action) = processor.on_tick() {
                                            on_action(action);
                                        }
                                    }
                                    0 if key_down => {
                                        debug!("evdev: hotkey release");
                                        key_down = false;
                                        if let Some(action) = processor.on_key_release() {
                                            on_action(action);
                                        }
                                    }
                                    _ => {}
                                }
                            } else if code == esc_key && value == 1 {
                                if let Some(action) = processor.on_escape() {
                                    on_action(action);
                                }
                            } else if value == 1 && key_down {
                                if let Some(action) = processor.on_other_key() {
                                    on_action(action);
                                }
                            }
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    debug!("evdev device error: {}", e);
                }
            }
        }

        // Tick for hold detection
        if let Some(action) = processor.on_tick() {
            on_action(action);
        }

        // Small sleep to avoid busy-waiting
        std::thread::sleep(Duration::from_millis(20));
    }

    true
}

/// Map a human-readable key name to an evdev KeyCode.
fn map_key_name(name: &str) -> Option<evdev::KeyCode> {
    use evdev::KeyCode;
    match name {
        "Super" | "Super_L" | "KEY_LEFTMETA" => Some(KeyCode::KEY_LEFTMETA),
        "Super_R" | "KEY_RIGHTMETA" => Some(KeyCode::KEY_RIGHTMETA),
        "Alt" | "Alt_L" | "KEY_LEFTALT" => Some(KeyCode::KEY_LEFTALT),
        "Alt_R" | "KEY_RIGHTALT" => Some(KeyCode::KEY_RIGHTALT),
        "Control" | "Ctrl" | "Control_L" | "KEY_LEFTCTRL" => Some(KeyCode::KEY_LEFTCTRL),
        "Control_R" | "KEY_RIGHTCTRL" => Some(KeyCode::KEY_RIGHTCTRL),
        "Shift" | "Shift_L" | "KEY_LEFTSHIFT" => Some(KeyCode::KEY_LEFTSHIFT),
        "Shift_R" | "KEY_RIGHTSHIFT" => Some(KeyCode::KEY_RIGHTSHIFT),
        "space" | "Space" | "KEY_SPACE" => Some(KeyCode::KEY_SPACE),
        _ => None,
    }
}

/// Socket-based activation loop.
///
/// Listens on a Unix datagram socket for commands:
/// - "toggle" → toggle recording
/// - "stop" → stop recording
/// - "cancel" → cancel recording
///
/// This allows external tools (system keyboard shortcuts, scripts)
/// to trigger Canario recording.
fn socket_loop(
    running: &Arc<AtomicBool>,
    on_action: &Arc<dyn Fn(HotkeyAction) + Send + Sync>,
) -> Result<()> {
    let socket_path = socket_path();

    // Clean up stale socket
    let _ = std::fs::remove_file(&socket_path);

    let sock = UnixDatagram::bind(&socket_path)
        .context("Failed to bind hotkey socket")?;

    sock.set_nonblocking(true)?;

    info!("Hotkey socket listening at {:?}", socket_path);

    let mut buf = [0u8; 64];
    let mut recording = false;

    while running.load(Ordering::SeqCst) {
        match sock.recv_from(&mut buf) {
            Ok((len, _addr)) => {
                let cmd = std::str::from_utf8(&buf[..len]).unwrap_or("").trim();
                debug!("Socket command: {:?}", cmd);

                match cmd {
                    "toggle" => {
                        if recording {
                            recording = false;
                            on_action(HotkeyAction::StopRecording);
                        } else {
                            recording = true;
                            on_action(HotkeyAction::StartRecording);
                        }
                    }
                    "start" => {
                        recording = true;
                        on_action(HotkeyAction::StartRecording);
                    }
                    "stop" => {
                        recording = false;
                        on_action(HotkeyAction::StopRecording);
                    }
                    "cancel" => {
                        recording = false;
                        on_action(HotkeyAction::CancelRecording);
                    }
                    _ => {
                        debug!("Unknown socket command: {:?}", cmd);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => {
                error!("Socket recv error: {}", e);
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = std::fs::remove_file(&socket_path);
    info!("Socket listener stopped");
    Ok(())
}

fn socket_path() -> PathBuf {
    std::env::temp_dir().join("canario-hotkey.sock")
}
