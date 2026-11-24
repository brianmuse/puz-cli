//! Crosswordese: short answers (3 to 5 letters) that published puzzles use all the time.
//! The list is `data/crosswordese.txt` (words only), built from the NYT puzzles in the history by
//! a script outside this repo (it reads a puzzle history). A web app may read the same file and the
//! same threshold rule; keep the two in step.

use crate::puzzle::Puzzle;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::OnceLock;

const LIST: &str = include_str!("../data/crosswordese.txt");

fn set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| LIST.lines().map(str::trim).filter(|w| !w.is_empty()).collect())
}

/// True for a crosswordese answer (any case; spaces and punctuation ignored).
pub fn is_crosswordese(word: &str) -> bool {
    let w: String = word.chars().filter(char::is_ascii_alphabetic).map(|c| c.to_ascii_uppercase()).collect();
    set().contains(w.as_str())
}

/// The whole list, sorted.
pub fn words() -> Vec<&'static str> {
    let mut v: Vec<_> = set().iter().copied().collect();
    v.sort_unstable();
    v
}

/// How many crosswordese answers a grid may hold before we warn: 6, or 17% of its answers.
/// NYT 15x15s average about 10 of 74; this warns on roughly the worst 1 in 6 of them.
pub fn threshold(answers: usize) -> usize {
    6.max((answers * 17).div_ceil(100))
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub slot: String,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Crosswordese answers among the complete answers.
    pub entries: Vec<Entry>,
    pub count: usize,
    /// All answers in the grid, complete or not.
    pub answers: usize,
    pub threshold: usize,
    /// More than `threshold`: worth a warning.
    pub over: bool,
}

pub fn report(p: &Puzzle) -> Report {
    let slots = p.slots();
    let entries: Vec<Entry> = slots
        .iter()
        .filter_map(|s| s.answer().filter(|a| is_crosswordese(a)).map(|a| Entry { slot: s.key.clone(), answer: a }))
        .collect();
    let threshold = threshold(slots.len());
    Report { count: entries.len(), answers: slots.len(), threshold, over: entries.len() > threshold, entries }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_and_rule() {
        for w in ["ERA", "AAH", "ALOE", "OREO", "ARIA", "erie", "E-P-E-E"] {
            assert!(is_crosswordese(w), "{w}");
        }
        assert!(!is_crosswordese("ZEBRAS"));
        assert!(words().iter().all(|w| (3..=5).contains(&w.len())));
        assert_eq!(threshold(20), 6);
        assert_eq!(threshold(74), 13);
    }
}
