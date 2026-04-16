use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Selected model variant
    pub model: ModelVariant,

    /// Global hotkey (key names, e.g., ["Super", "Space"])
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

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            model: ModelVariant::ParakeetV3,
            hotkey: vec!["Super".into(), "Space".into()],
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

    /// Check if model files exist locally
    pub fn is_model_downloaded(&self) -> bool {
        let dir = self.local_model_dir();
        dir.join("encoder.int8.onnx").exists()
            && dir.join("decoder.int8.onnx").exists()
            && dir.join("joiner.int8.onnx").exists()
            && dir.join("tokens.txt").exists()
    }
}
