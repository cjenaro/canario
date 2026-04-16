/// System tray icon using ksni (D-Bus StatusNotifierItem).
///
/// The tray reads `is_recording` from a shared `AtomicBool` so it
/// always reflects the current recording state.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::ui::AppMessage;

/// The tray icon state
pub struct CanarioTray {
    tx: Sender<AppMessage>,
    is_recording: Arc<AtomicBool>,
}

impl CanarioTray {
    pub fn new(tx: Sender<AppMessage>, is_recording: Arc<AtomicBool>) -> Self {
        Self { tx, is_recording }
    }

    fn recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }
}

impl ksni::Tray for CanarioTray {
    fn id(&self) -> String {
        "canario".into()
    }

    fn title(&self) -> String {
        "Canario".into()
    }

    fn status(&self) -> ksni::Status {
        ksni::Status::Active
    }

    fn icon_name(&self) -> String {
        if self.recording() {
            "media-record".into()
        } else {
            "audio-input-microphone".into()
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let title = if self.recording() {
            "Canario — Recording…"
        } else {
            "Canario — Voice to Text"
        };
        ksni::ToolTip {
            title: title.into(),
            description: "Press the hotkey or use the menu to start recording".into(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;

        let recording = self.recording();
        let record_label = if recording {
            "⏹  Stop Recording"
        } else {
            "⏺  Start Recording"
        };

        vec![
            StandardItem {
                label: record_label.into(),
                icon_name: if recording {
                    "media-playback-stop".into()
                } else {
                    "media-record".into()
                },
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(AppMessage::ToggleRecording);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "⚙  Settings".into(),
                icon_name: "preferences-system".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(AppMessage::ShowSettings);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(AppMessage::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}
