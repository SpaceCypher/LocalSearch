use unicode_normalization::UnicodeNormalization;
use rust_stemmers::{Algorithm, Stemmer};
use std::path::PathBuf;
use std::collections::HashMap;

/// A token extracted from text with its position
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub term: String,
    pub position: u32,
}

/// Query intent classification
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryIntent {
    Lookup,
    Recovery,
    Exploration,
    Verification,
}

/// Parsed query with tokens, scope, filters, and intent
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub tokens: Vec<String>,
    pub scope: Option<PathBuf>,
    pub intent: QueryIntent,
    pub filters: HashMap<String, String>,
}

impl Query {
    /// Parse a query string into structured Query
    pub fn parse(text: &str) -> Result<Self, String> {
        let tokenizer = Tokenizer::new();
        let mut tokens = Vec::new();
        let mut scope = None;
        let mut filters = HashMap::new();
        
        // Split query into parts
        let parts: Vec<&str> = text.split_whitespace().collect();
        
        for part in parts {
            if part.starts_with("in:") {
                // Extract scope
                let path = part.strip_prefix("in:").unwrap();
                scope = Some(PathBuf::from(path));
            } else if part.contains(':') && part.chars().any(|c| c.is_alphabetic()) {
                // Extract filters (after:, before:, ext:, etc.)
                let mut split = part.splitn(2, ':');
                if let (Some(key), Some(value)) = (split.next(), split.next()) {
                    filters.insert(key.to_string(), value.to_string());
                }
            } else {
                // Regular token - tokenize it
                let token_objs = tokenizer.tokenize(part);
                for token in token_objs {
                    tokens.push(token.term);
                }
            }
        }
        
        // If results are still thin, try tokenizing the full string minus filters.
        // This helps with multi-word queries like "CD LAB" which might have 
        // special tokenization when treated as a phrase.
        if tokens.is_empty() && !text.is_empty() {
             let filtered: String = text.split_whitespace()
                .filter(|p| !p.contains(':'))
                .collect::<Vec<_>>()
                .join(" ");
             if !filtered.is_empty() {
                 let fallback_tokens = tokenizer.tokenize(&filtered);
                 for t in fallback_tokens {
                     tokens.push(t.term);
                 }
             }
        }
        
        // Classify intent based on query patterns
        let intent = Self::classify_intent(&tokens, &filters);
        
        Ok(Query {
            tokens,
            scope,
            intent,
            filters,
        })
    }

    
    fn classify_intent(tokens: &[String], filters: &HashMap<String, String>) -> QueryIntent {
        // Recovery: queries with time filters
        if filters.contains_key("after") || filters.contains_key("before") {
            return QueryIntent::Recovery;
        }
        
        // Exploration: queries with wildcards or broad terms
        for token in tokens {
            if token.contains('*') || token.contains('?') {
                return QueryIntent::Exploration;
            }
        }
        
        // Verification: queries checking file existence (heuristic: contains "exists", "find", "check")
        for token in tokens {
            if token == "exist" || token == "find" || token == "check" {
                return QueryIntent::Verification;
            }
        }
        
        // Default: Lookup
        QueryIntent::Lookup
    }
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

    // Task 10: Query Parser + Intent Classification tests
    #[test]
    fn test_parser_scope_extraction() {
        let q = Query::parse("report in:/Users/alice/Documents").unwrap();
        assert_eq!(q.scope, Some(PathBuf::from("/Users/alice/Documents")));
        assert_eq!(q.tokens, vec!["report"]);
    }

    #[test]
    fn test_intent_lookup() {
        let q = Query::parse("invoice").unwrap();
        assert_eq!(q.intent, QueryIntent::Lookup);
    }

    #[test]
    fn test_intent_recovery() {
        let q = Query::parse("budget report after:2024-01-01").unwrap();
        assert_eq!(q.intent, QueryIntent::Recovery);
    }
}
