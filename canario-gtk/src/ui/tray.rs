/// System tray icon using ksni (D-Bus StatusNotifierItem).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Actions that can be triggered from the system tray menu.
#[derive(Debug)]
pub enum TrayAction {
    ToggleRecording,
    ShowSettings,
    Quit,
}

pub struct CanarioTray {
    is_recording: Arc<AtomicBool>,
    action_tx: std::sync::mpsc::Sender<TrayAction>,
}

impl CanarioTray {
    pub fn new(
        is_recording: Arc<AtomicBool>,
        action_tx: std::sync::mpsc::Sender<TrayAction>,
    ) -> Self {
        Self {
            is_recording,
            action_tx,
        }
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
            description: "Press the hotkey to start recording".into(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let recording = self.recording();
        vec![
            StandardItem {
                label: if recording {
                    "⏹  Stop Recording".into()
                } else {
                    "⏺  Start Recording".into()
                },
                icon_name: if recording {
                    "media-playback-stop".into()
                } else {
                    "media-record".into()
                },
                activate: Box::new(|this: &mut Self| {
                    let _ = this.action_tx.send(TrayAction::ToggleRecording);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "⚙  Settings".into(),
                icon_name: "preferences-system".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.action_tx.send(TrayAction::ShowSettings);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.action_tx.send(TrayAction::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}
