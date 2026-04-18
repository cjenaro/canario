/// Model manager widget — download/delete models with progress bar.
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use canario_core::Canario;

pub struct ModelManagerWidget {
    pub widget: adw::ActionRow,
}

impl ModelManagerWidget {
    pub fn new(canario: &Canario) -> Self {
        let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let inner_row = adw::ActionRow::builder().title("Model Status").build();

        let status_label = gtk4::Label::new(None);
        status_label.set_widget_name("model-status-label");
        status_label.add_css_class("dim-label");
        status_label.set_valign(gtk4::Align::Center);

        let download_btn = gtk4::Button::new();
        download_btn.set_widget_name("model-download-btn");
        download_btn.set_icon_name("folder-download-symbolic");
        download_btn.add_css_class("flat");
        download_btn.set_tooltip_text(Some("Download model"));
        download_btn.set_valign(gtk4::Align::Center);

        let delete_btn = gtk4::Button::new();
        delete_btn.set_widget_name("model-delete-btn");
        delete_btn.set_icon_name("user-trash-symbolic");
        delete_btn.add_css_class("flat");
        delete_btn.set_tooltip_text(Some("Delete model"));
        delete_btn.set_valign(gtk4::Align::Center);

        inner_row.add_suffix(&status_label);
        inner_row.add_suffix(&download_btn);
        inner_row.add_suffix(&delete_btn);

        let progress_bar = gtk4::ProgressBar::new();
        progress_bar.set_widget_name("model-download-progress");
        progress_bar.set_fraction(0.0);
        progress_bar.set_visible(false);
        progress_bar.set_show_text(true);
        progress_bar.add_css_class("osd");
        progress_bar.set_margin_top(4);
        progress_bar.set_margin_bottom(4);

        outer.append(&inner_row);
        outer.append(&progress_bar);

        // Initial state
        let downloaded = canario.is_model_downloaded();
        if downloaded {
            status_label.set_label("✅ Ready");
            download_btn.set_visible(false);
            delete_btn.set_visible(true);
        } else {
            status_label.set_label("❌ Not downloaded");
            download_btn.set_visible(true);
            delete_btn.set_visible(false);
        }

        // Download
        let c = canario.clone();
        let status_dl = status_label.clone();
        let dl_btn = download_btn.clone();
        let _del_btn = delete_btn.clone();
        download_btn.connect_clicked(move |_btn| {
            if c.is_model_downloaded() { return; }
            status_dl.set_label("⬇ Downloading…");
            progress_bar.set_visible(true);
            progress_bar.set_fraction(0.0);
            dl_btn.set_sensitive(false);
            let _ = c.download_model();
            // Progress and completion are handled via Event::ModelDownloadProgress/Complete
            // in app.rs, which updates the progress bar by walking the widget tree.
        });

        // Delete
        let c = canario.clone();
        let status_del = status_label.clone();
        delete_btn.connect_clicked(move |btn| {
            let _ = c.delete_model();
            status_del.set_label("❌ Not downloaded");
            download_btn.set_visible(true);
            download_btn.set_sensitive(true);
            btn.set_visible(false);
        });

        let row = adw::ActionRow::new();
        row.set_title("");
        row.set_child(Some(&outer));

        Self { widget: row }
    }
}
