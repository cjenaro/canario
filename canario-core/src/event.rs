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

    /// Transcription is ready (after post-processing, and after
    /// transformation when `transform.enabled` — fgm.3 D3: the pipeline
    /// runs BEFORE this event, so what arrives here is canonical).
    /// `text` is the final string (the transformed transcript when a
    /// transformation ran — this is what gets pasted and stored as
    /// history `text`).
    /// `duration_secs` is the recording length in seconds.
    /// `raw_text` carries the pre-transformation transcript, present
    /// ONLY when a transformation changed the text (D3: the
    /// reveal-raw affordance; history stores it alongside `text`).
    /// `transform_failed` — fgm.1 D5d: a transformation was attempted
    /// and failed or timed out, so the raw transcript flowed on.
    /// Frontends use it for the fallback affordance (fgm.4). Absent
    /// (false) covers everything else: not configured, no rule, or a
    /// clean transformation. Both fields are skipped on the wire when
    /// unset, so pre-fgm.3 frontends see the exact old shape.
    TranscriptionReady {
        text: String,
        duration_secs: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_text: Option<String>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        transform_failed: bool,
    },

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

    /// fgm.3 D5a/D3: with no transformation (disabled, no rule, or a
    /// clean no-op) the wire shape is BYTE-IDENTICAL to the
    /// pre-fgm.3 event — old frontends must never see new fields.
    #[test]
    fn transcription_ready_without_transform_keeps_the_old_wire_shape() {
        let json = serde_json::to_string(&Event::TranscriptionReady {
            text: "hello".into(),
            duration_secs: 1.5,
            raw_text: None,
            transform_failed: false,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"event":"TranscriptionReady","text":"hello","duration_secs":1.5}"#
        );
    }

    /// fgm.3: a changed transcript carries the raw one alongside, and
    /// a failed transformation flags itself (D5d) — the two new keys
    /// the renderer (fgm.4) will read.
    #[test]
    fn transcription_ready_serializes_raw_text_and_failure_flag() {
        let json = serde_json::to_string(&Event::TranscriptionReady {
            text: "Hello, world.".into(),
            duration_secs: 2.0,
            raw_text: Some("hello wrld".into()),
            transform_failed: true,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"event":"TranscriptionReady","text":"Hello, world.","duration_secs":2.0,"raw_text":"hello wrld","transform_failed":true}"#
        );
    }
}
