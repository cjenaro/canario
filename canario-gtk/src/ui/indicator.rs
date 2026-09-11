/// Recording indicator — a small floating overlay that shows recording state.
///
/// Appears when recording starts, shows an audio level meter, and during
/// long recordings a bounded live caption (the latest `PartialTranscript`
/// preview — authoritative text still arrives via `TranscriptionReady`).
/// Disappears when the recording stops.
///
/// Positioning limitations: GTK4 deliberately offers no API to position
/// top-level windows, keep them above others, or make them click-through —
/// on Wayland that is exclusively layer-shell territory. So this window:
/// - is undecorated and small (240x80, growing to a bounded caption size
///   while live captions are shown), and
/// - never accepts keyboard focus (it must not steal focus from the app
///   the user is dictating into).
///
/// Its on-screen position is left to the compositor. Proper top-center
/// overlay placement (active-monitor workarea) + keep-on-top require
/// gtk4-layer-shell — tracked as bead canario-7ah.10.
use gtk4::prelude::*;
use libadwaita as adw;

/// Caption-less starting size of the indicator window.
const WINDOW_DEFAULT_WIDTH: i32 = 240;
const WINDOW_DEFAULT_HEIGHT: i32 = 80;

/// Bounded size while a live caption is shown. For a non-resizable window
/// GTK sizes to `MAX(content minimum, default size)`
/// (`gtk_window_compute_default_size`). The label's character/line limits
/// bound growth from transcript content; larger accessibility fonts can
/// still increase the minimum beyond this preferred size.
const CAPTION_WINDOW_WIDTH: i32 = 400;
const CAPTION_WINDOW_HEIGHT: i32 = 140;

/// Caption label width request, in characters. `gtk_label_measure` derives
/// the label's minimum and natural widths from `width_chars` /
/// `max_width_chars` — not from the text — so these bound the geometry
/// independently of the (untrusted) content.
const CAPTION_WIDTH_CHARS: i32 = 26;
const CAPTION_MAX_WIDTH_CHARS: i32 = 46;

/// Line cap for the caption label. `gtk_label_set_lines` maps to a Pango
/// height cap of N lines *per paragraph*; `caption_text` collapses line
/// breaks so the transcript is always a single paragraph, making this a
/// total cap.
const CAPTION_MAX_LINES: i32 = 3;

/// Hard cap on characters handed to the label per partial. Wrap + the line
/// cap already bound the geometry; this additionally bounds layout work
/// per update (ASR partials are short, but the text is untrusted).
const CAPTION_MAX_CHARS: usize = 600;

/// Static methods to show/hide the indicator from the GTK main loop.
/// The indicator is identified by its widget name.
pub struct RecordingIndicator;

impl RecordingIndicator {
    /// Show the recording indicator. Creates one if none exists.
    pub fn show(app: &adw::Application) {
        let existing = app
            .windows()
            .into_iter()
            .find(|w| w.widget_name() == "canario-indicator");
        if existing.is_some() {
            return;
        }

        let win = build_indicator_window(app);
        win.present();
    }

    /// Hide the recording indicator
    pub fn hide(app: &adw::Application) {
        let existing = app
            .windows()
            .into_iter()
            .find(|w| w.widget_name() == "canario-indicator");
        if let Some(win) = existing {
            win.close();
        }
    }

    /// Update the audio level meter (0.0 – 1.0)
    pub fn update_level(app: &adw::Application, level: f64) {
        let existing = app
            .windows()
            .into_iter()
            .find(|w| w.widget_name() == "canario-indicator");
        if let Some(win) = existing {
            let clamped = level.clamp(0.0, 1.0);
            // Walk the widget tree to find the progress bar
            walk_indicator(win.upcast_ref::<gtk4::Widget>(), &mut |w| {
                if let Ok(pb) = w.clone().downcast::<gtk4::ProgressBar>() {
                    pb.set_fraction(clamped);
                }
            });
        }
    }

    /// Show the latest live-caption preview (`PartialTranscript`) in the
    /// indicator, growing it once to the bounded caption size.
    ///
    /// Preview only — the authoritative text arrives via
    /// `TranscriptionReady`. Updates an *existing* indicator only: a late
    /// partial racing a stop/cancel finds no window and is dropped; it must
    /// never (re)open the indicator.
    pub fn update_caption(app: &adw::Application, raw: &str) {
        let existing = app
            .windows()
            .into_iter()
            .find(|w| w.widget_name() == "canario-indicator");
        if let Some(win) = existing {
            let text = caption_text(raw);
            if text.is_empty() {
                return;
            }
            let mut shown = false;
            walk_indicator(win.upcast_ref::<gtk4::Widget>(), &mut |w| {
                if w.widget_name() == "canario-caption-label" {
                    if let Ok(label) = w.clone().downcast::<gtk4::Label>() {
                        // Plain text: set_text renders the transcript
                        // literally — no markup or accelerator parsing.
                        label.set_text(&text);
                        label.set_visible(true);
                        shown = true;
                    }
                }
            });
            if shown {
                // Live while mapped: for a non-resizable window a new
                // default size takes effect on the next size negotiation.
                win.set_default_size(CAPTION_WINDOW_WIDTH, CAPTION_WINDOW_HEIGHT);
            }
        }
    }

    /// Clear the live caption and return the indicator to its starting size.
    /// Called on stop/cancel, before the window is hidden.
    pub fn clear_caption(app: &adw::Application) {
        let existing = app
            .windows()
            .into_iter()
            .find(|w| w.widget_name() == "canario-indicator");
        if let Some(win) = existing {
            walk_indicator(win.upcast_ref::<gtk4::Widget>(), &mut |w| {
                if w.widget_name() == "canario-caption-label" {
                    if let Ok(label) = w.clone().downcast::<gtk4::Label>() {
                        label.set_text("");
                        label.set_visible(false);
                    }
                }
            });
            win.set_default_size(WINDOW_DEFAULT_WIDTH, WINDOW_DEFAULT_HEIGHT);
        }
    }
}

/// Visit every widget in the indicator tree, including the root window's
/// child. Restricting recursion to boxes would stop at the window itself.
fn walk_indicator(widget: &gtk4::Widget, f: &mut dyn FnMut(&gtk4::Widget)) {
    f(widget);
    let mut child = widget.first_child();
    while let Some(c) = child {
        walk_indicator(&c, f);
        child = c.next_sibling();
    }
}

/// Normalize an untrusted partial transcript for the caption label.
///
/// - Control characters and all whitespace (including hard line and
///   paragraph breaks) collapse to single spaces: `gtk_label_set_lines`
///   caps lines *per paragraph*, so embedded breaks would defeat the
///   3-line bound — and ASR output is a word stream where they carry
///   no meaning. Reflowed plain text is the correct rendering here.
/// - Leading/trailing whitespace is dropped.
/// - Length is capped at [`CAPTION_MAX_CHARS`] chars.
fn caption_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(CAPTION_MAX_CHARS * 4));
    let mut kept = 0usize;
    let mut pending_space = false;
    for c in raw.chars() {
        if kept >= CAPTION_MAX_CHARS {
            break;
        }
        if c.is_whitespace() || c.is_control() {
            pending_space = true;
            continue;
        }
        if pending_space && kept > 0 {
            // Do not leave a trailing space or exceed the cap by appending
            // both a separator and a character with only one slot left.
            if kept + 1 >= CAPTION_MAX_CHARS {
                break;
            }
            out.push(' ');
            kept += 1;
        }
        pending_space = false;
        out.push(c);
        kept += 1;
    }
    out
}

fn build_indicator_window(app: &adw::Application) -> gtk4::Window {
    let win = gtk4::Window::new();
    win.set_widget_name("canario-indicator");
    win.set_title(Some("Canario — Recording"));
    win.set_default_size(WINDOW_DEFAULT_WIDTH, WINDOW_DEFAULT_HEIGHT);
    win.set_decorated(false);
    win.set_resizable(false);
    // No focus steal: the indicator is passive UI; it must never pull
    // keyboard focus away from the app receiving the transcription.
    win.set_can_focus(false);
    win.set_focusable(false);

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

    // Live caption preview — hidden until the first partial arrives, so
    // the window keeps its 240x80 starting size; `update_caption` shows it
    // and grows the window to the bounded caption size.
    let caption = gtk4::Label::new(None);
    caption.set_widget_name("canario-caption-label");
    caption.set_wrap(true);
    // Bounded geometry, independent of the text: wrapping + the 3-line cap
    // + ellipsize bound the height; width_chars/max_width_chars bound the
    // width request (see CAPTION_WIDTH_CHARS above).
    caption.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    caption.set_lines(CAPTION_MAX_LINES);
    caption.set_width_chars(CAPTION_WIDTH_CHARS);
    caption.set_max_width_chars(CAPTION_MAX_WIDTH_CHARS);
    caption.set_xalign(0.0);
    // Passive like the window: never selectable/focusable.
    caption.set_can_focus(false);
    caption.set_selectable(false);
    caption.set_visible(false);
    caption.add_css_class("caption");
    caption.add_css_class("dim-label");

    container.append(&top_row);
    container.append(&level_bar);
    container.append(&caption);

    win.set_child(Some(&container));
    win.set_application(Some(app));

    win
}

#[cfg(test)]
mod tests {
    use super::caption_text;

    #[test]
    fn collapses_line_and_paragraph_breaks_into_spaces() {
        assert_eq!(
            caption_text("first line\nsecond line\r\nthird\u{2028}para\u{2029}end"),
            "first line second line third para end"
        );
    }

    #[test]
    fn strips_control_characters_and_whitespace_runs() {
        assert_eq!(caption_text("a\u{0}\u{1}b   \t c\u{7}d"), "a b c d");
    }

    #[test]
    fn trims_leading_and_trailing_whitespace() {
        assert_eq!(caption_text("  \n\t hello world \n "), "hello world");
    }

    #[test]
    fn bounds_untrusted_length() {
        let text = caption_text(&"a".repeat(100_000));
        assert_eq!(text.chars().count(), super::CAPTION_MAX_CHARS);
        let text = caption_text(&format!("{} b", "a".repeat(super::CAPTION_MAX_CHARS - 1)));
        assert!(text.chars().count() <= super::CAPTION_MAX_CHARS);
        assert!(!text.ends_with(' '));
    }

    #[test]
    fn control_only_partial_is_empty() {
        assert_eq!(caption_text("\n\r\t\u{0}\u{7} \u{a0}"), "");
    }

    #[test]
    fn preserves_unicode_text() {
        assert_eq!(
            caption_text("¿Cómo estás? — naïve café 日本語"),
            "¿Cómo estás? — naïve café 日本語"
        );
    }

    #[test]
    #[ignore = "requires a graphical display"]
    fn indicator_updates_widgets_and_drops_late_captions() {
        use super::*;

        adw::init().unwrap();
        let app = adw::Application::new(None, gio::ApplicationFlags::NON_UNIQUE);
        app.register(None::<&gio::Cancellable>).unwrap();
        RecordingIndicator::show(&app);
        let win = app
            .windows()
            .into_iter()
            .find(|w| w.widget_name() == "canario-indicator")
            .unwrap();
        let container = win.child().unwrap();
        let caption = container
            .last_child()
            .unwrap()
            .downcast::<gtk4::Label>()
            .unwrap();
        let progress = container
            .first_child()
            .unwrap()
            .next_sibling()
            .unwrap()
            .downcast::<gtk4::ProgressBar>()
            .unwrap();
        assert!(!caption.is_visible());
        assert!(!win.is_focusable());

        RecordingIndicator::update_caption(&app, "<b>Hello</b>\nworld");
        assert_eq!(caption.text(), "<b>Hello</b> world");
        assert!(caption.is_visible());
        assert!(!caption.uses_markup());
        assert_eq!(win.default_width(), CAPTION_WINDOW_WIDTH);
        RecordingIndicator::update_level(&app, 0.75);
        assert_eq!(progress.fraction(), 0.75);

        RecordingIndicator::clear_caption(&app);
        assert_eq!(caption.text(), "");
        assert!(!caption.is_visible());
        assert_eq!(win.default_width(), WINDOW_DEFAULT_WIDTH);
        RecordingIndicator::hide(&app);
        RecordingIndicator::update_caption(&app, "late partial");
        assert!(app.windows().is_empty());
    }
}
