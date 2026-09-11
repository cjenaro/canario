//! Integration tests for `AppConfig` disk persistence.
//!
//! `AppConfig::load()`/`save()` use paths derived from `dirs::config_dir()`,
//! which honors `$XDG_CONFIG_HOME` (read at call time, not cached). Tests
//! redirect that variable to a per-test temp dir; a process-wide mutex
//! serializes them because env vars are shared across threads in this
//! test binary. Pure serde-layer tests need no isolation.

use std::sync::{Mutex, MutexGuard};

use canario_core::{AppConfig, ModelVariant};

/// Serializes tests that mutate process env vars.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    // Keep the temp dir alive for the duration of the test.
    _tmp: tempfile::TempDir,
}

/// Point XDG_CONFIG_HOME at a fresh temp dir and return its config file path.
fn isolated_config_dir() -> (EnvGuard, std::path::PathBuf) {
    let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", tmp.path());
    let config_file = tmp.path().join("canario").join("config.json");
    (
        EnvGuard {
            _lock: lock,
            _tmp: tmp,
        },
        config_file,
    )
}

// ── Disk-backed load/save ────────────────────────────────────────────────

#[test]
fn load_creates_default_config_file_when_missing() {
    let (_guard, config_file) = isolated_config_dir();
    assert!(!config_file.exists());

    let config = AppConfig::load().unwrap();

    // A default config was written to disk...
    assert!(config_file.exists());
    // ...and it round-trips: loading again yields the same values.
    let reloaded = AppConfig::load().unwrap();
    assert_eq!(
        serde_json::to_value(&reloaded).unwrap(),
        serde_json::to_value(&config).unwrap()
    );
}

#[test]
fn saved_config_file_contains_config_version() {
    let (_guard, config_file) = isolated_config_dir();

    AppConfig::default().save().unwrap();

    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_file).unwrap()).unwrap();
    let version = raw
        .get("config_version")
        .and_then(|v| v.as_u64())
        .expect("saved config must contain config_version");
    assert!(
        version >= 1,
        "config_version should be a positive schema version"
    );
}

#[test]
fn save_then_load_round_trip_via_disk() {
    let (_guard, _config_file) = isolated_config_dir();

    let config = AppConfig {
        model: ModelVariant::ParakeetV2,
        hotkey: vec!["Ctrl".into(), "Shift".into(), "V".into()],
        auto_paste: false,
        num_threads: 8,
        minimum_key_time: 0.35,
        ..AppConfig::default()
    };
    config.save().unwrap();

    let loaded = AppConfig::load().unwrap();
    assert_eq!(loaded.model, ModelVariant::ParakeetV2);
    assert_eq!(loaded.hotkey, vec!["Ctrl", "Shift", "V"]);
    assert!(!loaded.auto_paste);
    assert_eq!(loaded.num_threads, 8);
    assert!((loaded.minimum_key_time - 0.35).abs() < f64::EPSILON);
}

#[test]
fn load_tolerates_old_config_file_with_missing_fields() {
    let (_guard, config_file) = isolated_config_dir();

    // Simulates a config written by an older version: no config_version,
    // most fields absent.
    std::fs::create_dir_all(config_file.parent().unwrap()).unwrap();
    std::fs::write(
        &config_file,
        r#"{
            "model": "ParakeetV2",
            "hotkey": ["Ctrl", "Space"],
            "auto_paste": false
        }"#,
    )
    .unwrap();

    let config = AppConfig::load().unwrap();
    assert_eq!(config.model, ModelVariant::ParakeetV2);
    assert_eq!(config.hotkey, vec!["Ctrl", "Space"]);
    assert!(!config.auto_paste);
    // Missing fields fall back to defaults.
    assert!(config.config_version >= 1);
    assert_eq!(config.num_threads, 4);
    assert!(config.sound_effects);
    assert!(config.show_tray_icon);
}

#[test]
fn load_tolerates_unknown_fields_in_config_file() {
    let (_guard, config_file) = isolated_config_dir();

    // Simulates a config written by a NEWER version with fields this
    // build doesn't know about.
    std::fs::create_dir_all(config_file.parent().unwrap()).unwrap();
    std::fs::write(
        &config_file,
        r#"{
            "model": "ParakeetV3",
            "some_future_field": 42,
            "another": {"nested": true}
        }"#,
    )
    .unwrap();

    let config = AppConfig::load().unwrap();
    assert_eq!(config.model, ModelVariant::ParakeetV3);
}

#[test]
fn load_save_round_trip_preserves_unknown_fields_on_disk() {
    // canario-dmp.22 downgrade safety: a user upgrades (the new binary
    // writes new fields), then downgrades — the moment this (older)
    // build saves, the newer fields must survive. Load captures them
    // into `extra`; save writes them back alongside the known fields.
    let (_guard, config_file) = isolated_config_dir();
    std::fs::create_dir_all(config_file.parent().unwrap()).unwrap();
    std::fs::write(
        &config_file,
        r#"{
            "model": "ParakeetV2",
            "auto_paste": false,
            "future_number": 42,
            "future_bool": true,
            "future_object": {"nested": {"deep": [1, 2, 3]}}
        }"#,
    )
    .unwrap();

    let config = AppConfig::load().unwrap();
    assert_eq!(config.model, ModelVariant::ParakeetV2);
    assert!(!config.auto_paste);
    config.save().unwrap();

    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_file).unwrap()).unwrap();
    // Known fields keep their loaded values…
    assert_eq!(raw["model"], serde_json::json!("ParakeetV2"));
    assert_eq!(raw["auto_paste"], serde_json::json!(false));
    // …and the unknown keys survived the save with identical values.
    assert_eq!(raw["future_number"], serde_json::json!(42));
    assert_eq!(raw["future_bool"], serde_json::json!(true));
    assert_eq!(
        raw["future_object"],
        serde_json::json!({ "nested": { "deep": [1, 2, 3] } })
    );
    // A reload still sees them (stable across repeated round trips).
    let reloaded = AppConfig::load().unwrap();
    assert_eq!(
        reloaded.extra.get("future_number"),
        Some(&serde_json::json!(42))
    );
}

#[test]
fn corrupt_json_file_is_quarantined_and_defaults_load() {
    // canario-dmp.16: a corrupt config no longer hard-errors from
    // `load` (which aborted the sidecar and the GTK app at boot,
    // surfacing as "sidecar not running"). It is quarantined next to
    // the live file and defaults are served instead.
    let (_guard, config_file) = isolated_config_dir();
    std::fs::create_dir_all(config_file.parent().unwrap()).unwrap();
    let corrupt = "{ this is not json";
    std::fs::write(&config_file, corrupt).unwrap();

    let config = AppConfig::load().unwrap();
    assert_eq!(config.model, ModelVariant::ParakeetV3);

    // Original bytes preserved in a .corrupt-* sibling...
    let dir = config_file.parent().unwrap();
    let mut quarantined: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("config.json.corrupt-"))
                .unwrap_or(false)
        })
        .collect();
    quarantined.sort();
    assert_eq!(quarantined.len(), 1);
    assert_eq!(std::fs::read_to_string(&quarantined[0]).unwrap(), corrupt);

    // ...and the live file is now valid defaults that reload cleanly.
    let reloaded = AppConfig::load().unwrap();
    assert_eq!(
        serde_json::to_value(&reloaded).unwrap(),
        serde_json::to_value(AppConfig::default()).unwrap()
    );
}

// ── Pure serde layer (no env isolation needed) ───────────────────────────

#[test]
fn empty_json_deserializes_to_all_defaults() {
    let config: AppConfig = serde_json::from_str("{}").unwrap();
    let default = AppConfig::default();
    assert_eq!(
        serde_json::to_value(&config).unwrap(),
        serde_json::to_value(&default).unwrap()
    );
}

#[test]
fn serde_round_trip_preserves_all_fields() {
    let config = AppConfig {
        model: ModelVariant::Custom,
        custom_encoder_path: Some("/tmp/enc.onnx".into()),
        custom_decoder_path: Some("/tmp/dec.onnx".into()),
        custom_tokens_path: Some("/tmp/tokens.txt".into()),
        autostart: true,
        double_tap_only: true,
        ..AppConfig::default()
    };

    let json = serde_json::to_string_pretty(&config).unwrap();
    let loaded: AppConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(
        serde_json::to_value(&loaded).unwrap(),
        serde_json::to_value(&config).unwrap()
    );
}
