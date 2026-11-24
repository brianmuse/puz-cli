//! Live analysis, like Ingrid's: is the grid fillable, and which words in each answer can be part of
//! a full fill? Each step can be stopped with an abort flag, so the server can restart it after an edit.
//!
//! How words get settled:
//! - Arc consistency rules out words that can't cross their neighbors (dead).
//! - Every full fill the search finds proves each word in it is viable.
//! - A word not in any fill yet is tried on its own: fix it in its slot and search.
//!   A fill means viable (and settles the rest of that fill too); no fill means dead.
//!
//! Reuse (`Known`, `Analysis::reuse`): a grid we saw before gets back everything we knew. A grid that
//! only adds letters to an earlier one keeps that grid's dead words (more letters only rule out
//! fills) and every earlier fill that still matches its letters (each one proves its words viable,
//! and the grid fillable).

use crate::engine::{build_config, check_with, fill_grid, search_heat, slot_key, Cfg, CheckReport, Heat, Options};
use crate::puzzle::Puzzle;
use anyhow::Result;
use ingrid_core::backtracking_search::{find_fill_with_diagnostics, FillFailure};
use ingrid_core::grid_config::{Choice, GridConfig};
use ingrid_core::types::WordId;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Part of at least one full fill.
    Viable,
    /// Can't be part of any full fill.
    Dead,
    /// The search ran out of time for this word.
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fillable {
    /// Not known yet.
    Unknown,
    Yes,
    No,
    /// The search ran out of time.
    Timeout,
}

pub enum SearchOutcome {
    Filled(Vec<String>),
    Impossible(Heat),
    Timeout(Heat),
    Aborted,
}

pub struct Analysis {
    pub puzzle: Puzzle,
    cfg: Cfg,
    /// Words left per slot after arc consistency, best fill candidates first.
    pruned: Vec<Vec<WordId>>,
    pub check: CheckReport,
    /// What we know per slot.
    verdicts: Vec<HashMap<WordId, Verdict>>,
    /// One full fill per viable word, per slot. Words in the same fill share it.
    fills: Vec<HashMap<WordId, Arc<Vec<String>>>>,
    pub fillable: Fillable,
    pub sample_fill: Option<Vec<String>>,
    pub hotspots: Option<Heat>,
    /// Every full fill found so far, oldest first.
    found: Vec<Arc<Vec<String>>>,
    /// The word list options, as text, so `Known` can tell whether two analyses used the same list.
    opts_key: String,
    min_score: u16,
    /// `reuse` ran already (a kept analysis that a later call goes on with).
    pub(crate) reused: bool,
    /// Words that timed out and were tried again with a longer limit (`retry_timeout`).
    retried: HashSet<(usize, WordId)>,
}

/// What an analysis has learned, kept so a later grid can start from it (see `Analysis::reuse`).
/// Words are stored as text, so this does not depend on word ids.
#[derive(Clone)]
pub struct Known {
    /// The grid in capitals.
    grid: Vec<String>,
    /// Marks that change the answers.
    rebus: BTreeMap<String, String>,
    bars: BTreeMap<String, Vec<String>>,
    opts: String,
    /// Words the search proved dead, by slot key.
    dead: HashMap<String, Vec<String>>,
    /// Words that ran out of time, by slot key. Reused only for the same grid.
    timeout: HashMap<String, Vec<String>>,
    /// Every full fill found. Each proves all its words viable.
    fills: Vec<Arc<Vec<String>>>,
    fillable: Fillable,
    sample_fill: Option<Vec<String>>,
    hotspots: Option<Heat>,
}

/// Most full fills and dead words one `Known` keeps, so a long session can't grow it without end.
/// Past these, the newest fills and an even share of dead words per answer are kept.
pub const KNOWN_MAX_FILLS: usize = 1000;
pub const KNOWN_MAX_DEAD: usize = 20_000;

impl Known {
    /// A test grid with `fills` copies of a big fill, to check the memory caps.
    #[cfg(test)]
    pub(crate) fn for_test(tag: usize, fills: usize) -> Known {
        let row = "A".repeat(21);
        let fill = Arc::new(vec![row.clone(); 21]);
        Known {
            grid: vec![format!("{tag:021}")],
            rebus: BTreeMap::new(),
            bars: BTreeMap::new(),
            opts: String::new(),
            dead: HashMap::new(),
            timeout: HashMap::new(),
            fills: vec![fill; fills],
            fillable: Fillable::Unknown,
            sample_fill: None,
            hotspots: None,
        }
    }

    /// A rough size in bytes, for the memory budget.
    pub fn approx_bytes(&self) -> usize {
        let rows = |g: &[String]| g.iter().map(|r| r.len() + 24).sum::<usize>() + 24;
        let words = |m: &HashMap<String, Vec<String>>| m.iter().map(|(k, v)| k.len() + 48 + v.iter().map(|w| w.len() + 24).sum::<usize>()).sum::<usize>();
        rows(&self.grid) + self.fills.iter().map(|f| rows(f)).sum::<usize>() + self.sample_fill.as_deref().map_or(0, rows) + words(&self.dead) + words(&self.timeout) + 256
    }

    /// Same grid, marks and word list: everything known carries over.
    pub fn same_grid(&self, other: &Known) -> bool {
        self.grid == other.grid && self.rebus == other.rebus && self.bars == other.bars && self.opts == other.opts
    }
}

/// What `Analysis::reuse` carried over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reuse {
    Nothing,
    /// The same grid: every verdict and the fillable status.
    Exact,
    /// An earlier grid with fewer letters: dead words, and fills that still match.
    Carried { dead: usize, fills: usize },
}

fn upper(grid: &[String]) -> Vec<String> {
    grid.iter().map(|r| r.to_uppercase()).collect()
}

/// One word in a slot, for display.
#[derive(Debug, Clone, Serialize)]
pub struct WordVerdict {
    pub word: String,
    pub score: u16,
    pub verdict: Option<Verdict>,
    /// For a viable word: index of a full fill that uses it, in the list `words_with_fills` returns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<usize>,
}

/// What we know about one slot.
#[derive(Debug, Clone, Serialize)]
pub struct SlotSummary {
    pub key: String,
    /// Words left after arc consistency.
    pub options: usize,
    pub viable: usize,
    pub dead: usize,
    pub timeout: usize,
}

impl Analysis {
    /// Build the config and run the fast crossing check.
    pub fn new(p: &Puzzle, o: &Options, abort: Arc<AtomicBool>) -> Result<Self> {
        let mut cfg = build_config(p, o)?;
        cfg.abort = Some(abort.clone());
        let (check, elim) = check_with(p, o, &cfg);
        let n = cfg.slot_configs.len();
        // Verdicts only hold words in the pruned lists. Words the crossing check rules out are
        // simply not there, which keeps this small.
        let verdicts = vec![HashMap::new(); n];
        let pruned: Vec<Vec<WordId>> = (0..n)
            .map(|i| {
                if check.consistent {
                    cfg.slot_options[i].iter().copied().filter(|&w| !elim[i].contains(w)).collect()
                } else {
                    cfg.slot_options[i].clone()
                }
            })
            .collect();
        let (fillable, hotspots) = if check.consistent { (Fillable::Unknown, None) } else { (Fillable::No, check.hotspots.clone()) };
        Ok(Analysis {
            puzzle: p.clone(),
            cfg,
            pruned,
            check,
            verdicts,
            fills: vec![HashMap::new(); n],
            fillable,
            sample_fill: None,
            hotspots,
            found: vec![],
            opts_key: format!("{o:?}"),
            min_score: o.min_score,
            reused: false,
            retried: HashSet::new(),
        })
    }

    fn config_with<'a>(&'a self, options: &'a [Vec<WordId>]) -> GridConfig<'a> {
        let mut r = self.cfg.to_config_ref();
        r.slot_options = options;
        r
    }

    fn record_fill(&mut self, choices: &[Choice], grid: Vec<String>) {
        let grid = Arc::new(grid);
        for c in choices {
            self.verdicts[c.slot_id].insert(c.word_id, Verdict::Viable);
            self.fills[c.slot_id].entry(c.word_id).or_insert_with(|| grid.clone());
        }
        self.found.push(grid);
        self.trim_found();
    }

    /// Look for one full fill.
    pub fn search(&mut self, timeout: Duration) -> SearchOutcome {
        if !self.check.consistent {
            return SearchOutcome::Impossible(self.hotspots.clone().unwrap_or_else(|| self.check.hotspots.clone().unwrap()));
        }
        let options = self.pruned.clone();
        let r = self.config_with(&options);
        let (res, diag) = find_fill_with_diagnostics(&r, Some(timeout), None);
        match res {
            Ok(ok) => {
                let grid = fill_grid(&self.puzzle, &r, &ok.choices);
                self.record_fill(&ok.choices, grid.clone());
                self.fillable = Fillable::Yes;
                self.sample_fill = Some(grid.clone());
                SearchOutcome::Filled(grid)
            }
            Err(FillFailure::Abort) => SearchOutcome::Aborted,
            Err(FillFailure::Timeout) => {
                let h = search_heat(&self.puzzle, &self.cfg, &diag);
                self.fillable = Fillable::Timeout;
                self.hotspots = Some(h.clone());
                SearchOutcome::Timeout(h)
            }
            Err(_) => {
                let h = search_heat(&self.puzzle, &self.cfg, &diag);
                self.fillable = Fillable::No;
                self.hotspots = Some(h.clone());
                SearchOutcome::Impossible(h)
            }
        }
    }

    /// Slot index for a key like "3,4,across".
    pub fn slot_index(&self, key: &str) -> Option<usize> {
        (0..self.cfg.slot_configs.len()).find(|&i| slot_key(&self.cfg, i) == key)
    }

    /// Words left in a slot after the crossing check.
    pub fn option_count(&self, slot: usize) -> usize {
        self.pruned[slot].len()
    }

    pub fn slot_count(&self) -> usize {
        self.cfg.slot_configs.len()
    }

    /// Words still in play for a slot, in display order (score, then alphabetical).
    fn display_order(&self, slot: usize) -> Vec<WordId> {
        let len = self.cfg.slot_configs[slot].length;
        let words = &self.cfg.word_list.words[len];
        let mut ws = self.pruned[slot].clone();
        ws.sort_by(|&a, &b| words[b].score.cmp(&words[a].score).then_with(|| words[a].canonical_string.cmp(&words[b].canonical_string)));
        ws
    }

    /// The next word in this slot (within the first `limit`) with no verdict yet.
    pub fn next_unsettled(&self, slot: usize, limit: usize) -> Option<WordId> {
        self.display_order(slot).into_iter().take(limit).find(|w| !self.verdicts[slot].contains_key(w))
    }

    /// Like `next_unsettled`, but only words whose place in display order is `index` modulo `count`,
    /// so `count` workers can split one answer's words.
    pub fn next_unsettled_in(&self, slot: usize, limit: usize, (index, count): (usize, usize)) -> Option<WordId> {
        let count = count.max(1);
        self.display_order(slot).into_iter().take(limit).enumerate().filter(|(j, _)| j % count == index).map(|(_, w)| w).find(|w| !self.verdicts[slot].contains_key(w))
    }

    /// Try one word: fix it in its slot and look for a full fill. Returns None if aborted.
    pub fn settle(&mut self, slot: usize, word: WordId, timeout: Duration) -> Option<Verdict> {
        if self.fillable == Fillable::No {
            self.verdicts[slot].insert(word, Verdict::Dead);
            return Some(Verdict::Dead);
        }
        let mut options = self.pruned.clone();
        self.hint(&mut options);
        options[slot] = vec![word];
        let (res, grid) = {
            let r = self.config_with(&options);
            let (res, _) = find_fill_with_diagnostics(&r, Some(timeout), None);
            let grid = res.as_ref().ok().map(|ok| fill_grid(&self.puzzle, &r, &ok.choices));
            (res, grid)
        };
        let v = match res {
            Ok(ok) => {
                self.record_found(&ok.choices, grid.unwrap_or_default());
                Verdict::Viable
            }
            Err(FillFailure::Abort) => return None,
            Err(FillFailure::Timeout) => Verdict::Timeout,
            Err(_) => Verdict::Dead,
        };
        self.verdicts[slot].insert(word, v);
        Some(v)
    }

    /// Put the newest fill's word first in each answer's list. The search tries the first few
    /// words of a list first, so it starts close to a fill that works and mostly changes the
    /// answers near the word being tried. In a hard corner this is much faster than starting over.
    fn hint(&self, options: &mut [Vec<WordId>]) {
        let Some(base) = self.found.last() else { return };
        let rows: Vec<Vec<char>> = base.iter().map(|r| r.chars().collect()).collect();
        for (i, opts) in options.iter_mut().enumerate() {
            let Some(w) = word_at(&self.cfg.slot_configs[i].cell_coords(), &rows) else { continue };
            let Some(&id) = self.cfg.word_list.word_id_by_string.get(&w) else { continue };
            if let Some(at) = opts.iter().position(|&o| o == id) {
                opts[..=at].rotate_right(1);
            }
        }
    }

    /// The next word in this slot (within the first `limit`) that timed out and was not tried
    /// again yet. Mark it tried. For a second pass with a longer limit.
    pub fn next_timeout(&mut self, slot: usize, limit: usize) -> Option<WordId> {
        let w = self.display_order(slot).into_iter().take(limit).find(|w| self.verdicts[slot].get(w) == Some(&Verdict::Timeout) && !self.retried.contains(&(slot, *w)))?;
        self.retried.insert((slot, w));
        Some(w)
    }

    /// Like `next_timeout`, for one of `count` workers (see `next_unsettled_in`).
    pub fn next_timeout_in(&mut self, slot: usize, limit: usize, (index, count): (usize, usize)) -> Option<WordId> {
        let count = count.max(1);
        let w = self
            .display_order(slot)
            .into_iter()
            .take(limit)
            .enumerate()
            .filter(|(j, _)| j % count == index)
            .map(|(_, w)| w)
            .find(|w| self.verdicts[slot].get(w) == Some(&Verdict::Timeout) && !self.retried.contains(&(slot, *w)))?;
        self.retried.insert((slot, w));
        Some(w)
    }

    /// A fill found while settling a word: it proves the grid fillable and every word in it viable.
    fn record_found(&mut self, choices: &[Choice], grid: Vec<String>) {
        if self.sample_fill.is_none() {
            self.sample_fill = Some(grid.clone());
        }
        self.fillable = Fillable::Yes;
        self.record_fill(choices, grid);
    }

    /// True when every word left in this slot is proven dead. Then no full fill exists.
    pub fn all_dead(&self, slot: usize) -> bool {
        self.pruned[slot].iter().all(|w| self.verdicts[slot].get(w) == Some(&Verdict::Dead))
    }

    /// Words for a slot with their verdicts, in display order.
    pub fn words(&self, slot: usize, limit: usize) -> Vec<WordVerdict> {
        let len = self.cfg.slot_configs[slot].length;
        let all_dead = self.fillable == Fillable::No;
        // Words that work come first and proven-dead words last, so words that work always make the list.
        let mut order = self.display_order(slot);
        order.sort_by_key(|w| match self.verdicts[slot].get(w) {
            Some(Verdict::Viable) => 0,
            Some(Verdict::Dead) => 2,
            _ => 1,
        });
        order
            .into_iter()
            .take(limit)
            .map(|w| {
                let word = &self.cfg.word_list.words[len][w];
                let verdict = if all_dead { Some(Verdict::Dead) } else { self.verdicts[slot].get(&w).copied() };
                WordVerdict { word: word.canonical_string.clone(), score: word.score, verdict, fill: None }
            })
            .collect()
    }

    /// Like `words`, plus one full fill for each viable word. Words that share a fill point to the
    /// same entry in the returned list, so the list stays short.
    pub fn words_with_fills(&self, slot: usize, limit: usize) -> (Vec<WordVerdict>, Vec<Vec<String>>) {
        let mut words = self.words(slot, limit);
        if self.fillable == Fillable::No {
            return (words, vec![]);
        }
        let len = self.cfg.slot_configs[slot].length;
        let ids: HashMap<&str, WordId> = self.pruned[slot].iter().map(|&w| (self.cfg.word_list.words[len][w].canonical_string.as_str(), w)).collect();
        let mut fills: Vec<Arc<Vec<String>>> = vec![];
        for wv in words.iter_mut().filter(|w| w.verdict == Some(Verdict::Viable)) {
            let Some(g) = ids.get(wv.word.as_str()).and_then(|w| self.fills[slot].get(w)) else { continue };
            wv.fill = Some(fills.iter().position(|f| Arc::ptr_eq(f, g)).unwrap_or_else(|| {
                fills.push(g.clone());
                fills.len() - 1
            }));
        }
        (words, fills.into_iter().map(|f| (*f).clone()).collect())
    }

    pub fn summaries(&self) -> Vec<SlotSummary> {
        (0..self.slot_count())
            .map(|i| {
                let count = |v: Verdict| self.pruned[i].iter().filter(|w| self.verdicts[i].get(w) == Some(&v)).count();
                let (viable, timeout) = (count(Verdict::Viable), count(Verdict::Timeout));
                let dead = if self.fillable == Fillable::No { self.pruned[i].len() } else { count(Verdict::Dead) };
                SlotSummary { key: slot_key(&self.cfg, i), options: self.pruned[i].len(), viable, dead, timeout }
            })
            .collect()
    }

    /// Slots in the order to settle them: fewest words first, so tight spots resolve early.
    pub fn settle_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.slot_count()).filter(|&i| self.pruned[i].len() > 1).collect();
        order.sort_by_key(|&i| self.pruned[i].len());
        order
    }
}

impl Analysis {
    /// Keep `found` to twice the cap (the per-slot `fills` maps still hold each word's witness).
    fn trim_found(&mut self) {
        if self.found.len() > 2 * KNOWN_MAX_FILLS {
            self.found.drain(..self.found.len() - KNOWN_MAX_FILLS);
        }
    }

    /// What this analysis knows now, to reuse for a later grid.
    pub fn known(&self) -> Known {
        let mut dead: HashMap<String, Vec<String>> = HashMap::new();
        let mut timeout: HashMap<String, Vec<String>> = HashMap::new();
        for (i, vs) in self.verdicts.iter().enumerate() {
            let words = &self.cfg.word_list.words[self.cfg.slot_configs[i].length];
            for (&w, &v) in vs {
                let to = match v {
                    Verdict::Dead => &mut dead,
                    Verdict::Timeout => &mut timeout,
                    Verdict::Viable => continue,
                };
                to.entry(slot_key(&self.cfg, i)).or_default().push(words[w].normalized_string.clone());
            }
        }
        // Keep the newest fills, and an even share of dead words per answer, within the caps.
        let fills = self.found[self.found.len().saturating_sub(KNOWN_MAX_FILLS)..].to_vec();
        let total: usize = dead.values().map(Vec::len).sum();
        if total > KNOWN_MAX_DEAD {
            let share = KNOWN_MAX_DEAD / dead.len().max(1);
            dead.values_mut().for_each(|v| v.truncate(share));
        }
        timeout.values_mut().for_each(|v| v.truncate(200));
        Known { dead, timeout, fills, fillable: self.fillable, sample_fill: self.sample_fill.clone(), hotspots: self.hotspots.clone(), ..self.known_grid() }
    }

    /// Start from what earlier analyses learned (`prev`, newest first). The same grid gets back
    /// everything. A grid that only adds letters to an earlier one gets that grid's dead words and
    /// every earlier fill that still matches its letters; one matching fill makes it fillable, so
    /// the search can be skipped. Call this right after `new`.
    pub fn reuse(&mut self, prev: &[Arc<Known>]) -> Reuse {
        self.reused = true;
        if !self.check.consistent {
            return Reuse::Nothing;
        }
        let keys: HashMap<String, usize> = (0..self.slot_count()).map(|i| (slot_key(&self.cfg, i), i)).collect();
        let me = self.known_grid();
        if let Some(k) = prev.iter().find(|k| k.same_grid(&me)) {
            self.carry_words(&keys, &k.dead, Verdict::Dead);
            self.carry_words(&keys, &k.timeout, Verdict::Timeout);
            for f in &k.fills {
                self.adopt_fill(f);
            }
            self.fillable = k.fillable;
            self.sample_fill = k.sample_fill.clone().or_else(|| self.sample_fill.clone());
            self.hotspots = k.hotspots.clone();
            return Reuse::Exact;
        }
        let (mut dead, mut fills) = (0, 0);
        let mut seen: HashSet<*const Vec<String>> = HashSet::new();
        let mut any = false;
        let from: Vec<&Arc<Known>> = prev.iter().filter(|k| self.extends(k)).collect();
        for k in from {
            any = true;
            dead += self.carry_words(&keys, &k.dead, Verdict::Dead);
            for f in &k.fills {
                if seen.insert(Arc::as_ptr(f)) && self.agrees(f) {
                    self.adopt_fill(f);
                    fills += 1;
                }
            }
        }
        if !any {
            return Reuse::Nothing;
        }
        if fills > 0 {
            self.fillable = Fillable::Yes;
        }
        Reuse::Carried { dead, fills }
    }

    /// A key for the grid, marks and word list options of a puzzle: two puzzles with the same key
    /// get the same analysis (a clue edit keeps it).
    pub fn grid_key(p: &Puzzle, o: &Options) -> String {
        format!("{:?}|{:?}|{:?}|{o:?}", upper(&p.grid), p.rebus, p.bars)
    }

    /// The grid and marks part of `Known`, with nothing learned yet.
    fn known_grid(&self) -> Known {
        Known {
            grid: upper(&self.puzzle.grid),
            rebus: self.puzzle.rebus.clone(),
            bars: self.puzzle.bars.clone(),
            opts: self.opts_key.clone(),
            dead: HashMap::new(),
            timeout: HashMap::new(),
            fills: vec![],
            fillable: Fillable::Unknown,
            sample_fill: None,
            hotspots: None,
        }
    }

    /// Set a verdict for each listed word that has none yet. Returns how many were set.
    fn carry_words(&mut self, keys: &HashMap<String, usize>, words: &HashMap<String, Vec<String>>, v: Verdict) -> usize {
        let mut n = 0;
        for (key, ws) in words {
            let Some(&i) = keys.get(key) else { continue };
            let len = self.cfg.slot_configs[i].length;
            for w in ws {
                // Words the new crossing check ruled out drop out here.
                let Some(&id) = self.cfg.word_list.word_id_by_string.get(w) else { continue };
                if id < self.cfg.word_list.words[len].len() && self.pruned_has(i, id) && !self.verdicts[i].contains_key(&id) {
                    self.verdicts[i].insert(id, v);
                    n += 1;
                }
            }
        }
        n
    }

    fn pruned_has(&self, slot: usize, id: WordId) -> bool {
        self.pruned[slot].contains(&id)
    }

    /// Record a full fill found for an earlier grid that matches this one: each word in it is viable.
    /// Take full fills another worker found for this grid, each as the letters of the grid's empty
    /// squares in reading order (as `FocusSnap::fills` sends them). A fill that does not fit is
    /// skipped. One that fits proves the grid fillable, so the probe and full search are skipped.
    pub fn adopt_fills(&mut self, fills: &[String]) {
        if !self.check.consistent || fills.is_empty() {
            return;
        }
        let empty = self.puzzle.grid.iter().map(|r| r.chars().filter(|&c| c == '.').count()).sum::<usize>();
        for f in fills {
            let mut letters = f.chars();
            if f.chars().count() != empty {
                continue;
            }
            let grid: Vec<String> = self.puzzle.grid.iter().map(|r| r.chars().map(|c| if c == '.' { letters.next().unwrap_or('.').to_ascii_uppercase() } else { c.to_ascii_uppercase() }).collect()).collect();
            if self.agrees(&grid) && !self.found.iter().any(|g| **g == grid) {
                self.adopt_fill(&Arc::new(grid));
                self.fillable = Fillable::Yes;
            }
        }
    }

    fn adopt_fill(&mut self, f: &Arc<Vec<String>>) {
        let rows: Vec<Vec<char>> = f.iter().map(|r| r.chars().collect()).collect();
        for i in 0..self.slot_count() {
            let Some(w) = word_at(&self.cfg.slot_configs[i].cell_coords(), &rows) else { continue };
            let Some(&id) = self.cfg.word_list.word_id_by_string.get(&w) else { continue };
            self.verdicts[i].insert(id, Verdict::Viable);
            self.fills[i].entry(id).or_insert_with(|| f.clone());
        }
        if self.sample_fill.is_none() {
            self.sample_fill = Some((**f).clone());
        }
        self.found.push(f.clone());
        self.trim_found();
    }

    /// True if a full fill has every letter this grid has. Squares the fill left empty are squares
    /// no answer in the engine covers, so they don't matter.
    fn agrees(&self, f: &[String]) -> bool {
        f.len() == self.puzzle.grid.len()
            && self.puzzle.grid.iter().zip(f).all(|(row, frow)| {
                row.chars().count() == frow.chars().count() && row.chars().zip(frow.chars()).all(|(c, w)| c == '.' || c == '#' || w == '.' || c.eq_ignore_ascii_case(&w))
            })
    }

    /// True if this grid is `k`'s grid with letters added: same size, black squares, marks and
    /// word list, every old letter still there, and no new word that changes the rules (see
    /// `new_words_ok`). Then every fill of this grid is a fill of `k`'s grid too.
    fn extends(&self, k: &Known) -> bool {
        let g = upper(&self.puzzle.grid);
        let same_shape = k.grid.len() == g.len()
            && k.rebus == self.puzzle.rebus
            && k.bars == self.puzzle.bars
            && k.opts == self.opts_key
            && k.grid.iter().zip(&g).all(|(o, n)| {
                o.chars().count() == n.chars().count() && o.chars().zip(n.chars()).all(|(a, b)| (a == '#') == (b == '#') && (a == '.' || a == b))
            });
        same_shape && self.new_words_ok(&k.grid)
    }

    /// Answers the new letters complete must be words the old search could have used: in the list,
    /// not below the minimum score, and not a duplicate of another complete answer. The engine takes
    /// any complete answer as given (even one not in the list) and lets complete answers repeat, so
    /// without this check the new grid could have fills the old one didn't.
    fn new_words_ok(&self, old: &[String]) -> bool {
        let old_rows: Vec<Vec<char>> = old.iter().map(|r| r.chars().collect()).collect();
        let new_rows: Vec<Vec<char>> = self.puzzle.grid.iter().map(|r| r.chars().collect()).collect();
        let wl = &self.cfg.word_list;
        // (global word id, newly complete) for each complete answer.
        let mut done: Vec<((usize, WordId), bool)> = vec![];
        for sc in &self.cfg.slot_configs {
            let cells = sc.cell_coords();
            let Some(w) = word_at(&cells, &new_rows) else { continue };
            let Some(&id) = wl.word_id_by_string.get(&w) else { return false };
            let fresh = word_at(&cells, &old_rows).is_none();
            let word = &wl.words[sc.length][id];
            if fresh && (word.hidden || word.score < self.min_score) {
                return false;
            }
            done.push(((sc.length, id), fresh));
        }
        for (a, &(ga, fresh)) in done.iter().enumerate() {
            if !fresh {
                continue;
            }
            let dupes = wl.dupe_index.get_dupes_by_length(ga);
            let clash = done.iter().enumerate().any(|(b, &(gb, _))| a != b && (ga == gb || dupes.get(&gb.0).is_some_and(|s| s.contains(&gb.1))));
            if clash {
                return false;
            }
        }
        true
    }
}

/// The word in these squares, in lower case (the word list's form), or None if one is empty.
fn word_at(cells: &[(usize, usize)], rows: &[Vec<char>]) -> Option<String> {
    cells.iter().map(|&(x, y)| rows.get(y).and_then(|r| r.get(x)).copied().filter(|c| c.is_alphabetic()).map(|c| c.to_ascii_lowercase())).collect()
}

/// An empty square with only one possible letter after the crossing check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForcedSquare {
    pub x: usize,
    pub y: usize,
    pub letter: char,
}

/// An answer with empty squares and only one word left after the crossing check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForcedWord {
    pub key: String,
    pub label: String,
    pub word: String,
}

/// Forced letters and words. Both are empty when the crossing check finds no possible fill.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Forced {
    pub squares: Vec<ForcedSquare>,
    pub words: Vec<ForcedWord>,
}

impl Analysis {
    /// Empty squares where the words left all agree on one letter, and answers with only one word
    /// left. Words left: the crossing check's lists, minus words the search has proven dead. So an
    /// answer whose other words are all dead shows its last word as forced.
    /// Squares in an answer through a rebus square are left out (the engine doesn't see that answer).
    pub fn forced(&self) -> Forced {
        let mut out = Forced::default();
        if !self.check.consistent || self.fillable == Fillable::No {
            return out;
        }
        let p = &self.puzzle;
        let infos: HashMap<String, crate::puzzle::SlotInfo> = p.slots().into_iter().map(|s| (s.key.clone(), s)).collect();
        let rebus_cells: std::collections::HashSet<(usize, usize)> = infos.values().filter(|s| s.rebus).flat_map(|s| s.cells()).collect();
        let wl = &self.cfg.word_list;
        // Letter per square: Some(c) while every word so far agrees, None once two differ.
        let mut letters: HashMap<(usize, usize), Option<char>> = HashMap::new();
        let mut pinned: HashMap<(usize, usize), char> = HashMap::new();
        for (i, sc) in self.cfg.slot_configs.iter().enumerate() {
            let key = slot_key(&self.cfg, i);
            let Some(info) = infos.get(&key) else { continue };
            let cells = sc.cell_coords();
            let empty: Vec<usize> = (0..cells.len()).filter(|&j| p.cell(cells[j].0, cells[j].1) == '.').collect();
            let touches_rebus = empty.iter().any(|&j| rebus_cells.contains(&cells[j]));
            let empty: Vec<usize> = empty.into_iter().filter(|&j| !rebus_cells.contains(&cells[j])).collect();
            let alive: Vec<WordId> = self.pruned[i].iter().copied().filter(|w| self.verdicts[i].get(w) != Some(&Verdict::Dead)).collect();
            if empty.is_empty() || alive.is_empty() {
                continue;
            }
            let words = &wl.words[sc.length];
            let glyph = |w: WordId, j: usize| wl.glyphs[words[w].glyphs[j]].to_ascii_uppercase();
            if alive.len() == 1 && !touches_rebus {
                let w = alive[0];
                // An answer with one word left forces all its letters, even where the crossing
                // answer still has several words (those just haven't been proven dead yet).
                for &j in &empty {
                    pinned.insert(cells[j], glyph(w, j));
                }
                out.words.push(ForcedWord { key: key.clone(), label: crate::edit::label(info), word: (0..sc.length).map(|j| glyph(w, j)).collect() });
            }
            for &j in &empty {
                let first = glyph(alive[0], j);
                let same = alive.iter().all(|&w| glyph(w, j) == first);
                let e = letters.entry(cells[j]).or_insert(Some(first));
                if !same || *e != Some(first) {
                    *e = None;
                }
            }
        }
        letters.extend(pinned.into_iter().map(|(k, c)| (k, Some(c))));
        out.squares = letters.into_iter().filter_map(|((x, y), c)| c.map(|letter| ForcedSquare { x, y, letter })).collect();
        out.squares.sort_by_key(|s| (s.y, s.x));
        out.words.sort_by_key(|w| infos.get(&w.key).map(|s| (s.dir == crate::puzzle::Dir::Down, s.number)));
        out
    }
}

/// Forced letters and words for a puzzle (the crossing check only, no search).
pub fn forced(p: &Puzzle, o: &Options) -> Result<Forced> {
    Ok(Analysis::new(p, o, Arc::new(AtomicBool::new(false)))?.forced())
}

/// Write forced letters into the grid: all of them, or only those in one answer (a label like
/// "17A" or a key) or one square ("x,y"). Returns how many squares changed.
pub fn write_forced(p: &mut Puzzle, f: &Forced, target: Option<&str>) -> Result<usize> {
    let only: Option<Vec<(usize, usize)>> = match target.map(str::trim).filter(|t| !t.is_empty()) {
        None => None,
        Some(t) => Some(match crate::edit::parse_square(t) {
            Ok(xy) if t.matches(',').count() == 1 => vec![xy],
            _ => p.find_slot(t)?.cells(),
        }),
    };
    let mut n = 0;
    for s in &f.squares {
        if only.as_ref().is_some_and(|c| !c.contains(&(s.x, s.y))) || p.cell(s.x, s.y) != '.' {
            continue;
        }
        let row: Vec<char> = p.grid[s.y].chars().enumerate().map(|(i, old)| if i == s.x { s.letter } else { old }).collect();
        p.grid[s.y] = row.into_iter().collect();
        n += 1;
    }
    p.tidy();
    Ok(n)
}

/// One place a word could go.
#[derive(Debug, Clone, Serialize)]
pub struct Spot {
    pub key: String,
    pub label: String,
    /// The answer's letters now, '.' for empty.
    pub pattern: String,
    /// Viable: the grid still fills with the word here. Dead: it can't. Timeout: no answer in time.
    pub verdict: Verdict,
    /// True if the word is already in this answer.
    pub already: bool,
    /// One full fill with the word here, when viable.
    pub fill: Option<Vec<String>>,
}

/// Find every answer a word fits (right length, no clashing letters), and check whether the grid
/// still fills with the word there. Viable spots come first.
pub fn find_spots(p: &Puzzle, o: &Options, word: &str, per_spot: Duration) -> Result<(String, Vec<Spot>)> {
    let word: String = word.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_uppercase();
    anyhow::ensure!(!word.is_empty(), "type a word");
    let mut spots = vec![];
    // Answers through a rebus square are left out: one letter can't go in a rebus square.
    for s in p.slots().into_iter().filter(|s| !s.rebus) {
        if s.len != word.chars().count() {
            continue;
        }
        let fits = s.pattern.chars().zip(word.chars()).all(|(a, b)| a == '.' || a.to_ascii_uppercase() == b);
        if !fits {
            continue;
        }
        let already = s.pattern.to_uppercase() == word;
        let mut q = p.clone();
        crate::edit::place(&mut q, &s.key, &word)?;
        let mut a = Analysis::new(&q, o, Arc::new(AtomicBool::new(false)))?;
        let (verdict, fill) = match a.search(per_spot) {
            SearchOutcome::Filled(g) => (Verdict::Viable, Some(g)),
            SearchOutcome::Timeout(_) => (Verdict::Timeout, None),
            _ => (Verdict::Dead, None),
        };
        spots.push(Spot { label: label_for(p, &s.key), key: s.key, pattern: s.pattern, verdict, already, fill });
    }
    let rank = |v: Verdict| match v {
        Verdict::Viable => 0,
        Verdict::Timeout => 1,
        Verdict::Dead => 2,
    };
    spots.sort_by_key(|s| rank(s.verdict));
    Ok((word, spots))
}

/// `label` like "41A" for a slot key, from the puzzle's own numbering.
pub fn label_for(p: &Puzzle, key: &str) -> String {
    p.slots()
        .into_iter()
        .find(|s| s.key == key)
        .map(|s| format!("{}{}", s.number, if s.dir == crate::puzzle::Dir::Across { "A" } else { "D" }))
        .unwrap_or_else(|| key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Options {
        Options { min_score: 50, max_shared_substring: None, wordlists: vec![], no_default_list: false, ..Default::default() }
    }

    #[test]
    fn settles_words() {
        crate::need_list!();
        let mut p = Puzzle::blank("t", 3, 3, "none");
        p.grid = vec!["CAT".into(), "...".into(), "...".into()];
        let mut a = Analysis::new(&p, &opts(), Arc::new(AtomicBool::new(false))).unwrap();
        assert!(matches!(a.search(Duration::from_secs(10)), SearchOutcome::Filled(_)));
        assert_eq!(a.fillable, Fillable::Yes);
        let slot = a.slot_index("0,1,across").unwrap();
        let mut n = 0;
        while let Some(w) = a.next_unsettled(slot, 20) {
            assert!(a.settle(slot, w, Duration::from_secs(5)).is_some());
            n += 1;
        }
        assert!(n <= 20);
        let words = a.words(slot, 20);
        assert!(words.iter().all(|w| w.verdict.is_some()));
        assert!(words.iter().any(|w| w.verdict == Some(Verdict::Viable)));
        // Each viable word has a full fill that uses it, in its answer.
        let (words, fills) = a.words_with_fills(slot, 20);
        for w in words.iter().filter(|w| w.verdict == Some(Verdict::Viable)) {
            let g = &fills[w.fill.expect("viable word has a fill")];
            assert_eq!(g[0], "CAT");
            assert_eq!(g[1].to_lowercase(), w.word.to_lowercase());
            assert!(!g.iter().any(|r| r.contains('.')), "{g:?}");
        }
        assert!(words.iter().filter(|w| w.verdict != Some(Verdict::Viable)).all(|w| w.fill.is_none()));
    }

    #[test]
    fn finds_spots() {
        crate::need_list!();
        let mut p = Puzzle::blank("t", 3, 3, "none");
        p.grid = vec!["CAT".into(), "...".into(), "...".into()];
        let (word, spots) = find_spots(&p, &opts(), "a-p-e", Duration::from_secs(5)).unwrap();
        assert_eq!(word, "APE");
        // Fits 0,1,across, 0,2,across and 1,0,down (starts with A); not 0,0,down (C) or 0,0,across.
        let keys: Vec<_> = spots.iter().map(|s| s.key.as_str()).collect();
        assert!(keys.contains(&"1,0,down"), "{keys:?}");
        assert!(!keys.contains(&"0,0,down") && !keys.contains(&"0,0,across"));
        assert!(spots.iter().any(|s| s.verdict == Verdict::Viable));
    }

    /// Options that use only the given words (score 60 each), so tests don't depend on the big list.
    fn small_list(name: &str, words: &[&str]) -> Options {
        let path = std::env::temp_dir().join(format!("cgh-forced-{name}-{}.dict", std::process::id()));
        std::fs::write(&path, words.iter().map(|w| format!("{w};60\n")).collect::<String>()).unwrap();
        Options { min_score: 50, max_shared_substring: None, wordlists: vec![path], no_default_list: true, ..Default::default() }
    }

    fn grid(rows: &[&str]) -> Puzzle {
        let mut p = Puzzle::blank("t", rows[0].len(), rows.len(), "none");
        p.grid = rows.iter().map(|r| r.to_string()).collect();
        p
    }

    #[test]
    fn forced_letter_without_forced_word() {
        // CART and CARD agree on A and R, not on the last letter.
        let o = small_list("letter", &["cart", "card"]);
        let f = forced(&grid(&["C..."]), &o).unwrap();
        let got: Vec<_> = f.squares.iter().map(|s| (s.x, s.y, s.letter)).collect();
        assert_eq!(got, vec![(1, 0, 'A'), (2, 0, 'R')]);
        assert!(f.words.is_empty());
    }

    #[test]
    fn crossing_check_forces_a_word() {
        // 1A C.T fits CAT or COT, but 2D must be AX or IX, so only CAT crosses. 2D is then AX.
        let o = small_list("cross", &["cat", "cot", "ax", "ix"]);
        let mut p = grid(&["C.T", "#.#"]);
        let f = forced(&p, &o).unwrap();
        let got: Vec<_> = f.squares.iter().map(|s| (s.x, s.y, s.letter)).collect();
        assert_eq!(got, vec![(1, 0, 'A'), (1, 1, 'X')]);
        let words: Vec<_> = f.words.iter().map(|w| (w.label.as_str(), w.word.as_str())).collect();
        assert_eq!(words, vec![("1A", "CAT"), ("2D", "AX")]);

        // Write one square, then the rest.
        assert_eq!(write_forced(&mut p, &f, Some("1,1")).unwrap(), 1);
        assert_eq!(p.grid, vec!["C.T", "#X#"]);
        assert_eq!(write_forced(&mut p, &f, Some("1A")).unwrap(), 1);
        assert_eq!(p.grid, vec!["CAT", "#X#"]);
        assert_eq!(write_forced(&mut p, &f, None).unwrap(), 0, "filled squares stay as they are");
        assert!(forced(&p, &o).unwrap().squares.is_empty());
    }

    #[test]
    fn no_forced_letters_without_a_fill() {
        let o = small_list("dead", &["cat", "ax"]);
        let f = forced(&grid(&["Q..", "#.#"]), &o).unwrap();
        assert!(f.squares.is_empty() && f.words.is_empty());
    }

    #[test]
    fn rebus_answers_get_no_forced_letters() {
        // 1A goes through a rebus square, so the engine leaves it out. 2D (A. -> AX) still crosses it.
        let o = small_list("rebus", &["ax", "cat"]);
        let mut p = grid(&["C.T", "#.#"]);
        p.rebus.insert("0,0".into(), "CA".into());
        let f = forced(&p, &o).unwrap();
        assert!(f.squares.iter().all(|s| (s.x, s.y) != (1, 0)), "{:?}", f.squares);
        assert!(f.words.is_empty(), "{:?}", f.words);
    }

    fn analysis(p: &Puzzle) -> Analysis {
        Analysis::new(p, &opts(), Arc::new(AtomicBool::new(false))).unwrap()
    }

    fn verdict_of(a: &Analysis, slot: usize, word: &str) -> Option<Verdict> {
        let id = a.cfg.word_list.word_id_by_string.get(&word.to_lowercase())?;
        a.verdicts[slot].get(id).copied()
    }

    /// The 3x3 test grid, and one full fill for it.
    fn cat_grid() -> (Puzzle, Analysis, Vec<String>) {
        let p = grid(&["CAT", "...", "..."]);
        let mut a = analysis(&p);
        let SearchOutcome::Filled(g) = a.search(Duration::from_secs(10)) else { panic!("no fill") };
        (p, a, g)
    }

    #[test]
    fn placing_a_viable_word_needs_no_search() {
        crate::need_list!();
        let (p, a, g) = cat_grid();
        let k = Arc::new(a.known());
        // Place the fill's middle row: the old fill still matches, so the grid is fillable at once.
        let mut q = p.clone();
        q.grid[1] = g[1].clone();
        let mut b = analysis(&q);
        assert_eq!(b.fillable, Fillable::Unknown);
        assert_eq!(b.reuse(&[k]), Reuse::Carried { dead: 0, fills: 1 });
        assert_eq!(b.fillable, Fillable::Yes);
        assert_eq!(b.sample_fill.as_ref(), Some(&g));
        let last = b.slot_index("0,2,across").unwrap();
        assert_eq!(verdict_of(&b, last, &g[2]), Some(Verdict::Viable));
        let (words, fills) = b.words_with_fills(last, 50);
        let w = words.iter().find(|w| w.word.eq_ignore_ascii_case(&g[2])).expect("word in list");
        assert_eq!(fills[w.fill.unwrap()], g);
        // Settling picks up where the old analysis left off: that word needs no new check.
        assert!(b.next_unsettled(last, usize::MAX).is_none_or(|w| b.cfg.word_list.words[3][w].normalized_string != g[2].to_lowercase()));
    }

    #[test]
    fn viable_word_with_a_clashing_fill_is_not_carried() {
        crate::need_list!();
        let (p, a, g) = cat_grid();
        let k = Arc::new(a.known());
        let mid = a.slot_index("0,1,across").unwrap();
        assert_eq!(verdict_of(&a, mid, &g[1]), Some(Verdict::Viable));
        // Put a different letter where the fill has its bottom-left letter.
        let mut found = false;
        for c in ['S', 'E', 'A', 'D', 'O'] {
            if g[2].starts_with(c) {
                continue;
            }
            let mut q = p.clone();
            q.grid[2] = format!("{c}..");
            let mut b = analysis(&q);
            if !b.check.consistent {
                continue;
            }
            assert_eq!(b.reuse(&[k.clone()]), Reuse::Carried { dead: 0, fills: 0 });
            assert_eq!(b.fillable, Fillable::Unknown, "no fill matches, so the search must run");
            assert_ne!(verdict_of(&b, mid, &g[1]), Some(Verdict::Viable));
            assert!(b.sample_fill.is_none());
            found = true;
            break;
        }
        assert!(found);
    }

    #[test]
    fn dead_words_carry_over() {
        crate::need_list!();
        let (p, a, g) = cat_grid();
        // The new grid: one more letter, from the fill.
        let mut q = p.clone();
        q.grid[2] = format!("{}..", &g[2][..1]);
        let b0 = analysis(&q);
        let mid = b0.slot_index("0,1,across").unwrap();
        let len = b0.cfg.slot_configs[mid].length;
        let word = |w: WordId| b0.cfg.word_list.words[len][w].normalized_string.clone();
        let victim = b0.pruned[mid].iter().map(|&w| word(w)).find(|w| !w.eq_ignore_ascii_case(&g[1])).expect("another word");
        // Say the old search proved `victim` dead in the middle row (the search finds such words
        // in bigger grids; arc consistency catches every dead word in a grid this small).
        let mut k = a.known();
        k.dead.entry("0,1,across".into()).or_default().push(victim.clone());
        // A word the new crossing check rules out is not carried.
        k.dead.entry("0,1,across".into()).or_default().push("zzz".into());
        drop(b0);
        let mut b = analysis(&q);
        assert_eq!(b.reuse(&[Arc::new(k)]), Reuse::Carried { dead: 1, fills: 1 });
        assert_eq!(verdict_of(&b, mid, &victim), Some(Verdict::Dead));
        assert!(b.summaries()[mid].dead >= 1);
        // Dead words drop out of the forced letters, and the fill's words stay viable.
        let _ = b.forced();
        assert_eq!(verdict_of(&b, mid, &g[1]), Some(Verdict::Viable));
        // A grid that removes a letter is not an extension: nothing carries.
        let mut c = analysis(&grid(&["CA.", "...", "..."]));
        let kq = Arc::new(b.known());
        assert_eq!(c.reuse(&[kq]), Reuse::Nothing);
    }

    #[test]
    fn same_grid_gets_everything_back() {
        crate::need_list!();
        let (p, mut a, _) = cat_grid();
        let mid = a.slot_index("0,1,across").unwrap();
        for _ in 0..10 {
            let Some(w) = a.next_unsettled(mid, 50) else { break };
            a.settle(mid, w, Duration::from_secs(5)).unwrap();
        }
        let k = Arc::new(a.known());
        let mut b = analysis(&p);
        assert_eq!(b.reuse(&[k]), Reuse::Exact);
        assert_eq!(b.fillable, a.fillable);
        assert_eq!(b.sample_fill, a.sample_fill);
        let json = |x: &Analysis| serde_json::to_string(&(x.summaries(), x.words_with_fills(mid, 50))).unwrap();
        assert_eq!(json(&a), json(&b));
    }

    #[test]
    fn new_word_off_the_list_blocks_carry() {
        // Completing an answer with a word not in the list: the engine takes it as given, so the
        // new grid may have fills the old one didn't. Nothing carries.
        let o = small_list("offlist", &["cat", "cot", "ax", "ix", "ox"]);
        let p = grid(&["C.T", "#.#"]);
        let mut a = Analysis::new(&p, &o, Arc::new(AtomicBool::new(false))).unwrap();
        assert!(matches!(a.search(Duration::from_secs(5)), SearchOutcome::Filled(_)));
        let k = Arc::new(a.known());
        // CUT and UX are not in the list; complete answers are taken as given, so the check passes.
        let mut b = Analysis::new(&grid(&["CUT", "#X#"]), &o, Arc::new(AtomicBool::new(false))).unwrap();
        assert!(b.check.consistent);
        assert_eq!(b.reuse(&[k.clone()]), Reuse::Nothing);
        // Completing it with list words carries the old fill.
        let mut c = Analysis::new(&grid(&["CAT", "#X#"]), &o, Arc::new(AtomicBool::new(false))).unwrap();
        assert!(matches!(c.reuse(&[k]), Reuse::Carried { fills: 1, .. }));
        assert_eq!(c.fillable, Fillable::Yes);
    }
}
