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

    /// Recording was cancelled (Escape). The audio was discarded —
    /// no transcription will follow. Frontends should hide any
    /// recording UI; do NOT paste or store in history.
    RecordingCancelled,

    /// Transcription is ready (after post-processing).
    /// `text` is the final processed string.
    /// `duration_secs` is the recording length in seconds.
    TranscriptionReady { text: String, duration_secs: f64 },

    /// Recording/transcription error. Do NOT paste or store in history.
    #[serde(rename = "Error")]
    Error { message: String },

    // ── Real-time feedback ──────────────────────────────────────────
    /// Audio level update during recording (0.0 = silence, 1.0 = clipping)
    AudioLevel { level: f64 },

    /// Live-preview transcript emitted periodically during long
    /// recordings (after the live-captions threshold). Produced by
    /// re-decoding a sliding window of the buffer — preview only:
    /// the authoritative text still arrives via `TranscriptionReady`,
    /// so frontends should NOT paste or store this in history.
    PartialTranscript { text: String },

    // ── Model management ────────────────────────────────────────────
    /// Model download progress (0.0 to 1.0)
    ModelDownloadProgress { progress: f64 },

    /// Model download completed successfully
    ModelDownloadComplete,

    /// Model download failed
    #[serde(rename = "ModelDownloadFailed")]
    ModelDownloadFailed { error: String },

    // ── Hotkey ──────────────────────────────────────────────────────
    /// Global hotkey was triggered — frontend should toggle recording
    HotkeyTriggered,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sidecar forwards events to the Electron main process as
    /// `event`-tagged JSON — the wire shape of each variant is
    /// protocol, so pin it for the live-caption preview event.
    #[test]
    fn partial_transcript_serializes_with_event_tag() {
        let json = serde_json::to_string(&Event::PartialTranscript {
            text: "live preview".into(),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"event":"PartialTranscript","text":"live preview"}"#
        );
    }
}
