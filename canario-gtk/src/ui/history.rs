/// Transcription history browser widget.
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use canario_core::Canario;

pub struct HistoryWidget {
    pub group: adw::PreferencesGroup,
    list_box: gtk4::ListBox,
}

impl HistoryWidget {
    pub fn new(canario: &Canario) -> Self {
        let group = adw::PreferencesGroup::new();
        group.set_title("History");

        let clear_btn = gtk4::Button::with_label("Clear All");
        clear_btn.add_css_class("destructive-action");
        let count_label = gtk4::Label::new(Some(&format!("{} entries", canario.history_count())));
        count_label.add_css_class("dim-label");

        let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        btn_box.append(&clear_btn);
        btn_box.append(&count_label);

        let header_row = adw::ActionRow::new();
        header_row.set_title("Recent Transcriptions");
        header_row.add_suffix(&btn_box);
        group.add(&header_row);

        let scrolled = gtk4::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_min_content_height(200);
        scrolled.set_max_content_height(400);
        scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);

        let list_box = gtk4::ListBox::new();
        list_box.set_widget_name("canario-history-list");
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk4::SelectionMode::None);
        scrolled.set_child(Some(&list_box));

        let list_row = adw::ActionRow::new();
        list_row.set_child(Some(&scrolled));
        group.add(&list_row);

        // Populate
        let entries = canario.recent_history(50);
        populate_list(&list_box, &entries, canario);

        // Clear all
        let c = canario.clone();
        let lb = list_box.clone();
        let cl = count_label.clone();
        clear_btn.connect_clicked(move |_| {
            c.clear_history();
            populate_empty(&lb);
            cl.set_text("0 entries");
        });

        Self { group, list_box }
    }
}

fn populate_list(list_box: &gtk4::ListBox, entries: &[canario_core::HistoryEntry], _canario: &Canario) {
    while let Some(child) = list_box.first_child() { list_box.remove(&child); }

    if entries.is_empty() {
        populate_empty(list_box);
        return;
    }

    for entry in entries {
        let row = adw::ActionRow::builder()
            .title(&truncate(&entry.text, 80))
            .subtitle(&format!(
                "{}  •  {:.1}s",
                entry.timestamp.with_timezone(&chrono::Local).format("%b %d, %H:%M"),
                entry.duration_secs,
            ))
            .build();

        let text = entry.text.clone();
        let copy_btn = gtk4::Button::new();
        copy_btn.set_icon_name("edit-copy-symbolic");
        copy_btn.add_css_class("flat");
        copy_btn.set_valign(gtk4::Align::Center);
        copy_btn.connect_clicked(move |_| {
            let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(&text));
        });
        row.add_suffix(&copy_btn);
        list_box.append(&row);
    }
}

fn populate_empty(list_box: &gtk4::ListBox) {
    while let Some(child) = list_box.first_child() { list_box.remove(&child); }
    let empty = adw::ActionRow::builder().title("No transcriptions yet").build();
    empty.add_css_class("dim-label");
    list_box.append(&empty);
}

fn truncate(text: &str, max_len: usize) -> String {
    if text.len() <= max_len { text.to_string() }
    else { text.chars().take(max_len).collect::<String>() + "…" }
}
