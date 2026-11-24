//! The steps of a live check, and what it reports (`Snapshot`). The server runs them in a background
//! job per open puzzle (on a server); a browser runs them in a Web Worker for a time
//! budget per call (`run_for`). Both go through `work`, so they report the same things.
//!
//! What a check learned is kept in `Memory` (the last few grids per puzzle). A new check starts from
//! there: a grid seen before (undo, redo) gets its old results back, and a grid that only adds
//! letters (placing a suggested word) keeps the dead words and matching fills (`Analysis::reuse`).

use crate::engine::{CheckReport, Heat, Options};
use crate::live::{Analysis, Fillable, Forced, Known, SearchOutcome, SlotSummary, WordVerdict};
use crate::puzzle::Puzzle;
use instant::Instant;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

/// How long one full search may run.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to try one word.
const PER_WORD: Duration = Duration::from_secs(2);
/// How long to try a word of the selected answer again after it ran out of time, once every
/// word there has had its first try. A host can cap it (`Host::retry_limit`).
pub const RETRY_PER_WORD: Duration = Duration::from_secs(6);
/// `run_for`: retries run only in calls with at least this budget, and end at most this long past it.
const RETRY_MIN_BUDGET: Duration = Duration::from_millis(2500);
const RETRY_MARGIN: Duration = Duration::from_secs(2);
/// Before the first fill: how long to try words one by one, and how long per word.
const PROBE_TIME: Duration = Duration::from_secs(12);
const PROBE_PER_WORD: Duration = Duration::from_secs(4);
/// Words to settle in the selected answer, and in every other answer.
const FOCUS_LIMIT: usize = 200;
/// The top of the selected answer's list, checked in full (with retries) before the rest.
const FOCUS_FIRST: usize = 40;
const OTHER_LIMIT: usize = 20;
/// Most fills to send for the selected answer's viable words (for the hover preview).
const FOCUS_FILLS: usize = 100;
/// Grids to remember per puzzle, and in all.
const MEMORY_PER_PATH: usize = 8;
const MEMORY_TOTAL: usize = 24;
/// Rough byte budget for all remembered grids. The oldest go first; the newest always stays.
const MEMORY_BYTES: usize = 64 << 20;

/// What recent checks learned, newest first: (puzzle path, check id, what it knows).
#[derive(Default)]
pub struct Memory {
    entries: VecDeque<(String, u64, Arc<Known>)>,
    /// The last analysis `run_for` used: (puzzle path, `Analysis::grid_key`, the analysis). The next
    /// call for the same path and grid goes on with it and skips `Analysis::new`.
    kept: Option<(String, String, Analysis)>,
}

impl Memory {
    /// Store (or update) what a check knows. An older entry for the same grid goes: the check
    /// started from it, so it knows at least as much.
    pub fn remember(&mut self, path: &str, job: u64, k: Known) {
        self.entries.retain(|(p, id, e)| !(p == path && (*id == job || e.same_grid(&k))));
        self.entries.push_front((path.to_string(), job, Arc::new(k)));
        let mut n = 0;
        self.entries.retain(|(p, _, _)| {
            if p == path {
                n += 1;
            }
            p != path || n <= MEMORY_PER_PATH
        });
        self.entries.truncate(MEMORY_TOTAL);
        let mut bytes = 0;
        let mut keep = 0;
        for (_, _, k) in &self.entries {
            bytes += k.approx_bytes();
            if keep > 0 && bytes > MEMORY_BYTES {
                break;
            }
            keep += 1;
        }
        self.entries.truncate(keep);
    }

    /// Drop the kept analysis and what earlier checks learned (the word list or the word choices
    /// changed, so a kept fill may use a word that is now banned).
    pub fn forget_analysis(&mut self) {
        self.kept = None;
        self.entries.clear();
    }

    pub fn for_path(&self, path: &str) -> Vec<Arc<Known>> {
        self.entries.iter().filter(|(p, _, _)| p == path).map(|(_, _, k)| k.clone()).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Building the word list and checking crossings.
    Checking,
    /// Looking for one full fill.
    Searching,
    /// Checking words one by one.
    Settling,
    /// Nothing left to check.
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct FocusSnap {
    pub key: String,
    /// Words in display order with verdicts (the first `FOCUS_LIMIT`).
    pub words: Vec<WordVerdict>,
    /// Words left after the crossing check.
    pub total: usize,
    /// Full fills for the viable words (`WordVerdict::fill` indexes this list), at most `FOCUS_FILLS`.
    /// Each is only the letters for the saved grid's empty squares, in reading order, to keep polls small.
    pub fills: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub phase: Phase,
    pub fillable: Fillable,
    pub check: Option<CheckReport>,
    pub hotspots: Option<Heat>,
    pub sample_fill: Option<Vec<String>>,
    pub slots: Vec<SlotSummary>,
    pub focus: Option<FocusSnap>,
    /// Empty squares with only one possible letter, and answers with one word left (crossing check only).
    pub forced: Option<Forced>,
    /// Seconds the full search took (or has taken so far).
    pub search_seconds: Option<f32>,
    /// Words checked so far.
    pub words_checked: usize,
    pub error: Option<String>,
}

impl Snapshot {
    /// The snapshot before any work.
    pub fn start() -> Snapshot {
        Snapshot {
            phase: Phase::Checking,
            fillable: Fillable::Unknown,
            check: None,
            hotspots: None,
            sample_fill: None,
            slots: vec![],
            focus: None,
            forced: None,
            search_seconds: None,
            words_checked: 0,
            error: None,
        }
    }

    /// Copy what the analysis knows into this snapshot. `focus` is the selected answer's key.
    pub fn update(&mut self, a: &Analysis, phase: Phase, words_checked: usize, focus: Option<String>) {
        let focus = focus.and_then(|k| {
            let i = a.slot_index(&k)?;
            let (mut words, fills) = a.words_with_fills(i, FOCUS_LIMIT);
            for w in &mut words {
                w.fill = w.fill.filter(|&f| f < FOCUS_FILLS);
            }
            let empty: Vec<(usize, usize)> = a.puzzle.grid.iter().enumerate().flat_map(|(y, r)| r.chars().enumerate().filter(|&(_, c)| c == '.').map(move |(x, _)| (x, y))).collect();
            let fills = fills.iter().take(FOCUS_FILLS).map(|g| empty.iter().map(|&(x, y)| g[y].chars().nth(x).unwrap_or('.')).collect()).collect();
            Some(FocusSnap { key: k, words, total: a.option_count(i), fills })
        });
        self.phase = phase;
        self.fillable = a.fillable;
        self.check = Some(a.check.clone());
        self.hotspots = a.hotspots.clone();
        self.sample_fill = a.sample_fill.clone();
        self.slots = a.summaries();
        // Proven-dead words drop out, so more letters can be forced as the search goes.
        self.forced = Some(a.forced());
        self.focus = focus;
        self.words_checked = words_checked;
    }
}

/// What runs a check: it says what is selected, when to stop, and takes the results.
pub trait Host {
    /// The selected answer's key, if any. It may change while the check runs.
    fn focus(&self) -> Option<String>;
    /// Stop now (a new revision, or the time is up).
    fn aborted(&self) -> bool;
    /// Stop the word-by-word phase (aborted, or nobody is looking any more).
    fn done_settling(&self) -> bool {
        self.aborted()
    }
    /// Split the selected answer's words among workers: `(index, count)`. This worker checks only
    /// its share of them, and a worker with `index > 0` checks nothing else. None: one worker.
    fn shard(&self) -> Option<(usize, usize)> {
        None
    }
    /// How long a word that ran out of time may be tried again now. None: no retries now.
    fn retry_limit(&self) -> Option<Duration> {
        Some(RETRY_PER_WORD)
    }
    /// The latest results. `focus` is the answer they were made for.
    fn publish(&self, a: &Analysis, phase: Phase, words_checked: usize, focus: Option<String>);
    /// Just the forced squares, before any search.
    fn publish_forced(&self, f: Forced);
    /// Answer at once with what the crossing check (and any shared fill) knows, and search nothing.
    /// For a browser's main worker while helper workers check the selected answer's words, so it
    /// stays free for quick calls (the word list) and each reply comes at once, as Ingrid's does.
    fn light(&self) -> bool {
        false
    }
    /// How long to try one word in the probe.
    fn probe_per_word(&self) -> Duration {
        PROBE_PER_WORD
    }
    /// How long earlier calls of this check already looked for a fill (a host that works in short
    /// calls, like the browser's). The probe and full search share `search_limit` with them.
    fn search_spent(&self) -> Duration {
        Duration::ZERO
    }
    /// The longest time to look for a fill, in all. When it is used up, the state is unknown.
    fn search_limit(&self) -> Duration {
        SEARCH_TIMEOUT
    }
    /// The full search starts now.
    fn search_started(&self);
    /// The full search took this long.
    fn search_seconds(&self, s: f32);
    /// Nothing left to check. Return true to keep answering focus changes (wait a moment first),
    /// false to stop.
    fn wait(&self) -> bool;
}

/// Check the grid: a probe word by word, a full search if needed, then settle words one by one,
/// the selected answer first. `prev` is what earlier checks of this puzzle learned.
pub fn work(a: &mut Analysis, prev: &[Arc<Known>], h: &dyn Host) {
    h.publish_forced(a.forced());
    if !a.check.consistent {
        h.publish(a, Phase::Done, 0, h.focus());
        return idle(a, 0, h);
    }
    // Start from earlier checks on this puzzle. A matching fill makes the grid fillable at once,
    // so the probe and the full search below are skipped.
    if !a.reused {
        a.reuse(prev);
    }
    if h.light() {
        // The helpers looked for the whole time allowed: the state is unknown.
        if a.fillable == Fillable::Unknown && h.search_spent() >= h.search_limit() {
            a.fillable = Fillable::Timeout;
        }
        let phase = match a.fillable {
            Fillable::Yes => Phase::Settling,
            Fillable::Unknown => Phase::Searching,
            _ => Phase::Done,
        };
        h.publish(a, phase, 0, h.focus());
        return;
    }
    h.publish(a, Phase::Searching, 0, h.focus());
    h.search_started();
    let t = Instant::now();
    let shard = h.shard().filter(|&(_, n)| n > 1);
    // A helper worker (index > 0) checks only its share of the selected answer.
    let helper = shard.is_some_and(|(i, _)| i > 0);
    let order = if helper { vec![] } else { a.settle_order() };
    let next_in = |a: &Analysis, slot: usize, limit: usize| match shard {
        Some(s) => a.next_unsettled_in(slot, limit, s),
        None => a.next_unsettled(slot, limit),
    };
    let mut checked = 0;
    let mut last_publish = Instant::now();

    // Probe, like Ingrid: try the selected answer's words one by one (or the tightest answer's
    // when none is selected). Each check is a full search with that word fixed, so the word list
    // fills in while we look. The first word that works proves the grid fillable. If every word
    // in the answer is dead, no fill exists.
    let mut proven_dead = false;
    let spent = h.search_spent();
    let limit = h.search_limit();
    let left = || limit.saturating_sub(spent + t.elapsed());
    while a.fillable == Fillable::Unknown && t.elapsed() < PROBE_TIME && !left().is_zero() {
        if h.aborted() {
            return;
        }
        let focus = h.focus();
        let Some(slot) = focus.as_deref().and_then(|k| a.slot_index(k)).filter(|&s| a.option_count(s) > 1).or(order.first().copied()) else { break };
        let Some(word) = next_in(a, slot, usize::MAX) else {
            proven_dead = a.all_dead(slot);
            break;
        };
        if a.settle(slot, word, h.probe_per_word().min(left()).max(Duration::from_millis(100))).is_none() {
            return;
        }
        checked += 1;
        if last_publish.elapsed() > Duration::from_millis(150) {
            h.publish(a, Phase::Searching, checked, h.focus());
            last_publish = Instant::now();
        }
    }

    // The time to look for a fill is used up (over several short calls): the state is unknown.
    if a.fillable == Fillable::Unknown && !proven_dead && left().is_zero() {
        a.fillable = Fillable::Timeout;
    }
    // A helper leaves the full search to the first worker.
    if helper && a.fillable == Fillable::Unknown && !proven_dead {
        h.publish(a, Phase::Done, checked, h.focus());
        return;
    }
    // No answer yet: fall back to one search over the whole grid. It also finds the hotspots.
    if a.fillable == Fillable::Unknown || proven_dead {
        h.publish(a, Phase::Searching, checked, h.focus());
        // When every word is already proven dead, the search only draws the hotspots: keep it short.
        let left = if proven_dead { Duration::from_secs(5) } else { left().max(Duration::from_secs(5)) };
        if matches!(a.search(left), SearchOutcome::Aborted) {
            return;
        }
        if proven_dead {
            a.fillable = Fillable::No;
        }
    }
    h.search_seconds(t.elapsed().as_secs_f32());
    if a.fillable != Fillable::Yes {
        h.publish(a, Phase::Done, checked, h.focus());
        return idle(a, checked, h);
    }

    // Settle words: the selected answer first, then the others, tightest first.
    let mut last_focus: Option<String> = None;
    // The phase of the last results handed to the host.
    let mut shown = Phase::Settling;
    h.publish(a, Phase::Settling, checked, h.focus());
    loop {
        if h.done_settling() {
            return;
        }
        let focus_key = h.focus();
        let focus = focus_key.as_deref().and_then(|k| a.slot_index(k));
        // The selected answer, top of the list first: each of the first `FOCUS_FIRST` words once,
        // then those that ran out of time once more with a longer limit, so few stay unsure. Then
        // the same for the rest of its first `FOCUS_LIMIT` words. Then the other answers.
        let retry = |a: &mut Analysis, f: usize, limit: usize| match shard {
            Some(s) => a.next_timeout_in(f, limit, s),
            None => a.next_timeout(f, limit),
        };
        let retry_limit = h.retry_limit();
        let target = focus.and_then(|f| {
            [FOCUS_FIRST, FOCUS_LIMIT].into_iter().find_map(|limit| {
                next_in(a, f, limit).map(|w| (f, w, PER_WORD)).or_else(|| {
                    let t = retry_limit?;
                    retry(a, f, limit).map(|w| (f, w, t))
                })
            })
        })
            .or_else(|| order.iter().find_map(|&s| a.next_unsettled(s, OTHER_LIMIT).map(|w| (s, w, PER_WORD))));
        let focus_changed = focus_key != last_focus;
        last_focus = focus_key.clone();
        match target {
            Some((slot, word, limit)) => {
                if a.settle(slot, word, limit).is_none() {
                    return;
                }
                checked += 1;
                if focus_changed || last_publish.elapsed() > Duration::from_millis(150) {
                    h.publish(a, Phase::Settling, checked, h.focus());
                    shown = Phase::Settling;
                    last_publish = Instant::now();
                }
            }
            None => {
                if focus_changed || shown != Phase::Done {
                    h.publish(a, Phase::Done, checked, focus_key);
                    shown = Phase::Done;
                }
                if !h.wait() {
                    return;
                }
            }
        }
    }
}

/// Nothing to settle, but keep answering focus changes (the word list per answer) until stopped.
fn idle(a: &Analysis, checked: usize, h: &dyn Host) {
    let mut last_focus: Option<String> = None;
    loop {
        if h.done_settling() {
            return;
        }
        let f = h.focus();
        if f != last_focus {
            h.publish(a, Phase::Done, checked, f.clone());
            last_focus = f;
        }
        if !h.wait() {
            return;
        }
    }
}

/// A host that works for a time budget, on the calling thread. For a browser's Web Worker.
struct Budget {
    budget: Duration,
    focus: Option<String>,
    shard: Option<(usize, usize)>,
    until: Instant,
    started: Instant,
    snap: std::cell::RefCell<Snapshot>,
    spent: Duration,
    limit: Duration,
    light: bool,
    search_started: std::cell::Cell<Option<Instant>>,
}

impl Host for Budget {
    fn focus(&self) -> Option<String> {
        self.focus.clone()
    }
    fn aborted(&self) -> bool {
        Instant::now() >= self.until
    }
    fn shard(&self) -> Option<(usize, usize)> {
        self.shard
    }
    /// Retries only in a long (idle) call, never right after an edit, and each ends at most
    /// `RETRY_MARGIN` after the budget, so the caller's cutoff (budget + 2.5 s in the browser)
    /// never stops the worker.
    fn retry_limit(&self) -> Option<Duration> {
        if self.budget < RETRY_MIN_BUDGET {
            return None;
        }
        let t = self.until.saturating_duration_since(Instant::now()) + RETRY_MARGIN;
        (t > PER_WORD).then(|| t.min(RETRY_PER_WORD))
    }
    fn publish(&self, a: &Analysis, phase: Phase, words_checked: usize, focus: Option<String>) {
        self.snap.borrow_mut().update(a, phase, words_checked, focus);
    }
    fn publish_forced(&self, f: Forced) {
        self.snap.borrow_mut().forced = Some(f);
    }
    fn light(&self) -> bool {
        self.light
    }
    /// With nothing selected, a probe word ends with the call, so a new selection gets an answer
    /// soon (a browser worker can't stop a call midway). The selected answer's words get full time.
    fn probe_per_word(&self) -> Duration {
        if self.focus.is_some() {
            return PROBE_PER_WORD;
        }
        self.until.saturating_duration_since(Instant::now()).max(Duration::from_millis(300)).min(PROBE_PER_WORD)
    }
    fn search_spent(&self) -> Duration {
        self.spent
    }
    fn search_limit(&self) -> Duration {
        self.limit
    }
    fn search_started(&self) {
        self.search_started.set(Some(Instant::now()));
    }
    fn search_seconds(&self, s: f32) {
        self.snap.borrow_mut().search_seconds = Some(s);
    }
    fn wait(&self) -> bool {
        false
    }
}

/// Run a live check for at most `budget` (plus the one step running when it ends), then return
/// the snapshot and the seconds it took. Call it again with the same `memory` and `path` to go on:
/// the next call starts from what this one learned. `id` tells checks apart in the memory; use a
/// new one per call. While the grid stays the same, the memory keeps the analysis itself, so the
/// next call skips building it. `shard`: see `Host::shard`.
pub fn run_for(p: &Puzzle, o: &Options, path: &str, focus: Option<String>, budget: Duration, memory: &mut Memory, id: u64) -> (Snapshot, f32) {
    run_shard(p, o, path, focus, budget, memory, id, None, &[], Duration::ZERO, SEARCH_TIMEOUT, false)
}

/// `run_for` for one of several workers that split the selected answer's words (`Host::shard`).
/// `fills`: full fills the other workers found; they prove the grid fillable here too, so this
/// worker does not keep probing on its own. `spent`: how long earlier calls already looked for a
/// fill for this grid; `limit`: the most time to look in all (then the state is unknown).
/// `light`: see `Host::light`.
#[allow(clippy::too_many_arguments)]
pub fn run_shard(p: &Puzzle, o: &Options, path: &str, focus: Option<String>, budget: Duration, memory: &mut Memory, id: u64, shard: Option<(usize, usize)>, fills: &[String], spent: Duration, limit: Duration, light: bool) -> (Snapshot, f32) {
    let started = Instant::now();
    let key = Analysis::grid_key(p, o);
    let mut a = match memory.kept.take() {
        // Same grid: go on with the same analysis. Its puzzle may differ in clues only; keep the new one.
        Some((kp, kk, mut a)) if kp == path && kk == key => {
            a.puzzle = p.clone();
            a
        }
        _ => match Analysis::new(p, o, Arc::new(AtomicBool::new(false))) {
            Ok(a) => a,
            Err(e) => {
                let mut s = Snapshot::start();
                s.phase = Phase::Error;
                s.error = Some(format!("{e:#}"));
                return (s, started.elapsed().as_secs_f32());
            }
        },
    };
    let h = Budget { budget, focus: focus.clone(), shard, until: started + budget, started, snap: std::cell::RefCell::new(Snapshot::start()), spent, limit, light, search_started: Default::default() };
    a.adopt_fills(fills);
    let prev = memory.for_path(path);
    work(&mut a, &prev, &h);
    memory.remember(path, id, a.known());
    memory.kept = Some((path.to_string(), key, a));
    let mut s = h.snap.into_inner();
    if s.phase == Phase::Searching {
        s.search_seconds = h.search_started.get().map(|t| t.elapsed().as_secs_f32());
    }
    // Don't hand back another answer's words.
    if s.focus.as_ref().is_some_and(|f| Some(&f.key) != focus.as_ref()) {
        s.focus = None;
    }
    (s, h.started.elapsed().as_secs_f32())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_stays_bounded() {
        let mut m = Memory::default();
        for i in 0..40 {
            m.remember("a.json", i, Known::for_test(i as usize, 10));
        }
        assert_eq!(m.for_path("a.json").len(), MEMORY_PER_PATH);
        for i in 0..40 {
            m.remember(&format!("p{i}.json"), 100 + i, Known::for_test(i as usize, 10));
        }
        assert_eq!(m.entries.len(), MEMORY_TOTAL);
        // Big entries: the byte budget drops the oldest, but the newest always stays.
        let big = MEMORY_BYTES / Known::for_test(0, 1000).approx_bytes() + 2;
        for i in 0..big as u64 {
            m.remember(&format!("big{i}.json"), 1000 + i, Known::for_test(i as usize, 1000));
        }
        let total: usize = m.entries.iter().map(|(_, _, k)| k.approx_bytes()).sum();
        assert!(total <= MEMORY_BYTES, "{total} bytes");
        assert!(!m.entries.is_empty());
        m.remember("huge.json", 9999, Known::for_test(0, 200_000));
        assert_eq!(m.entries.len(), 1, "a single entry over budget still stays, alone");
        // New word choices: no kept fill may come back.
        m.forget_analysis();
        assert!(m.for_path("huge.json").is_empty());
    }
}
