/// Wayland global hotkey support.
///
use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tracing::{debug, error, info, warn};

use super::processor::{HotkeyAction, HotkeyProcessor, ProcessorConfig};
use super::{HotkeyStatus, OnAction};

/// Result of probing read access to `/dev/input`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvdevAccess {
    /// At least one `/dev/input/event*` node is readable.
    Granted,
    /// Event nodes exist but none are readable — the user is not in
    /// the `input` group (the classic silent-hotkey failure).
    PermissionDenied,
    /// No event nodes at all (e.g. container or exotic system).
    NoDevices,
}

impl EvdevAccess {
    /// Map the probe result onto the frontend-facing status.
    ///
    /// `Granted` is optimistic: the listener thread refines it to
    /// `socket-fallback` if no device actually supports the configured
    /// key (see [`WaylandHotkey::start`]).
    pub(crate) fn into_status(self) -> HotkeyStatus {
        match self {
            Self::Granted => HotkeyStatus::evdev(),
            Self::PermissionDenied => HotkeyStatus::socket_fallback(
                "No keyboard devices readable in /dev/input — the current user is \
                 not in the 'input' group, so the hotkey cannot listen for key \
                 presses until access is granted and the session is restarted."
                    .into(),
                true,
            ),
            Self::NoDevices => HotkeyStatus::socket_fallback(
                "No readable keyboard devices under /dev/input; only external \
                 triggers (e.g. canario-cli --toggle-external) work."
                    .into(),
                false,
            ),
        }
    }
}

/// Probe read access to the system's raw-input devices (`/dev/input`).
pub(crate) fn probe_evdev_access() -> EvdevAccess {
    probe_evdev_access_in(Path::new("/dev/input"))
}

/// Open each `event*` node in `dir` read-only, closing immediately.
///
/// Opening an evdev node allocates a private kernel event queue and
/// closing it without reading consumes nothing, so probing is
/// side-effect free even while another process listens. This mirrors
/// what `evdev::enumerate()` does internally — when access is denied
/// it simply yields zero devices and the hotkey silently dies, which
/// is exactly the failure this probe makes detectable.
fn probe_evdev_access_in(dir: &Path) -> EvdevAccess {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return EvdevAccess::NoDevices,
    };

    let mut nodes = 0usize;
    let mut denied = false;
    for entry in entries.flatten() {
        // Mice, joysticks, … — evdev only consumes event nodes.
        if !entry.file_name().to_string_lossy().starts_with("event") {
            continue;
        }
        nodes += 1;
        match std::fs::File::open(entry.path()) {
            Ok(_) => return EvdevAccess::Granted,
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => denied = true,
            Err(_) => {}
        }
    }

    if nodes == 0 {
        EvdevAccess::NoDevices
    } else if denied {
        EvdevAccess::PermissionDenied
    } else {
        // Nodes exist but fail for other reasons (e.g. ENODEV
        // placeholders) — not a permissions problem.
        EvdevAccess::NoDevices
    }
}

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
    ///
    /// `status` receives live backend health: it is pre-populated by
    /// the caller from the synchronous `/dev/input` probe, and this
    /// method refines it if the evdev loop cannot actually start.
    pub fn start(
        &mut self,
        key_name: &str,
        modifiers: &[String],
        processor_config: ProcessorConfig,
        on_action: OnAction,
        status: Arc<Mutex<HotkeyStatus>>,
    ) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            bail!("Hotkey listener already running");
        }

        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);

        let key_name = key_name.to_string();
        let modifiers = modifiers.to_vec();

        let on_action: Arc<dyn Fn(HotkeyAction) + Send + Sync> = on_action;

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
                if try_evdev(
                    &running,
                    &key_name,
                    &modifiers,
                    &processor_config,
                    &on_action,
                ) {
                    return;
                }

                // evdev not available — socket is already running as fallback
                info!("evdev not available. Socket listener is running for external triggers.");
                // Refine the (possibly optimistic) status: if the probe
                // said /dev/input was readable but no device supports the
                // configured key, report the real backend. Never overwrite
                // a more specific verdict the probe already recorded
                // (permission denial / no devices).
                {
                    let mut st = super::lock(&status);
                    if st.backend == "evdev" {
                        *st = HotkeyStatus::socket_fallback(
                            format!(
                                "No keyboard devices found supporting key '{}'; only \
                                 external triggers work.",
                                key_name
                            ),
                            false,
                        );
                    }
                }
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
        let socket_path = super::hotkey_socket_path();
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
    let socket_path = super::hotkey_socket_path();

    // Clean up stale socket
    let _ = std::fs::remove_file(&socket_path);

    let sock = UnixDatagram::bind(&socket_path).context("Failed to bind hotkey socket")?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey::INPUT_GROUP_FIX_COMMAND;

    #[test]
    fn probe_granted_when_an_event_node_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("event3"), b"x").unwrap();
        assert_eq!(probe_evdev_access_in(dir.path()), EvdevAccess::Granted);
    }

    #[test]
    fn probe_ignores_non_event_nodes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mouse0"), b"x").unwrap();
        std::fs::write(dir.path().join("js0"), b"x").unwrap();
        assert_eq!(probe_evdev_access_in(dir.path()), EvdevAccess::NoDevices);
    }

    #[test]
    fn probe_missing_input_dir_is_no_devices() {
        assert_eq!(
            probe_evdev_access_in(Path::new("/nonexistent-dev-input")),
            EvdevAccess::NoDevices
        );
    }

    /// The regression this bead exists for: unreadable event nodes must
    /// be reported as a permissions failure (previously this state was
    /// indistinguishable from "no keyboards" and only reached the log).
    #[test]
    fn probe_reports_permission_denied_for_unreadable_nodes() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let node = dir.path().join("event0");
        std::fs::write(&node, b"x").unwrap();
        let mut perms = std::fs::metadata(&node).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&node, perms).unwrap();

        // Root (or a process with CAP_DAC_OVERRIDE) reads through mode
        // 000 — the probe legitimately returns Granted there, so skip.
        if std::fs::File::open(&node).is_ok() {
            return;
        }

        assert_eq!(
            probe_evdev_access_in(dir.path()),
            EvdevAccess::PermissionDenied
        );
    }

    /// One readable node wins even when a sibling is unreadable — the
    /// evdev backend can monitor *some* keyboard.
    #[test]
    fn probe_granted_beats_denied_sibling() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("event0"), b"x").unwrap();
        let denied = dir.path().join("event1");
        std::fs::write(&denied, b"x").unwrap();
        let mut perms = std::fs::metadata(&denied).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&denied, perms).unwrap();

        if std::fs::File::open(&denied).is_ok() {
            return; // running as root — mode bits don't block reads
        }

        assert_eq!(probe_evdev_access_in(dir.path()), EvdevAccess::Granted);
    }

    #[test]
    fn permission_denied_maps_to_socket_fallback_with_fix_command() {
        let status = EvdevAccess::PermissionDenied.into_status();
        assert_eq!(status.backend, "socket-fallback");
        assert!(status.permission_denied);
        assert_eq!(status.fix_command.as_deref(), Some(INPUT_GROUP_FIX_COMMAND));
        assert!(status.detail.as_deref().unwrap().contains("input"));
    }

    #[test]
    fn granted_and_no_devices_map_without_fix_command() {
        let granted = EvdevAccess::Granted.into_status();
        assert_eq!(granted.backend, "evdev");
        assert!(!granted.permission_denied);
        assert_eq!(granted.fix_command, None);

        let none = EvdevAccess::NoDevices.into_status();
        assert_eq!(none.backend, "socket-fallback");
        assert!(!none.permission_denied);
        assert_eq!(none.fix_command, None);
    }
}
