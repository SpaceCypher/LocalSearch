/// Phonetic encoding result with primary and secondary codes
#[derive(Debug, Clone, PartialEq)]
pub struct PhoneticCode {
    pub primary: String,
    pub secondary: Option<String>,
}

/// Double Metaphone phonetic encoding
/// Simplified implementation for common English names
pub fn double_metaphone(word: &str) -> PhoneticCode {
    let word = word.to_uppercase();
    let mut primary = String::new();
    let chars: Vec<char> = word.chars().collect();
    
    if chars.is_empty() {
        return PhoneticCode {
            primary: String::new(),
            secondary: None,
        };
    }
    
    let mut i = 0;
    
    // Skip leading silent letters
    if chars.len() >= 2 {
        if chars[0] == 'G' && chars[1] == 'N' {
            i = 1;
        } else if chars[0] == 'K' && chars[1] == 'N' {
            i = 1;
        } else if chars[0] == 'P' && chars[1] == 'N' {
            i = 1;
        } else if chars[0] == 'W' && chars[1] == 'R' {
            i = 1;
        }
    }
    
    // Process each character
    while i < chars.len() && primary.len() < 4 {
        let ch = chars[i];
        
        match ch {
            'A' | 'E' | 'I' | 'O' | 'U' => {
                if i == 0 {
                    primary.push('A');
                }
            }
            'B' => {
                primary.push('P');
                if i + 1 < chars.len() && chars[i + 1] == 'B' {
                    i += 1;
                }
            }
            'C' => {
                // CH -> X
                if i + 1 < chars.len() && chars[i + 1] == 'H' {
                    primary.push('X');
                    i += 1;
                } else {
                    primary.push('K');
                }
            }
            'D' => {
                primary.push('T');
            }
            'F' => {
                primary.push('F');
                if i + 1 < chars.len() && chars[i + 1] == 'F' {
                    i += 1;
                }
            }
            'G' => {
                // GH at end or before consonant -> silent
                if i + 1 < chars.len() && chars[i + 1] == 'H' {
                    if i + 2 >= chars.len() || !is_vowel(chars[i + 2]) {
                        i += 1;
                        i += 1;
                        continue;
                    }
                }
                primary.push('K');
            }
            'H' => {
                // H between vowels or at start
                if i == 0 || (i > 0 && is_vowel(chars[i - 1])) {
                    if i + 1 < chars.len() && is_vowel(chars[i + 1]) {
                        primary.push('H');
                    }
                }
            }
            'J' => {
                primary.push('J');
            }
            'K' => {
                primary.push('K');
                if i + 1 < chars.len() && chars[i + 1] == 'K' {
                    i += 1;
                }
            }
            'L' => {
                primary.push('L');
                if i + 1 < chars.len() && chars[i + 1] == 'L' {
                    i += 1;
                }
            }
            'M' => {
                primary.push('M');
                if i + 1 < chars.len() && chars[i + 1] == 'M' {
                    i += 1;
                }
            }
            'N' => {
                primary.push('N');
                if i + 1 < chars.len() && chars[i + 1] == 'N' {
                    i += 1;
                }
            }
            'P' => {
                // PH -> F
                if i + 1 < chars.len() && chars[i + 1] == 'H' {
                    primary.push('F');
                    i += 1;
                } else {
                    primary.push('P');
                }
            }
            'Q' => {
                primary.push('K');
            }
            'R' => {
                primary.push('R');
            }
            'S' => {
                // SH -> X
                if i + 1 < chars.len() && chars[i + 1] == 'H' {
                    primary.push('X');
                    i += 1;
                } else {
                    primary.push('S');
                }
            }
            'T' => {
                // TH -> 0
                if i + 1 < chars.len() && chars[i + 1] == 'H' {
                    primary.push('0');
                    i += 1;
                } else {
                    primary.push('T');
                }
            }
            'V' => {
                primary.push('F');
            }
            'W' => {
                if i + 1 < chars.len() && is_vowel(chars[i + 1]) {
                    primary.push('W');
                }
            }
            'X' => {
                primary.push('K');
                primary.push('S');
            }
            'Y' => {
                if i + 1 < chars.len() && is_vowel(chars[i + 1]) {
                    primary.push('Y');
                }
            }
            'Z' => {
                primary.push('S');
            }
            _ => {}
        }
        
        i += 1;
    }
    
    PhoneticCode {
        primary,
        secondary: None,
    }
}

fn is_vowel(ch: char) -> bool {
    matches!(ch, 'A' | 'E' | 'I' | 'O' | 'U')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_double_metaphone_mayer_matches_meyer() {
        let pm1 = double_metaphone("Mayer");
        let pm2 = double_metaphone("Meyer");
        assert_eq!(pm1.primary, pm2.primary);
    }

    #[test]
    fn test_double_metaphone_smith_smyth() {
        let pm1 = double_metaphone("Smith");
        let pm2 = double_metaphone("Smyth");
        assert_eq!(pm1.primary, pm2.primary);
    }

    #[test]
    fn test_double_metaphone_different_words() {
        let pm1 = double_metaphone("hello");
        let pm2 = double_metaphone("world");
        assert_ne!(pm1.primary, pm2.primary);
    }
}
