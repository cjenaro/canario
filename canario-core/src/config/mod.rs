pub mod autostart;

use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AudioBehavior {
    /// Don't touch system audio
    DoNothing,
    /// Mute system audio while recording
    Mute,
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
        assert!(config.custom_encoder_path.is_none());
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
