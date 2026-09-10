//! Integration tests for `History` CRUD, the 1000-entry cap, and disk
//! persistence.
//!
//! `History::load()`/`save()` (and therefore `add`/`delete`/`clear`, which
//! persist on every mutation) use a path derived from `dirs::data_dir()`,
//! which honors `$XDG_DATA_HOME` (read at call time, not cached). Tests
//! that touch disk redirect that variable to a per-test temp dir and hold
//! a process-wide mutex, because env vars are shared across threads in
//! this test binary. Pure in-memory tests (search/recent ordering) need
//! no isolation.

use std::sync::{Mutex, MutexGuard};

use canario_core::{History, HistoryEntry};

/// Serializes tests that mutate process env vars.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    // Keep the temp dir alive for the duration of the test.
    _tmp: tempfile::TempDir,
}

/// Point XDG_DATA_HOME at a fresh temp dir and return the history file path.
fn isolated_data_dir() -> (EnvGuard, std::path::PathBuf) {
    let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_DATA_HOME", tmp.path());
    let history_file = tmp.path().join("canario").join("history.json");
    (
        EnvGuard {
            _lock: lock,
            _tmp: tmp,
        },
        history_file,
    )
}

fn make_entry(id: &str, text: &str) -> HistoryEntry {
    HistoryEntry {
        id: id.to_string(),
        timestamp: chrono::Utc::now(),
        text: text.to_string(),
        duration_secs: 1.5,
        source_app: None,
    }
}

// ── CRUD (disk-backed via temp XDG_DATA_HOME) ────────────────────────────

#[test]
fn load_returns_empty_when_file_missing() {
    let (_guard, history_file) = isolated_data_dir();
    assert!(!history_file.exists());

    let history = History::load();
    assert!(history.entries.is_empty());
}

#[test]
fn add_assigns_unique_ids_and_persists() {
    let (_guard, history_file) = isolated_data_dir();

    let mut history = History::load();
    history.add("hello world".into(), 1.0, None);
    history.add("second entry".into(), 2.5, Some("firefox".into()));

    assert_eq!(history.entries.len(), 2);
    assert_ne!(history.entries[0].id, history.entries[1].id);
    assert!(!history.entries[0].id.is_empty());
    assert!(history_file.exists(), "add() should persist to disk");

    // Round-trip: a fresh load sees the same entries.
    let reloaded = History::load();
    assert_eq!(reloaded.entries.len(), 2);
    assert_eq!(reloaded.entries[0].text, "hello world");
    assert_eq!(reloaded.entries[1].text, "second entry");
    assert_eq!(reloaded.entries[1].source_app.as_deref(), Some("firefox"));
    assert_eq!(reloaded.entries[0].id, history.entries[0].id);
}

#[test]
fn delete_removes_entry_and_persists() {
    let (_guard, _history_file) = isolated_data_dir();

    let mut history = History::load();
    history.add("keep me".into(), 1.0, None);
    history.add("delete me".into(), 1.0, None);
    let doomed_id = history.entries[1].id.clone();

    history.delete(&doomed_id);

    assert_eq!(history.entries.len(), 1);
    assert_eq!(history.entries[0].text, "keep me");

    let reloaded = History::load();
    assert_eq!(reloaded.entries.len(), 1);
    assert!(reloaded.entries.iter().all(|e| e.id != doomed_id));
}

#[test]
fn delete_unknown_id_is_a_no_op() {
    let (_guard, _history_file) = isolated_data_dir();

    let mut history = History::load();
    history.add("still here".into(), 1.0, None);

    history.delete("no-such-id");

    assert_eq!(history.entries.len(), 1);
}

#[test]
fn clear_empties_history_and_persists() {
    let (_guard, history_file) = isolated_data_dir();

    let mut history = History::load();
    history.add("one".into(), 1.0, None);
    history.add("two".into(), 1.0, None);

    history.clear();

    assert!(history.entries.is_empty());
    let reloaded = History::load();
    assert!(reloaded.entries.is_empty());
    // The file exists (save writes it) and contains an empty entries array.
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&history_file).unwrap()).unwrap();
    assert_eq!(raw["entries"], serde_json::json!([]));
}

#[test]
fn enforces_1000_entry_cap_dropping_oldest() {
    let (_guard, _history_file) = isolated_data_dir();

    let mut history = History::load();
    for i in 0..1005 {
        history.add(format!("entry {i}"), 0.5, None);
    }

    assert_eq!(history.entries.len(), 1000, "cap must hold at 1000");
    // Oldest entries (0..5) were drained; the oldest survivor is #5.
    assert_eq!(history.entries.first().unwrap().text, "entry 5");
    assert_eq!(history.entries.last().unwrap().text, "entry 1004");

    // The cap survives a persistence round-trip.
    let reloaded = History::load();
    assert_eq!(reloaded.entries.len(), 1000);
    assert_eq!(reloaded.entries.first().unwrap().text, "entry 5");
}

#[test]
fn corrupt_history_file_falls_back_to_empty() {
    let (_guard, history_file) = isolated_data_dir();
    std::fs::create_dir_all(history_file.parent().unwrap()).unwrap();
    std::fs::write(&history_file, "{ not valid json").unwrap();

    let history = History::load();
    assert!(history.entries.is_empty());
}

// ── Pure in-memory behavior (no env isolation needed) ────────────────────

#[test]
fn recent_owned_returns_most_recent_first_up_to_limit() {
    let history = History {
        entries: vec![
            make_entry("1", "oldest"),
            make_entry("2", "middle"),
            make_entry("3", "newest"),
        ],
    };

    let recent = history.recent_owned(2);
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].text, "newest");
    assert_eq!(recent[1].text, "middle");

    // Limit larger than the store returns everything, still newest-first.
    let all = history.recent_owned(50);
    assert_eq!(all.len(), 3);
    assert_eq!(all[2].text, "oldest");

    assert!(history.recent_owned(0).is_empty());
}

#[test]
fn search_owned_is_case_insensitive_and_newest_first() {
    let history = History {
        entries: vec![
            make_entry("1", "The Quick Brown Fox"),
            make_entry("2", "nothing here"),
            make_entry("3", "quick brown fox jumps"),
        ],
    };

    let hits = history.search_owned("QUICK");
    assert_eq!(hits.len(), 2);
    // Newest match first.
    assert_eq!(hits[0].id, "3");
    assert_eq!(hits[1].id, "1");

    assert!(history.search_owned("zebra").is_empty());
    // Empty query matches everything.
    assert_eq!(history.search_owned("").len(), 3);
}

#[test]
fn history_json_round_trip_via_serde() {
    let history = History {
        entries: vec![
            make_entry("a", "first"),
            HistoryEntry {
                source_app: Some("code".into()),
                ..make_entry("b", "second")
            },
        ],
    };

    let json = serde_json::to_string_pretty(&history).unwrap();
    let loaded: History = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.entries.len(), 2);
    assert_eq!(loaded.entries[0].id, "a");
    assert_eq!(loaded.entries[1].source_app.as_deref(), Some("code"));
}
