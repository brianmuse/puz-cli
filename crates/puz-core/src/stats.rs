//! Grid stats (answer counts, black squares, unchecked squares, interlock, symmetry, cheaters,
//! fill progress) and checks against newspaper submission rules. The `stats` command calls `report`.

use crate::edit;
use crate::puzzle::{Dir, Puzzle, SlotInfo, BOTTOM, RIGHT};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

// ---------------------------------------------------------------- stats

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub width: usize,
    pub height: usize,
    pub answers: AnswerCounts,
    pub white: usize,
    pub blocks: usize,
    /// Black squares as a percent of all squares.
    pub block_pct: f64,
    /// Average answer length, in squares.
    pub avg_len: f64,
    /// How many answers have each length, shortest first.
    pub by_length: Vec<LenCount>,
    pub three_letter: usize,
    pub longest_len: usize,
    pub longest: Vec<AnswerRef>,
    pub two_letter: Vec<AnswerRef>,
    /// White squares in fewer than two answers (one answer, or none).
    pub unchecked: Vec<[usize; 2]>,
    /// Separate groups of white squares linked by answers. 1 means all-over interlock.
    pub parts: usize,
    /// The size (in squares) of each part, largest first.
    pub part_sizes: Vec<usize>,
    /// The symmetries the black squares and walls have: "rotational", "rotational-90", "mirror"
    /// (left/right), "mirror-up-down", "diagonal", "anti-diagonal". Empty means none.
    pub symmetry_found: Vec<String>,
    /// The puzzle's `symmetry` setting.
    pub symmetry_setting: String,
    /// The grid has the symmetry the setting asks for.
    pub symmetry_matches: bool,
    /// Black squares that don't change the word count: making one white only makes an across
    /// answer and a down answer one longer.
    pub cheaters: Vec<[usize; 2]>,
    pub fill: FillProgress,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnswerCounts {
    pub across: usize,
    pub down: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct LenCount {
    pub len: usize,
    pub count: usize,
    /// The answers of this length, across first.
    pub answers: Vec<AnswerRef>,
}

/// An answer, by label (9A) and slot key (x,y,across).
#[derive(Debug, Clone, Serialize)]
pub struct AnswerRef {
    pub label: String,
    pub key: String,
    pub len: usize,
    pub pattern: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FillProgress {
    /// White squares with a letter.
    pub filled: usize,
    pub white: usize,
    pub pct: f64,
    /// Answers with every square filled.
    pub complete_answers: usize,
    pub clued: usize,
    pub answers: usize,
}

fn answer_ref(s: &SlotInfo) -> AnswerRef {
    AnswerRef { label: edit::label(s), key: s.key.clone(), len: s.len, pattern: s.pattern.clone() }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// How many answers cover each square.
fn coverage(p: &Puzzle, slots: &[SlotInfo]) -> Vec<Vec<usize>> {
    let mut cov = vec![vec![0; p.width()]; p.height()];
    for s in slots {
        for (x, y) in s.cells() {
            cov[y][x] += 1;
        }
    }
    cov
}

/// Groups of white squares that answers link together. Squares in no answer are their own group.
fn parts(p: &Puzzle, slots: &[SlotInfo]) -> Vec<usize> {
    let (w, h) = (p.width(), p.height());
    let mut parent: Vec<usize> = (0..w * h).collect();
    fn find(parent: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while parent[r] != r {
            r = parent[r];
        }
        let mut i = i;
        while parent[i] != r {
            let n = parent[i];
            parent[i] = r;
            i = n;
        }
        r
    }
    for s in slots {
        let cells = s.cells();
        let a = cells[0].1 * w + cells[0].0;
        for &(x, y) in &cells[1..] {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, y * w + x));
            parent[rb] = ra;
        }
    }
    let mut sizes: BTreeMap<usize, usize> = BTreeMap::new();
    for y in 0..h {
        for x in 0..w {
            if p.cell(x, y) != '#' {
                *sizes.entry(find(&mut parent, y * w + x)).or_default() += 1;
            }
        }
    }
    let mut out: Vec<usize> = sizes.into_values().collect();
    out.sort_unstable_by(|a, b| b.cmp(a));
    out
}

/// The symmetries the black squares and walls have.
pub fn symmetries(p: &Puzzle) -> Vec<String> {
    let (w, h) = (p.width(), p.height());
    // A wall is the pair of squares it sits between, smaller first.
    let walls: HashSet<[(usize, usize); 2]> = p
        .bars
        .iter()
        .filter_map(|(k, sides)| crate::puzzle::parse_square_key(k).map(|q| (q, sides)))
        .flat_map(|((x, y), sides)| {
            sides.iter().filter_map(move |s| match s.as_str() {
                RIGHT => Some([(x, y), (x + 1, y)]),
                BOTTOM => Some([(x, y), (x, y + 1)]),
                _ => None,
            })
        })
        .collect();
    let holds = |f: &dyn Fn(usize, usize) -> (usize, usize)| {
        let blocks_ok = (0..h).all(|y| (0..w).all(|x| {
            let (mx, my) = f(x, y);
            (p.cell(x, y) == '#') == (p.cell(mx, my) == '#')
        }));
        blocks_ok
            && walls.iter().all(|[a, b]| {
                let (ma, mb) = (f(a.0, a.1), f(b.0, b.1));
                walls.contains(&[ma.min(mb), ma.max(mb)])
            })
    };
    let mut out = vec![];
    for name in ["rotational", "rotational-90", "mirror", "mirror-up-down", "diagonal", "anti-diagonal"] {
        if crate::blocks::image(name, w, h, 0, 0).is_some() && holds(&|x, y| crate::blocks::image(name, w, h, x, y).unwrap()) {
            out.push(name.to_string());
        }
    }
    out
}

/// Black squares that don't change the word count: making one white only makes one across and
/// one down answer one square longer. (The square is at the end of both, and no answers join.)
fn cheaters(p: &Puzzle, old: &[SlotInfo]) -> Vec<[usize; 2]> {
    let old: HashSet<(usize, usize, Dir, usize)> = old.iter().map(|s| (s.x, s.y, s.dir, s.len)).collect();
    let mut out = vec![];
    for y in 0..p.height() {
        for x in 0..p.width() {
            if p.cell(x, y) != '#' {
                continue;
            }
            let mut q = p.clone();
            let mut row: Vec<char> = q.grid[y].chars().collect();
            row[x] = '.';
            q.grid[y] = row.into_iter().collect();
            let slots = q.slots();
            if slots.len() != old.len() {
                continue;
            }
            let mine: Vec<&SlotInfo> = slots.iter().filter(|s| s.cells().contains(&(x, y))).collect();
            let extends = |s: &SlotInfo| {
                let (first, last) = (s.cells()[0], s.cells()[s.len - 1]);
                let (nx, ny) = if s.dir == Dir::Across { (s.x + 1, s.y) } else { (s.x, s.y + 1) };
                (first == (x, y) && old.contains(&(nx, ny, s.dir, s.len - 1))) || (last == (x, y) && old.contains(&(s.x, s.y, s.dir, s.len - 1)))
            };
            if mine.len() == 2 && mine[0].dir != mine[1].dir && mine.iter().all(|s| extends(s)) {
                out.push([x, y]);
            }
        }
    }
    out
}

pub fn stats(p: &Puzzle) -> Stats {
    let (w, h) = (p.width(), p.height());
    let slots = p.slots();
    let across = slots.iter().filter(|s| s.dir == Dir::Across).count();
    let total = slots.len();
    let blocks = p.grid.iter().map(|r| r.chars().filter(|&c| c == '#').count()).sum::<usize>();
    let white = w * h - blocks;
    let mut by_len: BTreeMap<usize, usize> = BTreeMap::new();
    for s in &slots {
        *by_len.entry(s.len).or_default() += 1;
    }
    let longest_len = slots.iter().map(|s| s.len).max().unwrap_or(0);
    let cov = coverage(p, &slots);
    let unchecked = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .filter(|&(x, y)| p.cell(x, y) != '#' && cov[y][x] < 2)
        .map(|(x, y)| [x, y])
        .collect();
    let part_sizes = parts(p, &slots);
    let found = symmetries(p);
    let setting = if p.symmetry == "left-right" { "mirror".to_string() } else { p.symmetry.clone() };
    let filled = p.grid.iter().map(|r| r.chars().filter(|&c| c != '#' && c != '.').count()).sum::<usize>();
    let complete = slots.iter().filter(|s| !s.pattern.contains('.')).count();
    let clued = slots.iter().filter(|s| p.clue_for(s).is_some()).count();
    Stats {
        width: w,
        height: h,
        answers: AnswerCounts { across, down: total - across, total },
        white,
        blocks,
        block_pct: round1(blocks as f64 * 100.0 / (w * h) as f64),
        avg_len: if total == 0 { 0.0 } else { round1(slots.iter().map(|s| s.len).sum::<usize>() as f64 / total as f64) },
        by_length: by_len
            .iter()
            .map(|(&len, &count)| LenCount { len, count, answers: slots.iter().filter(|s| s.len == len).map(answer_ref).collect() })
            .collect(),
        three_letter: by_len.get(&3).copied().unwrap_or(0),
        longest_len,
        longest: slots.iter().filter(|s| s.len == longest_len).map(answer_ref).collect(),
        two_letter: slots.iter().filter(|s| s.len == 2).map(answer_ref).collect(),
        unchecked,
        parts: part_sizes.len(),
        part_sizes,
        symmetry_matches: setting == "none" || found.contains(&setting),
        symmetry_found: found,
        symmetry_setting: setting,
        cheaters: cheaters(p, &slots),
        fill: FillProgress {
            filled,
            white,
            pct: if white == 0 { 0.0 } else { round1(filled as f64 * 100.0 / white as f64) },
            complete_answers: complete,
            clued,
            answers: total,
        },
    }
}

// ---------------------------------------------------------------- rule presets

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Pass,
    Warn,
    Fail,
}

/// How many black squares a paper allows. Always a soft limit (a warning).
#[derive(Debug, Clone, Copy)]
pub enum BlockLimit {
    /// At most this share of all squares, e.g. 1/6.
    Share(usize, usize),
    /// Fewer than this many.
    Under(usize),
}

/// One paper's rules. Every preset is a common guideline, not the paper's official spec sheet.
#[derive(Debug, Clone)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub source: &'static str,
    /// The grid size the paper expects. Another size warns.
    pub size: Option<(usize, usize)>,
    /// Most answers for a themed puzzle.
    pub max_answers: Option<usize>,
    /// Most answers for a themeless puzzle.
    pub max_answers_themeless: Option<usize>,
    /// What an answer under 3 letters gets.
    pub short: Level,
    /// What an unchecked square gets.
    pub unchecked: Level,
    /// What a grid in separate parts gets.
    pub interlock: Level,
    /// Rotational symmetry required. Mirror symmetry gets `mirror`, no symmetry fails.
    pub rotational: Option<Level>,
    pub max_blocks: Option<BlockLimit>,
}

const NOTE: &str = "Common guideline. Check the paper's current spec sheet before you submit.";

/// Every preset. `auto` picks one from the puzzle's `target`, then from the grid size.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "nyt-daily",
        name: "NYT daily (15x15)",
        source: "NYT submission guidelines as quoted by constructor guides: 78 answers themed, 72 themeless, 3+ letters, all squares checked, all-over interlock, rotational symmetry (mirror for theme reasons), black squares about 1/6 of the grid at most.",
        size: Some((15, 15)),
        max_answers: Some(78),
        max_answers_themeless: Some(72),
        short: Level::Fail,
        unchecked: Level::Fail,
        interlock: Level::Fail,
        rotational: Some(Level::Warn),
        max_blocks: Some(BlockLimit::Share(1, 6)),
    },
    Preset {
        id: "nyt-sunday",
        name: "NYT Sunday (21x21)",
        source: "NYT submission guidelines as quoted by constructor guides: 140 answers at most, otherwise the same as the daily.",
        size: Some((21, 21)),
        max_answers: Some(140),
        max_answers_themeless: None,
        short: Level::Fail,
        unchecked: Level::Fail,
        interlock: Level::Fail,
        rotational: Some(Level::Warn),
        max_blocks: Some(BlockLimit::Share(1, 6)),
    },
    Preset {
        id: "nyt-mini",
        name: "NYT Mini (5x5)",
        source: "Light conventions seen in published minis: 3+ letters and all squares checked are usual, symmetry is not required. The Mini has no public spec sheet.",
        size: Some((5, 5)),
        max_answers: None,
        max_answers_themeless: None,
        short: Level::Warn,
        unchecked: Level::Warn,
        interlock: Level::Fail,
        rotational: None,
        max_blocks: None,
    },
    Preset {
        id: "lat-daily",
        name: "LA Times daily (15x15)",
        source: "LA Times submission guidelines (2022) as quoted by constructor guides: 78 answers themed, 72 themeless, under 43 black squares preferred, rotational symmetry, standard American rules.",
        size: Some((15, 15)),
        max_answers: Some(78),
        max_answers_themeless: Some(72),
        short: Level::Fail,
        unchecked: Level::Fail,
        interlock: Level::Fail,
        rotational: Some(Level::Warn),
        max_blocks: Some(BlockLimit::Under(43)),
    },
    Preset {
        id: "wsj-daily",
        name: "WSJ daily (15x15)",
        source: "WSJ guidelines as quoted by constructor guides: 78 answers for a 15x15, rotational symmetry, standard American rules.",
        size: Some((15, 15)),
        max_answers: Some(78),
        max_answers_themeless: None,
        short: Level::Fail,
        unchecked: Level::Fail,
        interlock: Level::Fail,
        rotational: Some(Level::Warn),
        max_blocks: None,
    },
    Preset {
        id: "basic",
        name: "American-style basics",
        source: "The rules most American papers share: 3+ letters, all squares checked, all-over interlock, symmetric black squares.",
        size: None,
        max_answers: None,
        max_answers_themeless: None,
        short: Level::Fail,
        unchecked: Level::Warn,
        interlock: Level::Fail,
        rotational: Some(Level::Pass),
        max_blocks: None,
    },
];

/// The preset for `rules` ("auto", "" or a preset id). Returns the preset and whether auto picked it.
pub fn preset_for(p: &Puzzle, rules: &str) -> Result<(&'static Preset, bool)> {
    let rules = rules.trim();
    if rules.is_empty() || rules == "auto" {
        if let Some(t) = p.target.as_deref().filter(|t| !t.is_empty() && *t != "auto") {
            return Ok((find_preset(t)?, false));
        }
        let id = match (p.width(), p.height()) {
            (5, 5) => "nyt-mini",
            (15, 15) => "nyt-daily",
            (21, 21) => "nyt-sunday",
            _ => "basic",
        };
        return Ok((find_preset(id)?, true));
    }
    Ok((find_preset(rules)?, false))
}

pub fn find_preset(id: &str) -> Result<&'static Preset> {
    match PRESETS.iter().find(|r| r.id == id) {
        Some(r) => Ok(r),
        None => bail!("unknown rules {id:?}. Use auto or one of: {}", preset_ids().join(", ")),
    }
}

/// The `target` value to store for a `--target` argument: None for "auto" or "".
pub fn parse_target(t: &str) -> Result<Option<String>> {
    let t = t.trim();
    if t.is_empty() || t == "auto" {
        return Ok(None);
    }
    find_preset(t)?;
    Ok(Some(t.to_string()))
}

pub fn preset_ids() -> Vec<&'static str> {
    PRESETS.iter().map(|r| r.id).collect()
}

/// One rule check.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub label: String,
    pub level: Level,
    pub detail: String,
    /// Squares at fault, as [x, y].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub squares: Vec<[usize; 2]>,
    /// Answers at fault.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<AnswerRef>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RulesReport {
    pub id: &'static str,
    pub name: &'static str,
    pub source: &'static str,
    pub note: &'static str,
    /// The preset came from `auto` and the grid size.
    pub auto: bool,
    /// Most answers for a themed puzzle, and for a themeless one.
    pub max_answers: Option<usize>,
    pub max_answers_themeless: Option<usize>,
    pub checks: Vec<Check>,
    pub pass: usize,
    pub warn: usize,
    pub fail: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub stats: Stats,
    pub rules: RulesReport,
    /// Every preset, for a picker: [id, name].
    pub presets: Vec<[&'static str; 2]>,
}

fn check(id: &'static str, label: &str, level: Level, detail: String) -> Check {
    Check { id, label: label.into(), level, detail, squares: vec![], answers: vec![] }
}

fn squares_text(sq: &[[usize; 2]]) -> String {
    let shown: Vec<String> = sq.iter().take(8).map(|[x, y]| format!("{x},{y}")).collect();
    let more = if sq.len() > 8 { format!(" and {} more", sq.len() - 8) } else { String::new() };
    format!("{}{more}", shown.join(" "))
}

pub fn check_rules(st: &Stats, r: &Preset) -> Vec<Check> {
    let mut out = vec![];
    if let Some((w, h)) = r.size {
        let ok = (st.width, st.height) == (w, h);
        out.push(check(
            "size",
            "Grid size",
            if ok { Level::Pass } else { Level::Warn },
            if ok { format!("{w}x{h}") } else { format!("{}x{}; this paper expects {w}x{h}", st.width, st.height) },
        ));
    }
    if let Some(max) = r.max_answers {
        let n = st.answers.total;
        let (level, detail) = match r.max_answers_themeless {
            Some(tl) if n <= tl => (Level::Pass, format!("{n} of {tl} (themeless max)")),
            Some(tl) if n <= max => (Level::Pass, format!("{n} of {max}. Fine for a themed puzzle; a themeless one allows {tl}")),
            _ if n <= max => (Level::Pass, format!("{n} of {max}")),
            _ => (Level::Fail, format!("{n}; the max is {max}")),
        };
        out.push(check("answers", "Answer count", level, detail));
    }
    let short = &st.two_letter;
    let mut c = check(
        "short",
        "Answers are 3+ letters",
        if short.is_empty() { Level::Pass } else { r.short },
        if short.is_empty() { "None under 3".into() } else { format!("{} two-letter answer{}", short.len(), if short.len() == 1 { "" } else { "s" }) },
    );
    c.answers = short.clone();
    out.push(c);
    let mut c = check(
        "checked",
        "Every square is checked",
        if st.unchecked.is_empty() { Level::Pass } else { r.unchecked },
        if st.unchecked.is_empty() { "Yes".into() } else { format!("{} unchecked square{}", st.unchecked.len(), if st.unchecked.len() == 1 { "" } else { "s" }) },
    );
    c.squares = st.unchecked.clone();
    out.push(c);
    out.push(check(
        "interlock",
        "All-over interlock",
        if st.parts <= 1 { Level::Pass } else { r.interlock },
        if st.parts <= 1 { "Yes".into() } else { format!("{} separate parts ({})", st.parts, st.part_sizes.iter().map(|n| format!("{n} squares")).collect::<Vec<_>>().join(", ")) },
    ));
    if let Some(mirror) = r.rotational {
        let found = &st.symmetry_found;
        let (level, detail) = if found.iter().any(|s| s == "rotational") {
            (Level::Pass, "Rotational".to_string())
        } else if !found.is_empty() {
            let what = if found.iter().any(|s| s == "mirror") { "Mirror (left/right)" } else { "Mirror (up/down)" };
            (mirror, format!("{what} only. Rotational is the norm; mirror is for theme reasons"))
        } else {
            (Level::Fail, "None. The black squares need rotational symmetry".to_string())
        };
        out.push(check("symmetry", "Symmetry", level, detail));
    }
    if let Some(limit) = r.max_blocks {
        let n = st.blocks;
        let (ok, detail) = match limit {
            BlockLimit::Share(a, b) => {
                let max = st.width * st.height * a / b;
                (n <= max, format!("{n} ({}%), about {max} max ({a}/{b})", st.block_pct))
            }
            BlockLimit::Under(m) => (n < m, format!("{n}; under {m} preferred")),
        };
        out.push(check("blocks", "Black squares", if ok { Level::Pass } else { Level::Warn }, detail));
    }
    out
}

/// Stats and rule checks for a puzzle. `rules` is "auto" or a preset id.
pub fn report(p: &Puzzle, rules: &str) -> Result<Report> {
    let st = stats(p);
    let (preset, auto) = preset_for(p, rules)?;
    let checks = check_rules(&st, preset);
    let count = |l: Level| checks.iter().filter(|c| c.level == l).count();
    Ok(Report {
        rules: RulesReport {
            id: preset.id,
            name: preset.name,
            source: preset.source,
            note: NOTE,
            auto,
            max_answers: preset.max_answers,
            max_answers_themeless: preset.max_answers_themeless,
            pass: count(Level::Pass),
            warn: count(Level::Warn),
            fail: count(Level::Fail),
            checks,
        },
        stats: st,
        presets: PRESETS.iter().map(|r| [r.id, r.name]).collect(),
    })
}

/// The report as plain text, for the CLI.
pub fn render(r: &Report) -> String {
    let s = &r.stats;
    let mut o = vec![];
    o.push(format!("Size            {}x{}", s.width, s.height));
    o.push(format!("Answers         {} ({} across, {} down)", s.answers.total, s.answers.across, s.answers.down));
    o.push(format!("Black squares   {} ({}%)", s.blocks, s.block_pct));
    o.push(format!("Average length  {}", s.avg_len));
    o.push(format!("3-letter        {}", s.three_letter));
    o.push(format!("Longest         {} letters: {}", s.longest_len, s.longest.iter().map(|a| a.label.as_str()).collect::<Vec<_>>().join(", ")));
    o.push(format!("By length       {}", s.by_length.iter().map(|l| format!("{}:{}", l.len, l.count)).collect::<Vec<_>>().join("  ")));
    o.push(format!("2-letter        {}", if s.two_letter.is_empty() { "none".into() } else { s.two_letter.iter().map(|a| a.label.as_str()).collect::<Vec<_>>().join(", ") }));
    o.push(format!("Unchecked       {}", if s.unchecked.is_empty() { "none".into() } else { format!("{}: {}", s.unchecked.len(), squares_text(&s.unchecked)) }));
    o.push(format!("Interlock       {}", if s.parts <= 1 { "yes".to_string() } else { format!("no, {} parts", s.parts) }));
    let found = if s.symmetry_found.is_empty() { "none".to_string() } else { s.symmetry_found.join(", ") };
    o.push(format!("Symmetry        {found} (setting: {}{})", s.symmetry_setting, if s.symmetry_matches { "" } else { ", not met" }));
    o.push(format!("Cheaters        {}", if s.cheaters.is_empty() { "none".into() } else { format!("{}: {}", s.cheaters.len(), squares_text(&s.cheaters)) }));
    o.push(format!("Filled          {}/{} squares ({}%), {}/{} answers complete, {}/{} clued", s.fill.filled, s.fill.white, s.fill.pct, s.fill.complete_answers, s.fill.answers, s.fill.clued, s.fill.answers));
    let rr = &r.rules;
    o.push(String::new());
    o.push(format!("Rules: {}{} ({} pass, {} warn, {} fail)", rr.name, if rr.auto { ", picked from the grid size" } else { "" }, rr.pass, rr.warn, rr.fail));
    for c in &rr.checks {
        let mark = match c.level {
            Level::Pass => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        let who = if !c.squares.is_empty() {
            format!(": {}", squares_text(&c.squares))
        } else if !c.answers.is_empty() {
            format!(": {}", c.answers.iter().map(|a| a.label.as_str()).collect::<Vec<_>>().join(", "))
        } else {
            String::new()
        };
        o.push(format!("  {mark}  {:<24} {}{who}", c.label, c.detail));
    }
    o.push(format!("Source: {}", rr.source));
    o.push(rr.note.to_string());
    o.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&str], symmetry: &str) -> Puzzle {
        let mut p = Puzzle::blank("t", rows[0].len(), rows.len(), symmetry);
        p.grid = rows.iter().map(|r| r.to_string()).collect();
        p
    }

    #[test]
    fn counts_answers_and_lengths() {
        let p = grid(&["#....", ".....", ".....", ".....", "....#"], "rotational");
        let s = stats(&p);
        assert_eq!((s.answers.across, s.answers.down, s.answers.total), (5, 5, 10));
        assert_eq!(s.blocks, 2);
        assert_eq!(s.by_length.iter().map(|l| (l.len, l.count)).collect::<Vec<_>>(), vec![(4, 4), (5, 6)]);
        assert_eq!(s.longest_len, 5);
        assert!(s.unchecked.is_empty());
        assert_eq!(s.parts, 1);
        assert!(s.symmetry_found.contains(&"rotational".to_string()));
        assert!(!s.symmetry_found.contains(&"mirror".to_string()));
        assert!(s.symmetry_matches);
    }

    #[test]
    fn walls_and_unchecked_squares() {
        let mut p = grid(&["...", "...", "..."], "none");
        // A wall under 0,0 leaves 0,0 in the across answer only.
        p.bars.insert("0,0".into(), vec![BOTTOM.into()]);
        let s = stats(&p);
        assert_eq!(s.unchecked, vec![[0, 0]]);
        assert_eq!(s.answers.total, 6); // 3 across, and down: 0,1-0,2 (2 letters), columns 1 and 2
        assert_eq!(s.two_letter.len(), 1);
    }

    #[test]
    fn interlock_parts() {
        let p = grid(&["...#...", "...#...", "...#..."], "mirror");
        let s = stats(&p);
        assert_eq!(s.parts, 2);
        assert_eq!(s.part_sizes, vec![9, 9]);
        assert!(s.symmetry_found.contains(&"mirror".to_string()));
        assert!(s.symmetry_found.contains(&"rotational".to_string()));
        assert!(s.symmetry_found.contains(&"mirror-up-down".to_string()));
    }

    #[test]
    fn symmetry_none_and_mismatch() {
        let p = grid(&[".#...", ".....", ".....", ".....", "....."], "rotational");
        let s = stats(&p);
        assert!(s.symmetry_found.is_empty());
        assert!(!s.symmetry_matches);
    }

    #[test]
    fn cheater_squares() {
        // A lone corner block is a cheater: making it white only makes 1A and 1D one longer.
        let p = grid(&["#....", ".....", ".....", ".....", "....#"], "rotational");
        assert_eq!(stats(&p).cheaters, vec![[0, 0], [4, 4]]);
        // A block between two answers is not: making it white joins them, so the count drops.
        let q = grid(&[".......", ".......", "...#...", ".......", "......."], "rotational");
        assert!(stats(&q).cheaters.is_empty());
    }

    #[test]
    fn nyt_daily_preset() {
        let mut p = Puzzle::blank("t", 15, 15, "rotational");
        let r = report(&p, "auto").unwrap();
        assert_eq!(r.rules.id, "nyt-daily");
        assert!(r.rules.auto);
        // An open 15x15 has 30 answers and no black squares: everything passes.
        assert_eq!(r.rules.fail, 0, "{:?}", r.rules.checks);
        // A black square at 1,0 makes a 1-letter run (unchecked 0,0) and breaks symmetry.
        p.grid[0] = ".#.............".into();
        let r = report(&p, "nyt-daily").unwrap();
        let level = |id: &str| r.rules.checks.iter().find(|c| c.id == id).unwrap().level;
        assert_eq!(level("checked"), Level::Fail);
        assert_eq!(level("symmetry"), Level::Fail);
        assert_eq!(level("answers"), Level::Pass);
        // A target in the file wins over the size.
        p.target = Some("lat-daily".into());
        assert_eq!(report(&p, "auto").unwrap().rules.id, "lat-daily");
        assert!(report(&p, "nope").is_err());
    }
}
