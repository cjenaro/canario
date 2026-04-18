/// Settings window — thin GTK4 wrapper around Canario core.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gtk4::prelude::*;
use glib::translate::IntoGlib;
use libadwaita as adw;
use libadwaita::prelude::*;

use canario_core::{AudioBehavior, Canario, ModelVariant};

use crate::ui::model_manager::ModelManagerWidget;
use crate::ui::word_remapping::WordRemappingWidget;
use crate::ui::history::HistoryWidget;

pub struct SettingsWindow;

impl SettingsWindow {
    pub fn present(app: &adw::Application, canario: &Canario) {
        let existing = app.windows().into_iter().find(|w| {
            w.widget_name() == "canario-settings"
        });
        if let Some(win) = existing {
            win.present();
            return;
        }
        let window = build_settings_window(app, canario);
        window.present();
    }
}

fn build_settings_window(app: &adw::Application, canario: &Canario) -> adw::ApplicationWindow {
    let win = adw::ApplicationWindow::new(app);
    win.set_widget_name("canario-settings");
    win.set_title(Some("Canario Settings"));
    win.set_default_size(600, 580);
    win.set_hide_on_close(true);

    let content = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    content.add_top_bar(&header);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled.set_vexpand(true);

    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 24);
    main_box.set_margin_start(20);
    main_box.set_margin_end(20);
    main_box.set_margin_top(20);
    main_box.set_margin_bottom(20);

    let config = canario.config();

    // ── Model section ───────────────────────────────────────────────
    let model_group = adw::PreferencesGroup::new();
    model_group.set_title("Model");

    let model_row = adw::ComboRow::new();
    model_row.set_title("Model Variant");
    let model_list = gtk4::StringList::new(&[
        "Parakeet TDT v3 — Multilingual",
        "Parakeet TDT v2 — English only",
    ]);
    model_row.set_model(Some(&model_list));
    match config.model {
        ModelVariant::ParakeetV3 => model_row.set_selected(0),
        ModelVariant::ParakeetV2 => model_row.set_selected(1),
        ModelVariant::Custom => {}
    }

    let c = canario.clone();
    model_row.connect_notify(Some("selected-item"), move |row, _| {
        let variant = match row.selected() {
            0 => ModelVariant::ParakeetV3,
            1 => ModelVariant::ParakeetV2,
            _ => ModelVariant::ParakeetV3,
        };
        let _ = c.update_config(|cfg| cfg.model = variant);
    });
    model_group.add(&model_row);

    let manager = ModelManagerWidget::new(canario);
    model_group.add(&manager.widget);
    main_box.append(&model_group);

    // ── Behavior section ────────────────────────────────────────────
    let behavior_group = adw::PreferencesGroup::new();
    behavior_group.set_title("Behavior");

    let paste_row = adw::SwitchRow::new();
    paste_row.set_title("Auto-paste Transcription");
    paste_row.set_subtitle("Automatically type transcription into the focused app");
    paste_row.set_active(config.auto_paste);
    let c = canario.clone();
    paste_row.connect_notify(Some("active"), move |row, _| {
        let _ = c.update_config(|cfg| cfg.auto_paste = row.is_active());
    });
    behavior_group.add(&paste_row);

    let audio_row = adw::ComboRow::new();
    audio_row.set_title("Audio During Recording");
    audio_row.set_subtitle("System audio behavior while recording voice");
    let audio_list = gtk4::StringList::new(&["Do nothing", "Mute system audio"]);
    audio_row.set_model(Some(&audio_list));
    match config.recording_audio_behavior {
        AudioBehavior::DoNothing => audio_row.set_selected(0),
        AudioBehavior::Mute => audio_row.set_selected(1),
    }
    let c = canario.clone();
    audio_row.connect_notify(Some("selected-item"), move |row, _| {
        let behavior = match row.selected() {
            0 => AudioBehavior::DoNothing,
            1 => AudioBehavior::Mute,
            _ => AudioBehavior::DoNothing,
        };
        let _ = c.update_config(|cfg| cfg.recording_audio_behavior = behavior);
    });
    behavior_group.add(&audio_row);

    let sound_row = adw::SwitchRow::new();
    sound_row.set_title("Sound Effects");
    sound_row.set_subtitle("Play a beep when recording starts and stops");
    sound_row.set_active(config.sound_effects);
    let c = canario.clone();
    sound_row.connect_notify(Some("active"), move |row, _| {
        let _ = c.update_config(|cfg| cfg.sound_effects = row.is_active());
    });
    behavior_group.add(&sound_row);

    let autostart_row = adw::SwitchRow::new();
    autostart_row.set_title("Start on Login");
    autostart_row.set_subtitle("Launch Canario automatically when you log in");
    autostart_row.set_active(canario_core::autostart::is_autostart_enabled().unwrap_or(false));
    autostart_row.connect_notify(Some("active"), move |row, _| {
        if row.is_active() {
            let _ = canario_core::autostart::enable_autostart();
        } else {
            let _ = canario_core::autostart::disable_autostart();
        }
    });
    behavior_group.add(&autostart_row);
    main_box.append(&behavior_group);

    // ── Hotkey section ──────────────────────────────────────────────
    let hotkey_group = adw::PreferencesGroup::new();
    hotkey_group.set_title("Hotkey");

    let hotkey_row = adw::ActionRow::new();
    hotkey_row.set_title("Global Hotkey");
    let hotkey_label = gtk4::Label::new(Some(&format_hotkey_display(&config.hotkey)));
    hotkey_label.add_css_class("heading");
    hotkey_row.add_suffix(&hotkey_label);

    let change_btn = gtk4::Button::with_label("Change");
    change_btn.add_css_class("suggested-action");
    change_btn.set_valign(gtk4::Align::Center);
    hotkey_row.add_suffix(&change_btn);

    let capture_label = gtk4::Label::new(Some("Press new hotkey… (Escape to cancel)"));
    capture_label.add_css_class("accent");
    capture_label.set_visible(false);
    capture_label.set_halign(gtk4::Align::Center);
    capture_label.set_margin_top(6);

    hotkey_group.add(&hotkey_row);
    hotkey_group.add(&capture_label);

    // Hotkey capture
    let capturing: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let c = canario.clone();
    let hotkey_label_c = hotkey_label.clone();
    let capture_label_c = capture_label.clone();
    let change_btn_c = change_btn.clone();

    let ec = gtk4::EventControllerKey::new();
    ec.set_propagation_phase(gtk4::PropagationPhase::Capture);

    let capturing_c = capturing.clone();
    let config_snapshot = config.clone();
    ec.connect_key_pressed(move |_controller, key, _keycode, modifier_type| {
        if !capturing_c.load(Ordering::SeqCst) {
            return glib::Propagation::Proceed;
        }
        use gtk4::gdk::Key;
        if key == Key::Escape {
            capturing_c.store(false, Ordering::SeqCst);
            capture_label_c.set_visible(false);
            change_btn_c.set_sensitive(true);
            hotkey_label_c.set_text(&format_hotkey_display(&config_snapshot.hotkey));
            return glib::Propagation::Stop;
        }
        let mut parts: Vec<String> = Vec::new();
        if modifier_type.contains(gtk4::gdk::ModifierType::SUPER_MASK) { parts.push("Super".into()); }
        if modifier_type.contains(gtk4::gdk::ModifierType::CONTROL_MASK) { parts.push("Ctrl".into()); }
        if modifier_type.contains(gtk4::gdk::ModifierType::SHIFT_MASK) { parts.push("Shift".into()); }
        if modifier_type.contains(gtk4::gdk::ModifierType::ALT_MASK) { parts.push("Alt".into()); }
        let key_name = key_to_name(&key);
        if key_name_is_modifier(&key_name) {
            let mut preview = parts.clone();
            preview.push(key_name);
            hotkey_label_c.set_text(&format_hotkey_display(&preview));
            return glib::Propagation::Stop;
        }
        parts.push(key_name);
        hotkey_label_c.set_text(&format_hotkey_display(&parts));
        capturing_c.store(false, Ordering::SeqCst);
        capture_label_c.set_visible(false);
        change_btn_c.set_sensitive(true);
        let _ = c.update_config(|cfg| cfg.hotkey = parts);
        let _ = c.restart_hotkey();
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

    let double_tap_row = adw::SwitchRow::new();
    double_tap_row.set_title("Double-tap to Lock");
    double_tap_row.set_subtitle("Double-tap the hotkey to toggle recording on/off");
    double_tap_row.set_active(config.double_tap_lock);
    let c = canario.clone();
    double_tap_row.connect_notify(Some("active"), move |row, _| {
        let _ = c.update_config(|cfg| cfg.double_tap_lock = row.is_active());
    });
    hotkey_group.add(&double_tap_row);

    let hold_row = adw::SpinRow::new(
        Some(&gtk4::Adjustment::new(config.minimum_key_time, 0.05, 1.0, 0.05, 0.1, 0.1)),
        0.05, 2,
    );
    hold_row.set_title("Minimum Hold Time");
    hold_row.set_subtitle("Seconds to hold before recording starts");
    let c = canario.clone();
    hold_row.connect_notify(Some("value"), move |row, _| {
        let _ = c.update_config(|cfg| cfg.minimum_key_time = row.value());
    });
    hotkey_group.add(&hold_row);
    main_box.append(&hotkey_group);

    // ── Word Remapping section ──────────────────────────────────────
    let remapping_widget = WordRemappingWidget::new(canario);
    main_box.append(&remapping_widget.group);

    // ── History section ─────────────────────────────────────────────
    let history_widget = HistoryWidget::new(canario);
    main_box.append(&history_widget.group);

    // ── About section ───────────────────────────────────────────────
    let about_group = adw::PreferencesGroup::new();
    about_group.set_title("About");
    let about_row = adw::ActionRow::new();
    about_row.set_title("Canario");
    about_row.set_subtitle("Native Linux voice-to-text using Parakeet TDT");
    let version_label = gtk4::Label::new(Some("v0.1.1"));
    version_label.add_css_class("dim-label");
    about_row.add_suffix(&version_label);
    about_group.add(&about_row);
    main_box.append(&about_group);

    scrolled.set_child(Some(&main_box));
    content.set_content(Some(&scrolled));
    content.add_controller(ec);
    win.set_content(Some(&content));
    win
}

fn format_hotkey_display(parts: &[String]) -> String {
    if parts.is_empty() { "Not set".into() } else { parts.join(" + ") }
}

fn key_name_is_modifier(name: &str) -> bool {
    matches!(name, "Super" | "Ctrl" | "Shift" | "Alt" | "Hyper" | "Meta")
}

fn key_to_name(key: &gtk4::gdk::Key) -> String {
    use gtk4::gdk::Key;
    match key {
        k if *k == Key::Super_L || *k == Key::Super_R => "Super".into(),
        k if *k == Key::Alt_L || *k == Key::Alt_R => "Alt".into(),
        k if *k == Key::Control_L || *k == Key::Control_R => "Ctrl".into(),
        k if *k == Key::Shift_L || *k == Key::Shift_R => "Shift".into(),
        k if *k == Key::space => "Space".into(),
        k if *k == Key::Return => "Return".into(),
        _ => {
            let raw = key.into_glib() as u32;
            let f1 = Key::F1.into_glib() as u32;
            let f12 = Key::F12.into_glib() as u32;
            if raw >= f1 && raw <= f12 + 23 { return format!("F{}", raw - f1 + 1); }
            if let Some(c) = key.to_unicode() {
                if c.is_ascii_graphic() || c == ' ' { return c.to_uppercase().to_string(); }
            }
            format!("Key{:03X}", raw)
        }
    }
}
