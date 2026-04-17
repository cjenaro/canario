/// Model manager widget — download/delete models with progress bar.
///
/// Downloads run in a background thread, UI updates happen on the
/// GTK main thread via glib::timeout_add_local polling.
use std::sync::{Arc, Mutex};

use glib::ControlFlow;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::config::AppConfig;
use crate::inference;
use crate::ui::AppState;

pub struct ModelManagerWidget {
    pub widget: adw::ActionRow,
}

impl ModelManagerWidget {
    pub fn new(state: Arc<Mutex<AppState>>) -> Self {
        let row = adw::ActionRow::builder()
            .title("Model Status")
            .build();

        let status_label = gtk4::Label::new(None);
        status_label.add_css_class("dim-label");
        status_label.set_valign(gtk4::Align::Center);
        row.add_suffix(&status_label);

        // Progress bar — goes inside the row's child area, below the title
        let progress_bar = gtk4::ProgressBar::new();
        progress_bar.set_fraction(0.0);
        progress_bar.set_visible(false);
        progress_bar.set_show_text(true);
        progress_bar.add_css_class("osd");
        progress_bar.set_margin_top(4);
        progress_bar.set_margin_bottom(4);

        // We need the progress bar below the row content, so use a vertical box
        let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let inner_row = adw::ActionRow::builder()
            .title("Model Status")
            .build();

        // Small compact buttons with icons
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

        let status_label2 = gtk4::Label::new(None);
        status_label2.add_css_class("dim-label");
        status_label2.set_valign(gtk4::Align::Center);

        inner_row.add_suffix(&status_label2);
        inner_row.add_suffix(&download_btn);
        inner_row.add_suffix(&delete_btn);

        outer.append(&inner_row);
        outer.append(&progress_bar);

        // ── Initial state ────────────────────────────────────────────
        let downloaded = {
            let s = state.lock().unwrap();
            s.config.is_model_downloaded()
        };
        if downloaded {
            status_label2.set_label("✅ Ready");
            download_btn.set_visible(false);
            delete_btn.set_visible(true);
        } else {
            status_label2.set_label("❌ Not downloaded");
            download_btn.set_visible(true);
            delete_btn.set_visible(false);
        }

        // ── Download handler ─────────────────────────────────────────
        let status_for_dl = status_label2.clone();
        let status_for_del = status_label2.clone();
        let dl_btn_ref = download_btn.clone();
        let del_for_dl = delete_btn.clone();

        let state_dl = state.clone();
        download_btn.connect_clicked(move |_btn| {
            let config = {
                let s = state_dl.lock().unwrap();
                s.config.clone()
            };

            if config.is_model_downloaded() {
                return;
            }

            status_for_dl.set_label("⬇ Downloading…");
            progress_bar.set_visible(true);
            progress_bar.set_fraction(0.0);
            dl_btn_ref.set_sensitive(false);

            let model_dir = config.local_model_dir();
            let repo = config.model_hf_repo().to_string();

            let (result_tx, result_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();
            let (progress_tx, progress_rx) = std::sync::mpsc::channel::<f64>();

            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let result = rt.block_on(async {
                    inference::download_model_with_progress(
                        &model_dir, &repo, &progress_tx,
                    ).await
                });
                let _ = result_tx.send(result);
            });

            let status = status_for_dl.clone();
            let progress = progress_bar.clone();
            let dl_btn = dl_btn_ref.clone();
            let del_btn = del_for_dl.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
                // Check for progress updates
                while let Ok(p) = progress_rx.try_recv() {
                    progress.set_fraction(p);
                    progress.set_show_text(true);
                }

                match result_rx.try_recv() {
                    Ok(Ok(())) => {
                        status.set_label("✅ Downloaded");
                        progress.set_fraction(1.0);
                        dl_btn.set_visible(false);
                        dl_btn.set_sensitive(true);
                        del_btn.set_visible(true);
                        ControlFlow::Break
                    }
                    Ok(Err(e)) => {
                        status.set_label("❌ Download failed");
                        progress.set_visible(false);
                        dl_btn.set_sensitive(true);
                        tracing::error!("Model download failed: {}", e);
                        ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ControlFlow::Continue
                    }
                    Err(_) => {
                        status.set_label("❌ Download interrupted");
                        progress.set_visible(false);
                        dl_btn.set_sensitive(true);
                        ControlFlow::Break
                    }
                }
            });
        });

        // ── Delete handler ────────────────────────────────────────────
        let state_del = state.clone();
        delete_btn.connect_clicked(move |btn| {
            let config = {
                let s = state_del.lock().unwrap();
                s.config.clone()
            };

            let model_dir = config.local_model_dir();
            if model_dir.exists() {
                if let Err(e) = std::fs::remove_dir_all(&model_dir) {
                    tracing::error!("Failed to delete model: {}", e);
                } else {
                    tracing::info!("Model deleted: {:?}", model_dir);
                }
            }

            let vad_path = AppConfig::models_dir().join("silero_vad.onnx");
            if vad_path.exists() {
                let _ = std::fs::remove_file(&vad_path);
            }

            status_for_del.set_label("❌ Not downloaded");
            download_btn.set_visible(true);
            download_btn.set_sensitive(true);
            btn.set_visible(false);
        });

        // Return the outer box wrapped as the widget
        // Since the parent expects an ActionRow, let's reuse the row
        // Actually we should just use outer as the widget — but the type is ActionRow.
        // Simplest: just use the row we already have and add everything to it.
        // The issue is we can't easily put a progress bar below. Let's use the
        // row as-is with progress bar as a suffix.

        // Clear the original row and rebuild
        row.set_title("");
        row.set_child(Some(&outer));

        Self { widget: row }
    }
}
