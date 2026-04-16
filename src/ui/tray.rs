/// System tray icon using ksni (D-Bus StatusNotifierItem).
///
/// This runs in a background thread and communicates with the GTK4
/// main loop via an mpsc channel.
use std::sync::mpsc::Sender;

use crate::ui::AppMessage;

/// The tray icon state
pub struct CanarioTray {
    tx: Sender<AppMessage>,
    is_recording: bool,
    model_ready: bool,
}

impl CanarioTray {
    pub fn new(tx: Sender<AppMessage>) -> Self {
        Self {
            tx,
            is_recording: false,
            model_ready: false,
        }
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
        if self.is_recording {
            "media-record".into()
        } else {
            "audio-input-microphone".into()
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let title = if self.is_recording {
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

        let record_label = if self.is_recording {
            "⏹  Stop Recording"
        } else {
            "⏺  Start Recording"
        };

        vec![
            StandardItem {
                label: record_label.into(),
                icon_name: if self.is_recording {
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
