//! Structured stage-timing instrumentation for the dictation pipeline.
//!
//! Canario's product *is* latency (press-and-hold dictation), so every
//! pipeline stage is timestamped and the numbers are comparable across
//! runs and machines. Project rule (bead canario-oxi): a performance
//! change ships with before/after numbers for the stage it touches.
//!
//! # Model
//!
//! A "mark" is one JSON line naming a pipeline stage, stamped with the
//! wall clock (`ts_ms`, epoch milliseconds — one clock across processes,
//! so sidecar and Electron marks can be joined):
//!
//! ```json
//! {"stage":"mic_stream_started","ts_ms":1757581200123.456,"pid":12345}
//! ```
//!
//! Marks are appended in causal order by whoever reaches the stage:
//!
//! | Stage | Emitted from | Meaning |
//! |---|---|---|
//! | `hotkey_start_action` / `hotkey_stop_action` / `hotkey_cancel_action` | [`crate::Canario::start_hotkey`] callback | hotkey backend dispatched an action (after its poll loop noticed the key) |
//! | `start_recording_called` / `stop_recording_called` | [`crate::Canario`] | recording API entry (frontend toggle, CLI, sidecar command) |
//! | `recording_started_event` | [`crate::Canario::start_recording`] | `RecordingStarted` about to be sent to the frontend |
//! | `recording_thread_start` | `recording` | capture thread entered its loop |
//! | `mic_device_opened` | `recording` | default input device queried/configured |
//! | `mic_stream_started` | `recording` | `stream.play()` returned — capture requested |
//! | `first_audio` | `recording` | first sample block arrived from the mic |
//! | `stop_observed` | `recording` | capture loop noticed the stop flag (after its 50 ms poll sleep) |
//! | `audio_cloned` | `recording` | whole-buffer clone taken on stop |
//! | `mic_released` | `recording` | capture stream dropped |
//! | `beep_stop_start` / `beep_stop_done` | `recording` | stop beep (runs on the transcribing thread) |
//! | `resample_start` / `resample_done` | `recording` | resample to 16 kHz |
//! | `recognizer_load_start` / `recognizer_load_done` | `recording` | model load on cache miss (cold dictation) |
//! | `decode_start` / `decode_end` | `recording` | final whole-buffer decode |
//! | `transcript_ready` | `recording` | `TranscriptionReady` about to be sent |
//! | `paste_start` / `paste_done` | [`crate::paste_text`] | native paste (clipboard + injection) |
//! | `sidecar_cmd_*` / `sidecar_event_*` | `canario-electron` | sidecar observed a command / forwarded an event |
//! | `electron:paste_*` | Electron main | auto-paste in the real app |
//!
//! Derived metrics (computed by `scripts/bench-pipeline`):
//!
//! - **press-to-record** = `start_recording_called` → `mic_stream_started`/`first_audio`
//! - **release-to-transcript** = `stop_recording_called` → `transcript_ready`
//! - **transcript-to-paste** = `transcript_ready` → `paste_done` (native) or
//!   the Electron paste marks
//!
//! # Knobs (environment variables — deliberately not config)
//!
//! - `CANARIO_TIMING=1` — enable marks. Absent (or `0`/`false`) keeps the
//!   instrumentation a single relaxed atomic load: zero effect on the
//!   pipeline when off, so tests and normal use are unaffected.
//! - `CANARIO_TIMING_FILE=<path>` — append JSONL marks there instead of
//!   stderr. The file is created; concurrent writers append whole lines.
//! - `CANARIO_BENCH_DISABLE_PREWARM=1` — benchmark knob: skip the startup
//!   recognizer prewarm so a "cold" run pays the model load inside the
//!   first dictation (used by `scripts/bench-pipeline --cold`).
//!
//! Output goes to a dedicated sink, not through `tracing`: the sidecar
//! reserves stdout for JSON IPC and rotates logs, while timing marks must
//! survive as parseable one-line JSON wherever they land.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Where marks are written.
enum Sink {
    Stderr,
    File(Mutex<std::fs::File>),
}

static ENABLED: OnceLock<bool> = OnceLock::new();
static SINK: OnceLock<Sink> = OnceLock::new();

/// Is a truthy-enough env var set? Accepts the usual spellings; treats
/// empty and "0"/"false"/"no"/"off" as disabled.
fn env_flag(name: &str) -> bool {
    let Some(v) = std::env::var_os(name) else {
        return false;
    };
    let v = v.to_string_lossy().trim().to_ascii_lowercase();
    !v.is_empty() && v != "0" && v != "false" && v != "no" && v != "off"
}

/// Are timing marks enabled (`CANARIO_TIMING=1`)?
///
/// Resolved once; call sites check this before building any dynamic
/// stage names. When disabled, [`mark`] still checks and returns, so
/// plain `mark("literal")` call sites need no guard.
pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| env_flag("CANARIO_TIMING"))
}

/// Benchmark knob: `CANARIO_BENCH_DISABLE_PREWARM=1` (see module docs).
pub fn bench_disable_prewarm() -> bool {
    env_flag("CANARIO_BENCH_DISABLE_PREWARM")
}

/// Wall-clock milliseconds since the Unix epoch (fractional).
///
/// One shared clock for every process in the pipeline, so marks from the
/// sidecar and the Electron main process can be compared directly.
pub fn epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Resolve the mark sink: `CANARIO_TIMING_FILE` if usable, else stderr.
fn sink() -> &'static Sink {
    SINK.get_or_init(|| {
        if let Some(path) = std::env::var_os("CANARIO_TIMING_FILE") {
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => return Sink::File(Mutex::new(file)),
                Err(e) => eprintln!(
                    "canario timing: cannot open CANARIO_TIMING_FILE={:?} ({}); marks go to stderr",
                    path, e
                ),
            }
        }
        Sink::Stderr
    })
}

/// Record a pipeline stage mark as one JSON line.
///
/// No-op unless [`enabled`] — see the module docs for the schema and the
/// canonical stage names.
pub fn mark(stage: &str) {
    if !enabled() {
        return;
    }
    let line = format!(
        "{{\"stage\":\"{}\",\"ts_ms\":{:.3},\"pid\":{}}}\n",
        stage,
        epoch_ms(),
        std::process::id()
    );
    match sink() {
        Sink::Stderr => {
            let mut err = std::io::stderr().lock();
            let _ = err.write_all(line.as_bytes());
            let _ = err.flush();
        }
        Sink::File(file) => {
            if let Ok(mut file) = file.lock() {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The env parser accepts the documented truthy spellings and
    /// rejects the falsy ones, so `CANARIO_TIMING` knobs behave the same
    /// everywhere they are read.
    #[test]
    fn env_flag_truth_table() {
        // SAFETY-in-tests: single-threaded mutation of the process env
        // for the duration of each check. Each case sets and clears the
        // variable so cases cannot contaminate each other.
        for (value, expected) in [
            ("1", true),
            ("true", true),
            ("YES", true),
            ("on", true),
            ("", false),
            ("0", false),
            ("false", false),
            ("no", false),
            ("off", false),
        ] {
            std::env::set_var("CANARIO_TIMING_TEST_FLAG", value);
            assert_eq!(
                env_flag("CANARIO_TIMING_TEST_FLAG"),
                expected,
                "value {:?}",
                value
            );
            std::env::remove_var("CANARIO_TIMING_TEST_FLAG");
        }
        assert!(!env_flag("CANARIO_TIMING_TEST_FLAG"));
    }
}
