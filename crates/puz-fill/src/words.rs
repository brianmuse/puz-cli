//! Word quality, and the user's own word choices (banned and liked words).
//!
//! A word's quality starts at its word-list score (0-60; 50 is fine) and moves with the puzzle
//! history, when there is one:
//! - dated: used often in old puzzles, rarely in the last 10 years (ESNE, ERDA): -10.
//! - overused: a word under 50 seen hundreds of times (ESNE, ASEC): -10.
//! - rare: a low-scoring word of 5+ letters that published puzzles almost never use: -5.
//!
//! The host keeps the user's choices (a `.words.json` file) and passes them in. Banned words never go in new fill.
//! Liked words (good debuts, favorites) are added to the word list at the top score.
//!
//! This module has no disk access. The host says where the choices and the history counts come
//! from with `set_source` (a command reads files; a browser passes them in).

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

/// Quality at or above this is good fill.
pub const GOOD: i32 = 50;
/// The word-list score liked words get.
pub const LIKED_SCORE: u16 = 60;

/// Where the user's word choices and the puzzle history counts come from.
pub trait Source: Send + Sync {
    /// The user's banned and liked words. Called often, so cache.
    fn choices(&self) -> Arc<Choices>;
    /// Uses and uses in the last 10 years, per answer seen 3+ times. None without a history.
    fn history_counts(&self) -> Option<&'static HashMap<String, (u32, u32)>>;
}

static SOURCE: OnceLock<Box<dyn Source>> = OnceLock::new();

/// Set where word choices and history counts come from. Only the first call counts. Without a
/// call there are no choices and no history.
pub fn set_source(s: Box<dyn Source>) {
    let _ = SOURCE.set(s);
}

/// Letters and digits only, upper case: the form answers are stored and compared in.
pub fn normalize_answer(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

/// Letters only, upper case.
pub fn norm(word: &str) -> String {
    word.chars().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_uppercase()).collect()
}

// ---------------------------------------------------------------- the user's choices

/// The file as saved: words as the user typed them (e.g. "PET GATOR").
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Prefs {
    #[serde(default)]
    pub banned: Vec<String>,
    #[serde(default)]
    pub liked: Vec<String>,
}

impl Prefs {
    fn remove(list: &mut Vec<String>, n: &str) -> bool {
        let before = list.len();
        list.retain(|w| norm(w) != n);
        list.len() != before
    }

    /// Ban words. A banned word stops being liked.
    pub fn ban(&mut self, words: &[String]) -> Result<()> {
        for w in words {
            let n = check_word(w)?;
            Self::remove(&mut self.liked, &n);
            Self::remove(&mut self.banned, &n);
            self.banned.push(w.trim().to_uppercase());
        }
        self.banned.sort_by_key(|w| norm(w));
        Ok(())
    }

    /// Like words. A liked word stops being banned.
    pub fn like(&mut self, words: &[String]) -> Result<()> {
        for w in words {
            let n = check_word(w)?;
            Self::remove(&mut self.banned, &n);
            Self::remove(&mut self.liked, &n);
            self.liked.push(w.trim().to_uppercase());
        }
        self.liked.sort_by_key(|w| norm(w));
        Ok(())
    }

    /// Take words off both lists. Returns the words that were on neither.
    pub fn forget(&mut self, words: &[String]) -> Vec<String> {
        let mut missing = vec![];
        for w in words {
            let n = norm(w);
            let a = Self::remove(&mut self.banned, &n);
            let b = Self::remove(&mut self.liked, &n);
            if !a && !b {
                missing.push(w.clone());
            }
        }
        missing
    }
}

/// Ban, like and forget words, in that order. Returns the words to forget that were on neither
/// list.
pub fn apply(p: &mut Prefs, ban: &[String], like: &[String], forget: &[String]) -> Result<Vec<String>> {
    p.ban(ban)?;
    p.like(like)?;
    Ok(p.forget(forget))
}

fn check_word(w: &str) -> Result<String> {
    let n = norm(w);
    if n.len() < 2 {
        bail!("{w:?} needs at least 2 letters");
    }
    if w.chars().any(|c| !(c.is_ascii_alphabetic() || c == ' ' || c == '-' || c == '\'')) {
        bail!("{w:?}: use letters, spaces, hyphens or apostrophes only");
    }
    Ok(n)
}

/// The user's choices, normalized for lookups.
#[derive(Debug, Default)]
pub struct Choices {
    pub banned: HashSet<String>,
    /// Normalized liked words, sorted.
    pub liked: Vec<String>,
}

impl Choices {
    /// Choices from the saved lists: words normalized, liked words sorted for lookups.
    pub fn from_prefs(prefs: &Prefs) -> Choices {
        let mut liked: Vec<String> = prefs.liked.iter().map(|w| norm(w)).filter(|w| w.len() >= 2).collect();
        liked.sort();
        liked.dedup();
        Choices { banned: prefs.banned.iter().map(|w| norm(w)).collect(), liked }
    }
}

/// The user's choices, from the host's source.
pub fn choices() -> Arc<Choices> {
    SOURCE.get().map(|s| s.choices()).unwrap_or_default()
}

/// Uses and uses in the last 10 years, per answer seen 3+ times. None without a history.
pub(crate) fn history_counts() -> Option<&'static HashMap<String, (u32, u32)>> {
    SOURCE.get().and_then(|s| s.history_counts())
}

// ---------------------------------------------------------------- quality

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// The user banned it.
    Banned,
    /// The user liked it.
    Liked,
    /// Not in the word lists.
    NotInList,
    /// Word-list score under 50.
    LowScore,
    /// Common in old puzzles, rare in the last 10 years.
    Dated,
    /// A low-scoring word published hundreds of times (crosswordese).
    Overused,
    /// A low-scoring longer word published puzzles almost never use.
    Rare,
}

impl Flag {
    pub fn name(self) -> &'static str {
        match self {
            Flag::Banned => "banned",
            Flag::Liked => "liked",
            Flag::NotInList => "not in word list",
            Flag::LowScore => "low score",
            Flag::Dated => "dated",
            Flag::Overused => "overused",
            Flag::Rare => "rare",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Quality {
    pub score: i32,
    pub flags: Vec<Flag>,
}

impl Quality {
    pub fn good(&self) -> bool {
        self.score >= GOOD
    }
}

/// How good a word is as fill. `list_score` is its word-list score, if listed.
pub fn judge(word: &str, list_score: Option<u16>) -> Quality {
    judge_with(&norm(word), list_score, &choices(), history_counts())
}

fn judge_with(n: &str, list_score: Option<u16>, c: &Choices, hist: Option<&HashMap<String, (u32, u32)>>) -> Quality {
    if c.banned.contains(n) {
        return Quality { score: -100, flags: vec![Flag::Banned] };
    }
    if c.liked.binary_search_by(|w| w.as_str().cmp(n)).is_ok() {
        return Quality { score: LIKED_SCORE as i32, flags: vec![Flag::Liked] };
    }
    let mut flags = vec![];
    let base = match list_score {
        Some(s) => s as i32,
        None => {
            flags.push(Flag::NotInList);
            0
        }
    };
    if list_score.is_some() && base < GOOD {
        flags.push(Flag::LowScore);
    }
    let mut score = base;
    let (uses, recent) = hist.and_then(|h| h.get(n).copied()).unwrap_or((0, 0));
    if hist.is_some() {
        if uses >= 15 && recent * 8 < uses {
            flags.push(Flag::Dated);
            score -= 10;
        }
        // Most short glue scores 50 and is fine (ERA, CNN, OREO). Only a word already under 50
        // loses more for being crossword-only filler.
        if base < GOOD && uses >= 300 {
            flags.push(Flag::Overused);
            score -= 10;
        }
        if base <= GOOD && n.len() >= 5 && uses < 3 && list_score.is_some() {
            flags.push(Flag::Rare);
            score -= 5;
        }
    }
    Quality { score, flags }
}

/// Quality for many engine words at once: a memo keyed by the engine's normalized (lower case) word.
pub struct Judge {
    choices: Arc<Choices>,
    hist: Option<&'static HashMap<String, (u32, u32)>>,
}

impl Judge {
    pub fn new() -> Judge {
        Judge { choices: choices(), hist: history_counts() }
    }
    pub fn score(&self, normalized: &str, list_score: u16) -> i32 {
        judge_with(&normalized.to_ascii_uppercase(), Some(list_score), &self.choices, self.hist).score
    }
    pub fn banned(&self, normalized: &str) -> bool {
        !self.choices.banned.is_empty() && self.choices.banned.contains(&normalized.to_ascii_uppercase())
    }
}

/// One row of `words check`.
#[derive(Debug, Serialize)]
pub struct WordReport {
    pub word: String,
    pub list_score: Option<u32>,
    /// Uses in published puzzles; 0 means under 3 (the quality table skips rarer answers).
    pub uses: Option<u32>,
    pub uses_last_10_years: Option<u32>,
    pub quality: i32,
    pub good: bool,
    pub flags: Vec<Flag>,
}

pub fn report(word: &str) -> WordReport {
    let list = crate::engine::list_score(word);
    let q = judge(word, list.map(|s| s.min(u16::MAX as u32) as u16));
    let hist = history_counts().map(|h| h.get(&norm(word)).copied().unwrap_or((0, 0)));
    WordReport {
        word: word.trim().to_uppercase(),
        list_score: list,
        uses: hist.map(|h| h.0),
        uses_last_10_years: hist.map(|h| h.1),
        good: q.good(),
        quality: q.score,
        flags: q.flags,
    }
}

pub fn render_reports(rows: &[WordReport]) -> String {
    let mut out = String::new();
    for r in rows {
        let list = r.list_score.map(|s| s.to_string()).unwrap_or_else(|| "-".into());
        let hist = match (r.uses, r.uses_last_10_years) {
            (Some(0), _) => "under 3 uses".into(),
            (Some(u), Some(t)) => format!("{u} uses ({t} recent)"),
            _ => "no history".into(),
        };
        let flags: Vec<&str> = r.flags.iter().map(|f| f.name()).collect();
        out += &format!(
            "{:<18} {:>4}  list {:>3}  {:<22} {}{}\n",
            r.word,
            r.quality,
            list,
            hist,
            if r.good { "good" } else { "weak" },
            if flags.is_empty() { String::new() } else { format!(": {}", flags.join(", ")) }
        );
    }
    out
}

pub fn render_prefs(p: &Prefs) -> String {
    let line = |l: &[String]| if l.is_empty() { "(none)".to_string() } else { l.join(", ") };
    format!("Banned: {}\nLiked:  {}\n", line(&p.banned), line(&p.liked))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judge_flags() {
        let mut h = HashMap::new();
        h.insert("ESNE".to_string(), (616, 1));
        h.insert("ERS".to_string(), (2924, 1454));
        h.insert("SWOLE".to_string(), (86, 80));
        h.insert("ASEC".to_string(), (900, 300));
        h.insert("HORATII".to_string(), (1, 0));
        let c = Choices { banned: ["PSEC".to_string()].into(), liked: vec!["PETGATOR".to_string()] };
        let q = |w: &str, s| judge_with(w, Some(s), &c, Some(&h));
        assert_eq!(q("ESNE", 30).flags, vec![Flag::LowScore, Flag::Dated, Flag::Overused]);
        assert!(q("ERS", 50).good());
        assert!(!q("ASEC", 40).good());
        assert!(q("SWOLE", 55).good());
        assert!(!q("HORATII", 50).good());
        assert!(!q("PSEC", 60).good());
        assert!(judge_with("PETGATOR", None, &c, Some(&h)).good());
        // Without a history only the list score counts.
        assert!(judge_with("ESNE", Some(50), &c, None).good());
    }

    #[test]
    fn prefs_ban_like_forget() {
        let mut p = Prefs::default();
        p.ban(&["erda".into(), "Psec".into()]).unwrap();
        p.like(&["pet gator".into(), "ERDA".into()]).unwrap();
        assert_eq!(p.banned, vec!["PSEC"]);
        assert_eq!(p.liked, vec!["ERDA", "PET GATOR"]);
        assert_eq!(p.forget(&["petgator".into(), "nope".into()]), vec!["nope".to_string()]);
        assert_eq!(p.liked, vec!["ERDA"]);
        assert!(p.ban(&["a".into()]).is_err());
    }
}
