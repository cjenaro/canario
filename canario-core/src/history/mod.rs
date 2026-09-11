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
    /// The final transcribed text (after post-processing AND
    /// transformation, when one ran — fgm.3 D3: the canonical text the
    /// user received).
    pub text: String,
    /// Duration of the recording in seconds
    pub duration_secs: f64,
    /// Source application (if detectable)
    pub source_app: Option<String>,
    /// The pre-transformation transcript (fgm.3 D3), stored ONLY when
    /// a transformation changed the text — `None` for raw dictations
    /// (no storage doubling) and on transform failure. Old history
    /// files without the field load as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
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
    ///
    /// `raw_text` (fgm.3 D3) is stored only when it differs from
    /// `text` — a caller that passes `Some(equal)` (e.g. a provider
    /// echoing the input back unchanged) stores nothing extra.
    pub fn add(
        &mut self,
        text: String,
        duration_secs: f64,
        source_app: Option<String>,
        raw_text: Option<String>,
    ) {
        // D3: raw_text is stored only when it differs from the final
        // text — no storage doubling for raw dictations.
        let raw_text = raw_text.filter(|raw| raw != &text);
        let entry = HistoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            text,
            duration_secs,
            source_app,
            raw_text,
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
        self.entries.iter().rev().take(limit).cloned().collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// fgm.3 D3: raw_text is stored ONLY when it differs from `text` —
    /// history never doubles storage for raw dictations, and a
    /// transformation that changed nothing stores nothing extra.
    #[test]
    fn add_stores_raw_text_only_when_different() {
        let mut history = History::default();
        history.add("formal text".into(), 1.0, None, Some("raw text".into()));
        history.add("plain dictation".into(), 1.0, None, None);
        history.add(
            "unchanged".into(),
            1.0,
            None,
            Some("unchanged".into()), // provider echoed the input back
        );

        assert_eq!(history.entries[0].raw_text.as_deref(), Some("raw text"));
        assert_eq!(history.entries[1].raw_text, None);
        assert_eq!(
            history.entries[2].raw_text, None,
            "an unchanged transcript must not be stored twice"
        );
    }

    /// fgm.3: the field round-trips through JSON, and old entries
    /// without it load as `None` (serde default keeps old history
    /// files loading). `None` entries serialize WITHOUT the key — the
    /// on-disk shape old builds already understand.
    #[test]
    fn raw_text_round_trips_and_old_entries_load() {
        let mut history = History::default();
        history.add("formal text".into(), 1.0, None, Some("raw text".into()));
        history.add("plain".into(), 1.0, None, None);

        let json = serde_json::to_string(&history).unwrap();
        let reloaded: History = serde_json::from_str(&json).unwrap();
        assert_eq!(reloaded.entries[0].raw_text.as_deref(), Some("raw text"));
        assert_eq!(reloaded.entries[1].raw_text, None);

        // A pre-fgm.3 file (no raw_text key anywhere) loads untouched.
        let old = r#"{"entries":[{"id":"e1","timestamp":"2026-01-01T00:00:00Z","text":"old entry","duration_secs":1.0,"source_app":null}]}"#;
        let old: History = serde_json::from_str(old).unwrap();
        assert_eq!(old.entries.len(), 1);
        assert_eq!(old.entries[0].raw_text, None);

        // The serialized shape omits the key when unset.
        let one = serde_json::to_string(&history.entries[1]).unwrap();
        assert!(!one.contains("raw_text"), "{one}");
    }
}
