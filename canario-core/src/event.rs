/// Events emitted by the Canario backend.
///
/// Frontends receive these via the `Receiver<Event>` returned by `Canario::new()`.
/// All events are `Clone + Send` so they can be safely passed across threads.

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event")]
pub enum Event {
    // ── Recording lifecycle ─────────────────────────────────────────
    /// Recording has started (mic is open, audio is being captured)
    RecordingStarted,

    /// Recording has stopped (mic released, about to transcribe)
    RecordingStopped,

    /// Transcription is ready (after post-processing).
    /// `text` is the final processed string.
    /// `duration_secs` is the recording length in seconds.
    TranscriptionReady {
        text: String,
        duration_secs: f64,
    },

    /// Recording/transcription error. Do NOT paste or store in history.
    #[serde(rename = "Error")]
    Error {
        message: String,
    },

    // ── Real-time feedback ──────────────────────────────────────────
    /// Audio level update during recording (0.0 = silence, 1.0 = clipping)
    AudioLevel {
        level: f64,
    },

    // ── Model management ────────────────────────────────────────────
    /// Model download progress (0.0 to 1.0)
    ModelDownloadProgress {
        progress: f64,
    },

    /// Model download completed successfully
    ModelDownloadComplete,

    /// Model download failed
    #[serde(rename = "ModelDownloadFailed")]
    ModelDownloadFailed {
        error: String,
    },

    // ── Hotkey ──────────────────────────────────────────────────────
    /// Global hotkey was triggered — frontend should toggle recording
    HotkeyTriggered,
}
