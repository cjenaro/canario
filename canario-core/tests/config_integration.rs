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
fn corrupt_json_file_does_not_parse() {
    // Documents current behavior: a corrupt config file is a hard error
    // from `load` (unlike history, which falls back to empty). If the
    // policy changes to "fall back to defaults", update this test.
    let (_guard, config_file) = isolated_config_dir();
    std::fs::create_dir_all(config_file.parent().unwrap()).unwrap();
    std::fs::write(&config_file, "{ this is not json").unwrap();

    assert!(AppConfig::load().is_err());
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
