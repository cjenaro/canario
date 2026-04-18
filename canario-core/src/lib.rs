/// Canario Core — voice-to-text backend.
///
/// This library provides everything needed to capture audio, transcribe
/// speech, manage models, handle hotkeys, and paste results. Frontends
/// (GTK4, CLI, Electron, etc.) just need to:
///
/// 1. Call `Canario::new()` to get an instance + event receiver
/// 2. Call methods like `start_recording()`, `stop_recording()`
/// 3. Handle `Event`s from the receiver to update their UI
///
/// ```no_run
/// use canario_core::{Canario, Event};
///
/// let (canario, rx) = Canario::new().unwrap();
///
/// // Start recording
/// canario.start_recording().unwrap();
///
/// // Handle events
/// while let Ok(event) = rx.recv() {
///     match event {
///         Event::TranscriptionReady { text, .. } => println!("{}", text),
///         Event::RecordingStopped => break,
///         _ => {}
///     }
/// }
/// ```no_run

mod audio;
mod canario;
mod config;
mod event;
mod history;
mod hotkey;
mod inference;
mod paste;
mod recording;

// ── Public API ─────────────────────────────────────────────────────────────

pub use canario::Canario;
pub use config::{AppConfig, AudioBehavior, ModelVariant};
pub use event::Event;
pub use history::{History, HistoryEntry};
pub use hotkey::{HotkeyAction, HotkeyConfig, HotkeyListener};
pub use inference::postprocess::{PostProcessor, WordRemapping, WordRemoval};
pub use paste::paste_text;

// Re-export for convenience
pub use recording::RecordingHandle;

// Re-export submodules that frontends need
pub mod autostart {
    pub use crate::config::autostart::*;
}
pub mod audio_effects {
    pub use crate::audio::effects::*;
}
