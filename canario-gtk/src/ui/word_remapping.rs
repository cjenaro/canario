/// Word remapping management widget.
#[allow(deprecated)]
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use std::rc::Rc;
use std::cell::RefCell;

use canario_core::{Canario, WordRemapping, WordRemoval};

pub struct WordRemappingWidget {
    pub group: adw::PreferencesGroup,
    /// Widgets added to the group via `group.add()`. Tracked so we can
    /// remove only our rows — AdwPreferencesGroup's internal GtkBox cannot
    /// be removed via `group.remove()`, and `first_child()` would return it.
    added_rows: RefCell<Vec<gtk4::Widget>>,
    canario: Canario,
}

impl WordRemappingWidget {
    pub fn new(canario: &Canario) -> Rc<Self> {
        let group = adw::PreferencesGroup::new();
        group.set_title("Word Remapping");
        group.set_description(Some("Automatically replace or remove words in transcriptions"));

        let widget = Rc::new(Self {
            group,
            added_rows: RefCell::new(Vec::new()),
            canario: canario.clone(),
        });

        widget.rebuild();

        widget
    }

    /// Clear and rebuild the entire remapping group from config.
    fn rebuild(self: &Rc<Self>) {
        let group = &self.group;

        // Remove only widgets we previously added via group.add().
        for row in self.added_rows.borrow_mut().drain(..) {
            group.remove(&row);
        }

        // Header row with add button
        let header_row = adw::ActionRow::new();
        header_row.set_title("Rules");
        header_row.set_subtitle("Add replacements (from → to) or removals");

        let add_btn = gtk4::Button::new();
        add_btn.set_icon_name("list-add-symbolic");
        add_btn.add_css_class("flat");
        add_btn.set_valign(gtk4::Align::Center);
        header_row.add_suffix(&add_btn);
        group.add(&header_row);
        self.added_rows.borrow_mut().push(header_row.upcast::<gtk4::Widget>());

        // BUG-006: Wire the add button to show a dialog
        let w = Rc::clone(self);
        add_btn.connect_clicked(move |btn| {
            if let Some(win) = btn.root().and_then(|r| r.downcast::<gtk4::Window>().ok()) {
                show_add_remapping_dialog(&win, &w);
            }
        });

        // Populate from config
        let config = self.canario.config();
        for mapping in &config.post_processor.remappings {
            self.add_remapping_row(&mapping.from, &mapping.to);
        }
        for removal in &config.post_processor.removals {
            self.add_removal_row(&removal.word);
        }
        if config.post_processor.remappings.is_empty() && config.post_processor.removals.is_empty() {
            let empty = adw::ActionRow::builder()
                .title("No rules yet")
                .subtitle("Click + to add a word replacement or removal")
                .build();
            empty.add_css_class("dim-label");
            group.add(&empty);
            self.added_rows.borrow_mut().push(empty.upcast::<gtk4::Widget>());
        }
    }

    /// BUG-007: Row with a delete button for a remapping rule.
    fn add_remapping_row(self: &Rc<Self>, from: &str, to: &str) {
        let row = adw::ActionRow::builder()
            .title(&format!("↔  {}  →  {}", from, to))
            .subtitle("Remapping")
            .build();

        let del_btn = gtk4::Button::new();
        del_btn.set_icon_name("list-remove-symbolic");
        del_btn.add_css_class("flat");
        del_btn.set_valign(gtk4::Align::Center);
        del_btn.set_tooltip_text(Some("Remove this rule"));
        row.add_suffix(&del_btn);

        let c = self.canario.clone();
        let w = Rc::clone(self);
        let from = from.to_string();
        let to = to.to_string();
        del_btn.connect_clicked(move |_| {
            let _ = c.update_config(|cfg| {
                cfg.post_processor
                    .remappings
                    .retain(|r| r.from != from || r.to != to);
            });
            w.rebuild();
        });

        self.group.add(&row);
        self.added_rows.borrow_mut().push(row.upcast::<gtk4::Widget>());
    }

    /// BUG-007: Row with a delete button for a removal rule.
    fn add_removal_row(self: &Rc<Self>, word: &str) {
        let row = adw::ActionRow::builder()
            .title(&format!("✕  {}", word))
            .subtitle("Removal")
            .build();

        let del_btn = gtk4::Button::new();
        del_btn.set_icon_name("list-remove-symbolic");
        del_btn.add_css_class("flat");
        del_btn.set_valign(gtk4::Align::Center);
        del_btn.set_tooltip_text(Some("Remove this rule"));
        row.add_suffix(&del_btn);

        let c = self.canario.clone();
        let w = Rc::clone(self);
        let word = word.to_string();
        del_btn.connect_clicked(move |_| {
            let _ = c.update_config(|cfg| {
                cfg.post_processor.removals.retain(|r| r.word != word);
            });
            w.rebuild();
        });

        self.group.add(&row);
        self.added_rows.borrow_mut().push(row.upcast::<gtk4::Widget>());
    }
}

/// BUG-006: Dialog for adding a new word remapping or removal rule.
#[allow(deprecated)]
fn show_add_remapping_dialog(
    parent: &gtk4::Window,
    widget: &Rc<WordRemappingWidget>,
) {
    let dialog = gtk4::Window::new();
    dialog.set_transient_for(Some(parent));
    dialog.set_modal(true);
    dialog.set_title(Some("Add Word Rule"));
    dialog.set_default_size(380, 220);
    dialog.set_hide_on_close(true);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.set_margin_top(16);
    content.set_margin_bottom(16);

    // Mode selector
    let mode_label = gtk4::Label::new(Some("Type:"));
    mode_label.set_xalign(0.0);
    let mode_combo = gtk4::ComboBoxText::new();
    mode_combo.append(Some("remapping"), "Remapping (from → to)");
    mode_combo.append(Some("removal"), "Removal");
    mode_combo.set_active(Some(0));

    // From / Word entry
    let from_label = gtk4::Label::new(Some("From word:"));
    from_label.set_xalign(0.0);
    let from_entry = gtk4::Entry::new();
    from_entry.set_placeholder_text(Some("Word to replace or remove"));

    // To entry (only for remapping)
    let to_label = gtk4::Label::new(Some("To word:"));
    to_label.set_xalign(0.0);
    let to_entry = gtk4::Entry::new();
    to_entry.set_placeholder_text(Some("Replacement word"));

    let to_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    to_box.append(&to_label);
    to_box.append(&to_entry);

    // Show/hide "To" based on mode
    let to_box_clone = to_box.clone();
    mode_combo.connect_changed(move |combo| {
        #[allow(deprecated)]
        let is_remapping = combo.active_id().as_deref() == Some("remapping");
        to_box_clone.set_visible(is_remapping);
    });

    // Buttons
    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk4::Align::End);
    btn_box.set_margin_top(8);

    let cancel_btn = gtk4::Button::with_label("Cancel");
    cancel_btn.add_css_class("flat");
    let add_btn = gtk4::Button::with_label("Add Rule");
    add_btn.add_css_class("suggested-action");
    add_btn.set_sensitive(false);

    btn_box.append(&cancel_btn);
    btn_box.append(&add_btn);

    content.append(&mode_label);
    content.append(&mode_combo);
    content.append(&from_label);
    content.append(&from_entry);
    content.append(&to_box);
    content.append(&btn_box);

    dialog.set_child(Some(&content));

    // Enable "Add Rule" only when "From" entry is non-empty
    let ab = add_btn.clone();
    from_entry.connect_changed(move |entry| {
        ab.set_sensitive(!entry.text().is_empty());
    });

    // Cancel
    let dlg = dialog.clone();
    cancel_btn.connect_clicked(move |_| {
        dlg.close();
    });

    // Add Rule
    let c = widget.canario.clone();
    let w = Rc::clone(widget);
    let dlg2 = dialog.clone();
    let fe = from_entry.clone();
    let te = to_entry.clone();
    let md = mode_combo.clone();
    add_btn.connect_clicked(move |_| {
        let from = fe.text().to_string().trim().to_string();
        if from.is_empty() {
            return;
        }

        let _ = c.update_config(|cfg| {
            #[allow(deprecated)]
            let is_remapping = md.active_id().as_deref() == Some("remapping");
            if is_remapping {
                // Remapping
                let to = te.text().to_string().trim().to_string();
                if !to.is_empty() {
                    cfg.post_processor
                        .remappings
                        .push(WordRemapping { from, to });
                }
            } else {
                // Removal
                cfg.post_processor
                    .removals
                    .push(WordRemoval { word: from });
            }
        });

        w.rebuild();
        dlg2.close();
    });

    dialog.present();
}
