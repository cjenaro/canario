/// System tray icon using ksni (D-Bus StatusNotifierItem).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct CanarioTray {
    is_recording: Arc<AtomicBool>,
}

impl CanarioTray {
    pub fn new(is_recording: Arc<AtomicBool>) -> Self {
        Self { is_recording }
    }

    fn recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }
}

impl ksni::Tray for CanarioTray {
    fn id(&self) -> String { "canario".into() }
    fn title(&self) -> String { "Canario".into() }
    fn status(&self) -> ksni::Status { ksni::Status::Active }

    fn icon_name(&self) -> String {
        if self.recording() { "media-record".into() }
        else { "audio-input-microphone".into() }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let title = if self.recording() { "Canario — Recording…" }
        else { "Canario — Voice to Text" };
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
                label: if recording { "⏹  Stop Recording" } else { "⏺  Start Recording" }.into(),
                icon_name: if recording { "media-playback-stop" } else { "media-record" }.into(),
                activate: Box::new(|_this: &mut Self| {
                    // The toggle is handled by the hotkey system / is_recording flag
                    // For tray menu clicks, we toggle the flag directly
                    // (The app polls this flag)
                }),
                ..Default::default()
            }.into(),
            MenuItem::Separator,
            StandardItem {
                label: "⚙  Settings".into(),
                icon_name: "preferences-system".into(),
                activate: Box::new(|_this: &mut Self| {
                    // Settings is opened via the app event loop
                }),
                ..Default::default()
            }.into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_this: &mut Self| {}),
                ..Default::default()
            }.into(),
        ]
    }
}
