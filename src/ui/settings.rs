/// Settings window — model selection, auto-paste, hotkey config, etc.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use gtk4::prelude::*;
use glib::translate::IntoGlib;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::config::{AudioBehavior, ModelVariant};
use crate::history::History;
use crate::ui::model_manager::ModelManagerWidget;
use crate::ui::word_remapping::WordRemappingWidget;
use crate::ui::history::HistoryWidget;
use crate::ui::{AppMessage, AppState};

/// Present the settings window (creates one if none exists, or brings to front)
pub struct SettingsWindow;

impl SettingsWindow {
    pub fn present(app: &adw::Application, state: Arc<Mutex<AppState>>, tx: Sender<AppMessage>, history: Arc<Mutex<History>>) {
        // Check if a settings window already exists
        let existing = app.windows().into_iter().find(|w| {
            w.widget_name() == "canario-settings"
        });

        if let Some(win) = existing {
            win.present();
            return;
        }

        let window = build_settings_window(app, state, tx, history);
        window.present();
    }
}

fn build_settings_window(
    app: &adw::Application,
    state: Arc<Mutex<AppState>>,
    tx: Sender<AppMessage>,
    history: Arc<Mutex<History>>,
) -> adw::ApplicationWindow {
    let win = adw::ApplicationWindow::new(app);
    win.set_widget_name("canario-settings");
    win.set_title(Some("Canario Settings"));
    win.set_default_size(600, 580);
    win.set_hide_on_close(true);

    let content = adw::ToolbarView::new();

    // Header bar
    let header = adw::HeaderBar::new();
    content.add_top_bar(&header);

    // Main content in a scrolled window
    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled.set_vexpand(true);

    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 24);
    main_box.set_margin_start(20);
    main_box.set_margin_end(20);
    main_box.set_margin_top(20);
    main_box.set_margin_bottom(20);

    // ── Model section ───────────────────────────────────────────────
    let model_group = adw::PreferencesGroup::new();
    model_group.set_title("Model");

    // Model variant dropdown
    let model_row = adw::ComboRow::new();
    model_row.set_title("Model Variant");

    let model_list = gtk4::StringList::new(&[
        "Parakeet TDT v3 — Multilingual",
        "Parakeet TDT v2 — English only",
    ]);

    model_row.set_model(Some(&model_list));

    {
        let s = state.lock().unwrap();
        match s.config.model {
            ModelVariant::ParakeetV3 => model_row.set_selected(0),
            ModelVariant::ParakeetV2 => model_row.set_selected(1),
            ModelVariant::Custom => {} // not in dropdown
        }
    }

    let state_for_model = state.clone();
    model_row.connect_notify(Some("selected-item"), move |row, _| {
        let idx = row.selected();
        let mut s = state_for_model.lock().unwrap();
        s.config.model = match idx {
            0 => ModelVariant::ParakeetV3,
            1 => ModelVariant::ParakeetV2,
            _ => ModelVariant::ParakeetV3,
        };
        let _ = s.config.save();
    });

    model_group.add(&model_row);

    // Model manager (download/delete)
    let manager = ModelManagerWidget::new(state.clone());
    model_group.add(&manager.widget);

    main_box.append(&model_group);

    // ── Behavior section ────────────────────────────────────────────
    let behavior_group = adw::PreferencesGroup::new();
    behavior_group.set_title("Behavior");

    // Auto-paste toggle
    let paste_row = adw::SwitchRow::new();
    paste_row.set_title("Auto-paste Transcription");
    paste_row.set_subtitle("Automatically type transcription into the focused app");
    {
        let s = state.lock().unwrap();
        paste_row.set_active(s.config.auto_paste);
    }
    let state_for_paste = state.clone();
    paste_row.connect_notify(Some("active"), move |row, _| {
        let mut s = state_for_paste.lock().unwrap();
        s.config.auto_paste = row.is_active();
        let _ = s.config.save();
    });
    behavior_group.add(&paste_row);

    // Audio behavior during recording
    let audio_row = adw::ComboRow::new();
    audio_row.set_title("Audio During Recording");
    audio_row.set_subtitle("System audio behavior while recording voice");
    let audio_list = gtk4::StringList::new(&["Do nothing", "Mute system audio"]);
    audio_row.set_model(Some(&audio_list));
    {
        let s = state.lock().unwrap();
        match s.config.recording_audio_behavior {
            AudioBehavior::DoNothing => audio_row.set_selected(0),
            AudioBehavior::Mute => audio_row.set_selected(1),
        }
    }
    let state_for_audio = state.clone();
    audio_row.connect_notify(Some("selected-item"), move |row, _| {
        let mut s = state_for_audio.lock().unwrap();
        s.config.recording_audio_behavior = match row.selected() {
            0 => AudioBehavior::DoNothing,
            1 => AudioBehavior::Mute,
            _ => AudioBehavior::DoNothing,
        };
        let _ = s.config.save();
    });
    behavior_group.add(&audio_row);

    // Sound effects toggle
    let sound_row = adw::SwitchRow::new();
    sound_row.set_title("Sound Effects");
    sound_row.set_subtitle("Play a beep when recording starts and stops");
    {
        let s = state.lock().unwrap();
        sound_row.set_active(s.config.sound_effects);
    }
    let state_for_sound = state.clone();
    sound_row.connect_notify(Some("active"), move |row, _| {
        let mut s = state_for_sound.lock().unwrap();
        s.config.sound_effects = row.is_active();
        let _ = s.config.save();
    });
    behavior_group.add(&sound_row);

    // Autostart toggle
    let autostart_row = adw::SwitchRow::new();
    autostart_row.set_title("Start on Login");
    autostart_row.set_subtitle("Launch Canario automatically when you log in");
    {
        autostart_row.set_active(
            crate::config::autostart::is_autostart_enabled().unwrap_or(false)
        );
    }
    autostart_row.connect_notify(Some("active"), move |row, _| {
        if row.is_active() {
            if let Err(e) = crate::config::autostart::enable_autostart() {
                tracing::error!("Failed to enable autostart: {}", e);
            }
        } else {
            if let Err(e) = crate::config::autostart::disable_autostart() {
                tracing::error!("Failed to disable autostart: {}", e);
            }
        }
    });
    behavior_group.add(&autostart_row);

    main_box.append(&behavior_group);

    // ── Hotkey section ──────────────────────────────────────────────
    let hotkey_group = adw::PreferencesGroup::new();
    hotkey_group.set_title("Hotkey");

    // Hotkey capture row
    let hotkey_row = adw::ActionRow::new();
    hotkey_row.set_title("Global Hotkey");
    hotkey_row.set_activatable(true);

    // Current hotkey label
    let hotkey_label = gtk4::Label::new(None);
    hotkey_label.add_css_class("heading");
    {
        let s = state.lock().unwrap();
        let display = if s.config.hotkey.is_empty() {
            "Not set".to_string()
        } else {
            format_hotkey_display(&s.config.hotkey)
        };
        hotkey_label.set_text(&display);
    }
    hotkey_row.add_suffix(&hotkey_label);

    // "Change" button
    let change_btn = gtk4::Button::with_label("Change");
    change_btn.add_css_class("suggested-action");
    change_btn.set_valign(gtk4::Align::Center);
    hotkey_row.add_suffix(&change_btn);

    // Capture instruction label (hidden by default)
    let capture_label = gtk4::Label::new(Some("Press new hotkey… (Escape to cancel)"));
    capture_label.add_css_class("accent");
    capture_label.set_visible(false);
    capture_label.set_halign(gtk4::Align::Center);
    capture_label.set_margin_top(6);

    hotkey_group.add(&hotkey_row);
    hotkey_group.add(&capture_label);

    // ── Hotkey capture event controller ──────────────────────────
    let state_for_capture = state.clone();
    let tx_for_capture = tx.clone();
    let hotkey_label_c = hotkey_label.clone();
    let capture_label_c = capture_label.clone();
    let change_btn_c = change_btn.clone();

    let capturing: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let capturing_c = capturing.clone();

    let ec = gtk4::EventControllerKey::new();
    ec.set_propagation_phase(gtk4::PropagationPhase::Capture);

    ec.connect_key_pressed(move |_controller, key, _keycode, modifier_type| {
        if !capturing_c.load(Ordering::SeqCst) {
            return glib::Propagation::Proceed;
        }

        use gtk4::gdk::Key;

        // Escape cancels capture
        if key == Key::Escape {
            capturing_c.store(false, Ordering::SeqCst);
            capture_label_c.set_visible(false);
            change_btn_c.set_sensitive(true);
            // Restore original label
            let s = state_for_capture.lock().unwrap();
            let display = if s.config.hotkey.is_empty() {
                "Not set".to_string()
            } else {
                format_hotkey_display(&s.config.hotkey)
            };
            hotkey_label_c.set_text(&display);
            return glib::Propagation::Stop;
        }

        // Build the key combo from modifiers
        let mut parts: Vec<String> = Vec::new();

        if modifier_type.contains(gtk4::gdk::ModifierType::SUPER_MASK) {
            parts.push("Super".into());
        }
        if modifier_type.contains(gtk4::gdk::ModifierType::CONTROL_MASK) {
            parts.push("Ctrl".into());
        }
        if modifier_type.contains(gtk4::gdk::ModifierType::SHIFT_MASK) {
            parts.push("Shift".into());
        }
        if modifier_type.contains(gtk4::gdk::ModifierType::ALT_MASK) {
            parts.push("Alt".into());
        }

        // Convert key to name
        let key_name = key_to_name(&key);

        // If the key is a modifier, just show preview — wait for the actual key
        if key_name_is_modifier(&key_name) {
            let mut preview = parts.clone();
            preview.push(key_name);
            hotkey_label_c.set_text(&format_hotkey_display(&preview));
            return glib::Propagation::Stop;
        }

        // We have a full key combo!
        parts.push(key_name);

        // Update the label
        let display = format_hotkey_display(&parts);
        hotkey_label_c.set_text(&display);

        // Exit capture mode
        capturing_c.store(false, Ordering::SeqCst);
        capture_label_c.set_visible(false);
        change_btn_c.set_sensitive(true);

        // Send the change to the app
        let _ = tx_for_capture.send(AppMessage::HotkeyChanged(parts));

        glib::Propagation::Stop
    });

    let capturing_btn = capturing.clone();
    let capture_label_btn = capture_label.clone();
    let hotkey_label_btn = hotkey_label.clone();
    change_btn.connect_clicked(move |btn| {
        capturing_btn.store(true, Ordering::SeqCst);
        capture_label_btn.set_visible(true);
        btn.set_sensitive(false);
        hotkey_label_btn.set_text("Press new hotkey…");
    });

    // Double-tap lock toggle
    let double_tap_row = adw::SwitchRow::new();
    double_tap_row.set_title("Double-tap to Lock");
    double_tap_row.set_subtitle("Double-tap the hotkey to toggle recording on/off");
    {
        let s = state.lock().unwrap();
        double_tap_row.set_active(s.config.double_tap_lock);
    }
    let state_for_dtap = state.clone();
    double_tap_row.connect_notify(Some("active"), move |row, _| {
        let mut s = state_for_dtap.lock().unwrap();
        s.config.double_tap_lock = row.is_active();
        let _ = s.config.save();
    });
    hotkey_group.add(&double_tap_row);

    // Minimum hold time adjustment
    let hold_row = adw::SpinRow::new(
        Some(&gtk4::Adjustment::new(
            {
                let s = state.lock().unwrap();
                s.config.minimum_key_time
            },
            0.05,  // min
            1.0,   // max
            0.05,  // step
            0.1,   // page_inc
            0.1,   // page_size
        )),
        0.05,
        2,
    );
    hold_row.set_title("Minimum Hold Time");
    hold_row.set_subtitle("Seconds to hold before recording starts");
    let state_for_hold = state.clone();
    hold_row.connect_notify(Some("value"), move |row, _| {
        let mut s = state_for_hold.lock().unwrap();
        s.config.minimum_key_time = row.value();
        let _ = s.config.save();
    });
    hotkey_group.add(&hold_row);

    main_box.append(&hotkey_group);

    // ── Word Remapping section ───────────────────────────────────
    let remapping_widget = WordRemappingWidget::new(state.clone());
    main_box.append(&remapping_widget.group);

    // ── History section ─────────────────────────────────────────
    let history_widget = HistoryWidget::new(state.clone(), history);
    main_box.append(&history_widget.group);

    // ── About section ───────────────────────────────────────────────
    let about_group = adw::PreferencesGroup::new();
    about_group.set_title("About");

    let about_row = adw::ActionRow::new();
    about_row.set_title("Canario");
    about_row.set_subtitle("Native Linux voice-to-text using Parakeet TDT");
    let version_label = gtk4::Label::new(Some("v0.1.0"));
    version_label.add_css_class("dim-label");
    about_row.add_suffix(&version_label);
    about_group.add(&about_row);

    main_box.append(&about_group);

    scrolled.set_child(Some(&main_box));
    content.set_content(Some(&scrolled));

    // Attach the key capture controller to the content area
    content.add_controller(ec);

    win.set_content(Some(&content));
    win
}

/// Format a hotkey vector for display, e.g. ["Super", "Space"] → "Super + Space"
fn format_hotkey_display(parts: &[String]) -> String {
    parts.join(" + ")
}

/// Check if a key name represents a modifier key.
fn key_name_is_modifier(name: &str) -> bool {
    matches!(
        name,
        "Super" | "Ctrl" | "Shift" | "Alt" | "Hyper" | "Meta"
    )
}

/// Convert a `gdk::Key` to a human-readable key name stored in config.
fn key_to_name(key: &gtk4::gdk::Key) -> String {
    use gtk4::gdk::Key;

    match key {
        // Modifiers
        k if *k == Key::Super_L || *k == Key::Super_R => "Super".into(),
        k if *k == Key::Hyper_L || *k == Key::Hyper_R => "Hyper".into(),
        k if *k == Key::Meta_L || *k == Key::Meta_R => "Meta".into(),
        k if *k == Key::Alt_L || *k == Key::Alt_R => "Alt".into(),
        k if *k == Key::Control_L || *k == Key::Control_R => "Ctrl".into(),
        k if *k == Key::Shift_L || *k == Key::Shift_R => "Shift".into(),

        // Special keys
        k if *k == Key::space => "Space".into(),
        k if *k == Key::Return => "Return".into(),
        k if *k == Key::BackSpace => "Backspace".into(),
        k if *k == Key::Tab => "Tab".into(),
        k if *k == Key::Delete => "Delete".into(),
        k if *k == Key::Insert => "Insert".into(),
        k if *k == Key::Home => "Home".into(),
        k if *k == Key::End => "End".into(),
        k if *k == Key::Page_Up => "PageUp".into(),
        k if *k == Key::Page_Down => "PageDown".into(),
        k if *k == Key::Caps_Lock => "CapsLock".into(),
        k if *k == Key::Num_Lock => "NumLock".into(),

        // Arrow keys
        k if *k == Key::Left => "Left".into(),
        k if *k == Key::Right => "Right".into(),
        k if *k == Key::Up => "Up".into(),
        k if *k == Key::Down => "Down".into(),

        // Keypad
        k if *k == Key::KP_Enter => "KP_Enter".into(),
        k if *k == Key::KP_Space => "Space".into(),

        // Try function keys: F1 = 0xFFBE (65470), F12 = 0xFFC9 (65481)
        // GDK_KEY_F1 through F35
        _ => {
            let raw = key.into_glib() as u32;

            // Function keys
            let f1_raw = Key::F1.into_glib() as u32;
            let f12_raw = Key::F12.into_glib() as u32;
            let f35_raw = f12_raw + 23; // F13..F35 are consecutive after F12

            if raw >= f1_raw && raw <= f35_raw {
                let n = raw - f1_raw + 1;
                return format!("F{}", n);
            }

            // Regular printable character
            if let Some(c) = key.to_unicode() {
                if c.is_ascii_graphic() || c == ' ' {
                    return c.to_uppercase().to_string();
                }
            }

            // Fallback
            format!("Key{:03X}", raw)
        }
    }
}
