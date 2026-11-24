//! `autofill`: fill the empty squares with good words, and work on the spots where that fails.
//!
//! 1. Clean fill: only good words (`words::GOOD` and up), better words tried first, several
//!    random starts at once. If one works, keep the best.
//! 2. Concentrate: if no clean fill exists, allow weaker words only in the answers where the
//!    search got stuck (its hotspots), and grow that area until the grid fills. The weak words
//!    end up in one area instead of all over the grid.
//! 3. Polish: for each weak word, worst first, clear it and its crossings and refill that spot
//!    with the word left out. Keep the change if the grid gets better.
//! 4. Fixes for what is left: black-square pairs that give a clean fill, and the letter patterns
//!    where a good new word (a debut) would replace a weak one.
//!
//! Letters already in the grid stay. Only new answers are judged.

use crate::engine::{self, FillStatus, Heat, Options};
use crate::par::parallel;
use crate::puzzle::{Dir, Puzzle, SlotInfo};
use crate::pattern::add_block_closing;
use crate::suggest::grid_problems;
use crate::words::{self, Flag, GOOD};
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use instant::Instant;
use std::time::Duration;

pub struct AutofillOptions {
    /// Total time.
    pub seconds: Duration,
    /// The lowest word-list score allowed where the grid needs weaker words.
    pub relax_to: u16,
    /// Test black-square changes when weak words remain.
    pub try_blocks: bool,
    /// Stop at the first complete fill: no extra starts, no polish, no black-square tests. The
    /// web app shows that fill at once, then runs the full autofill. The command line leaves it off.
    pub first_only: bool,
    /// Random starts per round (default 4) and the first start's number (default 0). The web app
    /// runs autofill on several workers at once, each with its own starts, and keeps the best.
    pub starts: usize,
    pub seed_base: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeakWord {
    pub key: String,
    pub label: String,
    pub word: String,
    pub quality: i32,
    pub flags: Vec<Flag>,
    /// Weak words that cross each other share an area number.
    pub area: usize,
}

/// A weak word a good new word could replace.
#[derive(Debug, Clone, Serialize)]
pub struct Need {
    pub label: String,
    pub word: String,
    /// The letters already in the grid before the fill (theme answers, the user's letters),
    /// '.' for squares the fill chose.
    pub pattern: String,
    /// Good words in the word lists that match the pattern.
    pub good_fits: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockFix {
    /// New black squares: a square, its symmetric partner, and any squares that keep answers at
    /// 3+ letters.
    pub squares: Vec<(usize, usize)>,
    pub weak: usize,
    pub score: f32,
    pub weak_words: Vec<String>,
    /// The fill with the new black squares.
    pub fill: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AutofillReport {
    pub status: FillStatus,
    /// True when every new answer is a good word.
    pub clean: bool,
    pub grid: Option<Vec<String>>,
    /// Average quality of the new answers.
    pub score: Option<f32>,
    pub new_answers: usize,
    pub weak: Vec<WeakWord>,
    pub needs: Vec<Need>,
    pub block_fixes: Vec<BlockFix>,
    /// What it tried, in order.
    pub steps: Vec<String>,
    pub seconds: f32,
    /// Seconds to the first complete fill (only with `first_only` set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_fill_seconds: Option<f32>,
    /// Where the search got stuck, when nothing filled.
    pub hotspots: Option<Heat>,
}

// ---------------------------------------------------------------- scoring

#[derive(Clone)]
struct Scored {
    grid: Vec<String>,
    /// (key, label, word, quality, flags), worst first.
    weak: Vec<(String, String, String, i32, Vec<Flag>)>,
    penalty: i32,
    avg: f32,
    count: usize,
}

impl Scored {
    fn better_than(&self, o: &Scored) -> bool {
        self.penalty < o.penalty || (self.penalty == o.penalty && self.avg > o.avg + 0.01)
    }
}

fn label(s: &SlotInfo) -> String {
    format!("{}{}", s.number, if s.dir == Dir::Across { "A" } else { "D" })
}

/// Keys of the answers that have an empty square in `p`: the ones a fill writes.
fn open_keys(p: &Puzzle) -> HashSet<String> {
    p.slots().into_iter().filter(|s| !s.rebus && s.pattern.contains('.')).map(|s| s.key).collect()
}

fn score(p: &Puzzle, open: &HashSet<String>, grid: &[String]) -> Scored {
    let mut q = p.clone();
    q.grid = grid.to_vec();
    let mut weak = vec![];
    let (mut total, mut count, mut penalty) = (0i64, 0usize, 0i32);
    for s in q.slots() {
        if !open.contains(&s.key) {
            continue;
        }
        let Some(a) = s.answer() else { continue };
        let qual = words::judge(&a, engine::list_score(&a).map(|v| v.min(u16::MAX as u32) as u16));
        total += qual.score.max(0) as i64;
        count += 1;
        if !qual.good() {
            penalty += (GOOD - qual.score).max(1);
            weak.push((s.key.clone(), label(&s), a, qual.score, qual.flags));
        }
    }
    weak.sort_by_key(|w| w.3);
    Scored { grid: grid.to_vec(), weak, penalty, avg: if count > 0 { total as f32 / count as f32 } else { 0.0 }, count }
}

// ---------------------------------------------------------------- fill attempts

struct Attempt {
    fills: Vec<Vec<String>>,
    heat: Option<Heat>,
    impossible: bool,
}

/// Once one start has filled, the others get this much more time: twice as long as the first fill
/// took, at least 2s. A start still stuck by then rarely fills well, so stop waiting for it.
fn grace(first: Duration) -> Duration {
    (first * 2).max(Duration::from_secs(2))
}

/// Time for more starts once a fill exists. In WebAssembly the starts run one at a time, so a
/// stuck one would hold up the rest: give each no more than `grace` of the first fill's time.
fn more_time(round: Duration, first: Option<f32>) -> Duration {
    match first {
        Some(f) if crate::par::max_threads() == 1 => round.min(grace(Duration::from_secs_f32(f))),
        _ => round,
    }
}

/// Fill `p` from several random starts at once (one after another in WebAssembly). Starts still
/// stuck a short while after the first fill stop early (`grace`).
fn attempt(p: &Puzzle, o: &Options, timeout: Duration, variants: &[u64]) -> Result<Attempt> {
    let t0 = Instant::now();
    let results: Vec<Result<engine::FillReport>> = if crate::par::max_threads() == 1 || variants.len() == 1 {
        // One start at a time: give each a short try first, so one stuck start does not hold up
        // the others; only when none fills, give the stuck ones the full time.
        let short = timeout.min(Duration::from_secs(3));
        let mut out: Vec<Result<engine::FillReport>> = vec![];
        let mut first: Option<Duration> = None;
        for &v in variants {
            let t = match first {
                Some(f) => grace(f).min(short),
                None => short,
            };
            let r = engine::fill_variant(p, o, t, v);
            if first.is_none() && r.as_ref().is_ok_and(|r| matches!(r.status, FillStatus::Filled)) {
                first = Some(t0.elapsed());
            }
            out.push(r);
        }
        if first.is_none() && short < timeout {
            for (i, &v) in variants.iter().enumerate() {
                if !out[i].as_ref().is_ok_and(|r| matches!(r.status, FillStatus::Timeout)) {
                    continue;
                }
                let t = match first {
                    Some(f) => grace(f),
                    None => timeout,
                }
                .min(timeout.saturating_sub(t0.elapsed()).max(Duration::from_millis(500)));
                let r = engine::fill_variant(p, o, t, v);
                if first.is_none() && r.as_ref().is_ok_and(|r| matches!(r.status, FillStatus::Filled)) {
                    first = Some(t0.elapsed());
                }
                out[i] = r;
            }
        }
        out
    } else {
        attempt_threads(p, o, timeout, variants, t0)
    };
    let mut a = Attempt { fills: vec![], heat: None, impossible: true };
    for r in results {
        let r = r?;
        match r.status {
            FillStatus::Filled => a.fills.extend(r.grid),
            FillStatus::Timeout => a.impossible = false,
            FillStatus::Impossible => {}
        }
        if a.heat.is_none() {
            a.heat = r.hotspots;
        }
    }
    if !a.fills.is_empty() {
        a.impossible = false;
    }
    Ok(a)
}

/// The starts on threads, with a watcher that stops the stuck ones `grace` after the first fill.
#[cfg(not(target_arch = "wasm32"))]
fn attempt_threads(p: &Puzzle, o: &Options, timeout: Duration, variants: &[u64], t0: Instant) -> Vec<Result<engine::FillReport>> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let stop = Arc::new(AtomicBool::new(false));
    let first: Mutex<Option<Duration>> = Mutex::new(None);
    let running = AtomicUsize::new(variants.len());
    std::thread::scope(|s| {
        s.spawn(|| {
            while running.load(Ordering::Relaxed) > 0 {
                if let Some(f) = *first.lock().unwrap()
                    && t0.elapsed() >= f + grace(f)
                {
                    stop.store(true, Ordering::Relaxed);
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        parallel(variants, |&v| {
            let r = engine::fill_variant_until(p, o, timeout, v, Some(stop.clone()));
            if r.as_ref().is_ok_and(|r| matches!(r.status, FillStatus::Filled)) {
                first.lock().unwrap().get_or_insert(t0.elapsed());
            }
            running.fetch_sub(1, Ordering::Relaxed);
            r
        })
    })
}

#[cfg(target_arch = "wasm32")]
fn attempt_threads(p: &Puzzle, o: &Options, timeout: Duration, variants: &[u64], _: Instant) -> Vec<Result<engine::FillReport>> {
    parallel(variants, |&v| engine::fill_variant(p, o, timeout, v))
}

fn best_of(p: &Puzzle, open: &HashSet<String>, fills: &[Vec<String>]) -> Option<Scored> {
    let mut best: Option<Scored> = None;
    for f in fills {
        let s = score(p, open, f);
        if best.as_ref().is_none_or(|b| s.better_than(b)) {
            best = Some(s);
        }
    }
    best
}

/// Slot key -> keys of the open answers that cross it.
fn crossings(p: &Puzzle, open: &HashSet<String>) -> HashMap<String, Vec<String>> {
    let slots = p.slots();
    let mut by_cell: HashMap<(usize, usize), Vec<String>> = HashMap::new();
    for s in &slots {
        for c in s.cells() {
            by_cell.entry(c).or_default().push(s.key.clone());
        }
    }
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for s in &slots {
        let mut v: Vec<String> = s.cells().iter().flat_map(|c| by_cell[c].iter()).filter(|k| **k != s.key && open.contains(*k)).cloned().collect();
        v.sort();
        v.dedup();
        out.insert(s.key.clone(), v);
    }
    out
}

/// `grid` with the squares of these answers emptied, where `p` had them empty.
fn clear_answers(p: &Puzzle, grid: &[String], keys: &BTreeSet<String>) -> Puzzle {
    let mut q = p.clone();
    q.grid = grid.to_vec();
    let mut rows: Vec<Vec<char>> = q.grid.iter().map(|r| r.chars().collect()).collect();
    for s in p.slots() {
        if keys.contains(&s.key) {
            for (x, y) in s.cells() {
                if p.cell(x, y) == '.' {
                    rows[y][x] = '.';
                }
            }
        }
    }
    q.grid = rows.into_iter().map(|r| r.into_iter().collect()).collect();
    q
}

// ---------------------------------------------------------------- autofill

pub fn autofill(p: &Puzzle, o: &Options, ao: &AutofillOptions) -> Result<AutofillReport> {
    let start = Instant::now();
    let deadline = start + ao.seconds;
    let left = || deadline.saturating_duration_since(Instant::now());
    let open = open_keys(p);
    let mut steps: Vec<String> = vec![];
    let strict = Options { min_score: ao.relax_to.min(o.min_score), quality_floor: Some(GOOD), relaxed: None, ..o.clone() };
    let round = |frac: f32| left().mul_f32(frac).clamp(Duration::from_secs(2), Duration::from_secs(20)).min(left());
    let n = ao.starts.max(1) as u64;
    let seeds = |from: u64| -> Vec<u64> { (0..n).map(|i| ao.seed_base + from + i).collect() };
    let variants = seeds(0);

    let quick = ao.first_only;
    // 1. Clean fill. With `first_only`, one start at a time, and stop at the first fill.
    let a = if quick {
        let mut a = Attempt { fills: vec![], heat: None, impossible: true };
        for &v in &variants {
            let b = attempt(p, &strict, round(0.2), &[v])?;
            a.impossible &= b.impossible;
            a.heat = a.heat.or(b.heat);
            if !b.fills.is_empty() || b.impossible {
                a.fills = b.fills;
                break;
            }
        }
        a.impossible &= a.fills.is_empty();
        a
    } else {
        attempt(p, &strict, round(0.2), &variants)?
    };
    let mut hotspots = a.heat;
    let mut best = best_of(p, &open, &a.fills);
    let mut first_fill_seconds = best.as_ref().map(|_| start.elapsed().as_secs_f32());
    // True when the last fill tried was proven impossible (not just out of time).
    let mut impossible = a.impossible;
    if best.is_some() {
        steps.push("Clean fill: found one with only good words.".into());
        if !quick && left() > ao.seconds / 2 {
            let more = attempt(p, &strict, more_time(round(0.25), first_fill_seconds), &seeds(n))?;
            if let Some(m) = best_of(p, &open, &more.fills)
                && m.better_than(best.as_ref().unwrap())
            {
                best = Some(m);
            }
        }
    } else {
        steps.push(format!("Clean fill: {}.", if a.impossible { "none exists with only good words" } else { "none found in time" }));
        // 2. Concentrate the weak words where the search got stuck.
        let cross = crossings(p, &open);
        let mut relaxed: HashSet<String> = HashSet::new();
        if let Some(h) = &hotspots {
            let max = h.slots.first().map(|s| s.heat).unwrap_or(0.0);
            relaxed = h.slots.iter().filter(|s| open.contains(&s.key) && s.heat >= max * 0.35).take(6).map(|s| s.key.clone()).collect();
        }
        if relaxed.is_empty() {
            relaxed = open.clone();
        }
        let labels: HashMap<String, String> = p.slots().iter().map(|s| (s.key.clone(), label(s))).collect();
        let mut grows = 0;
        while left() > Duration::from_secs(1) {
            let o2 = Options { relaxed: Some(Arc::new(relaxed.clone())), ..strict.clone() };
            let a = attempt(p, &o2, round(0.2), &variants)?;
            impossible = a.impossible;
            let names = if relaxed.len() == open.len() {
                "every answer".to_string()
            } else {
                let mut l: Vec<&str> = relaxed.iter().filter_map(|k| labels.get(k).map(|s| s.as_str())).collect();
                l.sort();
                format!("{} answers ({})", relaxed.len(), l.iter().take(8).cloned().collect::<Vec<_>>().join(", ") + if l.len() > 8 { ", …" } else { "" })
            };
            if let Some(mut b) = best_of(p, &open, &a.fills) {
                first_fill_seconds = Some(start.elapsed().as_secs_f32());
                // More random starts with the same area, while there is time: fills differ a lot.
                let mut extra = 0;
                while !quick && left() > ao.seconds / 2 && extra < 3 {
                    let more = attempt(p, &o2, more_time(round(0.1), first_fill_seconds), &seeds(10 + extra * n))?;
                    if let Some(m) = best_of(p, &open, &more.fills)
                        && m.better_than(&b)
                    {
                        b = m;
                    }
                    extra += 1;
                }
                steps.push(format!("Weaker words allowed in {names}: filled ({} weak).", b.weak.len()));
                best = Some(b);
                break;
            }
            steps.push(format!("Weaker words allowed in {names}: {}.", if a.impossible { "no fill" } else { "no fill in time" }));
            if relaxed.len() == open.len() {
                break;
            }
            // Grow toward where this search got stuck. If it points nowhere new, add the
            // crossings. After many rounds, give up on keeping the weak words together.
            grows += 1;
            let mut grown = relaxed.clone();
            if let Some(h) = &a.heat {
                let max = h.slots.iter().filter(|s| open.contains(&s.key) && !relaxed.contains(&s.key)).map(|s| s.heat).fold(0.0, f32::max);
                grown.extend(h.slots.iter().filter(|s| open.contains(&s.key) && !relaxed.contains(&s.key) && s.heat >= max * 0.5).take(4).map(|s| s.key.clone()));
            }
            if grown.len() == relaxed.len() {
                grown.extend(relaxed.iter().flat_map(|k| cross.get(k).into_iter().flatten().cloned()));
            }
            relaxed = if grown.len() == relaxed.len() || grows >= 12 { open.clone() } else { grown };
        }
    }

    // 3. Polish weak words, worst first.
    if let Some(mut b) = best.clone().filter(|_| !quick) {
        let cross = crossings(p, &open);
        let reserve = if ao.try_blocks { ao.seconds.mul_f32(0.3) } else { Duration::ZERO };
        let mut tried: HashSet<String> = HashSet::new();
        let mut fixed = 0;
        let mut fixed_at_pass = 0;
        while left() > reserve + Duration::from_secs(1) {
            let Some(w) = b.weak.iter().find(|w| !tried.contains(&w.2)).cloned() else {
                // Another pass if the last one changed the grid: words that were stuck may move now.
                if fixed > fixed_at_pass {
                    fixed_at_pass = fixed;
                    tried.clear();
                    continue;
                }
                break;
            };
            tried.insert(w.2.clone());
            let near: BTreeSet<String> = std::iter::once(w.0.clone()).chain(cross.get(&w.0).into_iter().flatten().cloned()).collect();
            let wide: BTreeSet<String> = near.iter().flat_map(|k| cross.get(k).into_iter().flatten().cloned()).chain(near.iter().cloned()).collect();
            let avoid = Some(Arc::new(HashSet::from([words::norm(&w.2)])));
            let t = Duration::from_secs(3).min(left());
            let mut improved = false;
            for (region, loose) in [(&near, false), (&near, true), (&wide, false), (&wide, true)] {
                if left() <= reserve {
                    break;
                }
                let q = clear_answers(p, &b.grid, region);
                let o2 = Options {
                    avoid: avoid.clone(),
                    relaxed: loose.then(|| Arc::new(region.iter().cloned().collect::<HashSet<_>>())),
                    ..strict.clone()
                };
                let a = attempt(&q, &o2, t, &[0, 1])?;
                if let Some(c) = best_of(p, &open, &a.fills)
                    && c.better_than(&b)
                {
                    b = c;
                    improved = true;
                    break;
                }
            }
            // Then a box of squares around the word: frees nearby answers in both directions
            // without clearing all of a long crossing answer.
            if !improved {
                let cells: Vec<(usize, usize)> = p.slots().into_iter().find(|s| s.key == w.0).map(|s| s.cells()).unwrap_or_default();
                for pad in [1, 2, 3] {
                    if left() <= reserve {
                        break;
                    }
                    let q = clear_near(p, p, &b.grid, cells.iter(), pad);
                    let o2 = Options { avoid: avoid.clone(), ..strict.clone() };
                    let a = attempt(&q, &o2, t, &[0, 1])?;
                    if let Some(c) = best_of(p, &open, &a.fills)
                        && c.better_than(&b)
                    {
                        b = c;
                        improved = true;
                        break;
                    }
                }
            }
            if improved {
                fixed += 1;
            }
        }
        if !tried.is_empty() {
            steps.push(format!("Polish: tried {} weak word(s), improved the grid {fixed} time(s).", tried.len()));
        }
        best = Some(b);
    }

    // 4. Black-square fixes, tested where the weak words are: add a pair, clear the squares near
    //    it and the weak words of one area, and refill only that part with good words.
    let mut block_fixes: Vec<BlockFix> = vec![];
    let needs_fix = best.as_ref().is_none_or(|b| !b.weak.is_empty());
    if ao.try_blocks && !quick && needs_fix && left() > Duration::from_secs(2) {
        let slots: HashMap<String, SlotInfo> = p.slots().into_iter().map(|s| (s.key.clone(), s)).collect();
        // Groups of squares to work on: each area's weak answers, or the hotspots.
        let groups: Vec<Vec<(usize, usize)>> = match &best {
            Some(b) => {
                let cross = crossings(p, &open);
                let area = areas(&b.weak, &cross);
                let mut g: Vec<Vec<(usize, usize)>> = vec![vec![]; area.values().max().map_or(0, |m| m + 1)];
                for w in &b.weak {
                    if let Some(s) = slots.get(&w.0) {
                        g[area[&w.0]].extend(s.cells());
                    }
                }
                g.into_iter().take(4).collect()
            }
            None => vec![hotspots.iter().flat_map(|h| h.cells.iter().take(16).map(|c| (c.x, c.y))).collect()],
        };
        let (w, h) = (p.width() as i64, p.height() as i64);
        let mut seen = HashSet::new();
        let mut trials: Vec<(Vec<(usize, usize)>, Vec<(usize, usize)>, Puzzle)> = vec![];
        for g in &groups {
            let mut tried_here = 0;
            for &(x, y) in g {
                for (dx, dy) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= w || ny >= h || tried_here >= 8 {
                        continue;
                    }
                    let (nx, ny) = (nx as usize, ny as usize);
                    if p.cell(nx, ny) != '.' {
                        continue;
                    }
                    // The square plus any squares needed to avoid 1- and 2-letter answers.
                    let Some((t, sq)) = add_block_closing(p, nx, ny, 6) else { continue };
                    if !seen.insert(sq.clone()) || !grid_problems(&t).is_empty() {
                        continue;
                    }
                    trials.push((sq, g.clone(), t));
                    tried_here += 1;
                }
            }
        }
        let n = trials.len();
        let per = (left().as_secs_f32() / ((n as f32 / crate::par::max_threads() as f32).ceil().max(1.0))).clamp(1.0, 6.0);
        let current = best.as_ref().map(|b| (b.weak.len(), b.avg));
        let results = parallel(&trials, |(sq, group, t)| -> Result<Option<BlockFix>> {
            if left() < Duration::from_millis(500) {
                return Ok(None);
            }
            let q = match &best {
                Some(b) => clear_near(p, t, &b.grid, group.iter().chain(sq.iter()), 2),
                None => t.clone(),
            };
            let r = engine::fill(&q, &strict, Duration::from_secs_f32(per).min(left()))?;
            Ok(r.grid.map(|g| {
                let s = score(t, &open_keys(t), &g);
                BlockFix { squares: sq.clone(), weak: s.weak.len(), score: s.avg, weak_words: s.weak.iter().map(|w| w.2.clone()).collect(), fill: g }
            }))
        });
        for r in results {
            if let Some(f) = r?
                && current.is_none_or(|(weak, avg)| f.weak < weak || (f.weak == weak && f.score > avg + 0.3))
            {
                block_fixes.push(f);
            }
        }
        block_fixes.sort_by(|a, b| a.weak.cmp(&b.weak).then(b.score.total_cmp(&a.score)));
        block_fixes.truncate(3);
        steps.push(match block_fixes.len() {
            0 => format!("Black squares: none of {n} pairs near the weak words helped."),
            k => format!("Black squares: {k} of {n} pairs near the weak words help."),
        });
    }

    // 5. Areas and debut patterns for the weak words left.
    // Reported only with `first_only`, so the command line's JSON stays as it was.
    let first_fill_seconds = first_fill_seconds.filter(|_| ao.first_only);
    let seconds = start.elapsed().as_secs_f32();
    let Some(b) = best else {
        let status = if impossible { FillStatus::Impossible } else { FillStatus::Timeout };
        return Ok(AutofillReport {
            status,
            clean: false,
            grid: None,
            score: None,
            new_answers: 0,
            weak: vec![],
            needs: vec![],
            block_fixes,
            steps,
            seconds,
            first_fill_seconds,
            hotspots,
        });
    };
    hotspots = None;
    let cross = crossings(p, &open);
    let area = areas(&b.weak, &cross);
    let mut filled = p.clone();
    filled.grid = b.grid.clone();
    let slots: HashMap<String, SlotInfo> = filled.slots().into_iter().map(|s| (s.key.clone(), s)).collect();
    let needs = b
        .weak
        .iter()
        .filter_map(|w| {
            let s = slots.get(&w.0)?;
            let pattern: String = s
                .cells()
                .iter()
                .map(|&(x, y)| {
                    if p.cell(x, y) != '.' { filled.cell(x, y) } else { '.' }
                })
                .collect();
            Some(Need { label: w.1.clone(), word: w.2.clone(), good_fits: good_fits(&pattern, &w.2), pattern })
        })
        .collect();
    let weak = b.weak.iter().map(|w| WeakWord { key: w.0.clone(), label: w.1.clone(), word: w.2.clone(), quality: w.3, flags: w.4.clone(), area: area[&w.0] }).collect();
    Ok(AutofillReport {
        status: FillStatus::Filled,
        clean: b.weak.is_empty(),
        score: Some(b.avg),
        new_answers: b.count,
        grid: Some(b.grid),
        weak,
        needs,
        block_fixes,
        steps,
        seconds,
        first_fill_seconds,
        hotspots,
    })
}

/// Good words in the word lists (and liked words) that match a pattern, other than `except`.
/// Weak answers that cross each other share an area number (0, 1, …), worst area first.
fn areas(weak: &[(String, String, String, i32, Vec<Flag>)], cross: &HashMap<String, Vec<String>>) -> HashMap<String, usize> {
    let keys: HashSet<&String> = weak.iter().map(|w| &w.0).collect();
    let mut area: HashMap<String, usize> = HashMap::new();
    let mut next = 0;
    for w in weak {
        if area.contains_key(&w.0) {
            continue;
        }
        let mut stack = vec![w.0.clone()];
        while let Some(k) = stack.pop() {
            if area.insert(k.clone(), next).is_some() {
                continue;
            }
            for c in cross.get(&k).into_iter().flatten() {
                if keys.contains(c) && !area.contains_key(c) {
                    stack.push(c.clone());
                }
            }
        }
        next += 1;
    }
    area
}

/// `t` (the grid with new black squares) filled from `grid`, except the squares within `pad` of
/// `near`, which are emptied where `p` had them empty.
fn clear_near<'a>(p: &Puzzle, t: &Puzzle, grid: &[String], near: impl Iterator<Item = &'a (usize, usize)>, pad: usize) -> Puzzle {
    let near: Vec<(usize, usize)> = near.cloned().collect();
    let close = |x: usize, y: usize| near.iter().any(|&(a, b)| a.abs_diff(x) <= pad && b.abs_diff(y) <= pad);
    let rows: Vec<Vec<char>> = grid.iter().map(|r| r.chars().collect()).collect();
    let mut q = t.clone();
    q.grid = (0..t.height())
        .map(|y| {
            (0..t.width())
                .map(|x| {
                    let c = t.cell(x, y);
                    if c != '.' || p.cell(x, y) != '.' {
                        c
                    } else if close(x, y) {
                        '.'
                    } else {
                        rows[y][x]
                    }
                })
                .collect()
        })
        .collect();
    q
}

fn good_fits(pattern: &str, except: &str) -> usize {
    let n = pattern.chars().count();
    let fits = |w: &str| w.len() == n && w != except && w.chars().zip(pattern.chars()).all(|(a, b)| b == '.' || a == b);
    let mut count = 0;
    for line in engine::default_wordlist().lines() {
        let Some((w, s)) = line.split_once(';') else { continue };
        let w = words::norm(w);
        if fits(&w) && words::judge(&w, s.trim().parse().ok()).good() {
            count += 1;
        }
    }
    count + words::choices().liked.iter().filter(|w| fits(w)).count()
}

pub fn render(r: &AutofillReport) -> String {
    let mut out = String::new();
    for s in &r.steps {
        out += &format!("- {s}\n");
    }
    match &r.grid {
        None => {
            out += &format!("\nNo fill ({:?}) in {:.0}s.\n", r.status, r.seconds);
            if let Some(h) = &r.hotspots {
                let l: Vec<&str> = h.slots.iter().take(6).map(|s| s.label.as_str()).collect();
                out += &format!("Stuck around: {}\n", l.join(", "));
            }
        }
        Some(g) => {
            out += "\n";
            for row in g {
                out += row;
                out += "\n";
            }
            out += &format!(
                "\n{} new answers, average quality {:.1}. {}\n",
                r.new_answers,
                r.score.unwrap_or(0.0),
                if r.clean { "Every new answer is a good word.".to_string() } else { format!("{} weak:", r.weak.len()) }
            );
            for w in &r.weak {
                let flags: Vec<&str> = w.flags.iter().map(|f| f.name()).collect();
                out += &format!("  area {}  {:<5} {:<15} {:>4}  {}\n", w.area + 1, w.label, w.word, w.quality, flags.join(", "));
            }
            if !r.needs.is_empty() {
                out += "\nWeak spots (letters fixed before the fill; '.' = open):\n";
                for n in &r.needs {
                    out += &format!(
                        "  {:<5} {:<15} {}  {}\n",
                        n.label,
                        n.word,
                        n.pattern,
                        if n.good_fits == 0 { "no good word fits these letters: needs a debut or a black-square change".to_string() } else { format!("{} good words fit; the crossings around it are the problem", n.good_fits) }
                    );
                }
            }
        }
    }
    if !r.block_fixes.is_empty() {
        out += "\nBlack-square changes that help:\n";
        for (i, f) in r.block_fixes.iter().enumerate() {
            let sq: Vec<String> = f.squares.iter().map(|(x, y)| format!("{x},{y}")).collect();
            out += &format!(
                "  {}. add {}: {}, average {:.1}{}\n",
                i + 1,
                sq.join(" and "),
                if f.weak == 0 { "clean fill".to_string() } else { format!("{} weak", f.weak) },
                f.score,
                if f.weak_words.is_empty() { String::new() } else { format!(" ({})", f.weak_words.join(", ")) }
            );
        }
    }
    out += &format!("\n{:.0}s.\n", r.seconds);
    out
}

// ---------------------------------------------------------------- improve (after autofill)

/// A better fill for the area around one weak word.
#[derive(Debug, Serialize)]
pub struct Improvement {
    /// The weak word's answer key and label (`23A`).
    pub key: String,
    pub label: String,
    pub word: String,
    /// The whole grid with the better area.
    pub grid: Vec<String>,
    /// Squares whose letter changed.
    pub cells: Vec<(usize, usize)>,
    /// Average quality of the answers through the changed squares, before and after.
    pub before: f32,
    pub after: f32,
}

/// Average quality of the open answers through `cells` in `grid`.
fn area_avg(p: &Puzzle, open: &HashSet<String>, grid: &[String], cells: &HashSet<(usize, usize)>) -> f32 {
    let mut q = p.clone();
    q.grid = grid.to_vec();
    let (mut t, mut n) = (0i64, 0i64);
    for s in q.slots() {
        if !open.contains(&s.key) || !s.cells().iter().any(|c| cells.contains(c)) {
            continue;
        }
        let Some(a) = s.answer() else { continue };
        t += words::judge(&a, engine::list_score(&a).map(|v| v.min(u16::MAX as u32) as u16)).score.max(0) as i64;
        n += 1;
    }
    if n == 0 { 0.0 } else { t as f32 / n as f32 }
}

/// Keep working on autofill's result: for one weak word (the `round`-th, worst first, wrapping),
/// clear it and its crossings (then a wider area, then a box around it) and refill with new random
/// starts (`seed`). `p` is the puzzle before the fill (its letters stay); `grid` is the fill.
/// Returns a better grid when one is found within `timeout`. The web app calls it again and
/// again in the background; it never writes.
pub fn improve(p: &Puzzle, grid: &[String], o: &Options, relax_to: u16, round: usize, seed: u64, timeout: Duration) -> Result<Option<Improvement>> {
    let start = Instant::now();
    let left = || timeout.saturating_sub(start.elapsed());
    let open = open_keys(p);
    let strict = Options { min_score: relax_to.min(o.min_score), quality_floor: Some(GOOD), relaxed: None, ..o.clone() };
    let b = score(p, &open, grid);
    if b.weak.is_empty() {
        return Ok(None);
    }
    let w = b.weak[round % b.weak.len()].clone();
    let cross = crossings(p, &open);
    let near: BTreeSet<String> = std::iter::once(w.0.clone()).chain(cross.get(&w.0).into_iter().flatten().cloned()).collect();
    let wide: BTreeSet<String> = near.iter().flat_map(|k| cross.get(k).into_iter().flatten().cloned()).chain(near.iter().cloned()).collect();
    let avoid = Some(Arc::new(HashSet::from([words::norm(&w.2)])));
    let cells: Vec<(usize, usize)> = p.slots().into_iter().find(|s| s.key == w.0).map(|s| s.cells()).unwrap_or_default();
    // The same steps as autofill's polish: the word and its crossings, then wider, each first with
    // good words only and then with weak words allowed there; then boxes around the word.
    let (qn, qw) = (clear_answers(p, grid, &near), clear_answers(p, grid, &wide));
    let tries: Vec<Puzzle> = [qn.clone(), qn, qw.clone(), qw].into_iter().chain([1, 2, 3].map(|pad| clear_near(p, p, grid, cells.iter(), pad))).collect();
    let regions: Vec<Option<HashSet<String>>> = vec![None, Some(near.iter().cloned().collect()), None, Some(wide.iter().cloned().collect()), None, None, None];
    let per = Duration::from_millis(1500);
    for (q, relaxed) in tries.iter().zip(regions) {
        if left().is_zero() {
            break;
        }
        let o2 = Options { avoid: avoid.clone(), relaxed: relaxed.map(Arc::new), ..strict.clone() };
        let a = attempt(q, &o2, per.min(left()), &[seed, seed + 1])?;
        if let Some(c) = best_of(p, &open, &a.fills)
            && c.better_than(&b)
        {
            let old: Vec<Vec<char>> = grid.iter().map(|r| r.chars().collect()).collect();
            let changed: Vec<(usize, usize)> = c
                .grid
                .iter()
                .enumerate()
                .flat_map(|(y, r)| r.chars().enumerate().filter(|&(x, ch)| old[y][x] != ch).map(move |(x, _)| (x, y)).collect::<Vec<_>>())
                .collect();
            let set: HashSet<(usize, usize)> = changed.iter().cloned().chain(cells.iter().cloned()).collect();
            let (before, after) = (area_avg(p, &open, grid, &set), area_avg(p, &open, &c.grid, &set));
            return Ok(Some(Improvement { key: w.0, label: w.1, word: w.2, grid: c.grid, cells: changed, before, after }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Options {
        Options { min_score: 50, ..Default::default() }
    }

    #[test]
    fn mini_fills_clean_and_keeps_letters() {
        crate::need_list!();
        let mut p = Puzzle::blank("t", 5, 5, "rotational");
        p.grid[0] = "CRANE".into();
        let ao = AutofillOptions { seconds: Duration::from_secs(20), relax_to: 40, try_blocks: false, first_only: false, starts: 4, seed_base: 0 };
        let r = autofill(&p, &opts(), &ao).unwrap();
        let g = r.grid.expect("a 5x5 with one word fills");
        assert_eq!(g[0], "CRANE");
        assert!(g.iter().all(|row| !row.contains('.')));
        assert!(r.clean, "weak: {:?}", r.weak);
        assert_eq!(r.new_answers, 9);
    }

    #[test]
    fn avoided_words_stay_out() {
        crate::need_list!();
        let mut p = Puzzle::blank("t", 5, 5, "rotational");
        p.grid[0] = "CRANE".into();
        let first = engine::fill(&p, &opts(), Duration::from_secs(10)).unwrap().grid.unwrap();
        let word: String = first[1].clone();
        let o = Options { avoid: Some(Arc::new(HashSet::from([word.clone()]))), ..opts() };
        let again = engine::fill(&p, &o, Duration::from_secs(10)).unwrap().grid.unwrap();
        assert!(again.iter().all(|row| *row != word));
        let cols: Vec<String> = (0..5).map(|x| again.iter().map(|r| r.as_bytes()[x] as char).collect()).collect();
        assert!(!cols.contains(&word));
    }

    #[test]
    fn clear_near_empties_only_new_squares() {
        let p = Puzzle::blank("t", 5, 5, "none");
        let mut q = p.clone();
        q.grid[0] = "CRANE".into();
        let grid: Vec<String> = vec!["ABCDE".into(); 5];
        let c = clear_near(&q, &q, &grid, [(0, 4)].iter(), 1);
        assert_eq!(c.grid[0], "CRANE");
        assert_eq!(c.grid[3], "..CDE");
        assert_eq!(c.grid[2], "ABCDE");
    }

    #[test]
    fn improve_fixes_a_bad_area_and_nothing_else() {
        crate::need_list!();
        let mut p = Puzzle::blank("t", 5, 5, "rotational");
        p.grid[0] = "CRANE".into();
        let mut g = engine::fill(&p, &opts(), Duration::from_secs(10)).unwrap().grid.unwrap();
        g[4] = "ZZZZZ".into();
        let imp = improve(&p, &g, &opts(), 40, 0, 1000, Duration::from_secs(10)).unwrap().expect("a better fill");
        assert!(imp.after > imp.before);
        assert_eq!(imp.grid[0], "CRANE");
        assert_ne!(imp.grid[4], "ZZZZZ");
        for (y, row) in imp.grid.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                assert_eq!(c != g[y].as_bytes()[x] as char, imp.cells.contains(&(x, y)));
            }
        }
    }

    #[test]
    fn typed_answers_are_never_weak_or_cleared() {
        crate::need_list!();
        let mut p = Puzzle::blank("t", 5, 5, "none");
        p.grid[0] = "QZXJV".into();
        let open = open_keys(&p);
        let g: Vec<String> = vec!["QZXJV".into(), "ZZZZZ".into(), "ZZZZZ".into(), "ZZZZZ".into(), "ZZZZZ".into()];
        let s = score(&p, &open, &g);
        assert!(s.weak.iter().all(|w| w.2 != "QZXJV"), "a typed answer is never weak");
        let q = clear_answers(&p, &g, &open.iter().cloned().collect());
        assert_eq!(q.grid[0], "QZXJV");
        assert_eq!(clear_near(&p, &p, &g, [(2, 0)].iter(), 3).grid[0], "QZXJV");
    }
}
