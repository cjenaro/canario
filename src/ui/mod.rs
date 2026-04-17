pub mod app;
pub mod history;
pub mod indicator;
pub mod model_manager;
pub mod paste;
pub mod recording;
pub mod settings;
pub mod tray;
pub mod word_remapping;

use crate::config::AppConfig;

/// Messages from the tray / background threads to the GTK main loop
#[derive(Debug, Clone)]
pub enum AppMessage {
    /// Show the settings window
    ShowSettings,
    /// Toggle recording on/off
    ToggleRecording,
    /// Quit the application
    Quit,
    /// Transcription result ready (may be sent multiple times per session)
    TranscriptionReady(String),
    /// Recording thread has finished and exited
    RecordingStopped,
    /// Model download progress (0.0 - 1.0)
    DownloadProgress(f64),
    /// Model download complete
    DownloadComplete,
    /// Model download failed
    DownloadFailed(String),
    /// Audio level update (0.0 - 1.0)
    AudioLevel(f64),
    /// Hotkey changed in settings — restart the listener
    HotkeyChanged(Vec<String>),
}

/// Core application state shared between threads
#[derive(Debug)]
pub struct AppState {
    pub config: AppConfig,
    pub is_recording: bool,
    pub is_transcribing: bool,
    pub last_transcription: String,
    pub status: AppStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppStatus {
    /// App started, model not loaded
    Idle,
    /// Model is being downloaded
    Downloading(f64),
    /// Model is loading into memory
    Loading,
    /// Ready to record
    Ready,
    /// Currently recording audio
    Recording,
    /// Transcribing audio
    Transcribing,
    /// Error occurred
    Error(String),
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            is_recording: false,
            is_transcribing: false,
            last_transcription: String::new(),
            status: AppStatus::Idle,
        }
    }

    /// Check if the model is downloaded and ready
    pub fn is_model_ready(&self) -> bool {
        self.config.is_model_downloaded()
    }

    /// Check if we can start recording
    pub fn can_record(&self) -> bool {
        matches!(self.status, AppStatus::Ready) && !self.is_recording
    }
}
