/// Word remapping management widget for the settings window.
///
/// Dynamic list of remappings and removals. Each entry is an
/// `adw::ActionRow` inside the `PreferencesGroup`. The "Add" button
/// opens a dialog to create new rules.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::inference::postprocess::{WordRemapping, WordRemoval};
use crate::ui::AppState;

pub struct WordRemappingWidget {
    pub group: adw::PreferencesGroup,
    /// Track dynamic row indices so we can clear them on rebuild
    rows: RefCell<Vec<gtk4::Widget>>,
    state: Arc<Mutex<AppState>>,
}

impl WordRemappingWidget {
    pub fn new(state: Arc<Mutex<AppState>>) -> Self {
        let group = adw::PreferencesGroup::new();
        group.set_title("Word Remapping");
        group.set_description(Some("Automatically replace or remove words in transcriptions"));

        // "Add" button as the group header suffix
        let add_btn = gtk4::Button::new();
        add_btn.set_icon_name("list-add-symbolic");
        add_btn.add_css_class("flat");

        // We need to put the add button somewhere visible.
        // adw::PreferencesGroup has a `header_suffix` but it's set via property.
        // Use a separate header row instead.
        let header_row = adw::ActionRow::builder()
            .title("Rules")
            .subtitle("Add replacements (from → to) or removals")
            .build();
        header_row.add_suffix(&add_btn);

        group.add(&header_row);

        let widget = Self {
            group,
            rows: RefCell::new(Vec::new()),
            state,
        };

        // Populate initial rows
        widget.rebuild_rows();

        // Add button opens a dialog to create a new rule
        let state_clone = widget.state.clone();
        let group_clone = widget.group.clone();
        let rows_clone = widget.rows.clone();
        add_btn.connect_clicked(move |_| {
            // We need a parent window for the dialog — walk up from any widget
            // The add_btn doesn't have a reliable parent window at this point.
            // Instead, find the settings window by name.
            show_add_dialog(&state_clone, &group_clone, &rows_clone);
        });

        widget
    }

    /// Remove all dynamic rows and re-add from config.
    fn rebuild_rows(&self) {
        // Remove old dynamic rows
        for row in self.rows.borrow().iter() {
            self.group.remove(row);
        }
        self.rows.borrow_mut().clear();

        let s = self.state.lock().unwrap();
        let pp = &s.config.post_processor;

        for mapping in &pp.remappings {
            let row = self.make_remapping_row(&mapping.from, &mapping.to);
            self.group.add(&row);
            self.rows.borrow_mut().push(row.upcast());
        }

        for removal in &pp.removals {
            let row = self.make_removal_row(&removal.word);
            self.group.add(&row);
            self.rows.borrow_mut().push(row.upcast());
        }

        if pp.remappings.is_empty() && pp.removals.is_empty() {
            let empty = adw::ActionRow::builder()
                .title("No rules yet")
                .subtitle("Click + to add a word replacement or removal")
                .build();
            empty.add_css_class("dim-label");
            self.group.add(&empty);
            self.rows.borrow_mut().push(empty.upcast());
        }
    }

    fn make_remapping_row(&self, from: &str, to: &str) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(&format!("↔  {}  →  {}", from, to))
            .subtitle("Remapping")
            .build();

        let state = self.state.clone();
        let from_owned = from.to_string();
        let to_owned = to.to_string();
        let delete_btn = gtk4::Button::new();
        delete_btn.set_icon_name("user-trash-symbolic");
        delete_btn.add_css_class("flat");
        delete_btn.set_valign(gtk4::Align::Center);
        delete_btn.connect_clicked(move |_| {
            let mut s = state.lock().unwrap();
            s.config.post_processor
                .remappings
                .retain(|r| r.from != from_owned || r.to != to_owned);
            let _ = s.config.save();
            // Row will be rebuilt next time settings are opened
        });
        row.add_suffix(&delete_btn);
        row
    }

    fn make_removal_row(&self, word: &str) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(&format!("✕  {}", word))
            .subtitle("Removal")
            .build();

        let state = self.state.clone();
        let word_owned = word.to_string();
        let delete_btn = gtk4::Button::new();
        delete_btn.set_icon_name("user-trash-symbolic");
        delete_btn.add_css_class("flat");
        delete_btn.set_valign(gtk4::Align::Center);
        delete_btn.connect_clicked(move |_| {
            let mut s = state.lock().unwrap();
            s.config.post_processor
                .removals
                .retain(|r| r.word != word_owned);
            let _ = s.config.save();
        });
        row.add_suffix(&delete_btn);
        row
    }
}

/// Show a dialog to add a new remapping or removal rule.
fn show_add_dialog(
    state: &Arc<Mutex<AppState>>,
    group: &adw::PreferencesGroup,
    _rows: &RefCell<Vec<gtk4::Widget>>,
) {
    // Find the parent settings window
    let root = group.root();
    let parent: Option<gtk4::Window> = root.and_then(|r| r.downcast().ok());

    let dialog = adw::Dialog::new();
    dialog.set_title("Add Word Rule");
    dialog.set_content_width(400);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
    content.set_margin_start(20);
    content.set_margin_end(20);
    content.set_margin_top(20);
    content.set_margin_bottom(20);

    // Mode selector
    let mode_row = adw::ComboRow::new();
    mode_row.set_title("Rule Type");
    let mode_list = gtk4::StringList::new(&["Remapping (replace word)", "Removal (delete word)"]);
    mode_row.set_model(Some(&mode_list));
    mode_row.set_selected(0);

    // "From" entry
    let from_row = adw::EntryRow::new();
    from_row.set_title("Word to find");

    // "To" entry
    let to_row = adw::EntryRow::new();
    to_row.set_title("Replace with");

    // Info label
    let info_label = gtk4::Label::new(Some("Case-insensitive whole-word matching"));
    info_label.add_css_class("dim-label");
    info_label.set_wrap(true);

    // Toggle "to" visibility based on mode
    let to_row_clone = to_row.clone();
    mode_row.connect_notify_local(Some("selected-item"), move |row, _| {
        to_row_clone.set_visible(row.selected() == 0);
    });

    // Add button
    let add_btn = gtk4::Button::with_label("Add Rule");
    add_btn.add_css_class("suggested-action");
    add_btn.set_halign(gtk4::Align::End);

    content.append(&mode_row);
    content.append(&from_row);
    content.append(&to_row);
    content.append(&info_label);
    content.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    content.append(&add_btn);

    dialog.set_child(Some(&content));

    // Handle add
    let state_clone = state.clone();
    let dialog_clone = dialog.clone();
    let from_row_c = from_row.clone();
    let to_row_c = to_row.clone();
    let mode_row_c = mode_row.clone();

    add_btn.connect_clicked(move |_| {
        let from_text = from_row_c.text().to_string();
        let to_text = to_row_c.text().to_string();
        let is_removal = mode_row_c.selected() == 1;

        if from_text.is_empty() {
            return;
        }

        let mut s = state_clone.lock().unwrap();
        if is_removal {
            s.config.post_processor.removals.push(WordRemoval {
                word: from_text,
            });
        } else {
            s.config.post_processor.remappings.push(WordRemapping {
                from: from_text,
                to: to_text,
            });
        }
        let _ = s.config.save();
        drop(s);

        dialog_clone.close();
    });

    if let Some(parent) = parent {
        dialog.present(&parent);
    }
}
