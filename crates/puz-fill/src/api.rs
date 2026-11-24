//! The web API's answers, as JSON. A server reads the puzzle file and
//! calls these; a WebAssembly build gets the puzzle from the page and calls the same
//! functions. So both give back the same JSON.

use crate::engine::{self, Options};
use crate::livework::Snapshot;
use crate::profile::Profile;
use crate::puzzle::Puzzle;
use crate::{autofill, collisions, live, print, stats, submission, submit, suggest, words};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

/// A short hash of a puzzle file's text. The page sends it back on save, so the server can tell
/// when the file changed in between.
pub fn rev_of(text: &str) -> String {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// A puzzle from its file text, tidied and checked, with its rev.
pub fn parse_puzzle(text: &str) -> Result<(Puzzle, String)> {
    // ipuz (how files are saved now) or the older puz JSON.
    let mut p = Puzzle::from_json(text)?;
    p.tidy();
    p.validate()?;
    Ok((p, rev_of(text)))
}

/// A puzzle as the page gets it: with its path, rev and answers.
pub fn puzzle_payload(path: &str, p: &Puzzle, rev: &str) -> Value {
    json!({ "path": path, "rev": rev, "puzzle": p, "slots": p.slots() })
}

/// `GET /api/live`: a live check's snapshot, with the rev and the seconds since the check started.
pub fn live(snap: &Snapshot, rev: &str, elapsed: f32) -> Result<Value> {
    let mut v = serde_json::to_value(snap)?;
    v["rev"] = json!(rev);
    v["elapsed"] = json!(elapsed);
    Ok(v)
}

/// `GET /api/spots`: every answer a word fits, and whether the grid still fills with it there.
/// `per` is seconds per spot (default 5, at most 30).
pub fn spots(p: &Puzzle, rev: &str, o: &Options, word: &str, per: Option<u64>) -> Result<Value> {
    let per = per.unwrap_or(5).min(30);
    let (word, spots) = live::find_spots(p, o, word, Duration::from_secs(per))?;
    Ok(json!({ "rev": rev, "word": word, "spots": spots }))
}

/// `GET /api/stats`: grid stats and checks against a paper's rules (`rules` = "", auto or a preset id).
pub fn stats(p: &Puzzle, rev: &str, rules: &str) -> Result<Value> {
    let mut v = serde_json::to_value(stats::report(p, rules)?)?;
    v["rev"] = json!(rev);
    v["crosswordese"] = serde_json::to_value(crate::crosswordese::report(p))?;
    v["green_paint"] = serde_json::to_value(crate::green_paint::report(p))?;
    Ok(v)
}

/// `GET /api/check`: crossings, options per answer and hotspots.
pub fn check(p: &Puzzle, rev: &str, o: &Options) -> Result<Value> {
    Ok(json!({ "rev": rev, "report": engine::check(p, o)? }))
}

/// `GET /api/candidates`: the words that fit one answer (`slot`: 17a, 5d or x,y,across), best first.
pub fn candidates(p: &Puzzle, o: &Options, slot: &str) -> Result<Value> {
    let (label, words, total) = engine::candidates(p, o, slot, 200)?;
    Ok(json!({ "slot": label, "total": total, "words": words }))
}

/// `POST /api/fill` settings: seconds (default 60) and which random start (default 0).
#[derive(Deserialize, Default)]
pub struct FillParams {
    pub timeout: Option<u64>,
    pub variant: Option<u64>,
}

/// `POST /api/fill`: the raw engine fill.
pub fn fill(p: &Puzzle, rev: &str, o: &Options, f: &FillParams) -> Result<Value> {
    Ok(json!({ "rev": rev, "report": engine::fill_variant(p, o, Duration::from_secs(f.timeout.unwrap_or(60)), f.variant.unwrap_or(0))? }))
}

/// `POST /api/autofill` settings: seconds (default 45, 5 to 300), the weakest score to allow in
/// stuck spots (default 40), and whether to try black squares (default yes).
#[derive(Deserialize, Default)]
pub struct AutofillParams {
    pub seconds: Option<u64>,
    pub relax_to: Option<u16>,
    pub blocks: Option<bool>,
    /// Stop at the first complete fill (`AutofillOptions::first_only`). Default no.
    pub first: Option<bool>,
    /// Random starts per round (default 4, 1 to 8) and the first start's number (default 0).
    pub starts: Option<usize>,
    pub seed_base: Option<u64>,
}

/// `POST /api/autofill`: fill with good words, then work on the weak spots. Writes nothing.
pub fn autofill(p: &Puzzle, rev: &str, o: &Options, a: &AutofillParams) -> Result<Value> {
    let ao = autofill::AutofillOptions {
        seconds: Duration::from_secs(a.seconds.unwrap_or(45).clamp(5, 300)),
        relax_to: a.relax_to.unwrap_or(40),
        try_blocks: a.blocks.unwrap_or(true),
        first_only: a.first.unwrap_or(false),
        starts: a.starts.unwrap_or(4).clamp(1, 8),
        seed_base: a.seed_base.unwrap_or(0),
    };
    Ok(json!({ "rev": rev, "report": autofill::autofill(p, o, &ao)? }))
}

/// The browser's background improve step after autofill: `{fill, round?, seed?, seconds?, relax_to?}`.
#[derive(Deserialize, Default)]
pub struct ImproveParams {
    /// The grid before autofill (its letters stay). Default: the puzzle's own grid.
    pub base: Option<Vec<String>>,
    pub fill: Vec<String>,
    pub round: Option<usize>,
    pub seed: Option<u64>,
    pub seconds: Option<f32>,
    pub relax_to: Option<u16>,
}

/// One improve step: a better fill near one weak word, or `improvement: null`. Writes nothing.
pub fn improve(p: &Puzzle, rev: &str, o: &Options, a: &ImproveParams) -> Result<Value> {
    let t = Duration::from_secs_f32(a.seconds.unwrap_or(8.0).clamp(1.0, 60.0));
    let mut p = p.clone();
    if let Some(b) = &a.base {
        anyhow::ensure!(b.len() == p.grid.len(), "bad request: base has the wrong size");
        p.grid = b.clone();
    }
    anyhow::ensure!(a.fill.len() == p.grid.len(), "bad request: fill has the wrong size");
    let r = autofill::improve(&p, &a.fill, o, a.relax_to.unwrap_or(40), a.round.unwrap_or(0), a.seed.unwrap_or(1000), t)?;
    Ok(json!({ "rev": rev, "improvement": r }))
}

/// `POST /api/suggest` settings: seconds for the first fill try (default 10) and for each black
/// square try (default 5), and how many squares to try (default 16).
#[derive(Deserialize, Default)]
pub struct SuggestParams {
    pub probe_timeout: Option<u64>,
    pub trial_timeout: Option<u64>,
    pub max_squares: Option<usize>,
}

/// `POST /api/suggest`: black squares that make the grid fill.
pub fn suggest(p: &Puzzle, rev: &str, o: &Options, s: &SuggestParams) -> Result<Value> {
    let so = suggest::SuggestOptions {
        probe_timeout: Duration::from_secs(s.probe_timeout.unwrap_or(10)),
        trial_timeout: Duration::from_secs(s.trial_timeout.unwrap_or(5)),
        max_squares: s.max_squares.unwrap_or(16),
    };
    Ok(json!({ "rev": rev, "report": suggest::suggest(p, o, &so)? }))
}

/// `GET /api/words/check`: word quality for a comma-separated list.
pub fn words_check(list: &str) -> Value {
    let rows: Vec<words::WordReport> = list.split(',').map(str::trim).filter(|w| !w.is_empty()).map(words::report).collect();
    json!(rows)
}

/// A resize to work out. The page sends its own copy of the puzzle (it may have edits not yet
/// saved), gets the resized puzzle back and saves it like any edit, so undo works.
#[derive(Deserialize)]
pub struct ResizeBody {
    pub puzzle: Puzzle,
    pub size: Option<String>,
    pub anchor: Option<String>,
    #[serde(flatten)]
    pub edges: crate::edit::Edges,
}

/// `POST /api/resize`: the resized puzzle, its answers and what changed. It saves nothing.
pub fn resize(b: ResizeBody) -> Result<Value> {
    let mut p = b.puzzle;
    p.tidy();
    p.validate()?;
    let e = crate::edit::plan_resize(&p, b.size.as_deref(), b.anchor.as_deref(), b.edges)?;
    let report = crate::edit::resize(&mut p, e)?;
    Ok(json!({ "puzzle": p, "slots": p.slots(), "report": report }))
}

/// A file to download: its name, its media type and its bytes.
pub type File = (String, &'static str, Vec<u8>);

/// `GET /api/export`: the puzzle as `format` (jpz, puz, json, png, pdf, or pdf-preview: a PNG of
/// the PDF page). `answers` fills in the grid (png and pdf). `scale` is for png (default 2).
/// Fill in an empty author first if you have one (`profile::default_author`).
pub fn export(p: &Puzzle, format: &str, answers: bool, scale: Option<f32>) -> Result<File> {
    let stem = p.title.replace(['"', '/', '\\'], "");
    let stem = if stem.trim().is_empty() { "puzzle".to_string() } else { stem };
    let suffix = if answers { " (answers)" } else { "" };
    // A picture of the PDF page, for the export dialog's preview.
    if format == "pdf-preview" {
        return Ok((format!("{stem}{suffix}.png"), "image/png", print::page_png(p, print::PrintOptions { answers }, 1.0)?));
    }
    let e = crate::export::export(p, format, answers, scale.unwrap_or(2.0))?;
    let suffix = if matches!(format, "png" | "pdf") { suffix } else { "" };
    Ok((format!("{stem}{suffix}.{format}"), e.mime, e.bytes))
}

/// `GET /api/submit`: the NYT form answers and checks, with the rev and the PDF's page count.
/// `file_stem` is the puzzle file's name without `.json`.
pub fn submission(p: &Puzzle, rev: &str, file_stem: &str, kind: Option<&str>, blank: bool, prof: &Profile, look: submission::Look) -> Result<Value> {
    let kind = match kind.filter(|k| !k.is_empty()) {
        Some(k) => Some(submit::Kind::parse(k).ok_or_else(|| anyhow!("unknown type {k:?} (use themed, themeless or sunday)"))?),
        None => None,
    };
    let mut v = serde_json::to_value(submit::nyt(p, file_stem, kind, prof))?;
    v["rev"] = json!(rev);
    v["pages"] = json!(submission::submission_svgs(p, &submission::SubmissionOptions { look, ..submission::options(p, prof, blank) }).len());
    Ok(v)
}

/// `GET /api/submit/pdf` (with `page` None) and `/api/submit/page` (a PNG of one page, from 1).
pub fn submission_file(p: &Puzzle, file_stem: &str, blank: bool, prof: &Profile, look: submission::Look, page: Option<usize>) -> Result<File> {
    // The name the NYT asks for: LastName_PuzzleTitle.pdf.
    let name = submission::nyt_file_name(p, &prof.me.name, &submit::safe_file_name("", file_stem).trim_end_matches(".pdf").to_string());
    let o = submission::SubmissionOptions { look, ..submission::options(p, prof, blank) };
    if let Some(page) = page {
        return Ok((name.replace(".pdf", ".png"), "image/png", submission::submission_png(p, &o, page, 1.0)?));
    }
    Ok((name, "application/pdf", submission::submission_pdf(p, &o)?))
}

/// `GET /api/collisions`: answers that repeat across puzzles, given as (path, title, puzzle).
/// The puzzles are taken in path order.
pub fn collisions(folder: &str, recursive: bool, min_len: Option<usize>, mut puzzles: Vec<(String, String, Puzzle)>) -> Result<Value> {
    puzzles.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(serde_json::to_value(collisions::find(folder.trim_matches('/'), recursive, min_len.unwrap_or(3), &puzzles))?)
}
