/// Settings window — model selection, auto-paste, hotkey config, etc.
use std::sync::{Arc, Mutex};

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::config::{AudioBehavior, ModelVariant};
use crate::ui::model_manager::ModelManagerWidget;
use crate::ui::AppState;

/// Present the settings window (creates one if none exists, or brings to front)
pub struct SettingsWindow;

impl SettingsWindow {
    pub fn present(app: &adw::Application, state: Arc<Mutex<AppState>>) {
        // Check if a settings window already exists
        let existing = app.windows().into_iter().find(|w| {
            w.widget_name() == "canario-settings"
        });

        if let Some(win) = existing {
            win.present();
            return;
        }

        let window = build_settings_window(app, state);
        window.present();
    }
}

fn build_settings_window(
    app: &adw::Application,
    state: Arc<Mutex<AppState>>,
) -> adw::ApplicationWindow {
    let win = adw::ApplicationWindow::new(app);
    win.set_widget_name("canario-settings");
    win.set_title(Some("Canario Settings"));
    win.set_default_size(600, 500);
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

    main_box.append(&behavior_group);

    // ── Hotkey section ──────────────────────────────────────────────
    let hotkey_group = adw::PreferencesGroup::new();
    hotkey_group.set_title("Hotkey");

    let hotkey_row = adw::ActionRow::new();
    hotkey_row.set_title("Global Hotkey");
    hotkey_row.set_subtitle("Not yet configurable — coming in Phase 3");
    hotkey_row.set_sensitive(false);

    let hotkey_label = gtk4::Label::new(Some("Super + Space"));
    hotkey_label.add_css_class("dim-label");
    hotkey_row.add_suffix(&hotkey_label);

    hotkey_group.add(&hotkey_row);

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

    main_box.append(&hotkey_group);

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

    win.set_content(Some(&content));
    win
}
