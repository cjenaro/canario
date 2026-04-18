/// Transcription history — stores past transcriptions in a JSON file.
///
/// Each entry records the timestamp, text, duration, and optionally
/// the source application. History is browseable from the settings UI.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info};

/// A single transcription history entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Unique ID for this entry
    pub id: String,
    /// When the transcription happened (UTC)
    pub timestamp: DateTime<Utc>,
    /// The transcribed text (after post-processing)
    pub text: String,
    /// Duration of the recording in seconds
    pub duration_secs: f64,
    /// Source application (if detectable)
    pub source_app: Option<String>,
}

/// The history store — manages a JSON file of entries.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
}

impl History {
    /// Get the path to the history file.
    pub fn history_file() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/share"))
            .join("canario")
            .join("history.json")
    }

    /// Load history from disk. Returns empty history if file doesn't exist.
    pub fn load() -> Self {
        let path = Self::history_file();
        if !path.exists() {
            return Self::default();
        }

        match std::fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str::<History>(&data) {
                Ok(history) => {
                    debug!("Loaded {} history entries", history.entries.len());
                    history
                }
                Err(e) => {
                    tracing::warn!("Failed to parse history file: {}", e);
                    Self::default()
                }
            },
            Err(e) => {
                tracing::warn!("Failed to read history file: {}", e);
                Self::default()
            }
        }
    }

    /// Save history to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::history_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)?;
        debug!("Saved {} history entries", self.entries.len());
        Ok(())
    }

    /// Add a new entry to the history.
    pub fn add(&mut self, text: String, duration_secs: f64, source_app: Option<String>) {
        let entry = HistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            text,
            duration_secs,
            source_app,
        };
        info!(
            "History entry added: {} ({:.1}s)",
            entry.text.chars().take(50).collect::<String>(),
            entry.duration_secs
        );
        self.entries.push(entry);

        // Keep last 1000 entries
        if self.entries.len() > 1000 {
            let drain_count = self.entries.len() - 1000;
            self.entries.drain(0..drain_count);
        }

        if let Err(e) = self.save() {
            tracing::warn!("Failed to save history: {}", e);
        }
    }

    /// Delete a specific entry by ID.
    pub fn delete(&mut self, id: &str) {
        self.entries.retain(|e| e.id != id);
        let _ = self.save();
    }

    /// Clear all history.
    pub fn clear(&mut self) {
        self.entries.clear();
        let _ = self.save();
    }

    /// Get recent entries (most recent first), owned.
    pub fn recent_owned(&self, limit: usize) -> Vec<HistoryEntry> {
        self.entries
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Search entries by text content, owned.
    pub fn search_owned(&self, query: &str) -> Vec<HistoryEntry> {
        let query_lower = query.to_lowercase();
        self.entries
            .iter()
            .rev()
            .filter(|e| e.text.to_lowercase().contains(&query_lower))
            .cloned()
            .collect()
    }
}
