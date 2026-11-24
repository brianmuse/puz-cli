//! Black-square patterns. `pattern` builds whole patterns around the answers already in the grid
//! (plus any `--entry` answers) and ranks the ones that fill. `trim` removes black squares, a
//! symmetric pair at a time, while the grid still fills. Code does the search; the agent chooses.

use crate::engine::{self, FillReport, FillStatus, Options};
use crate::puzzle::{Dir, Puzzle};
use crate::suggest::grid_problems;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use instant::Instant;
use std::time::Duration;

pub use crate::par::{parallel, MAX_THREADS};

// ---------------------------------------------------------------- entries

/// An answer to place before the search: `x,y,across=WORD` or `x,y,down=WORD`.
#[derive(Debug, Clone)]
pub struct Entry {
    pub x: usize,
    pub y: usize,
    pub dir: Dir,
    pub word: String,
}

impl Entry {
    pub fn parse(s: &str) -> Result<Entry> {
        let (at, word) = s.split_once('=').with_context(|| format!("entry {s:?}: use x,y,across=WORD or x,y,down=WORD"))?;
        let parts: Vec<&str> = at.split(',').map(str::trim).collect();
        let [x, y, d] = parts[..] else { bail!("entry {s:?}: use x,y,across=WORD or x,y,down=WORD") };
        let dir = match d.to_ascii_lowercase().as_str() {
            "a" | "across" => Dir::Across,
            "d" | "down" => Dir::Down,
            _ => bail!("entry {s:?}: direction must be across or down"),
        };
        let word: String = word.chars().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_uppercase()).collect();
        if word.len() < 3 {
            bail!("entry {s:?}: the word needs 3 or more letters");
        }
        Ok(Entry { x: x.parse().context("entry x")?, y: y.parse().context("entry y")?, dir, word })
    }

    fn cells(&self) -> Vec<(usize, usize)> {
        (0..self.word.len()).map(|i| if self.dir == Dir::Across { (self.x + i, self.y) } else { (self.x, self.y + i) }).collect()
    }
}

// ---------------------------------------------------------------- the working grid

/// A grid of black and white squares, without letters, for fast pattern search.
#[derive(Clone)]
struct G {
    w: usize,
    h: usize,
    /// The symmetry name, as `puz_core::blocks::image` reads it.
    sym: &'static str,
    black: Vec<bool>,
    /// Squares that must stay white: letters and their symmetric partners.
    fixed_white: Vec<bool>,
    /// Squares that must stay black: the ends of kept answers and their partners.
    fixed_black: Vec<bool>,
    /// Kept answers as (start index, across, length). They may be longer than max_len.
    kept: HashSet<(usize, bool, usize)>,
    /// Walls after each square: `walls[0][i]` on its right, `walls[1][i]` below it. Walls end
    /// answers like black squares (as `Puzzle::slots` numbers them) but are not black squares.
    walls: [Vec<bool>; 2],
}

/// The puzzle's walls as `G::walls`.
fn walls_of(p: &Puzzle) -> [Vec<bool>; 2] {
    let (w, h) = (p.width(), p.height());
    let side = |s: &str| (0..w * h).map(|i| p.has_bar(i % w, i / w, s)).collect();
    [side(puz_core::puzzle::RIGHT), side(puz_core::puzzle::BOTTOM)]
}

impl G {
    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }
    fn xy(&self, i: usize) -> (usize, usize) {
        (i % self.w, i / self.w)
    }
    /// Square i and the squares that mirror it (up to four with 90-degree symmetry).
    fn orbit(&self, i: usize) -> Vec<usize> {
        let (x, y) = self.xy(i);
        let mut out = vec![i];
        out.extend(puz_core::blocks::orbit_of(self.sym, self.w, self.h, x, y).into_iter().map(|(a, b)| self.idx(a, b)));
        out
    }
    /// Is there a wall after square i (on its right when across, below it when down)?
    fn wall_after(&self, i: usize, across: bool) -> bool {
        self.walls[if across { 0 } else { 1 }][i]
    }

    /// Is a white run of `len` squares allowed? 3+ always; 1 only when a wall ends it (barred
    /// grids have unchecked squares), as in `blocks::grid_problems`.
    fn run_ok(&self, start: usize, across: bool, len: usize) -> bool {
        if len >= 3 {
            return true;
        }
        if len != 1 {
            return false;
        }
        let (x, y) = self.xy(start);
        let before = if across { x > 0 && self.wall_after(start - 1, true) } else { y > 0 && self.wall_after(start - self.w, false) };
        before || self.wall_after(start, across)
    }

    fn blocks(&self) -> usize {
        self.black.iter().filter(|&&b| b).count()
    }

    /// The white run through white square i: (start index, length).
    fn run(&self, i: usize, across: bool) -> (usize, usize) {
        let (x, y) = self.xy(i);
        let (mut s, mut e) = if across { (x, x) } else { (y, y) };
        let at = |k: usize| if across { self.idx(k, y) } else { self.idx(x, k) };
        let lim = if across { self.w } else { self.h };
        while s > 0 && !self.black[at(s - 1)] && !self.wall_after(at(s - 1), across) {
            s -= 1;
        }
        while e + 1 < lim && !self.black[at(e + 1)] && !self.wall_after(at(e), across) {
            e += 1;
        }
        (at(s), e - s + 1)
    }

    /// Every white run of 1+ squares: (start index, across, length).
    fn runs(&self) -> Vec<(usize, bool, usize)> {
        let mut out = vec![];
        for across in [true, false] {
            let (outer, inner) = if across { (self.h, self.w) } else { (self.w, self.h) };
            for o in 0..outer {
                let mut start = None;
                for k in 0..=inner {
                    let i = (k < inner).then(|| if across { self.idx(k, o) } else { self.idx(o, k) });
                    match (i.filter(|&i| !self.black[i]), start) {
                        (Some(i), None) => start = Some((i, k)),
                        (None, Some((si, sk))) => {
                            out.push((si, across, k - sk));
                            start = None;
                        }
                        _ => {}
                    }
                    // A wall after this square ends the run here.
                    if let (Some(i), Some((si, sk))) = (i, start)
                        && !self.black[i]
                        && self.wall_after(i, across)
                    {
                        out.push((si, across, k + 1 - sk));
                        start = None;
                    }
                }
            }
        }
        out
    }

    /// Can square i (and its partner) turn black without making a 1- or 2-letter answer?
    fn split_ok(&self, i: usize) -> bool {
        let orb = self.orbit(i);
        for &c in &orb {
            if self.black[c] || self.fixed_white[c] {
                return false;
            }
        }
        let mut g = self.clone();
        for &c in &orb {
            g.black[c] = true;
        }
        // Every run next to a new black square must still be allowed.
        for c in orb {
            let (x, y) = g.xy(c);
            let nb = [(x > 0).then(|| (c - 1, true)), (x + 1 < g.w).then(|| (c + 1, true)), (y > 0).then(|| (c - g.w, false)), (y + 1 < g.h).then(|| (c + g.w, false))];
            for (j, across) in nb.into_iter().flatten() {
                if !g.black[j] {
                    let (s, len) = g.run(j, across);
                    if !g.run_ok(s, across, len) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn connected(&self) -> bool {
        let Some(start) = (0..self.black.len()).find(|&i| !self.black[i]) else { return true };
        let mut seen = vec![false; self.black.len()];
        seen[start] = true;
        let mut q = VecDeque::from([start]);
        let mut count = 1;
        while let Some(i) = q.pop_front() {
            let (x, y) = self.xy(i);
            let mut n = vec![];
            if x > 0 {
                n.push(i - 1);
            }
            if x + 1 < self.w {
                n.push(i + 1);
            }
            if y > 0 {
                n.push(i - self.w);
            }
            if y + 1 < self.h {
                n.push(i + self.w);
            }
            for j in n {
                if !self.black[j] && !seen[j] {
                    seen[j] = true;
                    count += 1;
                    q.push_back(j);
                }
            }
        }
        count == self.black.iter().filter(|&&b| !b).count()
    }

    /// Turn short runs (1 or 2 squares) black, with partners, until none are left.
    /// Fails if a short run holds a letter.
    fn close_short_runs(&mut self) -> std::result::Result<(), usize> {
        loop {
            let short: Vec<(usize, bool, usize)> = self.runs().into_iter().filter(|&(s, a, l)| !self.run_ok(s, a, l)).collect();
            if short.is_empty() {
                return Ok(());
            }
            for (s, across, len) in short {
                for k in 0..len {
                    let i = if across { s + k } else { s + k * self.w };
                    for c in self.orbit(i) {
                        if self.fixed_white[c] {
                            return Err(c);
                        }
                        self.black[c] = true;
                    }
                }
            }
        }
    }

    /// Runs longer than max_len that are not kept answers.
    fn too_long(&self, max_len: usize) -> Vec<(usize, bool, usize)> {
        self.runs().into_iter().filter(|r| r.2 > max_len && !self.kept.contains(r)).collect()
    }

    /// Count of 2x2 all-black windows (clumps look bad and waste squares).
    fn clumps(&self) -> usize {
        let mut n = 0;
        for y in 0..self.h.saturating_sub(1) {
            for x in 0..self.w.saturating_sub(1) {
                let i = self.idx(x, y);
                if self.black[i] && self.black[i + 1] && self.black[i + self.w] && self.black[i + self.w + 1] {
                    n += 1;
                }
            }
        }
        n
    }
}

/// A small fast random number generator (xorshift64*), so searches repeat with the same seed.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ---------------------------------------------------------------- setup

/// The puzzle with entries placed, and the working grid with its fixed squares.
struct Setup {
    base: Puzzle,
    g: G,
}

fn sym_of(p: &Puzzle) -> &'static str {
    let s = if p.symmetry == "left-right" { "mirror" } else { p.symmetry.as_str() };
    puz_core::blocks::SYMMETRIES.iter().copied().find(|&n| n == s).unwrap_or("none")
}

/// Place the entries and work out which squares are fixed. With `keep_blocks` false, black
/// squares that don't end a kept answer are cleared, so the search starts fresh.
fn setup(p: &Puzzle, entries: &[Entry], keep_blocks: bool) -> Result<Setup> {
    let (w, h) = (p.width(), p.height());
    let mut grid: Vec<Vec<char>> = p.grid.iter().map(|r| r.chars().collect()).collect();
    // Kept answers: complete answers already in the grid, then the entries.
    let mut spans: Vec<(usize, usize, bool, usize, String)> = p
        .slots()
        .into_iter()
        .filter(|s| !s.pattern.contains('.') && s.len >= 3)
        .map(|s| (s.x, s.y, s.dir == Dir::Across, s.len, s.pattern.clone()))
        .collect();
    for e in entries {
        let cells = e.cells();
        if cells.iter().any(|&(x, y)| x >= w || y >= h) {
            bail!("{} at {},{} {} runs off the grid", e.word, e.x, e.y, e.dir.name());
        }
        for (&(x, y), ch) in cells.iter().zip(e.word.chars()) {
            let c = grid[y][x];
            if c != '.' && c != '#' && c != ch {
                bail!("{} at {},{} {} needs {ch} at {x},{y}, which has {c}", e.word, e.x, e.y, e.dir.name());
            }
            if c == '#' && keep_blocks {
                bail!("{} at {},{} {} runs into the black square at {x},{y}", e.word, e.x, e.y, e.dir.name());
            }
            grid[y][x] = ch;
        }
        let walled = cells.windows(2).any(|c| p.has_bar(c[0].0, c[0].1, if e.dir == Dir::Across { puz_core::puzzle::RIGHT } else { puz_core::puzzle::BOTTOM }));
        if walled {
            bail!("{} at {},{} {} crosses a wall", e.word, e.x, e.y, e.dir.name());
        }
        spans.push((e.x, e.y, e.dir == Dir::Across, e.word.len(), e.word.clone()));
    }
    let is_letter = |c: char| c != '.' && c != '#';
    let mut g = G {
        w,
        h,
        sym: sym_of(p),
        black: vec![false; w * h],
        fixed_white: vec![false; w * h],
        fixed_black: vec![false; w * h],
        kept: HashSet::new(),
        walls: walls_of(p),
    };
    for y in 0..h {
        for x in 0..w {
            let i = g.idx(x, y);
            if is_letter(grid[y][x]) {
                g.fixed_white[i] = true;
                for q in g.orbit(i) {
                    g.fixed_white[q] = true;
                }
            }
            if keep_blocks && grid[y][x] == '#' {
                g.black[i] = true;
            }
        }
    }
    for &(x, y, across, len, ref word) in &spans {
        let start = g.idx(x, y);
        g.kept.insert((start, across, len));
        let ends = if across { [(x.checked_sub(1), Some(y)), (Some(x + len).filter(|&e| e < w), Some(y))] } else { [(Some(x), y.checked_sub(1)), (Some(x), Some(y + len).filter(|&e| e < h))] };
        let last = if across { g.idx(x + len - 1, y) } else { g.idx(x, y + len - 1) };
        for (k, (ex, ey)) in ends.into_iter().enumerate() {
            let (Some(ex), Some(ey)) = (ex, ey) else { continue };
            let i = g.idx(ex, ey);
            // A wall already ends the answer here: no black square needed.
            if (k == 0 && g.wall_after(i, across)) || (k == 1 && g.wall_after(last, across)) {
                continue;
            }
            for c in g.orbit(i) {
                if g.fixed_white[c] {
                    let (cx, cy) = g.xy(c);
                    bail!("{word} at {x},{y} needs a black square at {ex},{ey}, but {} has a letter or mirrors one", if c == i { format!("{ex},{ey}") } else { format!("its partner {cx},{cy}") });
                }
                g.black[c] = true;
                g.fixed_black[c] = true;
            }
        }
    }
    if let Err(c) = g.close_short_runs() {
        let (x, y) = g.xy(c);
        bail!("the letters leave a 1- or 2-letter answer through {x},{y} that black squares can't fix; move an answer");
    }
    let mut base = p.clone();
    base.grid = (0..h).map(|y| (0..w).map(|x| if g.black[g.idx(x, y)] { '#' } else if grid[y][x] == '#' { '.' } else { grid[y][x] }).collect()).collect();
    Ok(Setup { base, g })
}

/// Add a black square at (x, y) and its partner, then close the 1- and 2-letter answers that
/// makes (with their partners). None if a square that would turn black has a letter, it takes
/// more than `max_new` black squares in all, or it cuts the grid in two. Returns the new grid
/// and its new black squares.
pub fn add_block_closing(p: &Puzzle, x: usize, y: usize, max_new: usize) -> Option<(Puzzle, Vec<(usize, usize)>)> {
    let (w, h) = (p.width(), p.height());
    let black: Vec<bool> = (0..w * h).map(|i| p.cell(i % w, i / w) == '#').collect();
    let fixed_white: Vec<bool> = (0..w * h).map(|i| !matches!(p.cell(i % w, i / w), '#' | '.')).collect();
    let mut g = G { w, h, sym: sym_of(p), black: black.clone(), fixed_white: fixed_white.clone(), fixed_black: vec![false; w * h], kept: HashSet::new(), walls: walls_of(p) };
    let i = g.idx(x, y);
    let orb = g.orbit(i);
    // Check them all first.
    if orb.iter().any(|&c| g.black[c] || g.fixed_white[c]) {
        return None;
    }
    for &c in &orb {
        g.black[c] = true;
    }
    g.close_short_runs().ok()?;
    if !g.connected() {
        return None;
    }
    let new: Vec<(usize, usize)> = (0..w * h).filter(|&k| g.black[k] && !black[k]).map(|k| g.xy(k)).collect();
    if new.len() > max_new {
        return None;
    }
    let mut q = p.clone();
    q.grid = to_grid(p, &g);
    q.tidy();
    Some((q, new))
}

fn to_grid(base: &Puzzle, g: &G) -> Vec<String> {
    base.grid
        .iter()
        .enumerate()
        .map(|(y, row)| row.chars().enumerate().map(|(x, c)| if g.black[g.idx(x, y)] { '#' } else if c == '#' { '.' } else { c }).collect())
        .collect()
}

/// Average word-list score of a filled grid's answers (higher is better fill), and the weakest answers.
fn fill_quality(p: &Puzzle, grid: &[String]) -> (Option<f32>, Vec<String>) {
    let mut q = p.clone();
    q.grid = grid.to_vec();
    // Only answers the fill wrote: answers that already had all their letters are the user's.
    let before: HashSet<String> = p.slots().into_iter().filter(|s| !s.pattern.contains('.')).map(|s| s.key).collect();
    let mut scored: Vec<(u32, String)> = q
        .slots()
        .into_iter()
        .filter(|s| !before.contains(&s.key))
        .filter_map(|s| s.answer().map(|a| (engine::list_score(&a).unwrap_or(0), a)))
        .collect();
    if scored.is_empty() {
        return (None, vec![]);
    }
    let avg = scored.iter().map(|s| s.0 as f32).sum::<f32>() / scored.len() as f32;
    scored.sort();
    (Some(avg), scored.into_iter().take(5).map(|s| s.1).collect())
}

// ---------------------------------------------------------------- pattern

pub struct PatternOptions {
    /// Black squares to aim for. None: about 1/6 of the grid (less for small grids).
    pub blocks: Option<usize>,
    /// Most answers allowed. None: the puzzle's rules (e.g. 78 for nyt-daily), else no limit.
    pub max_words: Option<usize>,
    /// Longest answer allowed, except kept answers. None: no limit.
    pub max_len: Option<usize>,
    pub entries: Vec<Entry>,
    pub keep_blocks: bool,
    /// Total time for the search, including fill tests.
    pub seconds: Duration,
    pub fill_timeout: Duration,
    /// How many patterns to return.
    pub count: usize,
    pub seed: u64,
}

#[derive(Debug, Serialize)]
pub struct PatternOption {
    pub n: usize,
    pub status: FillStatus,
    pub seconds: f32,
    pub blocks: usize,
    pub words: usize,
    pub three_letter: usize,
    pub longest: usize,
    /// Lower is better: distance from the black-square target, 3-letter answers, clumps.
    pub penalty: f32,
    /// The pattern with the grid's letters (not filled).
    pub grid: Vec<String>,
    pub sample_fill: Option<Vec<String>>,
    /// Average word-list score of the sample fill's answers (higher is better).
    pub fill_score: Option<f32>,
    /// The sample fill's lowest-scoring answers.
    pub weakest: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Rejected {
    pub too_many_blocks: usize,
    pub too_many_words: usize,
    pub too_long: usize,
    pub split_grid: usize,
    pub duplicate: usize,
    pub dead_end: usize,
    pub no_fill: usize,
}

#[derive(Debug, Serialize)]
pub struct PatternReport {
    pub target_blocks: usize,
    pub max_words: Option<usize>,
    pub max_len: Option<usize>,
    pub fixed_blocks: usize,
    pub attempts: usize,
    pub candidates: usize,
    pub fill_tested: usize,
    pub rejected: Rejected,
    pub patterns: Vec<PatternOption>,
    /// Advice when nothing filled.
    pub note: Option<String>,
}

/// The default black-square target for a grid size.
pub fn default_blocks(w: usize, h: usize) -> usize {
    let area = (w * h) as f32;
    let share = if w.min(h) >= 11 { 0.16 } else if w.min(h) >= 7 { 0.13 } else { 0.12 };
    (area * share).round() as usize
}

fn penalty(g: &G, target: usize) -> (f32, usize, usize, usize) {
    let runs: Vec<_> = g.runs().into_iter().filter(|r| r.2 >= 3).collect();
    let words = runs.len();
    let threes = runs.iter().filter(|r| r.2 == 3).count();
    let longest = runs.iter().map(|r| r.2).max().unwrap_or(0);
    // Long answers that aren't kept answers make a grid hard to fill, more so when they stack.
    // Up to `easy` letters is free: about half the grid's side, never under 5.
    let easy = (g.w.min(g.h) / 2).max(5);
    let open: f32 = runs.iter().filter(|r| !g.kept.contains(r)).map(|r| r.2.saturating_sub(easy).pow(2) as f32).sum();
    let b = g.blocks();
    let pen = 4.0 * (b as f32 - target as f32).abs() + 1.0 * threes as f32 + 1.5 * open + 3.0 * g.clumps() as f32;
    (pen, words, threes, longest)
}

/// Build one pattern: add symmetric black pairs at random, weighted toward long runs, until the
/// target is met and no answer is too long.
fn build(start: &G, target: usize, max_len: Option<usize>, rng: &mut Rng) -> std::result::Result<G, &'static str> {
    let mut g = start.clone();
    let n = g.black.len();
    let limit = target + 2 + target / 10;
    loop {
        let long = max_len.map(|m| g.too_long(m)).unwrap_or_default();
        let b = g.blocks();
        if b >= target && long.is_empty() {
            return Ok(g);
        }
        if b > limit {
            return Err(if long.is_empty() { "blocks" } else { "long" });
        }
        let in_long: HashSet<usize> = long
            .iter()
            .flat_map(|&(s, across, len)| (0..len).map(move |k| if across { s + k } else { s + k * start.w }))
            .collect();
        let mut weights: Vec<(usize, f64)> = (0..n)
            .filter(|&i| !g.black[i] && !g.fixed_white[i] && (b < target || in_long.contains(&i)))
            .map(|i| {
                let (_, a) = g.run(i, true);
                let (_, d) = g.run(i, false);
                let mut wt = (a * a + d * d) as f64;
                if in_long.contains(&i) {
                    wt *= 20.0;
                }
                // Clumps: neighbors that are black already.
                let (x, y) = g.xy(i);
                let nb = [(x > 0).then(|| i - 1), (x + 1 < g.w).then(|| i + 1), (y > 0).then(|| i - g.w), (y + 1 < g.h).then(|| i + g.w)]
                    .into_iter()
                    .flatten()
                    .filter(|&j| g.black[j])
                    .count();
                if nb >= 2 {
                    wt *= 0.3;
                }
                (i, wt)
            })
            .collect();
        let mut placed = false;
        while !weights.is_empty() {
            let total: f64 = weights.iter().map(|w| w.1).sum();
            let mut r = rng.unit() * total;
            let mut k = weights.len() - 1;
            for (j, w) in weights.iter().enumerate() {
                if r < w.1 {
                    k = j;
                    break;
                }
                r -= w.1;
            }
            let (i, _) = weights.swap_remove(k);
            if !g.split_ok(i) {
                continue;
            }
            let mut t = g.clone();
            for c in t.orbit(i) {
                t.black[c] = true;
            }
            if !t.connected() {
                continue;
            }
            g = t;
            placed = true;
            break;
        }
        if !placed {
            // No square can take a black square: the target is a goal, so keep what we have.
            return if long.is_empty() { Ok(g) } else { Err("long") };
        }
    }
}

pub fn pattern(p: &Puzzle, o: &Options, po: &PatternOptions) -> Result<PatternReport> {
    let started = Instant::now();
    let Setup { base, g: start } = setup(p, &po.entries, po.keep_blocks)?;
    let target = po.blocks.unwrap_or_else(|| default_blocks(p.width(), p.height())).max(start.blocks());
    let max_words = match po.max_words {
        Some(m) => Some(m),
        None => crate::stats::preset_for(p, "auto").ok().and_then(|(r, _)| r.max_answers),
    };
    let mut rejected = Rejected::default();

    // 1. Generate patterns (cheap, no engine) for about a quarter of the time.
    let gen_budget = po.seconds.mul_f32(0.25);
    let mut seen: HashSet<Vec<bool>> = HashSet::new();
    let mut found: Vec<(f32, G)> = vec![];
    let mut attempts = 0;
    let mut last_new = 0;
    while attempts < 20_000 && started.elapsed() < gen_budget && found.len() < 2_000 && attempts - last_new < 800 {
        let mut rng = Rng::new(po.seed.wrapping_mul(1_000_003).wrapping_add(attempts as u64));
        attempts += 1;
        let g = match build(&start, target, po.max_len, &mut rng) {
            Ok(g) => g,
            Err("long") => {
                rejected.too_long += 1;
                continue;
            }
            Err(_) => {
                rejected.too_many_blocks += 1;
                continue;
            }
        };
        if !g.connected() {
            rejected.split_grid += 1;
            continue;
        }
        let (pen, words, _, _) = penalty(&g, target);
        if max_words.is_some_and(|m| words > m) {
            rejected.too_many_words += 1;
            continue;
        }
        if !seen.insert(g.black.clone()) {
            rejected.duplicate += 1;
            continue;
        }
        found.push((pen, g));
        last_new = attempts;
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    let candidates = found.len();

    // 2. Fill tests, best patterns first, on a shared queue: each thread takes the next pattern as
    // soon as it is free, until enough fill or time runs out. A quick dead-end check (arc
    // consistency) skips patterns that can't fill before the slower fill test.
    let want = po.count.max(1);
    let next = AtomicUsize::new(0);
    let filled = AtomicUsize::new(0);
    let dead = AtomicUsize::new(0);
    let tested = AtomicUsize::new(0);
    let results: Mutex<Vec<(f32, Puzzle, usize, FillReport)>> = Mutex::new(vec![]);
    crate::par::workers(crate::par::threads(MAX_THREADS), || -> Result<()> {
        loop {
            let left = po.seconds.saturating_sub(started.elapsed());
            if filled.load(Ordering::Relaxed) >= want || left < Duration::from_millis(1500) {
                return Ok(());
            }
            let k = next.fetch_add(1, Ordering::Relaxed);
            let Some((pen, g)) = found.get(k) else { return Ok(()) };
            let mut q = base.clone();
            q.grid = to_grid(&base, g);
            if !grid_problems(&q).is_empty() || !engine::check(&q, o).map(|r| r.consistent).unwrap_or(false) {
                dead.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            tested.fetch_add(1, Ordering::Relaxed);
            let r = engine::fill(&q, o, po.fill_timeout.min(left))?;
            if matches!(r.status, FillStatus::Filled) {
                filled.fetch_add(1, Ordering::Relaxed);
            }
            results.lock().unwrap().push((*pen, q, k, r));
        }
    })?;
    rejected.dead_end = dead.into_inner();
    let tested = tested.into_inner();
    let mut options: Vec<PatternOption> = vec![];
    for (pen, q, k, r) in results.into_inner().unwrap() {
        if matches!(r.status, FillStatus::Impossible) {
            rejected.no_fill += 1;
            continue;
        }
        let g = &found[k].1;
        let (_, words, threes, longest) = penalty(g, target);
        let (fill_score, weakest) = r.grid.as_ref().map(|f| fill_quality(&q, f)).unwrap_or((None, vec![]));
        options.push(PatternOption {
            n: 0,
            status: r.status,
            seconds: r.seconds,
            blocks: g.blocks(),
            words,
            three_letter: threes,
            longest,
            penalty: pen,
            grid: q.grid,
            sample_fill: r.grid,
            fill_score,
            weakest,
        });
    }
    let rank = |s: &FillStatus| if matches!(s, FillStatus::Filled) { 0 } else { 1 };
    options.sort_by(|a, b| rank(&a.status).cmp(&rank(&b.status)).then(a.penalty.total_cmp(&b.penalty)));
    options.truncate(want);
    for (i, opt) in options.iter_mut().enumerate() {
        opt.n = i + 1;
    }
    let note = if options.iter().any(|o| matches!(o.status, FillStatus::Filled)) {
        None
    } else if candidates == 0 {
        Some(format!(
            "No pattern met the rules around these answers (target {target} black squares{}{}). Try a higher --blocks, a higher --max-len, or move an answer.",
            max_words.map(|m| format!(", at most {m} answers")).unwrap_or_default(),
            po.max_len.map(|m| format!(", answers up to {m} letters")).unwrap_or_default()
        ))
    } else if tested == 0 && rejected.dead_end > 0 {
        Some(format!(
            "All {} patterns tried hit a dead end: some answer has no word that fits its crossings. Move a theme answer, pick another, or allow more black squares.",
            rejected.dead_end
        ))
    } else {
        Some("No pattern filled in time. Try more --seconds, a higher --blocks, or answers with friendlier letters; or check which crossings are hard with the status command.".into())
    };
    Ok(PatternReport {
        target_blocks: target,
        max_words,
        max_len: po.max_len,
        fixed_blocks: start.blocks(),
        attempts,
        candidates,
        fill_tested: tested,
        rejected,
        patterns: options,
        note,
    })
}

// ---------------------------------------------------------------- trim

pub struct TrimOptions {
    /// Stop at this many black squares.
    pub to: usize,
    pub max_len: Option<usize>,
    pub seconds: Duration,
    pub fill_timeout: Duration,
    pub seed: u64,
}

#[derive(Debug, Serialize)]
pub struct TrimReport {
    pub start_blocks: usize,
    pub blocks: usize,
    pub words: usize,
    /// Squares turned white, a pair (or one center square) per step.
    pub removed: Vec<Vec<(usize, usize)>>,
    pub grid: Vec<String>,
    pub sample_fill: Option<Vec<String>>,
    pub fill_score: Option<f32>,
    pub reached_target: bool,
    /// Why it stopped early, if it did.
    pub stopped: Option<String>,
}

pub fn trim(p: &Puzzle, o: &Options, to: &TrimOptions) -> Result<TrimReport> {
    let started = Instant::now();
    let Setup { base, mut g } = setup(p, &[], true)?;
    let start_blocks = p.grid.iter().map(|r| r.chars().filter(|&c| c == '#').count()).sum::<usize>();
    if g.blocks() != start_blocks {
        bail!("the grid has 1- or 2-letter answers; fix them first (`puz stats` lists them)");
    }
    let mut rng = Rng::new(to.seed);
    let mut removed = vec![];
    let mut bad: HashSet<usize> = HashSet::new();
    let mut last_fill = None;
    let mut stopped = None;
    while g.blocks() > to.to {
        if started.elapsed() >= to.seconds {
            stopped = Some("ran out of time".into());
            break;
        }
        // Removable pairs, best first: the merged answers should be short and within max_len.
        let mut cands: Vec<(f64, usize)> = (0..g.black.len())
            .filter(|&i| g.black[i] && !g.fixed_black[i] && g.orbit(i).iter().all(|&q| i <= q) && !bad.contains(&i))
            .filter_map(|i| {
                let mut t = g.clone();
                let orb = t.orbit(i);
                for &c in &orb {
                    t.black[c] = false;
                }
                let mut worst = 0;
                for c in orb {
                    for across in [true, false] {
                        let (s, len) = t.run(c, across);
                        if !t.run_ok(s, across, len) {
                            return None;
                        }
                        worst = worst.max(len);
                    }
                }
                if let Some(m) = to.max_len
                    && worst > m
                {
                    return None;
                }
                let mut q2 = base.clone();
                q2.grid = to_grid(&base, &t);
                if !grid_problems(&q2).is_empty() {
                    return None;
                }
                Some((worst as f64 + rng.unit() * 3.0, i))
            })
            .collect();
        cands.sort_by(|a, b| a.0.total_cmp(&b.0));
        if cands.is_empty() {
            stopped = Some("no black square can go without breaking the rules".into());
            break;
        }
        // Try the best few in parallel; take the best one that fills.
        let left = to.seconds.saturating_sub(started.elapsed());
        let timeout = to.fill_timeout.min(left.max(Duration::from_secs(1)));
        let mut accepted = false;
        for chunk in cands.chunks(MAX_THREADS).take(3) {
            let trials: Vec<(usize, G, Puzzle)> = chunk
                .iter()
                .map(|&(_, i)| {
                    let mut t = g.clone();
                    for c in t.orbit(i) {
                        t.black[c] = false;
                    }
                    let mut pz = base.clone();
                    pz.grid = to_grid(&base, &t);
                    (i, t, pz)
                })
                .collect();
            let fills = parallel(&trials, |(_, _, pz)| engine::fill(pz, o, timeout));
            let mut hit = None;
            for ((i, t, pz), r) in trials.into_iter().zip(fills) {
                let r = r?;
                if matches!(r.status, FillStatus::Filled) && hit.is_none() {
                    hit = Some((i, t, pz, r.grid));
                } else if !matches!(r.status, FillStatus::Filled) {
                    bad.insert(i);
                }
            }
            if let Some((i, t, _, fill)) = hit {
                removed.push(g.orbit(i).into_iter().map(|c| g.xy(c)).collect());
                g = t;
                last_fill = fill;
                accepted = true;
                break;
            }
            if started.elapsed() >= to.seconds {
                break;
            }
        }
        if !accepted {
            stopped = Some("no removable black square left a grid that fills".into());
            break;
        }
    }
    let mut q = base.clone();
    q.grid = to_grid(&base, &g);
    let fill_score = last_fill.as_ref().and_then(|f| fill_quality(&q, f).0);
    let words = q.slots().len();
    let blocks = g.blocks();
    Ok(TrimReport { start_blocks, blocks, words, removed, grid: q.grid, sample_fill: last_fill, fill_score, reached_target: blocks <= to.to, stopped })
}

/// Print a pattern report for people.
pub fn render_pattern(r: &PatternReport) -> String {
    let mut out = format!(
        "Target {} black squares ({} fixed){}{}. Built {} patterns from {} tries; fill-tested {}.\n",
        r.target_blocks,
        r.fixed_blocks,
        r.max_words.map(|m| format!(", at most {m} answers")).unwrap_or_default(),
        r.max_len.map(|m| format!(", answers up to {m} letters")).unwrap_or_default(),
        r.candidates,
        r.attempts,
        r.fill_tested
    );
    for o in &r.patterns {
        let status = match o.status {
            FillStatus::Filled => format!("FILLS in {:.1}s", o.seconds),
            FillStatus::Timeout => format!("no fill in {:.0}s", o.seconds),
            FillStatus::Impossible => "impossible".into(),
        };
        out.push_str(&format!(
            "\n{}. {status}: {} black squares, {} answers ({} with 3 letters), longest {}{}\n",
            o.n,
            o.blocks,
            o.words,
            o.three_letter,
            o.longest,
            o.fill_score.map(|s| format!(", fill score {s:.0}")).unwrap_or_default()
        ));
        match &o.sample_fill {
            Some(f) => {
                for (g, f) in o.grid.iter().zip(f) {
                    out.push_str(&format!("  {g}   {f}\n"));
                }
            }
            None => {
                for g in &o.grid {
                    out.push_str(&format!("  {g}\n"));
                }
            }
        }
    }
    if let Some(n) = &r.note {
        out.push_str(&format!("\n{n}\n"));
    }
    out
}

pub fn render_trim(r: &TrimReport) -> String {
    let mut out = format!("Black squares: {} -> {} ({} answers).", r.start_blocks, r.blocks, r.words);
    if let Some(s) = &r.stopped {
        out.push_str(&format!(" Stopped: {s}."));
    }
    out.push('\n');
    let steps: Vec<String> = r.removed.iter().map(|sq| sq.iter().map(|(x, y)| format!("{x},{y}")).collect::<Vec<_>>().join("+")).collect();
    if !steps.is_empty() {
        out.push_str(&format!("Removed: {}\n", steps.join("  ")));
    }
    match &r.sample_fill {
        Some(f) => {
            for (g, f) in r.grid.iter().zip(f) {
                out.push_str(&format!("  {g}   {f}\n"));
            }
        }
        None => {
            for g in &r.grid {
                out.push_str(&format!("  {g}\n"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Options {
        Options { min_score: 50, max_shared_substring: None, wordlists: vec![], no_default_list: false, ..Default::default() }
    }

    #[test]
    fn entry_parse() {
        let e = Entry::parse("2,3,down=hot honey").unwrap();
        assert_eq!((e.x, e.y, e.dir, e.word.as_str()), (2, 3, Dir::Down, "HOTHONEY"));
        assert!(Entry::parse("2,3=WORD").is_err());
        assert!(Entry::parse("2,3,across=AB").is_err());
    }

    #[test]
    fn setup_blocks_entry_ends_and_partners() {
        let p = Puzzle::blank("t", 9, 9, "rotational");
        // PLANETS in columns 1-7 of the middle row: both end squares turn black (they mirror each other).
        let s = setup(&p, &[Entry::parse("1,4,across=PLANETS").unwrap()], false).unwrap();
        assert_eq!(s.base.grid[4], "#PLANETS#");
        // In columns 0-6 the end square 7,4 mirrors the L at 1,4, so it can't be black.
        assert!(setup(&p, &[Entry::parse("0,4,across=PLANETS").unwrap()], false).is_err());
    }

    #[test]
    fn pattern_builds_fillable_grids_around_entries() {
        crate::need_list!();
        let p = Puzzle::blank("t", 9, 9, "rotational");
        let po = PatternOptions {
            blocks: Some(12),
            max_words: None,
            max_len: Some(9),
            entries: vec![Entry::parse("0,4,across=STARGAZER").unwrap()],
            keep_blocks: false,
            seconds: Duration::from_secs(20),
            fill_timeout: Duration::from_secs(5),
            count: 2,
            seed: 7,
        };
        let r = pattern(&p, &opts(), &po).unwrap();
        assert!(!r.patterns.is_empty(), "{r:?}");
        let best = &r.patterns[0];
        assert!(matches!(best.status, FillStatus::Filled), "{r:?}");
        assert_eq!(best.grid[4], "STARGAZER");
        let mut q = p.clone();
        q.grid = best.grid.clone();
        assert!(grid_problems(&q).is_empty());
        for (y, row) in best.grid.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                assert_eq!(c == '#', best.grid[8 - y].as_bytes()[8 - x] == b'#', "not symmetric at {x},{y}");
            }
        }
    }

    #[test]
    fn trim_removes_blocks_and_keeps_rules() {
        crate::need_list!();
        let mut p = Puzzle::blank("t", 5, 5, "rotational");
        p.grid = ["#...#", ".....", ".....", ".....", "#...#"].iter().map(|s| s.to_string()).collect();
        assert!(grid_problems(&p).is_empty());
        let r = trim(&p, &opts(), &TrimOptions { to: 2, max_len: None, seconds: Duration::from_secs(30), fill_timeout: Duration::from_secs(5), seed: 1 }).unwrap();
        assert!(r.blocks < r.start_blocks, "{r:?}");
        let mut q = p.clone();
        q.grid = r.grid.clone();
        assert!(grid_problems(&q).is_empty(), "{:?}", r.grid);
        assert!(r.sample_fill.is_some());
    }

    #[test]
    fn new_symmetries_place_whole_orbits() {
        let mut p = Puzzle::blank("t", 9, 9, "rotational-90");
        let (q, _) = add_block_closing(&p, 4, 0, 9).unwrap();
        for (x, y) in [(4, 0), (8, 4), (4, 8), (0, 4)] {
            assert_eq!(q.cell(x, y), '#', "{:?}", q.grid);
        }
        assert!(puz_core::stats::symmetries(&q).contains(&"rotational-90".to_string()));
        p.symmetry = "diagonal".into();
        let q = puz_core::blocks::set_block(&p, 3, 0, true).unwrap();
        assert_eq!(q.cell(0, 3), '#');
        // Square-only symmetries do nothing on other grids.
        let r = Puzzle::blank("t", 9, 7, "diagonal");
        assert!(puz_core::blocks::orbit(&r, 3, 0).is_empty());
    }

    #[test]
    fn add_block_closing_follows_walls() {
        let mut p = Puzzle::blank("t", 7, 7, "none");
        // A wall after 2,0 splits row 0 into 3 + 4 squares.
        p.bars.insert("2,0".into(), vec![puz_core::puzzle::RIGHT.into()]);
        // A black square at 0,0 leaves 1,0-2,0: a 2-letter answer that walls end. It must close.
        let (q, new) = add_block_closing(&p, 0, 0, 6).unwrap();
        assert!(new.contains(&(1, 0)) && new.contains(&(2, 0)), "{new:?}");
        assert!(grid_problems(&q).is_empty(), "{:?}", q.grid);
        // A black square at 3,0 next to the wall needs nothing more.
        let (q, new) = add_block_closing(&p, 3, 0, 6).unwrap();
        assert_eq!(new, vec![(3, 0)]);
        assert!(grid_problems(&q).is_empty(), "{:?}", q.grid);
    }

    #[test]
    fn setup_keeps_walled_answer_ends_open() {
        let mut p = Puzzle::blank("t", 7, 7, "none");
        p.bars.insert("2,3".into(), vec![puz_core::puzzle::RIGHT.into()]);
        // CAT ends at the wall: 3,3 needs no black square.
        let s = setup(&p, &[Entry::parse("0,3,across=CAT").unwrap()], false).unwrap();
        assert_eq!(s.base.grid[3], "CAT....");
        assert!(setup(&p, &[Entry::parse("1,3,across=CAT").unwrap()], false).is_err());
    }

    #[test]
    fn trim_follows_walls() {
        crate::need_list!();
        let mut p = Puzzle::blank("t", 5, 5, "rotational");
        p.grid = ["#...#", ".....", ".....", ".....", "#...#"].iter().map(|s| s.to_string()).collect();
        // Walls make 0,2 and 4,2 unchecked across squares (allowed in barred grids).
        p.bars.insert("0,2".into(), vec![puz_core::puzzle::RIGHT.into()]);
        p.bars.insert("3,2".into(), vec![puz_core::puzzle::RIGHT.into()]);
        assert!(grid_problems(&p).is_empty());
        let r = trim(&p, &opts(), &TrimOptions { to: 2, max_len: None, seconds: Duration::from_secs(30), fill_timeout: Duration::from_secs(5), seed: 1 }).unwrap();
        let mut q = p.clone();
        q.grid = r.grid.clone();
        assert!(grid_problems(&q).is_empty(), "{:?}", r.grid);
        assert!(q.slots().iter().all(|s| s.len >= 3), "{:?}", r.grid);
    }
}
