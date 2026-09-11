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
/// ```
mod audio;
mod canario;
mod config;
pub mod diagnostics;
mod event;
mod history;
mod hotkey;
mod inference;
mod mic_warm;
mod paste;
mod recording;
pub mod timing;
/// LLM transformation provider client (canario-fgm.1 D1/D2/D5): OpenAI
/// chat-completions against any compatible base_url, API key passed in
/// memory-only as a function argument, payload limited to
/// transcript + instruction. Wired into the sidecar's transform
/// commands; the pipeline itself lands in canario-fgm.3/4.
pub mod transform;

// ── Public API ─────────────────────────────────────────────────────────────

pub use canario::Canario;
pub use canario::LifecycleStatus;
pub use config::{AppConfig, AudioBehavior, ModelPaths, ModelVariant};
pub use event::Event;
pub use history::{History, HistoryEntry};
pub use hotkey::hotkey_socket_path;
pub use hotkey::{HotkeyAction, HotkeyConfig, HotkeyListener, HotkeyStatus};
pub use inference::postprocess::{PostProcessor, WordRemapping, WordRemoval};
pub use paste::paste_text;

// Re-export for convenience
pub use inference::read_wav;
pub use inference::TranscriptionEngine;

// Benchmark reachability: `inference` is a private module, but the
// pipeline benches (benches/pipeline.rs) exercise `resample` directly —
// the same function the recording thread calls on stop.
pub use inference::resample;
pub use recording::RecordingHandle;

// Re-export submodules that frontends need
pub mod autostart {
    pub use crate::config::autostart::*;
}
pub mod audio_effects {
    pub use crate::audio::effects::*;
}
