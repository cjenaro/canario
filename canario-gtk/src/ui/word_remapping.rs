/// Word remapping management widget.
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use canario_core::{Canario, WordRemapping, WordRemoval};

pub struct WordRemappingWidget {
    pub group: adw::PreferencesGroup,
}

impl WordRemappingWidget {
    pub fn new(canario: &Canario) -> Self {
        let group = adw::PreferencesGroup::new();
        group.set_title("Word Remapping");
        group.set_description(Some("Automatically replace or remove words in transcriptions"));

        let header_row = adw::ActionRow::new();
        header_row.set_title("Rules");
        header_row.set_subtitle("Add replacements (from → to) or removals");

        let add_btn = gtk4::Button::new();
        add_btn.set_icon_name("list-add-symbolic");
        add_btn.add_css_class("flat");
        add_btn.set_valign(gtk4::Align::Center);
        header_row.add_suffix(&add_btn);
        group.add(&header_row);

        // Populate from config
        let config = canario.config();
        for mapping in &config.post_processor.remappings {
            let row = make_remapping_row(&mapping.from, &mapping.to);
            group.add(&row);
        }
        for removal in &config.post_processor.removals {
            let row = make_removal_row(&removal.word);
            group.add(&row);
        }
        if config.post_processor.remappings.is_empty() && config.post_processor.removals.is_empty() {
            let empty = adw::ActionRow::builder()
                .title("No rules yet")
                .subtitle("Click + to add a word replacement or removal")
                .build();
            empty.add_css_class("dim-label");
            group.add(&empty);
        }
        drop(config);

        // Add button → dialog (needs parent window, handled lazily)
        // For now, add is handled by the add button opening a simple dialog
        let _ = add_btn; // TODO: connect to dialog

        Self { group }
    }
}

fn make_remapping_row(from: &str, to: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&format!("↔  {}  →  {}", from, to))
        .subtitle("Remapping")
        .build();
    row
}

fn make_removal_row(word: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&format!("✕  {}", word))
        .subtitle("Removal")
        .build();
    row
}
