//! Crosswordese: short answers (3 to 5 letters) that published puzzles use all the time.
//! The list is `data/crosswordese.txt` (words only), built from the published puzzles in the history by
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
/// See `typical_benchmark` for what published puzzles typically hold.
pub fn threshold(answers: usize) -> usize {
    6.max((answers * 17).div_ceil(100))
}

/// typical benchmark: crosswordese answers per published puzzle, by grid size. Built once from the
/// history by `xwh/scripts/crosswordese_benchmark.py` in the private repo; never computed here.
const TYPICAL: &str = include_str!("../data/crosswordese-typical.json");

/// The typical crosswordese count in published puzzles of one grid size.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Benchmark {
    pub width: usize,
    pub height: usize,
    /// Published puzzles it was built from.
    pub puzzles: usize,
    pub p25: f64,
    pub median: f64,
    pub p75: f64,
}

impl Benchmark {
    /// Well above the typical range: past the 75th percentile by more than the range's width.
    pub fn well_above(&self, count: usize) -> bool {
        count as f64 > self.p75 + (self.p75 - self.p25).max(2.0)
    }
}

/// The typical benchmark for the grid size nearest `width` x `height` (by area).
pub fn typical_benchmark(width: usize, height: usize) -> Option<Benchmark> {
    static ALL: OnceLock<Vec<Benchmark>> = OnceLock::new();
    let all = ALL.get_or_init(|| {
        let v: serde_json::Value = serde_json::from_str(TYPICAL).unwrap_or_default();
        let Some(sizes) = v["sizes"].as_object() else { return Vec::new() };
        sizes
            .iter()
            .filter_map(|(k, s)| {
                let (w, h) = k.split_once('x')?;
                Some(Benchmark {
                    width: w.parse().ok()?,
                    height: h.parse().ok()?,
                    puzzles: s["puzzles"].as_u64()? as usize,
                    p25: s["p25"].as_f64()?,
                    median: s["median"].as_f64()?,
                    p75: s["p75"].as_f64()?,
                })
            })
            .collect()
    });
    let area = (width * height) as i64;
    all.iter().min_by_key(|b| ((b.width * b.height) as i64 - area).abs()).copied()
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
    /// The typical count for this grid size (nearest size).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typical: Option<Benchmark>,
}

pub fn report(p: &Puzzle) -> Report {
    let slots = p.slots();
    let entries: Vec<Entry> = slots
        .iter()
        .filter_map(|s| s.answer().filter(|a| is_crosswordese(a)).map(|a| Entry { slot: s.key.clone(), answer: a }))
        .collect();
    let threshold = threshold(slots.len());
    Report { count: entries.len(), answers: slots.len(), threshold, over: entries.len() > threshold, typical: typical_benchmark(p.width(), p.height()), entries }
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
        let b = typical_benchmark(15, 15).unwrap();
        assert_eq!((b.width, b.height), (15, 15));
        assert!(b.p25 <= b.median && b.median <= b.p75 && b.puzzles > 100);
        assert_eq!(typical_benchmark(23, 23).unwrap().width, 21);
        assert_eq!(typical_benchmark(5, 5).unwrap().width, 15);
    }
}
