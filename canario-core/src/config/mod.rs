pub mod autostart;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::inference::postprocess::PostProcessor;

/// Current config schema version. Bump when making breaking changes.
pub const CONFIG_VERSION: u32 = 1;

/// Defaults for missing fields come from the `Default` impl, so old
/// config files from earlier versions keep loading after upgrades.
/// Unknown fields are ignored by serde_json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Config schema version (for future migrations)
    pub config_version: u32,

    /// Selected model variant
    pub model: ModelVariant,

    /// Global hotkey (key names, e.g., ["Super", "Alt", "Space"])
    pub hotkey: Vec<String>,

    /// Minimum key hold time in seconds before recording starts
    pub minimum_key_time: f64,

    /// Enable double-tap to lock recording
    pub double_tap_lock: bool,

    /// Use double-tap only (no press-and-hold)
    pub double_tap_only: bool,

    /// Audio behavior during recording
    pub recording_audio_behavior: AudioBehavior,

    /// Auto-paste transcription result
    pub auto_paste: bool,

    /// Show system tray icon
    pub show_tray_icon: bool,

    /// Custom model paths (if not using built-in download)
    pub custom_encoder_path: Option<PathBuf>,
    pub custom_decoder_path: Option<PathBuf>,
    pub custom_tokens_path: Option<PathBuf>,

    /// Number of inference threads (0 = auto)
    pub num_threads: u32,

    /// Post-processing rules for transcription text
    pub post_processor: PostProcessor,

    /// Autostart on login
    pub autostart: bool,

    /// Play sound effects on recording start/stop
    pub sound_effects: bool,

    /// Stream live caption previews (PartialTranscript events) in the
    /// overlay during long recordings
    pub live_captions: bool,

    /// Seconds of continuous recording before live captions kick in.
    /// Shorter recordings stay silent (no partial decodes).
    pub live_captions_threshold_secs: f64,

    /// UI theme mode: dark, light, or follow the OS preference (system).
    /// The Electron renderer applies it before first paint — see
    /// canario-app/src/renderer/theme.ts and index.html.
    pub theme: ThemeMode,

    /// Custom accent color as a hex string (e.g. "#e94560"). `None` uses
    /// the per-theme default accent from themes.css.
    pub accent_color: Option<String>,

    /// User-dragged overlay island placement, per monitor. Keyed by the
    /// Electron `Display.id` (stable per connected monitor, serialized as
    /// a string because JSON object keys are strings); values are the
    /// island's top-left offset from that monitor's origin in DIPs.
    /// Monitors without an entry (or an empty map) fall back to the
    /// default top-center placement. Electron-side only for now — the
    /// GTK app keeps its fixed placement (see canario-aud.1).
    pub overlay_offsets: BTreeMap<String, OverlayOffset>,

    /// Animation preferences (Settings → Appearance → Motion, PRD §8.4).
    /// The Electron renderer resolves this block together with the OS
    /// `prefers-reduced-motion` media query into `data-animations` /
    /// `data-anim-*` attributes on the document root — see
    /// canario-app/src/renderer/primitives/animations.ts (resolution),
    /// motion.ts (application) and styles/animations.css (gating).
    /// Defaults keep every effect on (the pre-existing behavior).
    pub animations: AnimationSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ModelVariant {
    /// Parakeet TDT v2 - English only
    ParakeetV2,
    /// Parakeet TDT v3 - Multilingual
    ParakeetV3,
    /// Custom ONNX model
    Custom,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum AudioBehavior {
    /// Don't touch system audio
    DoNothing,
    /// Mute system audio while recording
    Mute,
}

/// Overlay island placement for one monitor: the island's top-left
/// offset from that monitor's origin, in device-independent pixels
/// (DIPs — CSS pixels in the Electron overlay window, which exactly
/// covers the monitor).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OverlayOffset {
    pub x: i32,
    pub y: i32,
}

/// UI theme mode (Settings → Appearance). Serialized lowercase to match
/// the renderer's vocabulary ("dark" | "light" | "system"). `System`
/// follows the OS `prefers-color-scheme` media query.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Dark,
    Light,
    System,
}

/// Animation preferences (Settings → Appearance → Motion). `enabled` is
/// the master switch; each flag gates one PRD §8.4 effect — the
/// recording overlay slide-in (`overlay_slide`), the pulsing recording
/// dot (`recording_dot_pulse`), the toggle-switch slide + color change
/// (`toggle_slide`), the history-item delete slide-out (`delete_slide`)
/// and the window-open fade + scale (`window_fade`).
///
/// The OS `prefers-reduced-motion` request overrides all of it in the
/// renderer (force-disable, independent of the stored toggles). All
/// flags default to true so old configs keep every effect running.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AnimationSettings {
    /// Master switch — off disables every effect.
    pub enabled: bool,
    /// Recording overlay slide-down + fade on appear.
    pub overlay_slide: bool,
    /// Pulsing red recording dot.
    pub recording_dot_pulse: bool,
    /// Toggle-switch slide + color change.
    pub toggle_slide: bool,
    /// History-item slide-left + fade on delete.
    pub delete_slide: bool,
    /// Window-open fade + scale.
    pub window_fade: bool,
}

impl Default for AnimationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            overlay_slide: true,
            recording_dot_pulse: true,
            toggle_slide: true,
            delete_slide: true,
            window_fade: true,
        }
    }
}

/// Resolved filesystem paths to the four sherpa-onnx model files.
///
/// Used as the recognizer cache key: a config change that resolves to
/// different paths reloads the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPaths {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

impl ModelPaths {
    /// All four model files exist on disk.
    pub fn all_exist(&self) -> bool {
        self.encoder.exists()
            && self.decoder.exists()
            && self.joiner.exists()
            && self.tokens.exists()
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            model: ModelVariant::ParakeetV3,
            hotkey: vec!["Super".into(), "Alt".into(), "Space".into()],
            minimum_key_time: 0.2,
            double_tap_lock: true,
            double_tap_only: false,
            recording_audio_behavior: AudioBehavior::DoNothing,
            auto_paste: true,
            show_tray_icon: true,
            custom_encoder_path: None,
            custom_decoder_path: None,
            custom_tokens_path: None,
            num_threads: 4,
            post_processor: PostProcessor::default(),
            autostart: false,
            sound_effects: true,
            live_captions: true,
            live_captions_threshold_secs: 8.0,
            theme: ThemeMode::Dark,
            accent_color: None,
            overlay_offsets: BTreeMap::new(),
            animations: AnimationSettings::default(),
        }
    }
}

impl AppConfig {
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("canario")
    }

    pub fn config_file() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    pub fn models_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/share"))
            .join("canario")
            .join("models")
    }

    pub fn load() -> anyhow::Result<Self> {
        let path = Self::config_file();
        if !path.exists() {
            let config = Self::default();
            config.save()?;
            return Ok(config);
        }
        let data = std::fs::read_to_string(&path)?;
        let config: AppConfig = serde_json::from_str(&data)?;
        Ok(config)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let dir = Self::config_dir();
        std::fs::create_dir_all(&dir)?;
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(Self::config_file(), data)?;
        Ok(())
    }

    /// Get the model download URLs based on selected variant
    pub fn model_hf_repo(&self) -> &'static str {
        match self.model {
            ModelVariant::ParakeetV2 => "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8",
            ModelVariant::ParakeetV3 => "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
            ModelVariant::Custom => "",
        }
    }

    /// Get the local model directory for the selected variant
    pub fn local_model_dir(&self) -> PathBuf {
        match self.model {
            ModelVariant::ParakeetV2 => Self::models_dir().join("sherpa-parakeet-tdt-v2"),
            ModelVariant::ParakeetV3 => Self::models_dir().join("sherpa-parakeet-tdt-v3"),
            ModelVariant::Custom => {
                if let Some(p) = &self.custom_encoder_path {
                    p.parent().unwrap_or(&Self::models_dir()).to_path_buf()
                } else {
                    Self::models_dir()
                }
            }
        }
    }

    /// Resolve the filesystem paths of the four model files.
    ///
    /// Built-in variants use fixed file names inside [`Self::local_model_dir`].
    /// `ModelVariant::Custom` uses the configured `custom_*_path`s (the
    /// joiner is expected next to the encoder); errors if any custom
    /// path is unset.
    pub fn model_paths(&self) -> anyhow::Result<ModelPaths> {
        match self.model {
            ModelVariant::ParakeetV2 | ModelVariant::ParakeetV3 => {
                let dir = self.local_model_dir();
                Ok(ModelPaths {
                    encoder: dir.join("encoder.int8.onnx"),
                    decoder: dir.join("decoder.int8.onnx"),
                    joiner: dir.join("joiner.int8.onnx"),
                    tokens: dir.join("tokens.txt"),
                })
            }
            ModelVariant::Custom => {
                let encoder = self.custom_encoder_path.clone().ok_or_else(|| {
                    anyhow::anyhow!("Custom model selected but custom_encoder_path is not set")
                })?;
                let decoder = self.custom_decoder_path.clone().ok_or_else(|| {
                    anyhow::anyhow!("Custom model selected but custom_decoder_path is not set")
                })?;
                let tokens = self.custom_tokens_path.clone().ok_or_else(|| {
                    anyhow::anyhow!("Custom model selected but custom_tokens_path is not set")
                })?;
                let joiner = encoder
                    .parent()
                    .map(|dir| dir.join("joiner.int8.onnx"))
                    .ok_or_else(|| {
                        anyhow::anyhow!("custom_encoder_path has no parent directory")
                    })?;
                Ok(ModelPaths {
                    encoder,
                    decoder,
                    joiner,
                    tokens,
                })
            }
        }
    }

    /// Check if model files exist locally
    pub fn is_model_downloaded(&self) -> bool {
        match self.model_paths() {
            Ok(paths) => paths.all_exist(),
            // Custom variant with unconfigured paths is never "ready".
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_old_config_missing_fields() {
        // Simulates a config written by an older version: no
        // `config_version`, and several fields missing entirely.
        let json = r#"{
            "model": "ParakeetV2",
            "hotkey": ["Ctrl", "Space"],
            "auto_paste": false
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.config_version, CONFIG_VERSION);
        assert_eq!(config.model, ModelVariant::ParakeetV2);
        assert_eq!(config.hotkey, vec!["Ctrl", "Space"]);
        assert!(!config.auto_paste);
        // Missing fields fall back to defaults
        assert_eq!(config.minimum_key_time, 0.2);
        assert!(config.double_tap_lock);
        assert!(!config.double_tap_only);
        assert_eq!(config.recording_audio_behavior, AudioBehavior::DoNothing);
        assert!(config.show_tray_icon);
        assert_eq!(config.num_threads, 4);
        assert!(!config.autostart);
        assert!(config.sound_effects);
        // Live captions default to on with the long-session threshold
        assert!(config.live_captions);
        assert_eq!(config.live_captions_threshold_secs, 8.0);
        assert!(config.custom_encoder_path.is_none());
        // Appearance defaults: dark theme, per-theme default accent
        assert_eq!(config.theme, ThemeMode::Dark);
        assert_eq!(config.accent_color, None);
        // Overlay placement defaults to "not user-positioned yet"
        assert!(config.overlay_offsets.is_empty());
        // Animations default to fully on (existing behavior)
        assert_eq!(config.animations, AnimationSettings::default());
    }

    #[test]
    fn ignores_unknown_fields() {
        // Simulates a config written by a NEWER version with fields
        // this build doesn't know about.
        let json = r#"{
            "model": "ParakeetV3",
            "some_future_field": 42,
            "another": {"nested": true}
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.model, ModelVariant::ParakeetV3);
    }

    #[test]
    fn round_trip_save_load() {
        let config = AppConfig::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.config_version, CONFIG_VERSION);
        assert_eq!(loaded.model, config.model);
        assert_eq!(loaded.hotkey, config.hotkey);
        assert_eq!(loaded.num_threads, config.num_threads);
        assert_eq!(loaded.auto_paste, config.auto_paste);
        assert_eq!(loaded.sound_effects, config.sound_effects);
        assert_eq!(loaded.live_captions, config.live_captions);
        assert_eq!(
            loaded.live_captions_threshold_secs,
            config.live_captions_threshold_secs
        );
        assert_eq!(loaded.theme, config.theme);
        assert_eq!(loaded.accent_color, config.accent_color);
        assert_eq!(loaded.overlay_offsets, config.overlay_offsets);
        assert_eq!(loaded.animations, config.animations);
    }

    #[test]
    fn parses_appearance_fields() {
        let json = r##"{ "theme": "system", "accent_color": "#3b82f6" }"##;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.theme, ThemeMode::System);
        assert_eq!(config.accent_color.as_deref(), Some("#3b82f6"));
        // Untouched fields fall back to defaults
        assert_eq!(config.model, ModelVariant::ParakeetV3);
    }

    #[test]
    fn appearance_fields_round_trip() {
        let config = AppConfig {
            theme: ThemeMode::Light,
            accent_color: Some("#e94560".into()),
            ..AppConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        // Serialized lowercase to match the renderer's vocabulary
        assert!(json.contains(r##""theme":"light""##));
        assert!(json.contains(r##""accent_color":"#e94560""##));
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.theme, ThemeMode::Light);
        assert_eq!(loaded.accent_color.as_deref(), Some("#e94560"));
    }

    #[test]
    fn accent_color_none_round_trips_as_null() {
        let config = AppConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains(r#""accent_color":null"#));
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.accent_color, None);
    }

    #[test]
    fn overlay_offsets_default_to_empty_map() {
        // Old configs (and `{}`) have no overlay_offsets key — the
        // island then uses the default top-center placement.
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(config.overlay_offsets.is_empty());
        // The default config serializes the empty map explicitly.
        let json = serde_json::to_string(&AppConfig::default()).unwrap();
        assert!(json.contains(r#""overlay_offsets":{}"#));
    }

    #[test]
    fn parses_overlay_offsets() {
        // Multi-monitor: one entry per display id (JSON keys are strings).
        let json = r#"{
            "overlay_offsets": {
                "2305843009213693953": { "x": 640, "y": 12 },
                "7": { "x": -20, "y": 900 }
            }
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.overlay_offsets.get("2305843009213693953"),
            Some(&OverlayOffset { x: 640, y: 12 })
        );
        assert_eq!(
            config.overlay_offsets.get("7"),
            Some(&OverlayOffset { x: -20, y: 900 })
        );
        assert_eq!(config.overlay_offsets.len(), 2);
        // Untouched fields fall back to defaults
        assert_eq!(config.model, ModelVariant::ParakeetV3);
    }

    #[test]
    fn overlay_offsets_round_trip() {
        let config = AppConfig {
            overlay_offsets: BTreeMap::from([
                ("42".to_string(), OverlayOffset { x: 100, y: 200 }),
                ("43".to_string(), OverlayOffset { x: 0, y: 0 }),
            ]),
            ..AppConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        // Per-monitor entries serialize as { "x": .., "y": .. } — the
        // wire shape the Electron renderer reads and writes.
        assert!(json.contains(r#""overlay_offsets":{"42":{"x":100,"y":200},"43":{"x":0,"y":0}}"#));
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.overlay_offsets, config.overlay_offsets);
    }

    #[test]
    fn overlay_offsets_apply_as_a_whole_key() {
        // update_config merges top-level keys wholesale: the renderer
        // always sends the FULL map (existing entries merged client-side
        // plus the changed monitor), so other monitors' placements
        // survive an update. Model that merge here.
        let current_json = serde_json::to_value(&AppConfig {
            overlay_offsets: BTreeMap::from([("1".to_string(), OverlayOffset { x: 10, y: 20 })]),
            ..AppConfig::default()
        })
        .unwrap();
        let mut merged = current_json.clone();
        merged["overlay_offsets"] = serde_json::json!({
            "1": { "x": 11, "y": 22 },
            "2": { "x": 300, "y": 400 }
        });
        let merged: AppConfig = serde_json::from_value(merged).unwrap();
        assert_eq!(
            merged.overlay_offsets.get("1"),
            Some(&OverlayOffset { x: 11, y: 22 })
        );
        assert_eq!(
            merged.overlay_offsets.get("2"),
            Some(&OverlayOffset { x: 300, y: 400 })
        );
        // Clearing the map (reset to default) round-trips as empty.
        let mut cleared = current_json;
        cleared["overlay_offsets"] = serde_json::json!({});
        let cleared: AppConfig = serde_json::from_value(cleared).unwrap();
        assert!(cleared.overlay_offsets.is_empty());
    }

    #[test]
    fn animations_default_to_all_on() {
        // Old configs (and `{}`) have no animations key — the master
        // switch stays on and every effect keeps running.
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.animations, AnimationSettings::default());
        assert!(config.animations.enabled);
        assert!(config.animations.overlay_slide);
        assert!(config.animations.recording_dot_pulse);
        assert!(config.animations.toggle_slide);
        assert!(config.animations.delete_slide);
        assert!(config.animations.window_fade);
        // The default config serializes the full block explicitly —
        // the wire shape the Electron renderer reads and writes.
        let json = serde_json::to_string(&AppConfig::default()).unwrap();
        assert!(json.contains(
            r#""animations":{"enabled":true,"overlay_slide":true,"recording_dot_pulse":true,"toggle_slide":true,"delete_slide":true,"window_fade":true}"#
        ));
    }

    #[test]
    fn parses_animations() {
        let json = r#"{
            "animations": {
                "enabled": false,
                "overlay_slide": true,
                "recording_dot_pulse": false,
                "toggle_slide": true,
                "delete_slide": false,
                "window_fade": true
            }
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert!(!config.animations.enabled);
        assert!(config.animations.overlay_slide);
        assert!(!config.animations.recording_dot_pulse);
        assert!(config.animations.toggle_slide);
        assert!(!config.animations.delete_slide);
        assert!(config.animations.window_fade);
        // Untouched fields fall back to defaults
        assert_eq!(config.model, ModelVariant::ParakeetV3);
    }

    #[test]
    fn animations_partial_block_uses_defaults() {
        // serde(default) on the block: subfields missing from the wire
        // (e.g. written by a NEWER version) keep their defaults.
        let config: AppConfig =
            serde_json::from_str(r#"{"animations":{"enabled":false}}"#).unwrap();
        assert!(!config.animations.enabled);
        assert!(config.animations.overlay_slide);
        assert!(config.animations.recording_dot_pulse);
        assert!(config.animations.toggle_slide);
        assert!(config.animations.delete_slide);
        assert!(config.animations.window_fade);
    }

    #[test]
    fn animations_round_trip() {
        let config = AppConfig {
            animations: AnimationSettings {
                enabled: true,
                overlay_slide: true,
                recording_dot_pulse: false,
                toggle_slide: false,
                delete_slide: true,
                window_fade: false,
            },
            ..AppConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        // Serialized with the snake_case keys the renderer's
        // primitives/animations.ts mirrors.
        assert!(json.contains(r#""recording_dot_pulse":false"#));
        assert!(json.contains(r#""window_fade":false"#));
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.animations, config.animations);
    }

    #[test]
    fn animations_apply_as_a_whole_key() {
        // update_config merges top-level keys wholesale: the renderer
        // always sends the FULL block (existing flags merged client-side
        // plus the change), so sibling toggles survive an update. Model
        // that merge here.
        let current_json = serde_json::to_value(&AppConfig {
            animations: AnimationSettings {
                enabled: true,
                overlay_slide: true,
                recording_dot_pulse: false,
                toggle_slide: true,
                delete_slide: true,
                window_fade: true,
            },
            ..AppConfig::default()
        })
        .unwrap();
        let mut merged = current_json.clone();
        merged["animations"] = serde_json::json!({
            "enabled": true,
            "overlay_slide": true,
            "recording_dot_pulse": false,
            "toggle_slide": false,
            "delete_slide": true,
            "window_fade": true
        });
        let merged: AppConfig = serde_json::from_value(merged).unwrap();
        assert!(!merged.animations.toggle_slide);
        // The sibling flag survived the whole-key replacement.
        assert!(!merged.animations.recording_dot_pulse);
        // A PARTIAL block resets unmentioned flags to defaults instead
        // of keeping them — which is why the renderer always sends
        // every key (animationsConfigPayload).
        let mut partial = current_json;
        partial["animations"] = serde_json::json!({ "enabled": false });
        let partial: AppConfig = serde_json::from_value(partial).unwrap();
        assert!(!partial.animations.enabled);
        assert!(partial.animations.recording_dot_pulse);
    }

    #[test]
    fn empty_json_uses_all_defaults() {
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.config_version, CONFIG_VERSION);
        assert_eq!(config.model, ModelVariant::ParakeetV3);
        assert_eq!(config.hotkey, vec!["Super", "Alt", "Space"]);
    }

    #[test]
    fn model_paths_builtin_uses_fixed_names() {
        let config = AppConfig {
            model: ModelVariant::ParakeetV3,
            ..AppConfig::default()
        };
        let paths = config.model_paths().unwrap();
        let dir = config.local_model_dir();
        assert_eq!(paths.encoder, dir.join("encoder.int8.onnx"));
        assert_eq!(paths.decoder, dir.join("decoder.int8.onnx"));
        assert_eq!(paths.joiner, dir.join("joiner.int8.onnx"));
        assert_eq!(paths.tokens, dir.join("tokens.txt"));
    }

    #[test]
    fn model_paths_custom_uses_configured_paths() {
        let config = AppConfig {
            model: ModelVariant::Custom,
            custom_encoder_path: Some(PathBuf::from("/opt/models/my-enc.onnx")),
            custom_decoder_path: Some(PathBuf::from("/opt/models/my-dec.onnx")),
            custom_tokens_path: Some(PathBuf::from("/opt/models/tokens.txt")),
            ..AppConfig::default()
        };
        let paths = config.model_paths().unwrap();
        assert_eq!(paths.encoder, PathBuf::from("/opt/models/my-enc.onnx"));
        assert_eq!(paths.decoder, PathBuf::from("/opt/models/my-dec.onnx"));
        assert_eq!(paths.tokens, PathBuf::from("/opt/models/tokens.txt"));
        // Joiner has no dedicated setting — expected next to the encoder.
        assert_eq!(paths.joiner, PathBuf::from("/opt/models/joiner.int8.onnx"));
    }

    #[test]
    fn model_paths_custom_errors_when_paths_missing() {
        let config = AppConfig {
            model: ModelVariant::Custom,
            ..AppConfig::default()
        };
        let err = config.model_paths().unwrap_err();
        assert!(err.to_string().contains("custom_encoder_path"));
        // And the variant therefore never reports ready.
        assert!(!config.is_model_downloaded());
    }

    #[test]
    fn is_model_downloaded_custom_checks_custom_paths() {
        let dir = std::env::temp_dir().join(format!("canario-cfg-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let encoder = dir.join("custom-enc.onnx");
        let decoder = dir.join("custom-dec.onnx");
        let joiner = dir.join("joiner.int8.onnx");
        let tokens = dir.join("custom-tokens.txt");

        let config = AppConfig {
            model: ModelVariant::Custom,
            custom_encoder_path: Some(encoder.clone()),
            custom_decoder_path: Some(decoder.clone()),
            custom_tokens_path: Some(tokens.clone()),
            ..AppConfig::default()
        };
        assert!(!config.is_model_downloaded());

        for f in [&encoder, &decoder, &joiner, &tokens] {
            std::fs::write(f, b"x").unwrap();
        }
        assert!(config.is_model_downloaded());

        std::fs::remove_dir_all(&dir).ok();
    }
}
