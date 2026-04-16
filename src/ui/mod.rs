pub mod paste;

use crate::audio::AudioCapture;
use crate::config::AppConfig;
use crate::inference::TranscriptionEngine;
use crate::ui::paste::paste_text;
use std::sync::{Arc, Mutex};

/// Core application state shared between threads
pub struct AppState {
    pub config: AppConfig,
    pub audio: AudioCapture,
    pub engine: TranscriptionEngine,
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
        let model_dir = config.local_model_dir();
        let num_threads = config.num_threads;

        Self {
            config,
            audio: AudioCapture::new(),
            engine: TranscriptionEngine::new(model_dir, num_threads),
            is_recording: false,
            is_transcribing: false,
            last_transcription: String::new(),
            status: AppStatus::Idle,
        }
    }
}

/// Handle a recording → transcription → paste cycle
pub fn handle_transcription_cycle(state: &Arc<Mutex<AppState>>) {
    let audio_samples = {
        let mut s = state.lock().unwrap();
        s.is_recording = false;
        s.status = AppStatus::Transcribing;
        s.is_transcribing = true;
        s.audio.stop_recording()
    };

    if audio_samples.is_empty() {
        let mut s = state.lock().unwrap();
        s.is_transcribing = false;
        s.status = AppStatus::Ready;
        return;
    }

    // Save to temp WAV for debugging
    let temp_wav = std::env::temp_dir().join(format!("canario-{}.wav", uuid::Uuid::new_v4()));
    if let Err(e) = crate::audio::save_wav(&temp_wav, &audio_samples) {
        tracing::warn!("Failed to save temp WAV: {}", e);
    }

    // Transcribe
    let text = {
        let s = state.lock().unwrap();
        s.engine.transcribe(&audio_samples).unwrap_or_default()
    };

    // Update state and paste
    {
        let mut s = state.lock().unwrap();
        s.is_transcribing = false;
        s.last_transcription = text.clone();
        s.status = AppStatus::Ready;
    }

    if !text.is_empty() {
        if let Err(e) = paste_text(&text) {
            tracing::error!("Failed to paste text: {}", e);
        }
    }

    // Clean up temp file
    let _ = std::fs::remove_file(temp_wav);
}
