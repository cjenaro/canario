#[cfg(any(target_os = "linux", target_os = "windows"))]
/// X11 global hotkey via XGrabKey.
///
pub struct X11Hotkey {
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl X11Hotkey {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    /// Start listening for the hotkey on a background thread.
    ///
    /// - `key_sym`: X11 keysym name (e.g., "Super_L", "space", "Alt_L")
    /// - `modifiers`: modifier names (e.g., ["Super"])
    /// - `processor_config`: configuration for the press-hold/double-tap processor
    /// - `on_action`: callback for when the processor emits an action
    pub fn start(
        &mut self,
        key_sym: &str,
        modifiers: &[String],
        processor_config: ProcessorConfig,
        on_action: OnAction,
    ) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            bail!("Hotkey listener already running");
        }

        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);

        let key_sym = key_sym.to_string();
        let modifiers = modifiers.to_vec();

        let handle = std::thread::Builder::new()
            .name("x11-hotkey".into())
            .spawn(move || {
                if let Err(e) = x11_loop(&running, &key_sym, &modifiers, processor_config, on_action)
                {
                    if !e.to_string().contains("Connection refused") {
                        error!("X11 hotkey loop error: {}", e);
                    }
                }
            })
            .context("Failed to spawn X11 hotkey thread")?;

        self.thread = Some(handle);
        Ok(())
    }

    /// Stop the hotkey listener.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Drop for X11Hotkey {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The main X11 event loop. Runs on the hotkey thread.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn x11_loop(
    running: &Arc<AtomicBool>,
    key_sym: &str,
    modifiers: &[String],
    processor_config: ProcessorConfig,
    on_action: OnAction,
) -> Result<()> {
    use x11rb::connection::Connection;
    use x11rb::rust_connection::RustConnection;

    // Connect to X11
    let (conn, screen_num) = match RustConnection::connect(None) {
        Ok(c) => c,
        Err(e) => {
            // No X11 display available — not an error on Wayland
            info!("No X11 display available: {}", e);
            return Ok(());
        }
    };
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    // Parse key symbol
    let keycode = keysym_to_keycode(&conn, key_sym)?;
    let mod_mask = parse_modifiers(modifiers);

    // Also grab with NumLock and CapsLock combinations, as those modifiers
    // can prevent the grab from matching.
    let extra_mods: &[ModMask] = &[
        ModMask::from(0u8), // no extra modifier
        ModMask::M2,        // CapsLock
        ModMask::M5,        // NumLock
        ModMask::M2 | ModMask::M5,
    ];

    for &extra in extra_mods {
        conn.grab_key(
            true,
            root,
            mod_mask | extra,
            keycode,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?;
    }
    conn.flush()?;

    info!(
        "X11 hotkey grabbed: key={} mods={:?} (keycode={})",
        key_sym, modifiers, keycode
    );

    let mut processor = HotkeyProcessor::new(processor_config);
    let mut key_grabbed = false;

    // Tick interval for checking hold duration
    let tick_interval = Duration::from_millis(20);

    while running.load(Ordering::SeqCst) {
        // Poll for events with a timeout
        let event = conn
            .poll_for_event()
            .context("Failed to poll X11 events")?;

        if let Some(event) = event {
            match event {
                // Key press
                x11rb::protocol::Event::KeyPress(kp) => {
                    if kp.detail == keycode {
                        debug!("Hotkey key press detected");
                        if let Some(action) = processor.on_key_press() {
                            on_action(action);
                        }
                        // Check tick immediately
                        if let Some(action) = processor.on_tick() {
                            on_action(action);
                            grab_keyboard_if_needed(&conn, root, &mut key_grabbed);
                        }
                    } else {
                        // Other key pressed — notify processor (for modifier cancellation)
                        if let Some(action) = processor.on_other_key() {
                            on_action(action);
                        }
                        // If processor is idle and keyboard is grabbed, ungrab
                        if !processor.is_recording() && key_grabbed {
                            let _ = conn.ungrab_keyboard(x11rb::CURRENT_TIME);
                            let _ = conn.flush();
                            key_grabbed = false;
                            debug!("Keyboard ungrabbed (other key)");
                        }
                    }
                }
                // Key release
                x11rb::protocol::Event::KeyRelease(kr) => {
                    if kr.detail == keycode {
                        debug!("Hotkey key release detected");
                        if let Some(action) = processor.on_key_release() {
                            on_action(action);
                        }
                        // Ungrab keyboard if no longer recording
                        if !processor.is_recording() && key_grabbed {
                            let _ = conn.ungrab_keyboard(x11rb::CURRENT_TIME);
                            let _ = conn.flush();
                            key_grabbed = false;
                            debug!("Keyboard ungrabbed (release)");
                        }
                    }
                }
                _ => {
                    // Ignore other events
                }
            }
        }

        // Tick for hold detection
        if let Some(action) = processor.on_tick() {
            on_action(action);
            grab_keyboard_if_needed(&conn, root, &mut key_grabbed);
        }

        let _ = conn.flush();
        std::thread::sleep(tick_interval);
    }

    // Cleanup: ungrab key and keyboard
    for &extra in extra_mods {
        let _ = conn.ungrab_key(keycode, root, mod_mask | extra);
    }
    if key_grabbed {
        let _ = conn.ungrab_keyboard(x11rb::CURRENT_TIME);
    }
    let _ = conn.flush();

    info!("X11 hotkey listener stopped");
    Ok(())
}

/// Try to grab the full keyboard so we can catch all key releases.
fn grab_keyboard_if_needed<C: x11rb::connection::Connection>(
    conn: &C,
    root: u32,
    key_grabbed: &mut bool,
) {
    if *key_grabbed {
        return;
    }
    match conn.grab_keyboard(
        true,
        root,
        x11rb::CURRENT_TIME,
        GrabMode::ASYNC,
        GrabMode::ASYNC,
    ) {
        Ok(cookie) => {
            // VoidCookie for grab_keyboard doesn't need check() in the same way;
            // it returns a Cookie<GrabKeyboardReply>. Let's just assume success.
            *key_grabbed = true;
            debug!("Keyboard grabbed");
            // Suppress unused variable warning
            let _ = cookie;
        }
        Err(e) => {
            warn!("Failed to grab keyboard: {}", e);
        }
    }
}

/// Parse an X11 keysym name to a keycode.
fn keysym_to_keycode<C: x11rb::connection::Connection>(
    conn: &C,
    name: &str,
) -> Result<Keycode> {
    let keysym: u32 = match name {
        "Super_L" | "Super" => 0xFFEB,      // XK_Super_L
        "Super_R" => 0xFFEC,                 // XK_Super_R
        "Alt_L" | "Alt" => 0xFFE9,           // XK_Alt_L
        "Alt_R" => 0xFFEA,                   // XK_Alt_R
        "Control_L" | "Control" | "Ctrl" => 0xFFE3, // XK_Control_L
        "Control_R" => 0xFFE4,               // XK_Control_R
        "Shift_L" | "Shift" => 0xFFE1,       // XK_Shift_L
        "Shift_R" => 0xFFE2,                 // XK_Shift_R
        "space" | "Space" => 0x0020,         // XK_space
        "Hyper_L" => 0xFFED,                 // XK_Hyper_L
        "Meta_L" | "Meta" => 0xFFE7,         // XK_Meta_L
        other => {
            // Try to parse as a single character keysym
            if other.len() == 1 {
                other.chars().next().unwrap() as u32
            } else {
                bail!("Unknown key name: {}", other);
            }
        }
    };

    // Get the keyboard mapping to find the keycode for this keysym
    let min_keycode = conn.setup().min_keycode;
    let max_keycode = conn.setup().max_keycode;
    let mapping = conn
        .get_keyboard_mapping(min_keycode, max_keycode - min_keycode + 1)?
        .reply()?;

    let keysyms_per_keycode = mapping.keysyms_per_keycode as usize;

    // X11 keymaps: each keycode has a list of keysyms.
    for (i, keysyms_for_keycode) in mapping.keysyms.chunks(keysyms_per_keycode).enumerate() {
        for &ks in keysyms_for_keycode {
            if ks == keysym {
                return Ok(min_keycode + i as u8);
            }
        }
    }

    bail!("Could not find keycode for keysym '{}' (0x{:X})", name, keysym)
}

/// Parse modifier names to an X11 modifier mask.
fn parse_modifiers(modifiers: &[String]) -> ModMask {
    let mut mask = ModMask::from(0u8);
    for m in modifiers {
        match m.as_str() {
            "Super" | "Super_L" | "Super_R" => mask |= ModMask::M4,
            "Alt" | "Alt_L" | "Alt_R" => mask |= ModMask::M1,
            "Control" | "Ctrl" | "Control_L" | "Control_R" => mask |= ModMask::CONTROL,
            "Shift" | "Shift_L" | "Shift_R" => mask |= ModMask::SHIFT,
            "Hyper" => mask |= ModMask::M3,
            _ => warn!("Unknown modifier: {}", m),
        }
    }
    mask
}
