/// Model manager widget — download/delete models with progress bar.
///
/// The widget registers its status/progress widgets in a thread-local slot so
/// that `Event::ModelDownload*` events (dispatched from the main loop in
/// app.rs) can update them directly — no widget-tree walking by name.
///
/// Custom models are local-only: the download/delete actions stay hidden and
/// the status reflects whether the configured custom files all exist
/// (`joiner.int8.onnx` next to the encoder included — see
/// `AppConfig::model_paths`).
use std::cell::RefCell;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use canario_core::{Canario, ModelVariant};

pub struct ModelManagerWidget {
    pub widget: adw::ActionRow,
}

/// Direct references to the download-status widgets of the currently open
/// settings window. GTK runs everything on the main thread, so a
/// thread-local is safe here. Holds a `Canario` clone so the module-level
/// event callbacks can re-derive the right state (e.g. a download that
/// finishes after the user switched to the Custom variant).
struct DownloadUi {
    canario: Canario,
    status_label: gtk4::Label,
    download_btn: gtk4::Button,
    delete_btn: gtk4::Button,
    progress_bar: gtk4::ProgressBar,
}

thread_local! {
    static DOWNLOAD_UI: RefCell<Option<DownloadUi>> = const { RefCell::new(None) };
}

impl DownloadUi {
    fn custom(&self) -> bool {
        self.canario.config().model == ModelVariant::Custom
    }

    /// Which actions are offered for the current config. Custom model
    /// files are user-owned local files — never download, never delete.
    fn sync_actions(&self) {
        let downloading = self.canario.is_downloading();
        self.download_btn.set_sensitive(!downloading);
        self.delete_btn.set_sensitive(!downloading);
        if self.custom() {
            self.download_btn.set_visible(false);
            self.delete_btn.set_visible(false);
        } else {
            let ready = self.canario.is_model_downloaded();
            self.download_btn.set_visible(!ready);
            self.delete_btn.set_visible(ready);
        }
    }

    /// Status text + actions for the current config.
    fn sync_status(&self) {
        let ready = self.canario.is_model_downloaded();
        if self.custom() {
            // No download exists for Custom — "missing files" is the
            // actionable state (custom_*_path(s) unset or not on disk).
            let status = if ready {
                "✅ Ready"
            } else {
                "⚠ Missing files"
            };
            self.status_label.set_label(status);
        } else {
            let status = if ready {
                "✅ Ready"
            } else {
                "❌ Not downloaded"
            };
            self.status_label.set_label(status);
        }
        self.sync_actions();
    }
}

/// Update download progress (0.0 – 1.0). No-op if no settings window is open.
pub fn download_progress(progress: f64) {
    DOWNLOAD_UI.with(|ui| {
        if let Some(ui) = ui.borrow().as_ref() {
            ui.progress_bar.set_visible(!ui.custom());
            ui.progress_bar.set_fraction(progress);
            ui.progress_bar.set_show_text(true);
        }
    });
}

/// Mark the model as downloaded. No-op if no settings window is open.
pub fn download_complete() {
    DOWNLOAD_UI.with(|ui| {
        if let Some(ui) = ui.borrow().as_ref() {
            ui.progress_bar.set_fraction(1.0);
            ui.progress_bar.set_visible(false);
            ui.download_btn.set_sensitive(true);
            // Re-derive from the current config — covers the case where the
            // user switched variants (e.g. to Custom) mid-download.
            ui.sync_status();
        }
    });
}

/// Mark the model download as failed. No-op if no settings window is open.
pub fn download_failed(err: &str) {
    DOWNLOAD_UI.with(|ui| {
        if let Some(ui) = ui.borrow().as_ref() {
            ui.progress_bar.set_visible(false);
            ui.download_btn.set_sensitive(true);
            if ui.custom() || ui.canario.is_model_downloaded() {
                // The selection moved on while the download ran (to Custom,
                // or to a variant already on disk) — the stale failure text
                // would mislabel the current selection, so re-derive instead.
                ui.sync_status();
            } else {
                ui.sync_actions();
                ui.status_label.set_label(&format!("❌ Failed: {}", err));
            }
        }
    });
}

impl ModelManagerWidget {
    pub fn new(canario: &Canario) -> Self {
        let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let inner_row = adw::ActionRow::builder().title("Model Status").build();

        let status_label = gtk4::Label::new(None);
        status_label.add_css_class("dim-label");
        status_label.set_valign(gtk4::Align::Center);

        let download_btn = gtk4::Button::new();
        download_btn.set_icon_name("folder-download-symbolic");
        download_btn.add_css_class("flat");
        download_btn.set_tooltip_text(Some("Download model"));
        download_btn.set_valign(gtk4::Align::Center);

        let delete_btn = gtk4::Button::new();
        delete_btn.set_icon_name("user-trash-symbolic");
        delete_btn.add_css_class("flat");
        delete_btn.set_tooltip_text(Some("Delete model"));
        delete_btn.set_valign(gtk4::Align::Center);

        inner_row.add_suffix(&status_label);
        inner_row.add_suffix(&download_btn);
        inner_row.add_suffix(&delete_btn);

        let progress_bar = gtk4::ProgressBar::new();
        progress_bar.set_fraction(0.0);
        progress_bar.set_visible(false);
        progress_bar.set_show_text(true);
        progress_bar.add_css_class("osd");
        progress_bar.set_margin_top(4);
        progress_bar.set_margin_bottom(4);

        outer.append(&inner_row);
        outer.append(&progress_bar);

        let ui = DownloadUi {
            canario: canario.clone(),
            status_label,
            download_btn,
            delete_btn,
            progress_bar,
        };

        // Initial state
        ui.sync_status();

        // Download. (Unreachable for Custom — the button stays hidden and
        // the core rejects it — but guard anyway.)
        let c = ui.canario.clone();
        let status_dl = ui.status_label.clone();
        let dl_btn = ui.download_btn.clone();
        let pb_dl = ui.progress_bar.clone();
        ui.download_btn.connect_clicked(move |_btn| {
            if c.config().model == ModelVariant::Custom || c.is_model_downloaded() {
                return;
            }
            status_dl.set_label("⬇ Downloading…");
            pb_dl.set_visible(true);
            pb_dl.set_fraction(0.0);
            dl_btn.set_sensitive(false);
            if let Err(error) = c.download_model() {
                download_failed(&error.to_string());
            } else {
                DOWNLOAD_UI.with(|slot| {
                    if let Some(ui) = slot.borrow().as_ref() {
                        ui.sync_actions();
                    }
                });
            }
            // Progress and completion arrive as Event::ModelDownload* and are
            // dispatched to the registered widgets via download_progress()/
            // download_complete()/download_failed() above (see app.rs).
        });

        // Delete. Custom files are user-owned — never deleted through here.
        let c = ui.canario.clone();
        let status_del = ui.status_label.clone();
        let dl_btn_del = ui.download_btn.clone();
        ui.delete_btn.connect_clicked(move |btn| {
            if c.config().model == ModelVariant::Custom {
                return;
            }
            if let Err(error) = c.delete_model() {
                status_del.set_label(&format!("❌ Failed: {}", error));
                return;
            }
            status_del.set_label("❌ Not downloaded");
            dl_btn_del.set_visible(true);
            dl_btn_del.set_sensitive(true);
            btn.set_visible(false);
        });

        // Register the download-status widgets so Event::ModelDownload* events
        // can reach them directly (see app.rs). Cleared when this widget is
        // destroyed (e.g. settings window closed for real).
        DOWNLOAD_UI.with(|slot| {
            *slot.borrow_mut() = Some(ui);
        });
        inner_row.connect_destroy(|_| {
            DOWNLOAD_UI.with(|slot| {
                slot.borrow_mut().take();
            });
        });

        let row = adw::ActionRow::new();
        row.set_title("");
        row.set_child(Some(&outer));

        Self { widget: row }
    }
}

/// Re-derive the status/actions from the current config. Call whenever the
/// model variant or a `custom_*_path` changes, so readiness (and the hidden
/// download/delete actions for Custom) track the core. No-op if no settings
/// window is open.
pub fn refresh() {
    DOWNLOAD_UI.with(|slot| {
        if let Some(ui) = slot.borrow().as_ref() {
            ui.sync_status();
            // A stale progress bar (e.g. left over from a finished download
            // before a variant switch) shouldn't linger.
            if ui.custom() || !ui.canario.is_downloading() {
                ui.progress_bar.set_visible(false);
            }
        }
    });
}
