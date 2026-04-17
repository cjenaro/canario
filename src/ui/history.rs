/// Transcription history browser widget for the settings window.
///
/// Shows recent transcriptions in a scrolled list with search,
/// copy-to-clipboard, delete, and clear-all actions.

use std::sync::{Arc, Mutex};

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::history::History;
use crate::ui::AppState;

pub struct HistoryWidget {
    pub group: adw::PreferencesGroup,
    _state: Arc<Mutex<AppState>>,
    history: Arc<Mutex<History>>,
    list_box: gtk4::ListBox,
    _search_entry: gtk4::SearchEntry,
}

impl HistoryWidget {
    pub fn new(state: Arc<Mutex<AppState>>, history: Arc<Mutex<History>>) -> Self {
        let group = adw::PreferencesGroup::new();
        group.set_title("History");

        // ── Search bar ──────────────────────────────────────────────
        let search_entry = gtk4::SearchEntry::new();
        search_entry.set_hexpand(true);
        search_entry.set_placeholder_text(Some("Search transcriptions…"));

        // ── Action buttons ──────────────────────────────────────────
        let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);

        let clear_btn = gtk4::Button::with_label("Clear All");
        clear_btn.add_css_class("destructive-action");

        let copy_count_label = gtk4::Label::new(None);
        copy_count_label.add_css_class("dim-label");

        btn_box.append(&clear_btn);
        btn_box.append(&copy_count_label);

        // ── Header row ──────────────────────────────────────────────
        let header_row = adw::ActionRow::new();
        header_row.set_title("Recent Transcriptions");
        header_row.add_suffix(&btn_box);

        group.add(&header_row);

        // ── Scrolled list ───────────────────────────────────────────
        let scrolled = gtk4::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_min_content_height(200);
        scrolled.set_max_content_height(400);
        scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);

        let list_box = gtk4::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk4::SelectionMode::None);

        scrolled.set_child(Some(&list_box));

        // Wrap the scrolled list in an ActionRow so it can go in the PreferencesGroup
        let list_row = adw::ActionRow::new();
        list_row.set_child(Some(&scrolled));

        group.add(&list_row);

        let widget = Self {
            group,
            _state: state,
            history,
            list_box,
            _search_entry: search_entry.clone(),
        };

        widget.refresh_list(None);

        // ── Search handler ──────────────────────────────────────────
        let history_search = widget.history.clone();
        let list_box_search = widget.list_box.clone();
        let search_text = widget._search_entry.clone();
        search_entry.connect_search_changed(move |_| {
            let query = search_text.text().to_string();
            let query = if query.is_empty() {
                None
            } else {
                Some(query)
            };
            populate_list(&list_box_search, &history_search, query.as_deref());
        });

        // ── Clear all handler ───────────────────────────────────────
        let history_clear = widget.history.clone();
        let list_box_clear = widget.list_box.clone();
        let count_label_clear = copy_count_label.clone();
        clear_btn.connect_clicked(move |_| {
            let mut h = history_clear.lock().unwrap();
            h.clear();
            drop(h);
            populate_list(&list_box_clear, &history_clear, None);
            update_count(&count_label_clear, &history_clear);
        });

        // Set initial count
        update_count(&copy_count_label, &widget.history);

        widget
    }

    /// Refresh the list from history (no filter).
    fn refresh_list(&self, query: Option<&str>) {
        populate_list(&self.list_box, &self.history, query);
    }
}

/// Populate the list box with history entries.
fn populate_list(
    list_box: &gtk4::ListBox,
    history: &Arc<Mutex<History>>,
    query: Option<&str>,
) {
    // Remove all existing children
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let entries = {
        let h = history.lock().unwrap();
        match query {
            Some(q) => h.search_owned(q),
            None => h.recent_owned(50),
        }
    };

    if entries.is_empty() {
        let empty_row = adw::ActionRow::builder()
            .title(match query {
                Some(_) => "No matching transcriptions",
                None => "No transcriptions yet",
            })
            .build();
        empty_row.add_css_class("dim-label");
        list_box.append(&empty_row);
        return;
    }

    for entry in entries {
        let row = adw::ActionRow::builder()
            .title(&truncate(&entry.text, 80))
            .subtitle(&format!(
                "{}  •  {:.1}s",
                entry.timestamp
                    .with_timezone(&chrono::Local)
                    .format("%b %d, %H:%M"),
                entry.duration_secs,
            ))
            .build();

        // Copy button
        let text = entry.text.clone();
        let copy_btn = gtk4::Button::new();
        copy_btn.set_icon_name("edit-copy-symbolic");
        copy_btn.add_css_class("flat");
        copy_btn.set_valign(gtk4::Align::Center);
        copy_btn.set_tooltip_text(Some("Copy to clipboard"));
        copy_btn.connect_clicked(move |_| {
            if let Err(e) = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(&text)) {
                tracing::warn!("Failed to copy to clipboard: {}", e);
            }
        });
        row.add_suffix(&copy_btn);

        // Delete button
        let id = entry.id.clone();
        let history_delete = history.clone();
        let list_box_delete = list_box.clone();
        let row_clone = row.clone();
        let delete_btn = gtk4::Button::new();
        delete_btn.set_icon_name("user-trash-symbolic");
        delete_btn.add_css_class("flat");
        delete_btn.set_valign(gtk4::Align::Center);
        delete_btn.set_tooltip_text(Some("Delete"));
        delete_btn.connect_clicked(move |_| {
            let mut h = history_delete.lock().unwrap();
            h.delete(&id);
            drop(h);
            // Remove this row from the list
            list_box_delete.remove(&row_clone);
        });
        row.add_suffix(&delete_btn);

        list_box.append(&row);
    }
}

/// Update the count label.
fn update_count(label: &gtk4::Label, history: &Arc<Mutex<History>>) {
    let count = history.lock().unwrap().entries.len();
    label.set_text(&format!("{} entries", count));
}

/// Truncate text for display.
fn truncate(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(max_len).collect();
        truncated + "…"
    }
}
