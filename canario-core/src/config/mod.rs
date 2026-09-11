pub mod autostart;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::inference::postprocess::PostProcessor;
use crate::transform::TransformRule;

/// Current config schema version. Bump when making breaking changes.
pub const CONFIG_VERSION: u32 = 1;

/// Defaults for missing fields come from the `Default` impl, so old
/// config files from earlier versions keep loading after upgrades.
/// Unknown fields are captured into [`Self::extra`] and preserved on
/// save, so a load→save round trip never destroys them.
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

    /// Window in milliseconds within which two taps of the hotkey count
    /// as a double-tap (locking the recording on). Read by the hotkey
    /// processor when the listener starts — frontends restart the
    /// hotkey after changing it.
    pub double_tap_timeout_ms: u64,

    /// Use double-tap only (no press-and-hold)
    pub double_tap_only: bool,

    /// Milliseconds a modifier-only hotkey must be held before it counts
    /// as an activation (any other key pressed during the window cancels
    /// it, so normal modifier use like Super+C is unaffected). Advanced
    /// knob — no settings UI; edit config.json directly.
    pub modifier_threshold_ms: u64,

    /// Audio behavior during recording
    pub recording_audio_behavior: AudioBehavior,

    /// Preferred audio input device for dictation, by name
    /// (canario-1hq.2). Empty (the default) = the system default
    /// device — the pre-picker behavior, so old configs keep loading
    /// unchanged. A name that no longer matches an enumerated device
    /// falls back to the default (with a warning) at open time. The
    /// backend pushes this into the warm-mic preference whenever the
    /// config loads or changes, so a switch applies to the next
    /// recording without a restart.
    pub input_device: String,

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

    /// Volume of the sound effects (0.0 = silent, 1.0 = loudest).
    /// Values outside the range are clamped when the beeps are
    /// generated (see `audio::effects::clamp_volume`), so a
    /// hand-edited config cannot produce an invalid amplitude.
    pub sound_effects_volume: f32,

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

    /// On-screen indicator presence while dictating (canario-aud.2):
    /// "full" — the recording island (pill, timer, live captions,
    ///          transcribing/transforming phases; the default and the
    ///          pre-setting behavior),
    /// "dot"  — a minimal pulsing dot while recording only,
    /// "tray" — no on-screen indicator at all; the tray icon's state
    ///          carries the signal.
    /// Stored as a raw string (input_device pattern) so unknown values
    /// degrade to the default instead of quarantining the whole config:
    /// the deserializer normalizes anything unrecognised to "full".
    #[serde(deserialize_with = "deserialize_overlay_presence")]
    pub overlay_presence: String,

    /// Animation preferences (Settings → Appearance → Motion, PRD §8.4).
    /// The Electron renderer resolves this block together with the OS
    /// `prefers-reduced-motion` media query into `data-animations` /
    /// `data-anim-*` attributes on the document root — see
    /// canario-app/src/renderer/primitives/animations.ts (resolution),
    /// motion.ts (application) and styles/animations.css (gating).
    /// Defaults keep every effect on (the pre-existing behavior).
    pub animations: AnimationSettings,

    /// Onboarding wizard completion flag (PRD §5.1, canario-xv9): true
    /// once the user finished (or skipped) the first-launch wizard. The
    /// Electron main process reads and flips it through the sidecar's
    /// `get_config` / `update_config` commands, so all app state lives
    /// in this one config. Defaults to false — fresh installs (and old
    /// config files written before the field existed) run the wizard.
    pub onboarding_completed: bool,

    /// LLM transformation provider settings (canario-fgm epic; wire
    /// shape and privacy contract from canario-fgm.1 D1/D5). Absent or
    /// disabled (the default) keeps dictation fully on-device —
    /// byte-identical to the pre-transform behavior.
    pub transform: TransformSettings,

    /// Fields this build doesn't know about, preserved verbatim on save
    /// so a downgrade never destroys newer config (canario-dmp.22).
    /// Populated by serde via flatten; unknown keys in the FILE land here
    /// and serialize back alongside the known fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
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

/// Default per-request timeout for LLM transform calls (fgm.1 D5d:
/// 4000 ms — dictation must never wait long before falling back to the
/// raw transcript).
pub const DEFAULT_TRANSFORM_TIMEOUT_MS: u64 = 4000;

/// Clamp window for a hand-edited `transform.timeout_ms`: absurdly
/// small values can't be distinguished from an instant failure, and a
/// huge value would delay the (fgm.3/4) fallback to raw text for the
/// full duration.
const MIN_TRANSFORM_TIMEOUT_MS: u64 = 250;
const MAX_TRANSFORM_TIMEOUT_MS: u64 = 60_000;

/// LLM transformation settings (canario-fgm.2; decisions from
/// canario-fgm.1 are binding).
///
/// Default OFF (D5a): an absent or disabled block means the pipeline
/// is fully on-device and behaves byte-identically to today. The API
/// key is deliberately NOT part of this block — it is persisted by the
/// Electron main process via safeStorage and reaches the sidecar
/// memory-only through `set_transform_credential` (D2), so
/// config.json can never contain it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TransformSettings {
    /// Master switch. Default false — nothing is sent anywhere until
    /// the user configures a provider.
    pub enabled: bool,

    /// OpenAI-compatible endpoint metadata.
    pub provider: TransformProvider,

    /// Per-request timeout in milliseconds. On timeout (or any
    /// transform error) the pipeline falls back to the raw transcript
    /// (D5d — enforced where the pipeline lands, canario-fgm.3/4).
    pub timeout_ms: u64,

    /// Per-app transformation rules (fgm.3): first match wins,
    /// case-insensitive substring of the focused-app identifier against
    /// `app_match`; the empty `app_match` is the default (catch-all)
    /// rule. See `canario_core::transform::match_rule`.
    pub rules: Vec<TransformRule>,
}

impl Default for TransformSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: TransformProvider::default(),
            timeout_ms: DEFAULT_TRANSFORM_TIMEOUT_MS,
            rules: Vec::new(),
        }
    }
}

impl TransformSettings {
    /// Request timeout with hand-edited configs clamped into a sane
    /// window: `0` falls back to the default (an "instant" timeout
    /// would fail every request before falling back), small values
    /// clamp up to 250 ms and large values down to 60 s so dictation
    /// is never wedged waiting on a provider (D5d).
    pub fn effective_timeout(&self) -> std::time::Duration {
        let ms = if self.timeout_ms == 0 {
            DEFAULT_TRANSFORM_TIMEOUT_MS
        } else {
            self.timeout_ms
                .clamp(MIN_TRANSFORM_TIMEOUT_MS, MAX_TRANSFORM_TIMEOUT_MS)
        };
        std::time::Duration::from_millis(ms)
    }
}

/// Provider endpoint metadata for LLM transformations. OpenAI
/// chat-completions wire against any compatible `base_url` — cloud
/// (e.g. `https://api.openai.com/v1`) or local loopback servers
/// (Ollama `http://localhost:11434/v1`, llama.cpp server
/// `http://localhost:8080/v1`), which are first-class (D5c).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct TransformProvider {
    /// Base URL including any version path. `Authorization` is only
    /// sent when the sidecar holds an in-memory credential (D2).
    pub base_url: String,

    /// Model name sent in the chat-completions payload.
    pub model: String,
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

/// Normalize a raw `overlay_presence` value (canario-aud.2): the two
/// non-default modes pass through, everything else — missing, empty, a
/// typo, a value written by a newer build — reads as "full", the
/// default. Used by the config deserializer so a hand-edited or
/// forward-written config loads instead of quarantining.
fn normalize_overlay_presence(value: &str) -> String {
    match value {
        "dot" | "tray" => value.to_string(),
        _ => "full".to_string(),
    }
}

/// `deserialize_with` for [`AppConfig::overlay_presence`]: parse the
/// wire string, then normalize unknown values to "full" (see
/// [`normalize_overlay_presence`]). A non-string value still fails the
/// field — and therefore quarantines the config — matching how every
/// other wrong-typed field behaves.
fn deserialize_overlay_presence<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    Ok(normalize_overlay_presence(&raw))
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            model: ModelVariant::ParakeetV3,
            hotkey: vec!["Super".into(), "Alt".into(), "Space".into()],
            minimum_key_time: 0.2,
            double_tap_lock: true,
            double_tap_timeout_ms: 300,
            double_tap_only: false,
            modifier_threshold_ms: 300,
            recording_audio_behavior: AudioBehavior::DoNothing,
            input_device: String::new(),
            auto_paste: true,
            show_tray_icon: true,
            custom_encoder_path: None,
            custom_decoder_path: None,
            custom_tokens_path: None,
            num_threads: 4,
            post_processor: PostProcessor::default(),
            autostart: false,
            sound_effects: true,
            sound_effects_volume: 0.3,
            live_captions: true,
            live_captions_threshold_secs: 8.0,
            theme: ThemeMode::Dark,
            accent_color: None,
            overlay_offsets: BTreeMap::new(),
            overlay_presence: "full".to_string(),
            animations: AnimationSettings::default(),
            onboarding_completed: false,
            transform: TransformSettings::default(),
            extra: BTreeMap::new(),
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

    /// Load the config from the default location. Never fails on a
    /// corrupt file: see [`Self::load_from`].
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(&Self::config_file())
    }

    /// Load the config stored at `path`.
    ///
    /// A file that cannot be read or parsed is quarantined (renamed to
    /// `config.json.corrupt-<timestamp>` next to it, original bytes
    /// preserved) and replaced with defaults, so a corrupted config
    /// never aborts startup — every other store degrades gracefully
    /// and the config must too.
    fn load_from(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            let config = Self::default();
            config.save_to(path)?;
            return Ok(config);
        }
        match std::fs::read_to_string(path)
            .map_err(anyhow::Error::from)
            .and_then(|data| serde_json::from_str(&data).map_err(anyhow::Error::from))
        {
            Ok(config) => Ok(config),
            Err(err) => Self::quarantine_and_reset(path, err),
        }
    }

    /// Quarantine the unusable config at `path` and continue from
    /// freshly saved defaults.
    ///
    /// Quarantining and writing the replacement are best-effort: even
    /// when both fail the app still boots with in-memory defaults
    /// rather than exiting with a cryptic "sidecar not running".
    fn quarantine_and_reset(path: &Path, err: anyhow::Error) -> anyhow::Result<Self> {
        let quarantine = corrupt_sibling_path(path);
        match std::fs::rename(path, &quarantine) {
            Ok(()) => tracing::warn!(
                "config file {} is unusable ({}); quarantined to {} and reset to defaults",
                path.display(),
                err,
                quarantine.display()
            ),
            Err(rename_err) => tracing::error!(
                "config file {} is unusable ({}); quarantining to {} failed ({}); resetting to defaults",
                path.display(),
                err,
                quarantine.display(),
                rename_err
            ),
        }
        let config = Self::default();
        if let Err(save_err) = config.save_to(path) {
            tracing::error!(
                "could not write default config to {} after quarantine ({}); continuing with in-memory defaults",
                path.display(),
                save_err
            );
        }
        Ok(config)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&Self::config_file())
    }

    fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)?;
        Ok(())
    }

    /// Quarantined config files in `dir`, oldest first.
    ///
    /// Consumed by the diagnostics blob so support can see that a
    /// config was reset and recover the user's settings from the
    /// preserved bytes.
    pub fn quarantined_files(dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("config.json.corrupt-"))
                    .unwrap_or(false)
            })
            .collect();
        files.sort();
        files
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

/// Quarantine target for a corrupt config: `config.json` becomes
/// `config.json.corrupt-<unix-nanos>` next to it. The nanosecond
/// timestamp makes repeat quarantines collision-free.
fn corrupt_sibling_path(path: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{file_name}.corrupt-{nanos}"))
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
        // Timing windows default to the old hardcoded 300 ms
        assert_eq!(config.double_tap_timeout_ms, 300);
        assert_eq!(config.modifier_threshold_ms, 300);
        assert_eq!(config.recording_audio_behavior, AudioBehavior::DoNothing);
        // No input_device key → the system default (canario-1hq.2)
        assert_eq!(config.input_device, "");
        assert!(config.show_tray_icon);
        assert_eq!(config.num_threads, 4);
        assert!(!config.autostart);
        assert!(config.sound_effects);
        // Beep loudness defaults to the old hardcoded 0.3 amplitude
        assert_eq!(config.sound_effects_volume, 0.3);
        // Live captions default to on with the long-session threshold
        assert!(config.live_captions);
        assert_eq!(config.live_captions_threshold_secs, 8.0);
        assert!(config.custom_encoder_path.is_none());
        // Appearance defaults: dark theme, per-theme default accent
        assert_eq!(config.theme, ThemeMode::Dark);
        assert_eq!(config.accent_color, None);
        // Overlay placement defaults to "not user-positioned yet"
        assert!(config.overlay_offsets.is_empty());
        // Indicator presence defaults to the full overlay (canario-aud.2)
        assert_eq!(config.overlay_presence, "full");
        // Animations default to fully on (existing behavior)
        assert_eq!(config.animations, AnimationSettings::default());
        // Onboarding defaults to not completed → old configs re-run the wizard
        assert!(!config.onboarding_completed);
        // Transform defaults to OFF with the D5 timeout → old configs
        // stay fully on-device (D5a).
        assert_eq!(config.transform, TransformSettings::default());
        assert!(!config.transform.enabled);
        assert_eq!(config.transform.timeout_ms, DEFAULT_TRANSFORM_TIMEOUT_MS);
        assert!(config.transform.rules.is_empty());
    }

    #[test]
    fn ignores_unknown_fields() {
        // Simulates a config written by a NEWER version with fields
        // this build doesn't know about. They don't break the load —
        // and (canario-dmp.22) they land in `extra` instead of being
        // dropped, so this build's saves can't destroy them.
        let json = r#"{
            "model": "ParakeetV3",
            "some_future_field": 42,
            "another": {"nested": true}
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.model, ModelVariant::ParakeetV3);
        assert_eq!(
            config.extra.get("some_future_field"),
            Some(&serde_json::json!(42))
        );
        assert_eq!(
            config.extra.get("another"),
            Some(&serde_json::json!({ "nested": true }))
        );
        assert_eq!(config.extra.len(), 2);
    }

    #[test]
    fn unknown_fields_survive_a_load_save_round_trip() {
        // canario-dmp.22 downgrade safety: a config written by a NEWER
        // version keeps its unknown keys (values byte-identical) after
        // this build loads and re-saves it.
        let json = r#"{
            "model": "ParakeetV3",
            "future_number": 42,
            "future_bool": false,
            "future_object": { "nested": { "deep": [1, 2, 3] } },
            "future_string": "kept"
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        let saved = serde_json::to_value(&config).unwrap();
        // Known field intact…
        assert_eq!(saved["model"], serde_json::json!("ParakeetV3"));
        // …and every unknown key survived with an identical value.
        assert_eq!(saved["future_number"], serde_json::json!(42));
        assert_eq!(saved["future_bool"], serde_json::json!(false));
        assert_eq!(
            saved["future_object"],
            serde_json::json!({ "nested": { "deep": [1, 2, 3] } })
        );
        assert_eq!(saved["future_string"], serde_json::json!("kept"));
        // A second round trip is stable (extras re-land in extra).
        let reloaded: AppConfig = serde_json::from_value(saved).unwrap();
        assert_eq!(
            serde_json::to_value(&reloaded).unwrap(),
            serde_json::to_value(&config).unwrap()
        );
    }

    #[test]
    fn default_config_serializes_without_extra_keys() {
        // An empty `extra` contributes nothing to the wire — the
        // serialized default is unchanged by the flatten field.
        let json = serde_json::to_string(&AppConfig::default()).unwrap();
        assert!(json.contains(r#""config_version":1"#));
        assert!(!json.contains("extra"));
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(loaded.extra.is_empty());
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
        assert_eq!(loaded.sound_effects_volume, config.sound_effects_volume);
        assert_eq!(loaded.double_tap_timeout_ms, config.double_tap_timeout_ms);
        assert_eq!(loaded.modifier_threshold_ms, config.modifier_threshold_ms);
        assert_eq!(loaded.live_captions, config.live_captions);
        assert_eq!(
            loaded.live_captions_threshold_secs,
            config.live_captions_threshold_secs
        );
        assert_eq!(loaded.theme, config.theme);
        assert_eq!(loaded.accent_color, config.accent_color);
        assert_eq!(loaded.overlay_offsets, config.overlay_offsets);
        assert_eq!(loaded.overlay_presence, config.overlay_presence);
        assert_eq!(loaded.animations, config.animations);
        assert_eq!(loaded.onboarding_completed, config.onboarding_completed);
        assert_eq!(loaded.transform, config.transform);
        assert_eq!(loaded.input_device, config.input_device);
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
    fn overlay_presence_defaults_to_full_and_round_trips() {
        // Old configs (and `{}`) have no overlay_presence key — the full
        // overlay stays on, byte-identical to the pre-setting behavior.
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.overlay_presence, "full");
        // Explicit values parse and survive a save/load round trip…
        for mode in ["full", "dot", "tray"] {
            let config: AppConfig =
                serde_json::from_str(&format!(r#"{{ "overlay_presence": "{mode}" }}"#)).unwrap();
            assert_eq!(config.overlay_presence, mode);
            let json = serde_json::to_string(&config).unwrap();
            assert!(json.contains(&format!(r#""overlay_presence":"{mode}""#)));
            let loaded: AppConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(loaded.overlay_presence, mode);
        }
        // Untouched fields fall back to defaults
        let config: AppConfig = serde_json::from_str(r#"{ "overlay_presence": "dot" }"#).unwrap();
        assert_eq!(config.model, ModelVariant::ParakeetV3);
        // The default config serializes the field explicitly (pattern of
        // overlay_offsets) — the wire shape the Electron renderer reads.
        let json = serde_json::to_string(&AppConfig::default()).unwrap();
        assert!(json.contains(r#""overlay_presence":"full""#));
    }

    #[test]
    fn overlay_presence_unknown_values_deserialize_to_full() {
        // A hand-edited or newer-build value degrades to the default
        // instead of quarantining the config (String field, input_device
        // pattern — unlike the theme enum, which would quarantine).
        for unknown in ["banana", "FULL", "Dot", "", "minimal", "none"] {
            let config: AppConfig =
                serde_json::from_str(&format!(r#"{{ "overlay_presence": "{unknown}" }}"#))
                    .unwrap_or_else(|e| panic!("value {unknown:?} must load: {e}"));
            assert_eq!(config.overlay_presence, "full", "value: {unknown:?}");
            // The normalization is sticky: a save/load round trip writes
            // the DEFAULT back, not the unknown value.
            let json = serde_json::to_string(&config).unwrap();
            assert!(json.contains(r#""overlay_presence":"full""#));
        }
        // Null/absent both read as the default: an ABSENT key uses the
        // container-level serde(default); null is a wrong type for the
        // String field and quarantines, like every other field.
        let config: AppConfig = serde_json::from_str(r#"{ "auto_paste": false }"#).unwrap();
        assert_eq!(config.overlay_presence, "full");
    }

    #[test]
    fn onboarding_completed_defaults_false_and_round_trips() {
        // Old configs (and `{}`) have no onboarding_completed key — the
        // wizard then runs on next launch. A completed flag parses,
        // serializes snake_case, and survives a round trip.
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(!config.onboarding_completed);
        let config: AppConfig =
            serde_json::from_str(r#"{ "onboarding_completed": true }"#).unwrap();
        assert!(config.onboarding_completed);
        // Untouched fields fall back to defaults
        assert_eq!(config.model, ModelVariant::ParakeetV3);
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains(r#""onboarding_completed":true"#));
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(loaded.onboarding_completed);
        // Flipping it back off round-trips too (Settings → About re-run)
        let mut flipped = loaded;
        flipped.onboarding_completed = false;
        let json = serde_json::to_string(&flipped).unwrap();
        assert!(json.contains(r#""onboarding_completed":false"#));
        let reloaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(!reloaded.onboarding_completed);
    }

    #[test]
    fn hotkey_timing_fields_parse_and_round_trip() {
        // Old configs (and `{}`) have no timing keys — both windows
        // default to the previously hardcoded 300 ms.
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.double_tap_timeout_ms, 300);
        assert_eq!(config.modifier_threshold_ms, 300);
        // Explicit values parse snake_case...
        let config: AppConfig = serde_json::from_str(
            r#"{ "double_tap_timeout_ms": 450, "modifier_threshold_ms": 250 }"#,
        )
        .unwrap();
        assert_eq!(config.double_tap_timeout_ms, 450);
        assert_eq!(config.modifier_threshold_ms, 250);
        // Untouched fields fall back to defaults
        assert_eq!(config.model, ModelVariant::ParakeetV3);
        // ...and survive a round trip through save/load.
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains(r#""double_tap_timeout_ms":450"#));
        assert!(json.contains(r#""modifier_threshold_ms":250"#));
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.double_tap_timeout_ms, 450);
        assert_eq!(loaded.modifier_threshold_ms, 250);
    }

    #[test]
    fn sound_effects_volume_parses_and_round_trips() {
        // Old configs (and `{}`) have no volume key — the beeps keep
        // the previously hardcoded 0.3 amplitude.
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.sound_effects_volume, 0.3);
        // Explicit values parse and round-trip; out-of-range values are
        // STORED as-is and clamped when the beeps are generated (see
        // audio::effects::clamp_volume), so a hand-edited config loads
        // instead of erroring.
        let config: AppConfig =
            serde_json::from_str(r#"{ "sound_effects_volume": 0.75 }"#).unwrap();
        assert_eq!(config.sound_effects_volume, 0.75);
        let mut over = config.clone();
        over.sound_effects_volume = 1.5;
        let mut under = config;
        under.sound_effects_volume = -0.2;
        for config in [over, under] {
            let json = serde_json::to_string(&config).unwrap();
            let loaded: AppConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(loaded.sound_effects_volume, config.sound_effects_volume);
        }
        // The default config serializes the field explicitly.
        let json = serde_json::to_string(&AppConfig::default()).unwrap();
        assert!(json.contains(r#""sound_effects_volume":0.3"#));
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
    fn parses_transform_block() {
        // Explicit block parses with the D1 wire shape (enabled,
        // provider{base_url,model}, timeout_ms, rules) — rules are the
        // fgm.3 shape: {app_match, instruction}, empty app_match = the
        // default rule.
        let json = r#"{
            "transform": {
                "enabled": true,
                "provider": {
                    "base_url": "http://localhost:11434/v1",
                    "model": "llama3"
                },
                "timeout_ms": 1500,
                "rules": [
                    { "app_match": "firefox", "instruction": "be terse" },
                    { "app_match": "", "instruction": "tidy everything" }
                ]
            }
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert!(config.transform.enabled);
        assert_eq!(
            config.transform.provider.base_url,
            "http://localhost:11434/v1"
        );
        assert_eq!(config.transform.provider.model, "llama3");
        assert_eq!(config.transform.timeout_ms, 1500);
        assert_eq!(
            config.transform.rules,
            vec![
                TransformRule {
                    app_match: "firefox".into(),
                    instruction: "be terse".into()
                },
                TransformRule {
                    app_match: String::new(),
                    instruction: "tidy everything".into()
                },
            ]
        );
        // Untouched fields fall back to defaults
        assert_eq!(config.model, ModelVariant::ParakeetV3);
    }

    #[test]
    fn transform_rules_placeholder_shapes_still_load() {
        // fgm.2 shipped rules as raw JSON placeholders; those entries
        // deserialize without quarantining the whole config — old files
        // keep loading. The placeholder `instruction` field happens to
        // match the real shape (it becomes a working default rule's
        // instruction); only the unknown `app` key is ignored.
        let config: AppConfig = serde_json::from_str(
            r#"{ "transform": { "rules": [ { "app": "firefox", "instruction": "be terse" } ] } }"#,
        )
        .unwrap();
        assert_eq!(
            config.transform.rules,
            vec![TransformRule {
                app_match: String::new(),
                instruction: "be terse".into()
            }]
        );
        // A rule with only one field set keeps it, defaults the other.
        let config: AppConfig =
            serde_json::from_str(r#"{ "transform": { "rules": [ { "app_match": "vim" } ] } }"#)
                .unwrap();
        assert_eq!(
            config.transform.rules,
            vec![TransformRule {
                app_match: "vim".into(),
                instruction: String::new()
            }]
        );
    }

    #[test]
    fn transform_partial_block_uses_defaults() {
        // serde(default) on the block: subfields missing from the wire
        // (e.g. written by a NEWER version) keep their defaults.
        let config: AppConfig = serde_json::from_str(r#"{"transform":{"enabled":true}}"#).unwrap();
        assert!(config.transform.enabled);
        assert_eq!(config.transform.provider.base_url, "");
        assert_eq!(config.transform.provider.model, "");
        assert_eq!(config.transform.timeout_ms, DEFAULT_TRANSFORM_TIMEOUT_MS);
        assert!(config.transform.rules.is_empty());
    }

    #[test]
    fn transform_round_trip() {
        let config = AppConfig {
            transform: TransformSettings {
                enabled: true,
                provider: TransformProvider {
                    base_url: "https://api.openai.com/v1".into(),
                    model: "gpt-4o-mini".into(),
                },
                timeout_ms: 2500,
                rules: vec![
                    TransformRule {
                        app_match: "whatsapp".into(),
                        instruction: "be informal".into(),
                    },
                    TransformRule {
                        app_match: String::new(),
                        instruction: "tidy everything".into(),
                    },
                ],
            },
            ..AppConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains(r#""transform":{"enabled":true,"provider":{"base_url":"https://api.openai.com/v1","model":"gpt-4o-mini"},"timeout_ms":2500,"rules":[{"app_match":"whatsapp","instruction":"be informal"},{"app_match":"","instruction":"tidy everything"}]}"#));
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.transform, config.transform);
    }

    #[test]
    fn transform_default_serializes_disabled_block() {
        // The default config serializes the block explicitly (pattern
        // of animations/overlay_offsets) — enabled=false is the wire
        // proof of the D5a default-off posture.
        let json = serde_json::to_string(&AppConfig::default()).unwrap();
        assert!(json.contains(r#""transform":{"enabled":false,"provider":{"base_url":"","model":""},"timeout_ms":4000,"rules":[]}"#));
    }

    #[test]
    fn transform_apply_as_a_whole_key() {
        // update_config merges top-level keys wholesale: the renderer
        // must always send the FULL block (see the renderer's
        // transformConfigPayload) — a partial replacement resets
        // unmentioned subfields to defaults, exactly like animations.
        let current_json = serde_json::to_value(AppConfig {
            transform: TransformSettings {
                enabled: true,
                provider: TransformProvider {
                    base_url: "http://localhost:8080/v1".into(),
                    model: "qwen".into(),
                },
                timeout_ms: 4000,
                rules: vec![TransformRule {
                    app_match: "vim".into(),
                    instruction: String::new(),
                }],
            },
            ..AppConfig::default()
        })
        .unwrap();
        let mut merged = current_json.clone();
        merged["transform"] = serde_json::json!({
            "enabled": false,
            "provider": { "base_url": "https://api.openai.com/v1", "model": "gpt-4o-mini" },
            "timeout_ms": 4000,
            "rules": [{ "app_match": "vim", "instruction": "code comments only" }]
        });
        let merged: AppConfig = serde_json::from_value(merged).unwrap();
        assert!(!merged.transform.enabled);
        assert_eq!(merged.transform.provider.model, "gpt-4o-mini");
        // The rules entry survived because the FULL block travelled…
        assert_eq!(
            merged.transform.rules,
            vec![TransformRule {
                app_match: "vim".into(),
                instruction: "code comments only".into()
            }]
        );

        // A PARTIAL block resets unmentioned subfields — which is why
        // the renderer always sends every key.
        let mut partial = current_json;
        partial["transform"] = serde_json::json!({ "enabled": false });
        let partial: AppConfig = serde_json::from_value(partial).unwrap();
        assert_eq!(partial.transform.provider.model, "");
        assert!(partial.transform.rules.is_empty());
    }

    #[test]
    fn transform_timeout_clamped_into_sane_window() {
        // 0 → the 4 s default (an instant timeout would fail every
        // request before the D5d fallback could matter).
        assert_eq!(
            TransformSettings {
                timeout_ms: 0,
                ..TransformSettings::default()
            }
            .effective_timeout(),
            std::time::Duration::from_millis(DEFAULT_TRANSFORM_TIMEOUT_MS)
        );
        // Explicit in-window values pass through unchanged.
        for ms in [250u64, 1500, 4000, 60_000] {
            assert_eq!(
                TransformSettings {
                    timeout_ms: ms,
                    ..TransformSettings::default()
                }
                .effective_timeout(),
                std::time::Duration::from_millis(ms)
            );
        }
        // Out-of-window hand edits clamp to [250 ms, 60 s].
        assert_eq!(
            TransformSettings {
                timeout_ms: 1,
                ..TransformSettings::default()
            }
            .effective_timeout(),
            std::time::Duration::from_millis(250)
        );
        assert_eq!(
            TransformSettings {
                timeout_ms: u64::MAX,
                ..TransformSettings::default()
            }
            .effective_timeout(),
            std::time::Duration::from_millis(60_000)
        );
    }

    #[test]
    fn input_device_defaults_to_system_default_and_round_trips() {
        // canario-1hq.2: old configs (and `{}`) have no input_device
        // key — the system default stays selected, byte-identical to
        // the pre-picker behavior.
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.input_device, "");
        // Explicit values parse…
        let config: AppConfig = serde_json::from_str(r#"{ "input_device": "Yeti SB" }"#).unwrap();
        assert_eq!(config.input_device, "Yeti SB");
        // Untouched fields fall back to defaults
        assert_eq!(config.model, ModelVariant::ParakeetV3);
        // …survive a save/load round trip…
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains(r#""input_device":"Yeti SB""#));
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.input_device, "Yeti SB");
        // …and clearing back to the default round-trips too.
        let mut cleared = loaded;
        cleared.input_device = String::new();
        let json = serde_json::to_string(&cleared).unwrap();
        assert!(json.contains(r#""input_device":""#));
        let reloaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(reloaded.input_device, "");
    }

    #[test]
    fn empty_json_uses_all_defaults() {
        let config: AppConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.config_version, CONFIG_VERSION);
        assert_eq!(config.model, ModelVariant::ParakeetV3);
        assert_eq!(
            config.hotkey,
            vec!["Super".to_string(), "Alt".to_string(), "Space".to_string()]
        );
        assert_eq!(config.transform, TransformSettings::default());
        assert_eq!(config.input_device, "");
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

    // --- Corrupt-config quarantine (canario-dmp.16) ---

    fn temp_config_file(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, contents).unwrap();
        (dir, path)
    }

    /// Assert that a quarantining load happened: exactly one
    /// `.corrupt-*` sibling preserving the original bytes, and a fresh
    /// `config.json` equal to the defaults.
    fn assert_quarantined_and_reset(dir: &Path, corrupt_contents: &str) {
        let mut quarantined: Vec<PathBuf> = std::fs::read_dir(dir)
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
        assert_eq!(
            quarantined.len(),
            1,
            "expected exactly one quarantined config, found {quarantined:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&quarantined[0]).unwrap(),
            corrupt_contents,
            "quarantine must preserve the original bytes"
        );

        let fresh: AppConfig =
            serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(
            serde_json::to_string(&fresh).unwrap(),
            serde_json::to_string(&AppConfig::default()).unwrap(),
            "fresh config must equal the defaults"
        );
    }

    #[test]
    fn truncated_json_is_quarantined_and_reset_to_defaults() {
        let corrupt = r#"{"model": "ParakeetV2", "auto_pas"#;
        let (dir, path) = temp_config_file(corrupt);

        let config = AppConfig::load_from(&path).unwrap();
        assert_eq!(config.model, ModelVariant::ParakeetV3);
        assert_quarantined_and_reset(dir.path(), corrupt);
    }

    #[test]
    fn wrong_typed_field_is_quarantined_and_reset_to_defaults() {
        let corrupt = r#"{"minimum_key_time": "fast"}"#;
        let (dir, path) = temp_config_file(corrupt);

        let config = AppConfig::load_from(&path).unwrap();
        assert_eq!(config.minimum_key_time, 0.2);
        assert_quarantined_and_reset(dir.path(), corrupt);
    }

    #[test]
    fn empty_config_file_is_quarantined_and_reset_to_defaults() {
        let (dir, path) = temp_config_file("");

        let config = AppConfig::load_from(&path).unwrap();
        assert_eq!(config.config_version, CONFIG_VERSION);
        assert_quarantined_and_reset(dir.path(), "");
    }

    #[test]
    fn valid_config_loads_without_quarantine() {
        let json = serde_json::to_string_pretty(&AppConfig::default()).unwrap();
        let (dir, path) = temp_config_file(&json);

        let config = AppConfig::load_from(&path).unwrap();
        assert_eq!(config.config_version, CONFIG_VERSION);
        assert!(AppConfig::quarantined_files(dir.path()).is_empty());
    }

    #[test]
    fn missing_file_saves_defaults_without_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        // Also exercises save_to creating parent directories.
        let path = dir.path().join("nested/config.json");

        let config = AppConfig::load_from(&path).unwrap();
        assert_eq!(config.model, ModelVariant::ParakeetV3);
        assert!(path.exists());
        assert!(AppConfig::quarantined_files(dir.path()).is_empty());
    }

    #[test]
    fn quarantined_files_lists_only_corrupt_siblings_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), b"{}").unwrap();
        std::fs::write(dir.path().join("config.json.corrupt-100"), b"a").unwrap();
        std::fs::write(dir.path().join("config.json.corrupt-200"), b"b").unwrap();
        std::fs::write(dir.path().join("unrelated.txt"), b"x").unwrap();

        let files = AppConfig::quarantined_files(dir.path());
        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("config.json.corrupt-100"));
        assert!(files[1].ends_with("config.json.corrupt-200"));
    }
}
