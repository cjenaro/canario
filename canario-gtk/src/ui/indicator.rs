/// Recording indicator — a small floating overlay that shows recording state.
///
/// Appears when recording starts, shows an audio level meter,
/// changes to "Transcribing…" when done, then disappears after paste.
///
/// Note: In GTK4, true overlay positioning requires gtk4-layer-shell.
/// For now this is a simple undecorated popup window. Phase 4 can add
/// proper layer-shell integration for Wayland.
use gtk4::prelude::*;
use libadwaita as adw;

/// Static methods to show/hide the indicator from the GTK main loop.
/// The indicator is identified by its widget name.
pub struct RecordingIndicator;

impl RecordingIndicator {
    /// Show the recording indicator. Creates one if none exists.
    pub fn show(app: &adw::Application) {
        let existing = app.windows().into_iter().find(|w| {
            w.widget_name() == "canario-indicator"
        });
        if existing.is_some() {
            return;
        }

        let win = build_indicator_window(app);
        win.present();
    }

    /// Hide the recording indicator
    pub fn hide(app: &adw::Application) {
        let existing = app.windows().into_iter().find(|w| {
            w.widget_name() == "canario-indicator"
        });
        if let Some(win) = existing {
            win.close();
        }
    }

    /// Update the audio level meter (0.0 – 1.0)
    pub fn update_level(app: &adw::Application, level: f64) {
        let existing = app.windows().into_iter().find(|w| {
            w.widget_name() == "canario-indicator"
        });
        if let Some(win) = existing {
            // Walk the widget tree to find the progress bar
            find_and_update_progress(win.upcast_ref::<gtk4::Widget>(), level);
        }
    }
}

fn find_and_update_progress(widget: &gtk4::Widget, level: f64) {
    if let Ok(pb) = widget.clone().downcast::<gtk4::ProgressBar>() {
        pb.set_fraction(level.clamp(0.0, 1.0));
        return;
    }
    if let Some(container) = widget.dynamic_cast_ref::<gtk4::Box>() {
        let mut child = container.first_child();
        while let Some(c) = child {
            find_and_update_progress(&c, level);
            child = c.next_sibling();
        }
    }
}

fn build_indicator_window(app: &adw::Application) -> gtk4::Window {
    let win = gtk4::Window::new();
    win.set_widget_name("canario-indicator");
    win.set_title(Some("Canario — Recording"));
    win.set_default_size(240, 80);
    win.set_decorated(false);
    win.set_resizable(false);

    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    container.set_margin_start(12);
    container.set_margin_end(12);
    container.set_margin_top(8);
    container.set_margin_bottom(8);
    container.add_css_class("osd");
    container.add_css_class("toolbar");

    // Recording dot + label
    let top_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);

    let dot = gtk4::Label::new(Some("🔴"));
    let label = gtk4::Label::new(Some("Recording…"));
    label.add_css_class("heading");

    top_row.append(&dot);
    top_row.append(&label);

    // Audio level progress bar
    let level_bar = gtk4::ProgressBar::new();
    level_bar.set_widget_name("canario-level-bar");
    level_bar.set_fraction(0.0);
    level_bar.set_show_text(false);
    level_bar.add_css_class("osd");

    container.append(&top_row);
    container.append(&level_bar);

    win.set_child(Some(&container));
    win.set_application(Some(app));

    win
}
