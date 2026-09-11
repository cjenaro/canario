/// Events emitted by the Canario backend.
///
/// Frontends receive these via the `Receiver<Event>` returned by `Canario::new()`.
/// All events are `Clone + Send` so they can be safely passed across threads.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

    /// Transcription is starting on the finished capture
    /// (canario-dmp.9): the stop was observed and the buffer is
    /// committed to the decode pipeline. Emitted before the decode
    /// work starts, after the too-short guard (a discarded-to-short
    /// capture never transcribes and never emits this). Frontends may
    /// flip their overlay to a "Transcribing…" state from this event
    /// OR derive it from a successful stop response — both paths are
    /// valid (PRD-ELECTRON.md Appendix B).
    TranscriptionStarted,

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
        /// Default-tolerant on deserialize so BOTH wire shapes parse:
        /// the old one (keys absent) and the fgm.3 one below — the
        /// golden-trace fixtures (canario-dmp.13) replay both through
        /// this enum.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        raw_text: Option<String>,
        #[serde(skip_serializing_if = "std::ops::Not::not", default)]
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

    // ── Configuration ───────────────────────────────────────────────
    /// The persisted config changed (canario-dmp.20): this instance
    /// wrote config.json (`Canario::update_config`), or a reload
    /// (`Canario::refresh_config`) detected that another writer —
    /// another frontend, the CLI, a manual edit — changed it.
    /// Payload-free by design: consumers pull `get_config` for the
    /// new state, so no snapshot can go stale on the wire.
    ConfigChanged,

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

    /// canario-dmp.9: the transcribing signal is payload-free — pin
    /// the wire shape so a stray field can't ride along unnoticed.
    #[test]
    fn transcription_started_serializes_with_event_tag() {
        let json = serde_json::to_string(&Event::TranscriptionStarted).unwrap();
        assert_eq!(json, r#"{"event":"TranscriptionStarted"}"#);
    }

    /// canario-dmp.20: config-change notification is payload-free —
    /// consumers pull get_config, so the line must carry nothing but
    /// the event tag.
    #[test]
    fn config_changed_serializes_with_event_tag() {
        let json = serde_json::to_string(&Event::ConfigChanged).unwrap();
        assert_eq!(json, r#"{"event":"ConfigChanged"}"#);
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

    // ── Exhaustive wire-shape pins (canario-dmp.13) ───────────────────
    //
    // Every variant of the enum has its exact serialized form asserted:
    // the sidecar forwards these lines to the Electron main process,
    // and the renderer's golden traces (canario-app
    // src/renderer/state/golden/*.json) replay the same shapes — a
    // stray field or renamed tag must fail here, not downstream.

    /// Recording lifecycle: the three payload-free states.
    #[test]
    fn recording_lifecycle_events_serialize_payload_free() {
        assert_eq!(
            serde_json::to_string(&Event::RecordingStarted).unwrap(),
            r#"{"event":"RecordingStarted"}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::RecordingStopped).unwrap(),
            r#"{"event":"RecordingStopped"}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::RecordingCancelled).unwrap(),
            r#"{"event":"RecordingCancelled"}"#
        );
    }

    /// Errors carry exactly one string field.
    #[test]
    fn error_serializes_with_message() {
        let json = serde_json::to_string(&Event::Error {
            message: "Audio device lost".into(),
        })
        .unwrap();
        assert_eq!(json, r#"{"event":"Error","message":"Audio device lost"}"#);
    }

    /// Real-time feedback: level is a bare float, partial transcript a
    /// bare string (PartialTranscript pinned above).
    #[test]
    fn audio_level_serializes_with_bare_float() {
        let json = serde_json::to_string(&Event::AudioLevel { level: 0.42 }).unwrap();
        assert_eq!(json, r#"{"event":"AudioLevel","level":0.42}"#);
    }

    /// Model management: progress is a bare float; completion is
    /// payload-free; failure carries exactly one string field.
    #[test]
    fn model_download_events_serialize_their_payloads() {
        assert_eq!(
            serde_json::to_string(&Event::ModelDownloadProgress { progress: 0.75 }).unwrap(),
            r#"{"event":"ModelDownloadProgress","progress":0.75}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::ModelDownloadComplete).unwrap(),
            r#"{"event":"ModelDownloadComplete"}"#
        );
        assert_eq!(
            serde_json::to_string(&Event::ModelDownloadFailed {
                error: "Download cancelled".into()
            })
            .unwrap(),
            r#"{"event":"ModelDownloadFailed","error":"Download cancelled"}"#
        );
    }

    /// Hotkey notifications are payload-free — the frontend toggles
    /// recording off the tag alone.
    #[test]
    fn hotkey_triggered_serializes_payload_free() {
        let json = serde_json::to_string(&Event::HotkeyTriggered).unwrap();
        assert_eq!(json, r#"{"event":"HotkeyTriggered"}"#);
    }

    /// canario-dmp.13: the golden-trace replay deserializes every
    /// fixture element through this enum — pin that BOTH
    /// TranscriptionReady wire shapes (old keys-absent and fgm.3
    /// raw_text) parse back, plus one payload-free variant.
    #[test]
    fn events_deserialize_from_their_wire_shapes() {
        let old: Event = serde_json::from_str(
            r#"{"event":"TranscriptionReady","text":"hello","duration_secs":1.5}"#,
        )
        .unwrap();
        match old {
            Event::TranscriptionReady {
                text,
                duration_secs,
                raw_text,
                transform_failed,
            } => {
                assert_eq!(text, "hello");
                assert_eq!(duration_secs, 1.5);
                assert_eq!(raw_text, None);
                assert!(!transform_failed);
            }
            other => panic!("wrong variant: {other:?}"),
        }

        let new: Event = serde_json::from_str(
            r#"{"event":"TranscriptionReady","text":"Hello.","duration_secs":1.8,"raw_text":"hello"}"#,
        )
        .unwrap();
        match new {
            Event::TranscriptionReady { raw_text, .. } => {
                assert_eq!(raw_text.as_deref(), Some("hello"));
            }
            other => panic!("wrong variant: {other:?}"),
        }

        let hotkey: Event = serde_json::from_str(r#"{"event":"HotkeyTriggered"}"#).unwrap();
        assert!(matches!(hotkey, Event::HotkeyTriggered));
    }
}
