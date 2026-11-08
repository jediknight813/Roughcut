//! Spellchecking, entirely offline: a Hunspell-compatible checker
//! (`spellbook`) with the SCOWL en_US dictionary built in (see
//! `dictionaries/LICENSE`), plus the words the user has taught it.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use unicode_segmentation::UnicodeSegmentation;

const AFF: &str = include_str!("../dictionaries/en_US.aff");
const DIC: &str = include_str!("../dictionaries/en_US.dic");
const MAX_SUGGESTIONS: usize = 5;

#[derive(Clone)]
pub struct Speller {
    dict: Arc<spellbook::Dictionary>,
    /// Learned words, lowercased.
    personal: HashSet<String>,
}

impl Speller {
    pub fn english() -> Speller {
        static DICTIONARY: OnceLock<Arc<spellbook::Dictionary>> = OnceLock::new();
        let dict = DICTIONARY
            .get_or_init(|| Arc::new(spellbook::Dictionary::new(AFF, DIC).expect("bundled dictionary parses")))
            .clone();
        Speller { dict, personal: HashSet::new() }
    }

    /// Accepts `word` from now on ("Learn Spelling").
    pub fn learn(&mut self, word: &str) {
        self.personal.insert(normalize(word).to_lowercase());
    }

    pub fn is_correct(&self, word: &str) -> bool {
        let word = normalize(word);
        if skip(&word) || self.personal.contains(&word.to_lowercase()) {
            return true;
        }
        self.dict.check(&word)
    }

    /// Byte ranges of misspelled words in `text`. Web addresses, email
    /// addresses, numbers and all-caps abbreviations are left alone.
    pub fn misspellings(&self, text: &str) -> Vec<Range<usize>> {
        let mut ignored = text
            .split_whitespace()
            .filter(|token| token.contains("://") || token.contains('@') || token.starts_with("www."))
            .map(|token| {
                let start = token.as_ptr() as usize - text.as_ptr() as usize;
                start..start + token.len()
            })
            .peekable();
        text.unicode_word_indices()
            .filter(|(start, _)| {
                while ignored.peek().is_some_and(|r| r.end <= *start) {
                    ignored.next();
                }
                !ignored.peek().is_some_and(|r| r.contains(start))
            })
            .filter(|(_, word)| !self.is_correct(word))
            .map(|(start, word)| start..start + word.len())
            .collect()
    }

    /// The best few corrections, most likely first.
    pub fn suggestions(&self, word: &str) -> Vec<String> {
        let mut out = Vec::new();
        self.dict.suggest(&normalize(word), &mut out);
        out.truncate(MAX_SUGGESTIONS);
        out
    }
}

/// Typographic apostrophes are checked as plain ones.
fn normalize(word: &str) -> String {
    word.replace('’', "'")
}

fn skip(word: &str) -> bool {
    let letters = word.chars().filter(|c| c.is_alphabetic()).count();
    letters == 0 || word.chars().any(|c| c.is_ascii_digit()) || (letters > 1 && word.chars().all(|c| !c.is_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(speller: &Speller, text: &str) -> Vec<String> {
        speller.misspellings(text).into_iter().map(|r| text[r].to_string()).collect()
    }

    #[test]
    fn finds_misspellings_with_their_positions() {
        let s = Speller::english();
        let text = "The wind smelled of rain and kelpe, and teh gull complained.";
        assert_eq!(words(&s, text), ["kelpe", "teh"]);
        let r = s.misspellings(text)[0].clone();
        assert_eq!(&text[r], "kelpe");
    }

    #[test]
    fn suggests_corrections() {
        let s = Speller::english();
        assert!(s.suggestions("kelpe").iter().any(|w| w == "kelp"), "{:?}", s.suggestions("kelpe"));
        assert_eq!(s.suggestions("teh").first().map(String::as_str), Some("the"));
    }

    #[test]
    fn leaves_contractions_names_codes_and_links_alone() {
        let s = Speller::english();
        let text = "Don't panic; it’s NASA's 2nd launch. See https://exampel.com or mail me@exampel.com";
        assert!(words(&s, text).is_empty(), "{:?}", words(&s, text));
    }

    #[test]
    fn learning_a_word_accepts_it_in_any_case() {
        let mut s = Speller::english();
        assert_eq!(words(&s, "Elias Marr kept the light"), ["Marr"]);
        s.learn("Marr");
        assert!(words(&s, "Elias Marr kept the light. marr").is_empty());
    }
}
