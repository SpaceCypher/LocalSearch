use unicode_normalization::UnicodeNormalization;
use rust_stemmers::{Algorithm, Stemmer};

/// A token extracted from text with its position
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub term: String,
    pub position: u32,
}

/// Unicode-aware tokenizer with camelCase splitting, stop word removal, and stemming
pub struct Tokenizer {
    stemmer: Stemmer,
    stop_words: Vec<&'static str>,
}

impl Tokenizer {
    pub fn new() -> Self {
        Self {
            stemmer: Stemmer::create(Algorithm::English),
            stop_words: vec![
                "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for",
                "of", "with", "by", "from", "as", "is", "was", "are", "were", "be",
                "been", "being", "have", "has", "had", "do", "does", "did", "will",
                "would", "should", "could", "may", "might", "must", "can", "this",
                "that", "these", "those", "i", "you", "he", "she", "it", "we", "they",
            ],
        }
    }

    pub fn tokenize(&self, text: &str) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut position = 0u32;

        // Normalize to NFC form
        let normalized: String = text.nfc().collect();

        // Split by whitespace and punctuation
        for word in normalized.split(|c: char| c.is_whitespace() || c.is_ascii_punctuation()) {
            if word.is_empty() {
                continue;
            }

            // Split camelCase words
            let camel_parts = self.split_camel_case(word);
            
            for part in camel_parts {
                let lower = part.to_lowercase();
                
                // Skip stop words
                if self.stop_words.contains(&lower.as_str()) {
                    continue;
                }

                // Apply stemming
                let stemmed = self.stemmer.stem(&lower).to_string();
                
                tokens.push(Token {
                    term: stemmed,
                    position,
                });
                position += 1;
            }
        }

        tokens
    }

    fn split_camel_case(&self, word: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut prev_was_lower = false;

        for ch in word.chars() {
            if ch.is_uppercase() && prev_was_lower && !current.is_empty() {
                // Found camelCase boundary
                parts.push(current.clone());
                current.clear();
            }
            current.push(ch);
            prev_was_lower = ch.is_lowercase();
        }

        if !current.is_empty() {
            parts.push(current);
        }

        if parts.is_empty() {
            vec![word.to_string()]
        } else {
            parts
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenizer_camelcase_split() {
        let t = Tokenizer::new();
        let tokens = t.tokenize("AppDelegate");
        let terms: Vec<_> = tokens.iter().map(|t| t.term.as_str()).collect();
        eprintln!("DEBUG: terms = {:?}", terms);
        assert!(terms.contains(&"app"));
        assert!(terms.contains(&"deleg") || terms.contains(&"delegate"));
    }

    #[test]
    fn test_tokenizer_stop_words_removed() {
        let t = Tokenizer::new();
        let tokens = t.tokenize("the file report");
        assert!(!tokens.iter().any(|t| t.term == "the"));
    }

    #[test]
    fn test_tokenizer_stems() {
        let t = Tokenizer::new();
        let tokens = t.tokenize("running");
        assert!(tokens.iter().any(|t| t.term == "run"));
    }
}
