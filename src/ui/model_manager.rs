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
            .activatable(true)
            .build();

        let status_label = gtk4::Label::new(None);
        status_label.add_css_class("dim-label");
        row.add_suffix(&status_label);

        let progress_bar = gtk4::ProgressBar::new();
        progress_bar.set_fraction(0.0);
        progress_bar.set_visible(false);
        progress_bar.set_show_text(true);
        progress_bar.add_css_class("osd");

        let download_btn = gtk4::Button::with_label("Download");
        download_btn.add_css_class("suggested-action");
        download_btn.add_css_class("pill");

        let delete_btn = gtk4::Button::with_label("Delete");
        delete_btn.add_css_class("destructive-action");
        delete_btn.add_css_class("pill");

        let button_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        button_box.append(&download_btn);
        button_box.append(&delete_btn);
        row.add_suffix(&button_box);

        // ── Initial state ────────────────────────────────────────────
        let downloaded = {
            let s = state.lock().unwrap();
            s.config.is_model_downloaded()
        };
        if downloaded {
            status_label.set_label("✅ Ready");
            download_btn.set_visible(false);
            delete_btn.set_visible(true);
        } else {
            status_label.set_label("❌ Not downloaded");
            download_btn.set_visible(true);
            delete_btn.set_visible(false);
        }

        // ── Download handler ─────────────────────────────────────────
        // Clone labels/buttons that both closures need
        let status_for_dl = status_label.clone();
        let status_for_del = status_label.clone();
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

            status_for_dl.set_label("Downloading…");
            progress_bar.set_visible(true);
            progress_bar.set_fraction(0.0);

            let model_dir = config.local_model_dir();
            let repo = config.model_hf_repo().to_string();

            let (result_tx, result_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();

            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let result = rt.block_on(async {
                    inference::download_model(&model_dir, &repo).await
                });
                let _ = result_tx.send(result);
            });

            let status = status_for_dl.clone();
            let progress = progress_bar.clone();
            let dl_btn = dl_btn_ref.clone();
            let del_btn = del_for_dl.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                match result_rx.try_recv() {
                    Ok(Ok(())) => {
                        status.set_label("✅ Downloaded");
                        progress.set_fraction(1.0);
                        dl_btn.set_visible(false);
                        del_btn.set_visible(true);
                        ControlFlow::Break
                    }
                    Ok(Err(e)) => {
                        status.set_label("❌ Download failed");
                        progress.set_visible(false);
                        tracing::error!("Model download failed: {}", e);
                        ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        progress.pulse();
                        ControlFlow::Continue
                    }
                    Err(_) => {
                        status.set_label("❌ Download interrupted");
                        progress.set_visible(false);
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
            btn.set_visible(false);
        });

        Self { widget: row }
    }
}
