//! Green paint: a multiword answer that is grammatical but arbitrary, not a phrase people say as
//! one unit (GREENPAINT, REDCAR, EATAPEAR). See a fill guide for the
//! full test an editor or agent runs; this is a cheap first pass that needs only the bundled word
//! list, so it works the same in the app and in the browser (WebAssembly).
//!
//! An answer is possible green paint when all of these hold:
//! 1. It has 6+ letters.
//! 2. The word list does not vouch for it as a phrase: it is not listed, or scores under
//!    `LIST_MAX` (Spread the Wordlist: 50-60 good, 40-45 fair, 35 and under weak, 0 = avoid).
//!    Score 0 means "avoid" for other reasons (names, crude words), not green paint, so it is skipped.
//! 3. It is not a form of a listed word (stem + S, ED, ING, ER, LY, NESS and so on).
//! 4. It splits into 2 or 3 common words: each at least 3 letters with a list score of 50+, or a
//!    short glue word (A, AN, TO, OF, ...). The last word is not glue. Not counting A, AN or THE, it is
//!    exactly 2 words: green paint is a pair (GREEN PAINT, EAT A PEAR); a longer phrase (GOD IS
//!    NOWHERE) is not.
//! 5. The user has not liked it, and (app only, with a puzzle history) no published puzzle ran it.
//!
//! Nothing else in the web app mirrors this rule: the page reads `green_paint` from /api/stats.

use crate::engine::list_score;
use crate::puzzle::Puzzle;
use crate::words;
use serde::Serialize;

/// Phrases the list scores under this are not vouched for. Real stock phrases sit at 35+
/// (DASHEDOFF, THROWRUGS); weak pairs and one-off theme entries sit at 10-20 (BLUEPAINT 10).
pub const LIST_MAX: u32 = 30;
/// A part must score at least this to count as a common word.
const WORD_MIN: u32 = 50;
const MIN_LEN: usize = 6;
const MAX_PARTS: usize = 3;
const GLUE: &[&str] = &["A", "AN", "I", "TO", "OF", "IN", "ON", "AT", "MY", "UP", "BY", "IS", "IT", "ME", "WE", "US", "OR", "SO", "NO", "GO", "DO", "BE", "HE", "AS", "IF"];
const SUFFIXES: &[&str] = &["S", "ES", "ED", "D", "ING", "ER", "ERS", "LY", "NESS", "LESS", "ISH", "IST", "ISTS", "ISM", "ION", "IONS", "TION", "ABLE", "FUL", "MENT", "Y"];

fn common(w: &str) -> bool {
    GLUE.contains(&w) || (w.len() >= 3 && list_score(w).is_some_and(|s| s >= WORD_MIN))
}

/// A listed word plus an ending (SEXTING = SEXT + ING, HEADLAMPS = HEADLAMP + S).
fn word_form(w: &str) -> bool {
    let fair = |s: &str| list_score(s).is_some_and(|x| x >= 40);
    SUFFIXES.iter().any(|suf| {
        let Some(stem) = w.strip_suffix(suf).filter(|s| s.len() >= 3) else { return false };
        let b = stem.as_bytes();
        fair(stem) || fair(&format!("{stem}E")) || (b.len() >= 4 && b[b.len() - 1] == b[b.len() - 2] && fair(&stem[..stem.len() - 1]))
    })
}

/// The fewest common words that spell `w` (2 or 3), if any.
pub fn split(w: &str) -> Option<Vec<String>> {
    let n = w.len();
    let mut best: Vec<Option<Vec<String>>> = vec![None; n + 1];
    best[0] = Some(vec![]);
    for i in 0..n {
        let Some(head) = best[i].clone() else { continue };
        if head.len() >= MAX_PARTS {
            continue;
        }
        for j in i + 1..=n {
            let part = &w[i..j];
            if j - i == n || !common(part) || best[j].as_ref().is_some_and(|b| b.len() <= head.len() + 1) {
                continue;
            }
            let mut v = head.clone();
            v.push(part.to_string());
            best[j] = Some(v);
        }
    }
    best[n].take().filter(|v| v.len() >= 2 && !GLUE.contains(&v[v.len() - 1].as_str()) && v.iter().filter(|p| !["A", "AN", "THE"].contains(&p.as_str())).count() == 2)
}

/// The words of a possible green-paint answer (any case; anything but letters ignored), or None.
pub fn green_paint_words(answer: &str) -> Option<Vec<String>> {
    let w = words::norm(answer);
    if w.len() < MIN_LEN {
        return None;
    }
    if let Some(s) = list_score(&w) {
        if s == 0 || s >= LIST_MAX {
            return None;
        }
    }
    if word_form(&w) || words::choices().liked.binary_search(&w).is_ok() {
        return None;
    }
    // App only: a published puzzle ran it, so an editor took it as a thing.
    if words::history_counts().is_some_and(|h| h.contains_key(&w)) {
        return None;
    }
    split(&w)
}

/// True for a possible green-paint answer.
pub fn is_green_paint(answer: &str) -> bool {
    green_paint_words(answer).is_some()
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub slot: String,
    pub answer: String,
    /// How it splits ("GREEN PAINT").
    pub words: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Possible green paint among the complete answers.
    pub entries: Vec<Entry>,
    pub count: usize,
}

pub fn report(p: &Puzzle) -> Report {
    let entries: Vec<Entry> = p
        .slots()
        .iter()
        .filter_map(|s| {
            let a = s.answer()?;
            let w = green_paint_words(&a)?;
            Some(Entry { slot: s.key.clone(), answer: a, words: w.join(" ") })
        })
        .collect();
    Report { count: entries.len(), entries }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_arbitrary_pairs_only() {
        crate::need_list!();
        for w in ["GREENPAINT", "REDCAR", "EATAPEAR", "BLUESHIRT", "WASHACAR", "bluepaint"] {
            assert!(is_green_paint(w), "{w}");
        }
        // Real phrases in the list, single words and word forms, short answers.
        for w in ["REDCARPET", "SPILLTHEBEANS", "ICEDTEA", "HOTDOG", "OPENSESAME", "SEXTING", "HEADLAMPS", "ZEBRAS", "BIGDOG", "ERA"] {
            assert!(!is_green_paint(w), "{w}");
        }
        // Three content words is a made-up phrase, not green paint.
        assert!(!is_green_paint("GODISNOWHERE"));
        assert_eq!(green_paint_words("EATAPEAR").unwrap(), ["EAT", "A", "PEAR"]);
    }
}
