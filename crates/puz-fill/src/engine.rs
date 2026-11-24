//! Thin layer over ingrid_core: word lists, grid checks, fill, and hotspot reports.

use crate::puzzle::{Dir, Puzzle};
use anyhow::{bail, Result};
use ingrid_core::arc_consistency::{establish_arc_consistency_for_static_grid, ArcConsistencyFailure, EliminationSet};
use ingrid_core::backtracking_search::{find_fill_variant_with_diagnostics, FillDiagnostics, FillFailure};
use ingrid_core::grid_config::{generate_grid_config, render_grid, Choice, Direction, GridConfig, OwnedGridConfig, SlotSpec};
use ingrid_core::word_list::{WordList, WordListSourceConfig, WordListSourceConfigProvider};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The default word list (Spread the Wordlist), built in only with the `bundled-wordlist` feature:
/// the build names the file in `PUZ_FILL_WORDLIST`. Otherwise the host loads it at run time with
/// `set_default_wordlist` (`puz` reads its downloaded copy; a browser downloads it from the source).
#[cfg(feature = "bundled-wordlist")]
const STWL: Option<&str> = Some(include_str!(env!("PUZ_FILL_WORDLIST")));
#[cfg(not(feature = "bundled-wordlist"))]
const STWL: Option<&str> = None;
static LOADED: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// The default word list's text, built in or loaded; None before a load.
fn stwl() -> Option<&'static str> {
    STWL.or_else(|| LOADED.get().copied())
}

/// Load the default word list ("word;score" per line) when it isn't built in. Only the first load counts.
pub fn set_default_wordlist(text: String) {
    if STWL.is_none() {
        let _ = LOADED.set(Box::leak(text.into_boxed_str()));
    }
}

/// Tests: load the list named by `PUZ_FILL_WORDLIST` at run time, when it isn't built in. Returns
/// whether a list is there. Tests that need one skip without it (`need_list!`).
#[cfg(test)]
pub(crate) fn test_list() -> bool {
    if let Some(text) = std::env::var_os("PUZ_FILL_WORDLIST").and_then(|f| std::fs::read_to_string(f).ok()) {
        set_default_wordlist(text);
    }
    has_default_wordlist()
}

/// Whether the default word list is available (built in or loaded).
pub fn has_default_wordlist() -> bool {
    stwl().is_some()
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub min_score: u16,
    pub max_shared_substring: Option<usize>,
    /// Extra word lists (`word;score` per line). Loaded after Spread the Wordlist.
    pub wordlists: Vec<PathBuf>,
    pub no_default_list: bool,
    /// New fill uses only words of at least this quality (`words::judge`), and tries better words
    /// first. None: only `min_score` counts.
    pub quality_floor: Option<i32>,
    /// Answers (slot keys) that may use words under `quality_floor`, down to `min_score`.
    pub relaxed: Option<Arc<HashSet<String>>>,
    /// Words (letters, upper case) to leave out of new fill, on top of the user's banned words.
    pub avoid: Option<Arc<HashSet<String>>>,
}

/// Which word list a pooled `WordList` was built for.
#[derive(Clone, PartialEq)]
struct PoolKey {
    no_default_list: bool,
    wordlists: Vec<PathBuf>,
    max_shared_substring: Option<usize>,
    max_side: usize,
    /// The user's liked words, which the list includes.
    liked: Vec<String>,
}

/// Loaded word lists, kept so the next check doesn't parse the lists again (about 1s each time).
/// Each one is ~100MB, so keep only a few.
static POOL: Mutex<Vec<(PoolKey, WordList)>> = Mutex::new(Vec::new());
const POOL_MAX: usize = 2;
/// Word lists in use right now (checked out of the pool or newly built).
static IN_USE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A grid config whose word list goes back to the pool when it is dropped.
pub struct Cfg {
    inner: Option<OwnedGridConfig>,
    key: PoolKey,
}

impl std::ops::Deref for Cfg {
    type Target = OwnedGridConfig;
    fn deref(&self) -> &OwnedGridConfig {
        self.inner.as_ref().expect("config taken")
    }
}
impl std::ops::DerefMut for Cfg {
    fn deref_mut(&mut self) -> &mut OwnedGridConfig {
        self.inner.as_mut().expect("config taken")
    }
}
impl Drop for Cfg {
    fn drop(&mut self) {
        if let Some(c) = self.inner.take() {
            IN_USE.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            let mut pool = POOL.lock().unwrap();
            // Drop the oldest list first, so a list for old settings (e.g. before a liked word
            // was added) doesn't hold a place forever.
            if pool.len() >= POOL_MAX {
                pool.remove(0);
            }
            pool.push((self.key.clone(), c.word_list));
        }
    }
}

/// The default word list's text ("word;score" per line); empty before it is loaded.
pub fn default_wordlist() -> &'static str {
    stwl().unwrap_or("")
}

/// A word's score in the bundled word list (letters only, any case), or None when it isn't listed.
/// Builds a lookup table on first use.
pub fn list_score(word: &str) -> Option<u32> {
    static SCORES: std::sync::OnceLock<HashMap<String, u32>> = std::sync::OnceLock::new();
    let list = stwl()?;
    let scores = SCORES.get_or_init(|| {
        list.lines()
            .filter_map(|l| {
                let (w, s) = l.split_once(';')?;
                Some((crate::words::normalize_answer(w), s.trim().parse().ok()?))
            })
            .collect()
    });
    scores.get(&crate::words::normalize_answer(word)).copied()
}

fn load_word_list(o: &Options, max_side: usize, liked: &[String]) -> Result<WordList> {
    let mut sources = vec![];
    if !liked.is_empty() {
        sources.push(WordListSourceConfig {
            id: "liked".into(),
            enabled: true,
            provider: WordListSourceConfigProvider::Memory { words: liked.iter().map(|w| (w.to_lowercase(), crate::words::LIKED_SCORE)).collect() },
            normalization: None,
        });
    }
    if !o.no_default_list {
        let Some(contents) = stwl() else { bail!("The word list is not loaded yet.") };
        sources.push(WordListSourceConfig {
            id: "stwl".into(),
            enabled: true,
            provider: WordListSourceConfigProvider::FileContents { contents },
            normalization: None,
        });
    }
    for (i, path) in o.wordlists.iter().enumerate() {
        sources.push(WordListSourceConfig {
            id: format!("extra{i}"),
            enabled: true,
            provider: WordListSourceConfigProvider::File { path: path.clone().into() },
            normalization: None,
        });
    }
    if sources.is_empty() {
        bail!("no word lists enabled");
    }
    let wl = WordList::new(sources, None, Some(max_side), o.max_shared_substring);
    for (id, errs) in wl.get_source_errors() {
        if let Some(e) = errs.first() {
            bail!("word list {id}: {e}");
        }
    }
    Ok(wl)
}

/// Load the default word list now, so the first check in the UI is fast.
pub fn warm_up(o: &Options, max_side: usize) {
    let key = pool_key(o, max_side);
    if let Ok(wl) = load_word_list(o, max_side, &key.liked) {
        let mut pool = POOL.lock().unwrap();
        if pool.len() < POOL_MAX {
            pool.push((key, wl));
        }
    }
}

fn pool_key(o: &Options, max_side: usize) -> PoolKey {
    PoolKey {
        no_default_list: o.no_default_list,
        wordlists: o.wordlists.clone(),
        max_shared_substring: o.max_shared_substring,
        // Bucket sizes so a 15x15 and a 13x13 share one list.
        max_side: max_side.max(15),
        liked: crate::words::choices().liked.clone(),
    }
}

pub fn build_config(p: &Puzzle, o: &Options) -> Result<Cfg> {
    let key = pool_key(o, p.width().max(p.height()));
    let take = || {
        let mut pool = POOL.lock().unwrap();
        pool.iter().position(|(k, _)| *k == key).map(|i| pool.swap_remove(i).1)
    };
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut pooled = take();
    // A stopped job may be about to hand its list back; waiting a moment beats loading a new one (~1s).
    // WebAssembly has one thread and can't sleep, so nothing else can hand a list back there.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let deadline = std::time::Instant::now() + Duration::from_millis(600);
        while pooled.is_none() && IN_USE.load(std::sync::atomic::Ordering::Relaxed) > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            pooled = take();
        }
    }
    let wl = match pooled {
        Some(wl) => wl,
        None => load_word_list(o, key.max_side, &key.liked)?,
    };
    IN_USE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (specs, fill) = engine_grid(p);
    let mut inner = generate_grid_config(wl, &specs, &fill, p.width(), p.height(), o.min_score);
    apply_choices(p, o, &mut inner);
    Ok(Cfg { inner: Some(inner), key })
}

/// Take banned and avoided words out of the answers new fill writes. With a quality floor, also
/// drop weak words (except in relaxed answers) and put better words first. Answers that already
/// have every letter keep their word.
fn apply_choices(p: &Puzzle, o: &Options, cfg: &mut OwnedGridConfig) {
    let judge = crate::words::Judge::new();
    let has_bans = !crate::words::choices().banned.is_empty() || o.avoid.as_ref().is_some_and(|a| !a.is_empty());
    if !has_bans && o.quality_floor.is_none() {
        return;
    }
    let open: HashSet<String> = p.slots().into_iter().filter(|s| s.pattern.contains('.')).map(|s| s.key).collect();
    let mut memo: HashMap<(usize, usize), i32> = HashMap::new();
    for i in 0..cfg.slot_configs.len() {
        let key = slot_key(cfg, i);
        if !open.contains(&key) {
            continue;
        }
        let len = cfg.slot_configs[i].length;
        let floor = match o.quality_floor {
            Some(f) if !o.relaxed.as_ref().is_some_and(|r| r.contains(&key)) => Some(f),
            _ => None,
        };
        let words = &cfg.word_list.words[len];
        let mut opts = std::mem::take(&mut cfg.slot_options[i]);
        opts.retain(|&w| {
            let word = &words[w];
            if judge.banned(&word.normalized_string) {
                return false;
            }
            if o.avoid.as_ref().is_some_and(|a| a.contains(&word.normalized_string.to_ascii_uppercase())) {
                return false;
            }
            match floor {
                Some(f) => *memo.entry((len, w)).or_insert_with(|| judge.score(&word.normalized_string, word.score)) >= f,
                None => true,
            }
        });
        if let Some(f) = o.quality_floor {
            // Better words first; the engine's own order (easiest to fill around) within a tier.
            opts.sort_by_cached_key(|&w| {
                let q = *memo.entry((len, w)).or_insert_with(|| judge.score(&words[w].normalized_string, words[w].score));
                if q >= f + 5 { 0 } else if q >= f { 1 } else { 2 }
            });
        }
        cfg.slot_options[i] = opts;
    }
}

/// The answers and letters the engine works on. The answers come from `Puzzle::slots()`, so walls
/// split them the same way everywhere. Answers through a rebus square are left out: the engine has
/// one letter per square, so it can't fill or check them. Their other letters still sit in the grid,
/// so they still limit the answers that cross them.
fn engine_grid(p: &Puzzle) -> (Vec<SlotSpec>, Vec<Option<String>>) {
    let specs = p
        .slots()
        .into_iter()
        .filter(|s| !s.rebus)
        .map(|s| SlotSpec { start_cell: (s.x, s.y), direction: if s.dir == Dir::Across { Direction::Across } else { Direction::Down }, length: s.len })
        .collect();
    let fill = (0..p.height())
        .flat_map(|y| (0..p.width()).map(move |x| (x, y)))
        .map(|(x, y)| match p.cell(x, y) {
            '#' | '.' => None,
            c => Some(c.to_lowercase().to_string()),
        })
        .collect();
    (specs, fill)
}

/// An answer through a rebus square. The engine leaves it out: it doesn't fill it or check it
/// against the word list.
#[derive(Debug, Clone, Serialize)]
pub struct RebusAnswer {
    pub key: String,
    pub label: String,
    /// Letters with the rebus in brackets, e.g. `ST[STAR]T`. '.' is an empty square.
    pub pattern: String,
    /// False if some square is still empty. A fill won't check this answer, so type it by hand.
    pub complete: bool,
}

/// Every answer through a rebus square.
pub fn rebus_answers(p: &Puzzle) -> Vec<RebusAnswer> {
    p.slots()
        .into_iter()
        .filter(|s| s.rebus)
        .map(|s| RebusAnswer { label: crate::edit::label(&s), complete: !s.pattern.contains('.'), key: s.key, pattern: s.pattern })
        .collect()
}

/// Plain-language lines about rebus answers, for the CLI.
pub fn rebus_notes(answers: &[RebusAnswer]) -> Vec<String> {
    answers
        .iter()
        .map(|r| {
            if r.complete {
                format!("{} {} has a rebus; not checked against the word list.", r.label, r.pattern)
            } else {
                format!("{} {} has a rebus and empty squares. The engine doesn't check it, so a fill may put any letters there. Type it in by hand first.", r.label, r.pattern)
            }
        })
        .collect()
}

pub fn slot_key(cfg: &OwnedGridConfig, slot_id: usize) -> String {
    let s = &cfg.slot_configs[slot_id];
    let d = match s.direction {
        Direction::Across => "across",
        Direction::Down => "down",
    };
    format!("{},{},{}", s.start_cell.0, s.start_cell.1, d)
}

/// crossing id -> (x, y)
fn crossing_cells(cfg: &OwnedGridConfig) -> HashMap<usize, (usize, usize)> {
    let mut m = HashMap::new();
    for s in &cfg.slot_configs {
        let coords = s.cell_coords();
        for (i, c) in s.crossings.iter().enumerate() {
            if let Some(c) = c {
                m.insert(c.crossing_id, coords[i]);
            }
        }
    }
    m
}

/// Heat for slots that have no possible words at all (pattern already a dead end).
/// Used when the engine fails before it searches, so it has no crossing weights.
fn heat_from_dead_slots(p: &Puzzle, cfg: &OwnedGridConfig) -> Option<Heat> {
    let mut grid = vec![vec![0.0f32; p.width()]; p.height()];
    let mut any = false;
    for s in &cfg.slot_configs {
        if cfg.slot_options[s.id].is_empty() {
            any = true;
            for (x, y) in s.cell_coords() {
                grid[y][x] = 1.0;
            }
        }
    }
    any.then(|| heat_from_grid(p, cfg, grid))
}

/// Turn per-crossing values into a normalized (0..1) heat grid plus a ranked list of slots.
fn heat_from_crossings(p: &Puzzle, cfg: &OwnedGridConfig, values: &HashMap<usize, f32>) -> Heat {
    let cells = crossing_cells(cfg);
    let mut grid = vec![vec![0.0f32; p.width()]; p.height()];
    for (cid, v) in values {
        if let Some(&(x, y)) = cells.get(cid) {
            grid[y][x] += *v;
        }
    }
    heat_from_grid(p, cfg, grid)
}

fn heat_from_grid(p: &Puzzle, cfg: &OwnedGridConfig, mut grid: Vec<Vec<f32>>) -> Heat {
    let max = grid.iter().flatten().cloned().fold(0.0f32, f32::max);
    if max > 0.0 {
        for v in grid.iter_mut().flatten() {
            *v /= max;
        }
    }
    let numbers: HashMap<String, usize> = p.slots().into_iter().map(|s| (s.key, s.number)).collect();
    let mut slots: Vec<SlotHeat> = cfg
        .slot_configs
        .iter()
        .map(|s| {
            let heat: f32 = s.cell_coords().iter().map(|&(x, y)| grid[y][x]).sum();
            let key = slot_key(cfg, s.id);
            let dir = if matches!(s.direction, Direction::Across) { "A" } else { "D" };
            SlotHeat { label: format!("{}{}", numbers.get(&key).copied().unwrap_or(0), dir), key, heat }
        })
        .filter(|s| s.heat > 0.0)
        .collect();
    slots.sort_by(|a, b| b.heat.total_cmp(&a.heat));
    let mut cells_ranked: Vec<CellHeat> = grid
        .iter()
        .enumerate()
        .flat_map(|(y, row)| row.iter().enumerate().map(move |(x, &heat)| CellHeat { x, y, heat }))
        .filter(|c| c.heat > 0.0)
        .collect();
    cells_ranked.sort_by(|a, b| b.heat.total_cmp(&a.heat));
    Heat { grid, slots, cells: cells_ranked }
}

#[derive(Debug, Clone, Serialize)]
pub struct Heat {
    /// Normalized 0..1 per square, rows then columns.
    pub grid: Vec<Vec<f32>>,
    pub slots: Vec<SlotHeat>,
    pub cells: Vec<CellHeat>,
}
#[derive(Debug, Clone, Serialize)]
pub struct SlotHeat {
    pub key: String,
    pub label: String,
    pub heat: f32,
}
#[derive(Debug, Clone, Serialize)]
pub struct CellHeat {
    pub x: usize,
    pub y: usize,
    pub heat: f32,
}

// ---------------------------------------------------------------- check

#[derive(Debug, Clone, Serialize)]
pub struct SlotReport {
    pub key: String,
    pub label: String,
    pub pattern: String,
    /// Words that fit the pattern and score cutoff, before looking at crossings.
    pub pattern_matches: usize,
    /// Words still possible once every crossing is taken into account.
    pub options: usize,
    pub complete: bool,
    /// For complete entries: is the word in an enabled word list (at or above min score)?
    pub in_word_list: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    /// False if some slot has no possible words once crossings are considered.
    pub consistent: bool,
    pub slots: Vec<SlotReport>,
    /// When inconsistent: which squares caused the dead end.
    pub hotspots: Option<Heat>,
    /// Answers through a rebus square. They are not in `slots`: the engine doesn't check them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rebus: Vec<RebusAnswer>,
}

pub fn check(p: &Puzzle, o: &Options) -> Result<CheckReport> {
    let cfg = build_config(p, o)?;
    Ok(check_with(p, o, &cfg).0)
}

/// The check report for a built config, plus the words arc consistency ruled out per slot.
pub fn check_with(p: &Puzzle, o: &Options, cfg: &OwnedGridConfig) -> (CheckReport, Vec<EliminationSet>) {
    let r = cfg.to_config_ref();
    let mut elim = EliminationSet::build_all(&cfg.slot_configs, &cfg.word_list);
    let ac = establish_arc_consistency_for_static_grid(&r, &mut elim);
    let info: HashMap<String, (usize, String)> = p.slots().into_iter().map(|s| (s.key.clone(), (s.number, s.pattern))).collect();
    let consistent = ac.is_ok();
    let slots = cfg
        .slot_configs
        .iter()
        .map(|s| {
            let key = slot_key(cfg, s.id);
            let (number, pattern) = info.get(&key).cloned().unwrap_or((0, String::new()));
            let opts = &cfg.slot_options[s.id];
            let complete = !pattern.contains('.');
            let in_word_list = complete.then(|| {
                opts.first().is_some_and(|&w| {
                    let word = &cfg.word_list.words[s.length][w];
                    !word.hidden && word.score >= o.min_score
                })
            });
            let options = if consistent { opts.iter().filter(|&&w| !elim[s.id].contains(w)).count() } else { opts.len() };
            let dir = if matches!(s.direction, Direction::Across) { "A" } else { "D" };
            SlotReport { label: format!("{number}{dir}"), key, pattern, pattern_matches: opts.len(), options, complete, in_word_list }
        })
        .collect();
    let hotspots = match ac {
        Err(ArcConsistencyFailure { weight_updates }) => {
            let h = heat_from_crossings(p, cfg, &weight_updates);
            Some(if h.cells.is_empty() { heat_from_dead_slots(p, cfg).unwrap_or(h) } else { h })
        }
        Ok(()) => None,
    };
    (CheckReport { consistent, slots, hotspots, rebus: rebus_answers(p) }, elim)
}

// ---------------------------------------------------------------- candidates

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub word: String,
    pub score: u16,
}

pub fn candidates(p: &Puzzle, o: &Options, slot: &str, limit: usize) -> Result<(String, Vec<Candidate>, usize)> {
    let info = p.find_slot(slot)?;
    let cfg = build_config(p, o)?;
    let r = cfg.to_config_ref();
    if info.rebus {
        bail!("{} has a rebus square; the word list has no words for it", crate::edit::label(&info));
    }
    let Some(sc) = cfg.slot_configs.iter().find(|s| slot_key(&cfg, s.id) == info.key) else {
        bail!("slot {} not found in engine config", info.key);
    };
    let mut elim = EliminationSet::build_all(&cfg.slot_configs, &cfg.word_list);
    let consistent = establish_arc_consistency_for_static_grid(&r, &mut elim).is_ok();
    let mut out: Vec<Candidate> = cfg.slot_options[sc.id]
        .iter()
        .filter(|&&w| !consistent || !elim[sc.id].contains(w))
        .map(|&w| {
            let word = &cfg.word_list.words[sc.length][w];
            Candidate { word: word.canonical_string.clone(), score: word.score }
        })
        .collect();
    out.sort_by(|a, b| b.score.cmp(&a.score).then(a.word.cmp(&b.word)));
    let total = out.len();
    out.truncate(limit);
    let label = format!("{}{}", info.number, if info.dir == Dir::Across { "A" } else { "D" });
    Ok((label, out, total))
}

// ---------------------------------------------------------------- fill

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FillStatus {
    Filled,
    /// Proven impossible with the current word lists and settings.
    Impossible,
    /// Ran out of time before finding a fill or proving it impossible.
    Timeout,
}

#[derive(Debug, Clone, Serialize)]
pub struct FillReport {
    pub status: FillStatus,
    pub grid: Option<Vec<String>>,
    pub seconds: f32,
    pub backtracks: Option<usize>,
    /// Where the search struggled. Present when the fill did not succeed.
    pub hotspots: Option<Heat>,
    /// Answers through a rebus square. The fill leaves them as they are.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rebus: Vec<RebusAnswer>,
}

/// Where a search struggled, as heat. Falls back to the engine's weights, then to slots with no words.
pub fn search_heat(p: &Puzzle, cfg: &OwnedGridConfig, diag: &FillDiagnostics) -> Heat {
    let vals: HashMap<usize, f32> = diag.wipeout_totals.iter().cloned().enumerate().filter(|(_, v)| *v > 0.0).collect();
    // If the totals are empty (e.g. failure during the very first propagation), fall back to the engine's weights.
    let vals = if vals.is_empty() {
        diag.crossing_weights.iter().map(|w| w - 1.0).enumerate().filter(|(_, v)| *v > 0.0).collect()
    } else {
        vals
    };
    let h = heat_from_crossings(p, cfg, &vals);
    if h.cells.is_empty() { heat_from_dead_slots(p, cfg).unwrap_or(h) } else { h }
}

/// A finished fill as grid rows, with '#' for blocks.
pub fn fill_grid(p: &Puzzle, r: &GridConfig, choices: &[Choice]) -> Vec<String> {
    // render_grid shows blocks as '.', so restore '#' from the puzzle. Squares no answer in the
    // engine covers (e.g. unchecked squares in a rebus answer) keep what the puzzle has.
    render_grid(r, choices)
        .lines()
        .zip(&p.grid)
        .map(|(line, orig)| line.chars().zip(orig.chars()).map(|(c, o)| if o == '#' { '#' } else if c == '.' { o } else { c.to_ascii_uppercase() }).collect())
        .collect()
}

pub fn fill(p: &Puzzle, o: &Options, timeout: Duration) -> Result<FillReport> {
    fill_variant(p, o, timeout, 0)
}

/// Fill with another random start. Each `variant` usually gives a different fill; 0 is the
/// same fill as `fill` (and as the live check's sample fill).
pub fn fill_variant(p: &Puzzle, o: &Options, timeout: Duration, variant: u64) -> Result<FillReport> {
    fill_variant_until(p, o, timeout, variant, None)
}

/// `fill_variant` that also stops (as a timeout) when `stop` turns true.
pub fn fill_variant_until(p: &Puzzle, o: &Options, timeout: Duration, variant: u64, stop: Option<Arc<std::sync::atomic::AtomicBool>>) -> Result<FillReport> {
    let mut cfg = build_config(p, o)?;
    cfg.abort = stop;
    let r = cfg.to_config_ref();
    let start = instant::Instant::now();
    let (res, diag) = find_fill_variant_with_diagnostics(&r, Some(timeout), variant);
    let seconds = start.elapsed().as_secs_f32();
    let heat = || search_heat(p, &cfg, &diag);
    Ok(match res {
        Ok(ok) => {
            let grid = fill_grid(p, &r, &ok.choices);
            FillReport { status: FillStatus::Filled, grid: Some(grid), seconds, backtracks: Some(ok.statistics.backtracks), hotspots: None, rebus: rebus_answers(p) }
        }
        Err(FillFailure::Timeout) | Err(FillFailure::Abort) => FillReport { status: FillStatus::Timeout, grid: None, seconds, backtracks: None, hotspots: Some(heat()), rebus: rebus_answers(p) },
        Err(_) => FillReport { status: FillStatus::Impossible, grid: None, seconds, backtracks: None, hotspots: Some(heat()), rebus: rebus_answers(p) },
    })
}

/// Text heat map: blocks as '#', letters kept, empty squares shown as heat 0-9 ('.' for none).
pub fn render_heat(p: &Puzzle, heat: &Heat) -> String {
    let mut out = String::new();
    for y in 0..p.height() {
        for x in 0..p.width() {
            let c = p.cell(x, y);
            let h = heat.grid[y][x];
            let s = if c == '#' {
                "##".to_string()
            } else if h > 0.0 {
                format!("{:>2}", ((h * 9.0).round() as u32).max(1))
            } else if c == '.' {
                " .".to_string()
            } else {
                format!(" {}", c.to_ascii_uppercase())
            };
            out.push_str(&s);
            out.push(' ');
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::puzzle::{BOTTOM, RIGHT};

    fn opts() -> Options {
        Options { min_score: 50, max_shared_substring: None, wordlists: vec![], no_default_list: false, ..Default::default() }
    }

    #[test]
    fn rebus_answers_are_left_out() {
        crate::need_list!();
        let mut p = Puzzle::blank("t", 3, 3, "none");
        p.grid = vec!["SAT".into(), "...".into(), "...".into()];
        p.rebus.insert("0,0".into(), "STAR".into());
        let r = check(&p, &opts()).unwrap();
        // 1A and 1D go through the rebus square, so the engine doesn't see them.
        let keys: Vec<_> = r.slots.iter().map(|s| s.key.as_str()).collect();
        assert!(!keys.contains(&"0,0,across") && !keys.contains(&"0,0,down"), "{keys:?}");
        assert_eq!(r.rebus.len(), 2);
        assert!(r.rebus.iter().find(|a| a.key == "0,0,across").unwrap().complete);
        assert!(!r.rebus.iter().find(|a| a.key == "0,0,down").unwrap().complete);
        assert!(r.consistent);
        assert!(rebus_notes(&r.rebus).iter().any(|n| n.contains("empty squares")));
        assert!(candidates(&p, &opts(), "1a", 5).is_err());
        // The fill keeps the rebus square's first letter and fills the rest.
        let f = fill(&p, &opts(), Duration::from_secs(20)).unwrap();
        assert!(matches!(f.status, FillStatus::Filled), "{:?}", f.status);
        let g = f.grid.unwrap();
        assert_eq!(&g[0][..1], "S");
        assert_eq!(&g[0][1..], "AT", "letters of a rebus answer still bind its crossings");
        assert!(!g.iter().any(|r| r.contains('.')), "{g:?}");
        // Once complete, the note says it isn't checked.
        let mut q = p.clone();
        q.grid = g;
        assert!(rebus_notes(&rebus_answers(&q))[0].contains("not checked"));
    }

    #[test]
    fn engine_slots_follow_walls() {
        crate::need_list!();
        let mut p = Puzzle::blank("t", 4, 4, "none");
        p.bars.insert("1,0".into(), vec![RIGHT.into()]);
        p.bars.insert("1,2".into(), vec![BOTTOM.into()]);
        let cfg = build_config(&p, &opts()).unwrap();
        let mut engine: Vec<String> = cfg.slot_configs.iter().map(|s| format!("{},{}", slot_key(&cfg, s.id), s.length)).collect();
        let mut ours: Vec<String> = p.slots().iter().map(|s| format!("{},{}", s.key, s.len)).collect();
        engine.sort();
        ours.sort();
        assert_eq!(engine, ours);
        assert!(ours.contains(&"0,0,across,2".to_string()) && ours.contains(&"1,0,down,3".to_string()));
        assert!(matches!(fill(&p, &opts(), Duration::from_secs(20)).unwrap().status, FillStatus::Filled));
    }
}
