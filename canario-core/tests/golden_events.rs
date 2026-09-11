//! Golden-trace validation (canario-dmp.13, Rust half).
//!
//! The shared renderer fixtures at
//! `canario-app/src/renderer/state/golden/*.json` are replayed through
//! the TypeScript state machine by the Electron-side tests. This is
//! the core-side half of the contract: every `events[]` element must
//! deserialize into a real `canario_core::Event` wire shape (so a
//! fixture can never drift from what the backend actually emits), and
//! the event ORDERS must satisfy core's invariants:
//!
//! - `record_*` traces begin with `RecordingStarted`;
//! - a trace containing `TranscriptionReady` has an earlier
//!   `TranscriptionStarted` and ends with `RecordingStopped`;
//! - `record_too_short` contains no transcription event at all;
//! - `record_cancel` ends with `RecordingCancelled`;
//! - `record_error` ends with `Error`;
//! - `download_*` progress values are non-decreasing (and in 0.0..=1.0)
//!   and the trace ends with `ModelDownloadComplete` or
//!   `ModelDownloadFailed`;
//! - `download_cancel`'s failure error is non-empty.

use canario_core::Event;

/// The fixtures are shared with the Electron renderer's replay tests —
/// reached across crate boundaries the same way the PROTOCOL_VERSION
/// pin (canario-electron/tests/protocol.rs) reads version.ts.
const GOLDEN_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../canario-app/src/renderer/state/golden"
);

/// The frozen trace set. Presence (not exclusivity) is asserted, so a
/// newly added fixture is validated too — it just can't silently
/// replace one of the contract traces.
const EXPECTED_FIXTURES: [&str; 8] = [
    "record_transcribe",
    "record_transform",
    "record_cancel",
    "record_too_short",
    "record_live_captions",
    "record_error",
    "download_complete",
    "download_cancel",
];

/// Wire tag of an event — the `event` discriminator every frontend
/// switches on.
fn tag_of(event: &Event) -> &'static str {
    match event {
        Event::RecordingStarted => "RecordingStarted",
        Event::RecordingStopped => "RecordingStopped",
        Event::RecordingCancelled => "RecordingCancelled",
        Event::TranscriptionStarted => "TranscriptionStarted",
        Event::TranscriptionReady { .. } => "TranscriptionReady",
        Event::Error { .. } => "Error",
        Event::AudioLevel { .. } => "AudioLevel",
        Event::PartialTranscript { .. } => "PartialTranscript",
        Event::ModelDownloadProgress { .. } => "ModelDownloadProgress",
        Event::ModelDownloadComplete => "ModelDownloadComplete",
        Event::ModelDownloadFailed { .. } => "ModelDownloadFailed",
        Event::ConfigChanged => "ConfigChanged",
        Event::HotkeyTriggered => "HotkeyTriggered",
    }
}

/// Assert core's ordering invariants on one parsed trace.
fn validate_trace(name: &str, events: &[Event]) {
    let tags: Vec<&str> = events.iter().map(tag_of).collect();

    if name.starts_with("record_") {
        assert_eq!(
            tags.first(),
            Some(&"RecordingStarted"),
            "{name}: every recording trace must begin with RecordingStarted"
        );
    }

    if name.starts_with("download_") {
        let mut last = f64::NEG_INFINITY;
        for event in events {
            if let Event::ModelDownloadProgress { progress } = event {
                assert!(
                    (0.0..=1.0).contains(progress),
                    "{name}: progress must stay within 0.0..=1.0, found {progress}"
                );
                assert!(
                    *progress >= last,
                    "{name}: download progress must be non-decreasing, went {last} → {progress}"
                );
                last = *progress;
            }
        }
        assert!(
            matches!(
                tags.last(),
                Some(&"ModelDownloadComplete") | Some(&"ModelDownloadFailed")
            ),
            "{name}: a download trace must end with ModelDownloadComplete or ModelDownloadFailed, ended with {:?}",
            tags.last()
        );
    }

    if name == "download_cancel" {
        let error = events.iter().rev().find_map(|event| match event {
            Event::ModelDownloadFailed { error } => Some(error.clone()),
            _ => None,
        });
        let error =
            error.unwrap_or_else(|| panic!("{name}: must contain a ModelDownloadFailed event"));
        assert!(
            !error.is_empty(),
            "{name}: the cancellation failure must carry a non-empty error"
        );
    }

    if name == "record_cancel" {
        assert_eq!(
            tags.last(),
            Some(&"RecordingCancelled"),
            "{name}: an escape-cancel ends with RecordingCancelled"
        );
    }

    if name == "record_error" {
        assert_eq!(
            tags.last(),
            Some(&"Error"),
            "{name}: a backend error is terminal"
        );
    }

    if name == "record_too_short" {
        assert!(
            !tags.contains(&"TranscriptionStarted") && !tags.contains(&"TranscriptionReady"),
            "{name}: a discarded too-short capture never transcribes"
        );
    }

    // Universal: a ready transcript implies the decode started earlier
    // and the recording closed before it was emitted.
    if let Some(ready_idx) = tags.iter().position(|t| *t == "TranscriptionReady") {
        let started_idx = tags
            .iter()
            .position(|t| *t == "TranscriptionStarted")
            .unwrap_or_else(|| {
                panic!("{name}: TranscriptionReady without an earlier TranscriptionStarted")
            });
        assert!(
            started_idx < ready_idx,
            "{name}: TranscriptionStarted must precede TranscriptionReady"
        );
        assert_eq!(
            tags.last(),
            Some(&"RecordingStopped"),
            "{name}: a trace with a ready transcript ends with RecordingStopped"
        );
    }
}

#[test]
fn golden_traces_are_real_wire_shapes_satisfying_core_invariants() {
    let dir = std::path::Path::new(GOLDEN_DIR);
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| {
        panic!(
            "cannot read golden fixture dir {} ({}); update this test if the fixtures moved — they are shared with the Electron renderer's replay tests",
            dir.display(),
            e
        )
    });

    let mut files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "golden fixture dir {} contains no .json traces",
        dir.display()
    );

    let mut seen: Vec<String> = Vec::new();
    for path in files {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        seen.push(stem.clone());

        let raw = std::fs::read_to_string(&path).unwrap();
        let fixture: serde_json::Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{}: fixture is not valid JSON ({e})", path.display()));
        let name = fixture["name"]
            .as_str()
            .unwrap_or_else(|| panic!("{}: fixture has no string \"name\"", path.display()))
            .to_string();
        assert_eq!(
            name,
            stem,
            "{}: fixture \"name\" must match the file stem",
            path.display()
        );
        let elements = fixture["events"]
            .as_array()
            .unwrap_or_else(|| panic!("{}: fixture has no \"events\" array", path.display()))
            .clone();
        assert!(
            !elements.is_empty(),
            "{}: fixture events[] must not be empty",
            path.display()
        );

        let mut events: Vec<Event> = Vec::with_capacity(elements.len());
        for (i, element) in elements.iter().enumerate() {
            let event: Event = serde_json::from_value(element.clone()).unwrap_or_else(|e| {
                panic!(
                    "{}: events[{i}] is not a real Event wire shape ({e}): {element}",
                    path.display()
                )
            });
            // Round-trip equality: the parsed event re-serializes to
            // exactly the fixture element — no stray keys, no renamed
            // tags, no non-canonical values riding in a trace.
            assert_eq!(
                serde_json::to_value(&event).unwrap(),
                *element,
                "{}: events[{i}] does not round-trip through Event (stray key or non-canonical value)",
                path.display()
            );
            events.push(event);
        }

        validate_trace(&name, &events);
    }

    for expected in EXPECTED_FIXTURES {
        assert!(
            seen.iter().any(|s| s == expected),
            "expected golden fixture {expected}.json is missing from {}",
            dir.display()
        );
    }
}
