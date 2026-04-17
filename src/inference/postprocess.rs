/// Post-processing for transcription text — word remapping and removal.
///
/// Applies user-defined substitutions to the raw transcription output
/// before it's pasted or stored in history. Inspired by Hex's
/// WordRemapping / WordRemoval feature.

use serde::{Deserialize, Serialize};
use tracing::debug;

/// A single word-level remapping rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WordRemapping {
    /// Text to search for (case-insensitive, whole word)
    pub from: String,
    /// Replacement text
    pub to: String,
}

/// A word to remove entirely from transcriptions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WordRemoval {
    /// Word to remove (case-insensitive, whole word)
    pub word: String,
}

/// Post-processor that applies remappings and removals.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PostProcessor {
    pub remappings: Vec<WordRemapping>,
    pub removals: Vec<WordRemoval>,
}

impl PostProcessor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply all post-processing rules to the transcription text.
    pub fn process(&self, text: &str) -> String {
        let mut result = text.to_string();

        // Apply removals first (so removed words don't interfere with remappings)
        for removal in &self.removals {
            result = remove_word(&result, &removal.word);
        }

        // Apply remappings
        for mapping in &self.remappings {
            result = replace_word(&result, &mapping.from, &mapping.to);
        }

        // Clean up double spaces left by removals
        while result.contains("  ") {
            result = result.replace("  ", " ");
        }
        let result = result.trim().to_string();

        if result != text {
            debug!("Post-processed: '{}' → '{}'", text, result);
        }

        result
    }
}

/// Replace whole-word occurrences of `from` with `to` (case-insensitive).
///
/// Uses word-boundary detection so "the" doesn't match "there".
fn replace_word(text: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return text.to_string();
    }

    let from_lower = from.to_lowercase();
    let text_lower = text.to_lowercase();

    // Find all case-insensitive match positions
    let mut matches: Vec<(usize, usize)> = Vec::new();
    for (i, _) in text_lower.match_indices(&from_lower) {
        let after_idx = i + from.len();

        let before_is_boundary = i == 0
            || !text
                .as_bytes()
                .get(i - 1)
                .map(|&b| b.is_ascii_alphanumeric() || b == b'_')
                .unwrap_or(false);

        let after_is_boundary = after_idx >= text.len()
            || !text
                .as_bytes()
                .get(after_idx)
                .map(|&b| b.is_ascii_alphanumeric() || b == b'_')
                .unwrap_or(false);

        if before_is_boundary && after_is_boundary {
            matches.push((i, after_idx));
        }
    }

    if matches.is_empty() {
        return text.to_string();
    }

    // Build result by replacing from right to left to preserve indices
    let mut result = text.to_string();
    for &(start, end) in matches.iter().rev() {
        let original = &text[start..end];
        let replacement = match_case(original, to);
        result.replace_range(start..end, &replacement);
    }

    result
}

/// Remove whole-word occurrences of `word` (case-insensitive).
fn remove_word(text: &str, word: &str) -> String {
    replace_word(text, word, "")
}

/// Try to match the case pattern of `original` in `replacement`.
///
/// - "FOO" + "bar" → "BAR"
/// - "Foo" + "bar" → "Bar"
/// - "foo" + "bar" → "bar"
fn match_case(original: &str, replacement: &str) -> String {
    let letters: Vec<char> = original.chars().filter(|c| c.is_alphabetic()).collect();
    let all_upper = !letters.is_empty() && letters.iter().all(|c| c.is_uppercase());
    let first_upper = letters.first().map(|c| c.is_uppercase()).unwrap_or(false);

    if all_upper {
        replacement.to_uppercase()
    } else if first_upper {
        let mut chars = replacement.chars();
        match chars.next() {
            Some(first) => {
                let upper: String = first.to_uppercase().collect();
                upper + chars.as_str()
            }
            None => replacement.to_string(),
        }
    } else {
        replacement.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_remapping() {
        let pp = PostProcessor {
            remappings: vec![WordRemapping {
                from: "I llama".into(),
                to: "I'll ama".into(),
            }],
            removals: vec![],
        };
        assert_eq!(pp.process("I llama going to the store"), "I'll ama going to the store");
    }

    #[test]
    fn test_case_preservation() {
        let pp = PostProcessor {
            remappings: vec![WordRemapping {
                from: "lol".into(),
                to: "haha".into(),
            }],
            removals: vec![],
        };
        assert_eq!(pp.process("That was lol"), "That was haha");
        assert_eq!(pp.process("That was LOL"), "That was HAHA");
        assert_eq!(pp.process("That was Lol"), "That was Haha");
    }

    #[test]
    fn test_word_removal() {
        let pp = PostProcessor {
            remappings: vec![],
            removals: vec![WordRemoval {
                word: "um".into(),
            }],
        };
        assert_eq!(pp.process("um I think um that's right"), "I think that's right");
    }

    #[test]
    fn test_whole_word_only() {
        let pp = PostProcessor {
            remappings: vec![WordRemapping {
                from: "the".into(),
                to: "a".into(),
            }],
            removals: vec![],
        };
        // "the" should match but "there" should not
        assert_eq!(pp.process("the cat is there"), "a cat is there");
    }

    #[test]
    fn test_combined() {
        let pp = PostProcessor {
            remappings: vec![WordRemapping {
                from: "canario".into(),
                to: "Canario".into(),
            }],
            removals: vec![WordRemoval {
                word: "uh".into(),
            }],
        };
        assert_eq!(pp.process("uh canario is great"), "Canario is great");
    }

    #[test]
    fn test_empty_processor() {
        let pp = PostProcessor::new();
        assert_eq!(pp.process("hello world"), "hello world");
    }
}
