//! Next-word prediction interface.
//!
//! The device implementation will be a flash-resident trie + bigram model
//! (memory-mapped, ~0 RAM). The core only depends on the [`Predictor`] trait, so
//! the keyboard logic is testable with a simple in-memory predictor.

use heapless::{String, Vec};

/// A single predicted word.
pub type Candidate = String<24>;

/// Up to four ranked candidates shown on the prediction row.
pub type Candidates = Vec<Candidate, 4>;

/// Produces word candidates for the current partial word.
pub trait Predictor {
    /// Fill `out` (cleared first) with up to four candidates for `prefix`.
    fn predict(&self, prefix: &str, out: &mut Candidates);
}

/// Prefix-match predictor backed by a static word list. Used in tests and as a
/// trivial fallback; the real device predictor ranks by a bigram model.
pub struct StaticPredictor<'a> {
    pub words: &'a [&'a str],
}

impl<'a> StaticPredictor<'a> {
    pub const fn new(words: &'a [&'a str]) -> Self {
        Self { words }
    }
}

impl Predictor for StaticPredictor<'_> {
    fn predict(&self, prefix: &str, out: &mut Candidates) {
        out.clear();
        if prefix.is_empty() {
            return;
        }
        for &w in self.words {
            if out.is_full() {
                break;
            }
            if w.starts_with(prefix) && w != prefix {
                let mut c = Candidate::new();
                if c.push_str(w).is_ok() {
                    let _ = out.push(c);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicts_prefix_matches_capped_at_four() {
        let p = StaticPredictor::new(&["hi", "hello", "help", "here", "hero", "ham"]);
        let mut out = Candidates::new();
        p.predict("he", &mut out);
        // "hello","help","here","hero" match "he"; capped at 4.
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].as_str(), "hello");
    }

    #[test]
    fn empty_prefix_yields_nothing() {
        let p = StaticPredictor::new(&["hello"]);
        let mut out = Candidates::new();
        p.predict("", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn exact_match_is_excluded() {
        let p = StaticPredictor::new(&["hi"]);
        let mut out = Candidates::new();
        p.predict("hi", &mut out);
        assert!(out.is_empty());
    }
}
