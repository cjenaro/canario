/// Model manager widget — download/delete models with progress bar.
///
/// Downloads run in a background thread, UI updates happen on the
/// GTK main thread via glib::timeout_add_local polling.
use std::sync::{Arc, Mutex};

use glib::ControlFlow;
use gtk4::prelude::*;
use libadwaita as adw;

use crate::config::AppConfig;
use crate::inference;
use crate::ui::AppState;

pub struct ModelManagerWidget {
    pub widget: adw::ActionRow,
    status_label: gtk4::Label,
    progress_bar: gtk4::ProgressBar,
}

impl ModelManagerWidget {
    pub fn new(state: Arc<Mutex<AppState>>) -> Self {
        let row = adw::ActionRow::new();
        row.set_title("Model Status");
        row.set_activatable(true);

        let status_label = gtk4::Label::new(None);
        status_label.add_css_class("dim-label");
        row.add_suffix(&status_label);

        let progress_bar = gtk4::ProgressBar::new();
        progress_bar.set_fraction(0.0);
        progress_bar.set_visible(false);
        progress_bar.set_show_text(true);
        progress_bar.add_css_class("osd");

        // Download & delete buttons
        let button_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);

        let download_btn = gtk4::Button::with_label("Download");
        download_btn.add_css_class("suggested-action");
        download_btn.add_css_class("pill");

        let delete_btn = gtk4::Button::with_label("Delete");
        delete_btn.add_css_class("destructive-action");
        delete_btn.add_css_class("pill");
        delete_btn.set_visible(false);

        button_box.append(&download_btn);
        button_box.append(&delete_btn);
        row.add_suffix(&button_box);

        // Initial state
        let mut mgr = Self {
            widget: row,
            status_label,
            progress_bar,
        };
        mgr.update_status(&state);

        // ── Download handler ─────────────────────────────────────────
        let state_dl = state.clone();
        let status_ref = mgr.status_label.clone();
        let progress_ref = mgr.progress_bar.clone();
        download_btn.connect_clicked(move |_| {
            let config = {
                let s = state_dl.lock().unwrap();
                s.config.clone()
            };

            // Check if model already exists
            if config.is_model_downloaded() {
                status_ref.set_label("✅ Already downloaded");
                return;
            }

            status_ref.set_label("Downloading…");
            progress_ref.set_visible(true);
            progress_ref.set_fraction(0.0);

            let model_dir = config.local_model_dir();
            let repo = config.model_hf_repo().to_string();

            // Channel for background thread → main thread result
            let (result_tx, result_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();

            // Spawn download in a background thread
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let result = rt.block_on(async {
                    inference::download_model(&model_dir, &repo).await
                });
                let _ = result_tx.send(result);
            });

            // Poll for download result on the main thread
            let status = status_ref.clone();
            let progress = progress_ref.clone();
            let btn = download_btn.clone();
            let del_btn = delete_btn.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                match result_rx.try_recv() {
                    Ok(Ok(())) => {
                        status.set_label("✅ Downloaded");
                        progress.set_fraction(1.0);
                        btn.set_visible(false);
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
                        // Still downloading — pulse the progress bar
                        progress.pulse();
                        ControlFlow::Continue
                    }
                    Err(_) => {
                        // Channel closed unexpectedly
                        status.set_label("❌ Download interrupted");
                        progress.set_visible(false);
                        ControlFlow::Break
                    }
                }
            });
        });

        // ── Delete handler ────────────────────────────────────────────
        let state_del = state.clone();
        let dl_btn = download_btn.clone();
        delete_btn.connect_clicked(move |_| {
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

            // Also remove VAD model
            let vad_path = AppConfig::models_dir().join("silero_vad.onnx");
            if vad_path.exists() {
                let _ = std::fs::remove_file(&vad_path);
            }

            dl_btn.set_visible(true);
            delete_btn.set_visible(false);
            mgr.update_status(&state_del);
        });

        mgr
    }

    fn update_status(&self, state: &Arc<Mutex<AppState>>) {
        let s = state.lock().unwrap();
        let downloaded = s.config.is_model_downloaded();

        if downloaded {
            self.status_label.set_label("✅ Ready");
        } else {
            self.status_label.set_label("❌ Not downloaded");
        }
    }
}
