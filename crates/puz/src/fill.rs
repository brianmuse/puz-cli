//! The fill commands: fill, autofill, checks, candidates, patterns and word quality, on one puzzle
//! file, with the puz-fill engine. They need a word list (`puz wordlist`).

use crate::wordlist;
use crate::Ctx;
use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use puz_core::edit;
use puz_core::puzzle::Puzzle;
use puz_fill::engine::{self, FillStatus, Options};
use puz_fill::{autofill, live, pattern, suggest, words};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Subcommand)]
pub enum Cmd {
    /// Count possible words per slot, taking crossings into account (fast, no search)
    Check {
        puzzle: String,
        /// Show slots with at most this many options (0 = all)
        #[arg(long, default_value_t = 5)]
        threshold: usize,
        #[command(flatten)]
        engine: EngineArgs,
    },
    /// Is the grid fillable? Checks crossings, then searches for one full fill (writes nothing).
    Status {
        puzzle: String,
        /// Seconds to search before giving up
        #[arg(long, default_value_t = 60)]
        timeout: u64,
        #[command(flatten)]
        engine: EngineArgs,
    },
    /// Check which words in a slot can be part of a full fill (viable) and which can't (dead).
    /// Each word is tried on its own; every fill found settles its other words too.
    Viable {
        puzzle: String,
        slot: String,
        /// Check this many words, best first
        #[arg(long, default_value_t = 30)]
        limit: usize,
        /// Seconds to spend on each word
        #[arg(long, default_value_t = 3)]
        per_word: u64,
        /// Seconds for the first full search
        #[arg(long, default_value_t = 60)]
        timeout: u64,
        /// Also show one full fill for each viable word (nothing is written)
        #[arg(long)]
        fills: bool,
        #[command(flatten)]
        engine: EngineArgs,
    },
    /// Find every answer a word fits, and check the grid still fills with it there
    FindSpot {
        puzzle: String,
        word: String,
        /// Seconds to spend on each spot
        #[arg(long, default_value_t = 5)]
        per_spot: u64,
        #[command(flatten)]
        engine: EngineArgs,
    },
    /// Crosswordese in a puzzle: complete 3-5 letter answers that published puzzles use all the
    /// time, and whether there are more than usual (over 6, or 17% of the answers). --list prints
    /// the whole word list.
    Crosswordese {
        #[arg(default_value = "")]
        puzzle: String,
        #[arg(long)]
        list: bool,
        #[arg(long)]
        json: bool,
    },
    /// Possible green paint in a puzzle: complete multiword answers that the word list does not
    /// vouch for and that split into 2 or 3 common words (GREENPAINT, REDCAR). A first pass only.
    GreenPaint {
        puzzle: String,
        #[arg(long)]
        json: bool,
    },
    /// Answers that repeat across puzzles. PATHS are puzzle files or folders of puzzles (default:
    /// the current folder). Answers with an empty square don't count.
    Collisions {
        paths: Vec<PathBuf>,
        /// Include puzzles in subfolders
        #[arg(long)]
        recursive: bool,
        /// Shortest answer to count
        #[arg(long, default_value_t = 3)]
        min_len: usize,
        #[arg(long)]
        json: bool,
    },
    /// List forced letters (empty squares with only one possible letter after the crossing check)
    /// and forced words (answers with only one word left). --write fills them in.
    Forced {
        puzzle: String,
        /// Write the forced letters: all of them, or only one answer's (e.g. 17A) or one square's (x,y)
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        write: Option<String>,
        #[command(flatten)]
        engine: EngineArgs,
    },
    /// List words that fit a slot (e.g. 41a, 17d, or 5,7,across)
    Candidates {
        puzzle: String,
        slot: String,
        #[arg(long, default_value_t = 30)]
        limit: usize,
        #[command(flatten)]
        engine: EngineArgs,
    },
    /// Fill the empty squares. On failure, report hotspots.
    Fill {
        puzzle: String,
        /// Seconds before giving up
        #[arg(long, default_value_t = 60)]
        timeout: u64,
        /// Write the fill back into the puzzle file
        #[arg(long)]
        write: bool,
        /// Try another fill: each number starts the search from a different random point.
        /// 0 gives the same fill as the live check.
        #[arg(long, default_value_t = 0)]
        variant: u64,
        #[command(flatten)]
        engine: EngineArgs,
    },
    /// Fill the empty squares with good words (better than `fill`). If no clean fill exists,
    /// weaker words go only where the grid is stuck. Then each weak word is refilled with it left
    /// out, and the report lists black-square changes and debut spots (letter patterns where a
    /// good new word would help) for what is left. Letters already in the grid stay. Uses your
    /// banned and liked words (`puz words`).
    Autofill {
        puzzle: String,
        /// Total seconds
        #[arg(long, default_value_t = 60)]
        seconds: u64,
        /// Lowest word-list score allowed where the grid needs weaker words
        #[arg(long, default_value_t = 40)]
        relax_to: u16,
        /// Don't test black-square changes
        #[arg(long)]
        no_blocks: bool,
        /// Write the fill into the puzzle file
        #[arg(long)]
        write: bool,
        #[command(flatten)]
        engine: EngineArgs,
    },
    /// Your word choices, kept in words.json beside your puz profile. Banned words never go in new fill. Liked words (good
    /// debuts, favorites) may always go in, at the top score. With no action, list them.
    Words {
        #[command(subcommand)]
        action: Option<WordsCmd>,
        /// Print machine-readable JSON
        #[arg(long, global = true)]
        json: bool,
    },
    /// Suggest black squares that make an unfillable grid fill
    SuggestBlocks {
        puzzle: String,
        /// Seconds for the first fill attempt that finds hotspots
        #[arg(long, default_value_t = 10)]
        probe_timeout: u64,
        /// Seconds for each trial fill
        #[arg(long, default_value_t = 5)]
        trial_timeout: u64,
        /// How many hotspot squares to try
        #[arg(long, default_value_t = 16)]
        max_squares: usize,
        /// Apply suggestion N (1 = best) to the file
        #[arg(long)]
        apply: Option<usize>,
        #[command(flatten)]
        engine: EngineArgs,
    },
    /// Build black-square patterns around the answers in the grid and rank the ones that fill.
    /// Letters stay where they are, and complete answers keep their length. Add theme answers with
    /// --entry. Existing black squares are cleared first unless --keep-blocks.
    Pattern {
        puzzle: String,
        /// Black squares to aim for (default: about 1/6 of the grid at 11x11 and up)
        #[arg(long)]
        blocks: Option<usize>,
        /// Most answers allowed (default: the puzzle's rules, e.g. 78 for nyt-daily, 140 for nyt-sunday)
        #[arg(long)]
        max_words: Option<usize>,
        /// Longest answer allowed, except kept answers (default: no limit)
        #[arg(long)]
        max_len: Option<usize>,
        /// Place an answer first: x,y,across=WORD or x,y,down=WORD (0,0 = top left). Repeat for more.
        #[arg(long = "entry")]
        entries: Vec<String>,
        /// Keep the black squares already in the grid and add to them
        #[arg(long)]
        keep_blocks: bool,
        /// Total seconds for the search, including fill tests
        #[arg(long, default_value_t = 45)]
        seconds: u64,
        /// Seconds for each pattern's fill test
        #[arg(long, default_value_t = 8)]
        fill_timeout: u64,
        /// How many patterns to show
        #[arg(long, default_value_t = 5)]
        count: usize,
        /// Another seed gives other patterns
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Save pattern N (1 = best, the default) to the file: black squares and entries, not the fill
        #[arg(long, num_args = 0..=1, default_missing_value = "1")]
        write: Option<usize>,
        #[command(flatten)]
        engine: EngineArgs,
    },
    /// Remove black squares, a symmetric pair at a time, while the grid still fills
    TrimBlocks {
        puzzle: String,
        /// Stop at this many black squares
        #[arg(long)]
        to: usize,
        /// Longest answer a removal may make
        #[arg(long)]
        max_len: Option<usize>,
        /// Total seconds
        #[arg(long, default_value_t = 60)]
        seconds: u64,
        /// Seconds for each fill test
        #[arg(long, default_value_t = 5)]
        fill_timeout: u64,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Save the trimmed grid (black squares only, not the fill)
        #[arg(long)]
        write: bool,
        #[command(flatten)]
        engine: EngineArgs,
    },
}

#[derive(Args, Clone)]
pub struct EngineArgs {
    /// Minimum word score to allow in new fill
    #[arg(long, default_value_t = 50)]
    pub min_score: u16,
    /// Longest substring two entries may share (3-10). Off by default.
    #[arg(long)]
    pub max_shared_substring: Option<usize>,
    /// Extra word list file(s), one `word;score` per line. Can repeat.
    #[arg(long = "wordlist")]
    pub wordlists: Vec<PathBuf>,
    /// Don't load the installed word list (use only --wordlist files)
    #[arg(long)]
    pub no_default_list: bool,
    /// Download the word list without asking if it is not installed
    #[arg(short, long)]
    pub yes: bool,
    /// Print machine-readable JSON
    #[arg(long)]
    pub json: bool,
}

impl EngineArgs {
    pub fn options(&self) -> Options {
        Options {
            min_score: self.min_score,
            max_shared_substring: self.max_shared_substring,
            wordlists: self.wordlists.clone(),
            no_default_list: self.no_default_list,
            ..Default::default()
        }
    }
}

#[derive(Subcommand)]
pub enum WordsCmd {
    /// Never use these words in new fill. Quote phrases: "PET GATOR".
    Ban { words: Vec<String> },
    /// Allow these words, at the top score (adds them to the word list if missing)
    Like { words: Vec<String> },
    /// Take words off the banned and liked lists
    Forget { words: Vec<String> },
    /// Show each word's quality: word-list score, your choices
    Check { words: Vec<String> },
}

fn print_json<T: serde::Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

/// Your banned and liked words: words.json beside your puz profile.
fn words_file() -> PathBuf {
    let profile = puz_core::profile::user_file();
    profile.parent().unwrap_or(Path::new(".")).join("words.json")
}

fn load_prefs(path: &Path) -> Result<words::Prefs> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(serde_json::from_str(&s)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(words::Prefs::default()),
        Err(e) => Err(e.into()),
    }
}

/// Your word choices for the engine. puz has no puzzle history.
struct Choices(Arc<words::Choices>);
impl words::Source for Choices {
    fn choices(&self) -> Arc<words::Choices> {
        self.0.clone()
    }
    fn history_counts(&self) -> Option<&'static std::collections::HashMap<String, (u32, u32)>> {
        None
    }
}

/// The puzzles for `collisions`: files as given, and the puzzle files in folders.
fn collisions(paths: &[PathBuf], recursive: bool, min_len: usize) -> Result<puz_fill::collisions::Report> {
    fn walk(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            let hidden = p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if p.is_dir() {
                if recursive {
                    walk(&p, recursive, out)?;
                }
            } else if p.extension().is_some_and(|e| e == "json" || e == "ipuz") {
                out.push(p);
            }
        }
        Ok(())
    }
    let paths = if paths.is_empty() { vec![PathBuf::from(".")] } else { paths.to_vec() };
    let mut files = vec![];
    for p in &paths {
        if p.is_dir() {
            walk(p, recursive, &mut files)?;
        } else if p.is_file() {
            files.push(p.clone());
        } else {
            bail!("no file or folder {}", p.display());
        }
    }
    let mut puzzles = vec![];
    for f in files {
        match Puzzle::load(&f) {
            Ok(p) => puzzles.push((f.display().to_string(), p.title.clone(), p)),
            Err(_) if !paths.contains(&f) => {} // not a puzzle: skip it
            Err(e) => return Err(e),
        }
    }
    let scope = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
    Ok(puz_fill::collisions::find(&scope, recursive, min_len, &puzzles))
}

/// Does this command need the word list, and may it download without asking?
fn needs_list(cmd: &Cmd) -> Option<bool> {
    let e = match cmd {
        Cmd::Crosswordese { .. } | Cmd::Collisions { .. } => return None,
        Cmd::Words { action: Some(WordsCmd::Check { .. }), .. } | Cmd::GreenPaint { .. } => return Some(false),
        Cmd::Words { .. } => return None,
        Cmd::Check { engine, .. }
        | Cmd::Status { engine, .. }
        | Cmd::Viable { engine, .. }
        | Cmd::FindSpot { engine, .. }
        | Cmd::Forced { engine, .. }
        | Cmd::Candidates { engine, .. }
        | Cmd::Fill { engine, .. }
        | Cmd::Autofill { engine, .. }
        | Cmd::SuggestBlocks { engine, .. }
        | Cmd::Pattern { engine, .. }
        | Cmd::TrimBlocks { engine, .. } => engine,
    };
    if e.no_default_list { None } else { Some(e.yes) }
}

/// Run one fill command.
pub fn run(cmd: Cmd, ctx: &Ctx) -> Result<()> {
    words::set_source(Box::new(Choices(Arc::new(words::Choices::from_prefs(&load_prefs(&words_file()).unwrap_or_default())))));
    if let Some(yes) = needs_list(&cmd) {
        wordlist::ensure(yes)?;
    }
    match cmd {
    Cmd::Check { puzzle, threshold, engine } => {
        let (p, _) = ctx.load(&puzzle)?;
        let r = engine::check(&p, &engine.options())?;
        if engine.json {
            println!("{}", serde_json::to_string_pretty(&r)?);
            return Ok(());
        }
        if !r.consistent {
            println!("DEAD END: some slot has no possible words given its crossings.");
            if let Some(h) = &r.hotspots {
                println!("{}", engine::render_heat(&p, h));
                println!("Blamed slots: {}", h.slots.iter().take(8).map(|s| s.label.as_str()).collect::<Vec<_>>().join(", "));
            }
        }
        for n in engine::rebus_notes(&r.rebus) {
            println!("{n}");
        }
        let not_in_list: Vec<_> = r.slots.iter().filter(|s| s.in_word_list == Some(false)).collect();
        if !not_in_list.is_empty() {
            println!("Entries not in the word list: {}", not_in_list.iter().map(|s| format!("{} {}", s.label, s.pattern)).collect::<Vec<_>>().join(", "));
        }
        let mut tight: Vec<_> = r.slots.iter().filter(|s| !s.complete && (threshold == 0 || s.options <= threshold)).collect();
        tight.sort_by_key(|s| s.options);
        let open = r.slots.iter().filter(|s| !s.complete).count();
        println!("{open} open slots. {} with {} or fewer options:", tight.len(), threshold);
        for s in tight {
            println!("  {:>4} {:<16} {:>6} options ({} match the pattern)", s.label, s.pattern, s.options, s.pattern_matches);
        }
    }
    Cmd::Status { puzzle, timeout, engine } => {
        let (p, _) = ctx.load(&puzzle)?;
        let mut a = live::Analysis::new(&p, &engine.options(), Default::default())?;
        let start = std::time::Instant::now();
        let outcome = a.search(Duration::from_secs(timeout));
        let secs = start.elapsed().as_secs_f32();
        if engine.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "fillable": a.fillable, "seconds": secs, "sample_fill": a.sample_fill,
                    "hotspots": a.hotspots, "slots": a.check.slots, "rebus": a.check.rebus,
                }))?
            );
            return Ok(());
        }
        for n in engine::rebus_notes(&a.check.rebus) {
            println!("{n}");
        }
        match outcome {
            live::SearchOutcome::Filled(grid) => {
                println!("Fillable. Found a fill in {secs:.1}s:");
                for row in grid {
                    println!("  {}", row.chars().map(|c| c.to_string()).collect::<Vec<_>>().join(" "));
                }
            }
            live::SearchOutcome::Impossible(h) | live::SearchOutcome::Timeout(h) => {
                if a.fillable == live::Fillable::No {
                    println!("Not fillable ({secs:.1}s). Hotspots (9 = worst):");
                } else {
                    println!("Unknown: no fill found in {timeout}s. Where the search struggled (9 = worst):");
                }
                println!("{}", engine::render_heat(&p, &h));
                println!("Hardest answers: {}", h.slots.iter().take(8).map(|s| s.label.as_str()).collect::<Vec<_>>().join(", "));
            }
            live::SearchOutcome::Aborted => bail!("stopped"),
        }
    }
    Cmd::Viable { puzzle, slot, limit, per_word, timeout, fills, engine } => {
        let (p, _) = ctx.load(&puzzle)?;
        let info = p.find_slot(&slot)?;
        let mut a = live::Analysis::new(&p, &engine.options(), Default::default())?;
        let idx = a.slot_index(&info.key).ok_or_else(|| anyhow::anyhow!("slot {slot} not found"))?;
        a.search(Duration::from_secs(timeout));
        if a.fillable == live::Fillable::Yes {
            while let Some(w) = a.next_unsettled(idx, limit) {
                if a.settle(idx, w, Duration::from_secs(per_word)).is_none() {
                    break;
                }
            }
        }
        let (words, grids) = if fills { a.words_with_fills(idx, limit) } else { (a.words(idx, limit), vec![]) };
        let label = live::label_for(&p, &info.key);
        if engine.json {
            let mut v = serde_json::json!({ "slot": label, "fillable": a.fillable, "words": words });
            if fills {
                v["fills"] = serde_json::json!(grids);
            }
            println!("{}", serde_json::to_string_pretty(&v)?);
            return Ok(());
        }
        match a.fillable {
            live::Fillable::No => println!("{label}: the grid has no fill, so no word here works. Run `puz status PUZZLE` for hotspots."),
            live::Fillable::Timeout => println!("{label}: no fill found in {timeout}s, so words can't be checked yet."),
            _ => {
                let n = |v| words.iter().filter(|w| w.verdict == Some(v)).count();
                println!("{label}: {} viable, {} dead, {} timed out (of the top {}):", n(live::Verdict::Viable), n(live::Verdict::Dead), n(live::Verdict::Timeout), words.len());
                for w in &words {
                    let mark = match w.verdict {
                        Some(live::Verdict::Viable) => "ok  ",
                        Some(live::Verdict::Dead) => "dead",
                        Some(live::Verdict::Timeout) => "?   ",
                        None => "-   ",
                    };
                    println!("  {mark} {:<16} {}", w.word.to_uppercase(), w.score);
                    if let Some(g) = w.fill.and_then(|i| grids.get(i)) {
                        for row in g {
                            println!("         {row}");
                        }
                    }
                }
            }
        }
    }
    Cmd::FindSpot { puzzle, word, per_spot, engine } => {
        let (p, _) = ctx.load(&puzzle)?;
        let (word, spots) = live::find_spots(&p, &engine.options(), &word, Duration::from_secs(per_spot))?;
        if engine.json {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "word": word, "spots": spots }))?);
            return Ok(());
        }
        if spots.is_empty() {
            println!("{word} fits no answer: none has {} squares with matching letters.", word.len());
            return Ok(());
        }
        println!("{word} fits {} answer{}:", spots.len(), if spots.len() == 1 { "" } else { "s" });
        for s in &spots {
            let mark = match s.verdict {
                live::Verdict::Viable => "ok  ",
                live::Verdict::Dead => "dead",
                live::Verdict::Timeout => "?   ",
            };
            let note = if s.already { "  (already here)" } else { "" };
            println!("  {mark} {:<5} {}{note}", s.label, s.pattern);
        }
        println!("ok = the grid still fills. dead = it can't. ? = no answer in {per_spot}s.");
        println!("Place it with: puz place PUZZLE SLOT {word}");
    }
    Cmd::Crosswordese { puzzle, list, json } => {
        if list {
            let w = puz_fill::crosswordese::words();
            if json {
                print_json(&w)?;
            } else {
                println!("{}", w.join("\n"));
            }
        } else {
            if puzzle.is_empty() {
                anyhow::bail!("name a puzzle, or pass --list");
            }
            let (p, _) = ctx.load(&puzzle)?;
            let r = puz_fill::crosswordese::report(&p);
            if json {
                print_json(&r)?;
            } else {
                let words: Vec<String> = r.entries.iter().map(|e| e.answer.clone()).collect();
                println!("{} crosswordese answers (warn over {}): {}", r.count, r.threshold, if words.is_empty() { "none".into() } else { words.join(", ") });
                if r.over {
                    println!("More than usual. Try fresher words in a few of them.");
                }
            }
        }
    }
    Cmd::GreenPaint { puzzle, json } => {
        let (p, _) = ctx.load(&puzzle)?;
        let r = puz_fill::green_paint::report(&p);
        if json {
            print_json(&r)?;
        } else if r.entries.is_empty() {
            println!("No possible green paint.");
        } else {
            println!("{} possible green paint (phrases that may not be a thing):", r.count);
            for e in &r.entries {
                println!("  {} {} ({})", e.slot, e.answer, e.words);
            }
        }
    }
    Cmd::Collisions { paths, recursive, min_len, json } => {
        let r = collisions(&paths, recursive, min_len)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&r)?);
        } else {
            print!("{}", puz_fill::collisions::render(&r));
        }
    }
    Cmd::Forced { puzzle, write, engine } => {
        let (mut p, file) = ctx.load(&puzzle)?;
        let f = live::forced(&p, &engine.options())?;
        if let Some(target) = write {
            let n = live::write_forced(&mut p, &f, Some(target.as_str()))?;
            if n > 0 {
                p.save(&file)?;
            }
            if engine.json {
                println!("{}", serde_json::json!({ "written": n, "grid": p.grid }));
            } else {
                println!("Wrote {n} forced letter(s).");
                if n > 0 {
                    println!("{}", p.print());
                }
            }
        } else if engine.json {
            println!("{}", serde_json::to_string_pretty(&f)?);
        } else if f.squares.is_empty() {
            println!("No forced letters.");
        } else {
            println!("Forced letters ({}):", f.squares.len());
            for s in &f.squares {
                println!("  {},{}  {}", s.x, s.y, s.letter);
            }
            if !f.words.is_empty() {
                println!("Forced words:");
                for w in &f.words {
                    println!("  {:<5} {}", w.label, w.word);
                }
            }
        }
    }
    Cmd::Candidates { puzzle, slot, limit, engine } => {
        let (p, _) = ctx.load(&puzzle)?;
        let (label, words, total) = engine::candidates(&p, &engine.options(), &slot, limit)?;
        if engine.json {
            println!("{}", serde_json::json!({ "slot": label, "total": total, "words": words }));
        } else {
            println!("{label}: {total} options");
            for w in &words {
                println!("  {:<20} {}", w.word, w.score);
            }
        }
    }

    Cmd::Fill { puzzle, timeout, write, variant, engine } => {
        let (mut p, file) = ctx.load(&puzzle)?;
        let r = engine::fill_variant(&p, &engine.options(), Duration::from_secs(timeout), variant)?;
        if engine.json {
            println!("{}", serde_json::to_string_pretty(&r)?);
        } else {
            match r.status {
                FillStatus::Filled => println!("Filled in {:.2}s:\n{}", r.seconds, r.grid.as_ref().unwrap().join("\n")),
                FillStatus::Impossible => println!("No fill exists with these word lists ({:.2}s). Hotspots (9 = worst):", r.seconds),
                FillStatus::Timeout => println!("No fill found in {:.0}s. Hotspots (9 = worst):", r.seconds),
            }
            for n in engine::rebus_notes(&r.rebus) {
                println!("{n}");
            }
            if let Some(h) = &r.hotspots {
                println!("{}", engine::render_heat(&p, h));
                println!("Hardest slots: {}", h.slots.iter().take(8).map(|s| format!("{} ({:.1})", s.label, s.heat)).collect::<Vec<_>>().join(", "));
            }
        }
        if write
            && let Some(g) = r.grid
        {
            edit::set_grid(&mut p, g)?;
            p.save(&file)?;
            eprintln!("Wrote fill to {}", file.display());
        }
    }
    Cmd::Autofill { puzzle, seconds, relax_to, no_blocks, write, engine } => {
        let (mut p, file) = ctx.load(&puzzle)?;
        let ao = autofill::AutofillOptions { seconds: Duration::from_secs(seconds), relax_to, try_blocks: !no_blocks, first_only: false, starts: 4, seed_base: 0 };
        let r = autofill::autofill(&p, &engine.options(), &ao)?;
        if engine.json {
            print_json(&r)?;
        } else {
            print!("{}", autofill::render(&r));
        }
        if write
            && let Some(g) = r.grid
        {
            edit::set_grid(&mut p, g)?;
            p.save(&file)?;
            eprintln!("Wrote fill to {}", file.display());
        }
    }
    Cmd::Words { action, json } => {
        let path = words_file();
        let none: Vec<String> = vec![];
        let (ban, like, forget) = match &action {
            Some(WordsCmd::Check { words: list }) => {
                let rows: Vec<words::WordReport> = list.iter().map(|w| words::report(w)).collect();
                if json {
                    print_json(&rows)?;
                } else {
                    print!("{}", words::render_reports(&rows));
                }
                return Ok(());
            }
            Some(WordsCmd::Ban { words: l }) => (l, &none, &none),
            Some(WordsCmd::Like { words: l }) => (&none, l, &none),
            Some(WordsCmd::Forget { words: l }) => (&none, &none, l),
            None => (&none, &none, &none),
        };
        let old = load_prefs(&path)?;
        let mut prefs = old.clone();
        let missing = words::apply(&mut prefs, ban, like, forget)?;
        if prefs != old {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, serde_json::to_string_pretty(&prefs)? + "\n")?;
        }
        for w in missing {
            eprintln!("{w} was not banned or liked");
        }
        if json {
            print_json(&prefs)?;
        } else {
            print!("{}", words::render_prefs(&prefs));
        }
    }
    Cmd::SuggestBlocks { puzzle, probe_timeout, trial_timeout, max_squares, apply, engine } => {
        let (p, file) = ctx.load(&puzzle)?;
        let so = suggest::SuggestOptions {
            probe_timeout: Duration::from_secs(probe_timeout),
            trial_timeout: Duration::from_secs(trial_timeout),
            max_squares,
        };
        let r = suggest::suggest(&p, &engine.options(), &so)?;
        if engine.json {
            println!("{}", serde_json::to_string_pretty(&r)?);
        } else if r.already_fillable {
            println!("The grid already fills. No black squares needed.");
        } else {
            if let Some(h) = &r.hotspots {
                println!("Hotspots from a {probe_timeout}s fill attempt:\n{}", engine::render_heat(&p, h));
            }
            println!("Tried {} placements ({} skipped for breaking grid rules).", r.tried, r.skipped_breaks_rules);
            for (i, s) in r.suggestions.iter().enumerate() {
                let status = match s.status {
                    FillStatus::Filled => format!("FILLS in {:.1}s", s.seconds),
                    FillStatus::Timeout => format!("no fill in {:.0}s", s.seconds),
                    FillStatus::Impossible => "impossible".into(),
                };
                let squares = s.squares.iter().map(|(x, y)| format!("{x},{y}")).collect::<Vec<_>>().join(" + ");
                println!("{:>3}. block {:<14} {:<18} {} words", i + 1, squares, status, s.word_count);
            }
            if let Some(best) = r.suggestions.first().filter(|s| matches!(s.status, FillStatus::Filled)) {
                println!("\nBest option with a sample fill:");
                for (g, f) in best.grid.iter().zip(best.sample_fill.as_ref().unwrap()) {
                    println!("  {g}   {f}");
                }
            }
        }
        if let Some(n) = apply {
            let Some(s) = r.suggestions.get(n.wrapping_sub(1)) else { bail!("no suggestion #{n}") };
            let mut q = p.clone();
            edit::set_grid(&mut q, s.grid.clone())?;
            q.save(&file)?;
            eprintln!("Applied suggestion #{n} to {}", file.display());
        }
    }
    Cmd::Pattern { puzzle, blocks, max_words, max_len, entries, keep_blocks, seconds, fill_timeout, count, seed, write, engine } => {
        let (p, file) = ctx.load(&puzzle)?;
        let po = pattern::PatternOptions {
            blocks,
            max_words,
            max_len,
            entries: entries.iter().map(|e| pattern::Entry::parse(e)).collect::<Result<_>>()?,
            keep_blocks,
            seconds: Duration::from_secs(seconds),
            fill_timeout: Duration::from_secs(fill_timeout),
            count,
            seed,
        };
        let r = pattern::pattern(&p, &engine.options(), &po)?;
        if engine.json {
            println!("{}", serde_json::to_string_pretty(&r)?);
        } else {
            print!("{}", pattern::render_pattern(&r));
        }
        if let Some(n) = write {
            let Some(o) = r.patterns.get(n.wrapping_sub(1)) else { bail!("no pattern #{n}") };
            let mut q = p.clone();
            edit::set_grid(&mut q, o.grid.clone())?;
            q.save(&file)?;
            eprintln!("Saved pattern #{n} to {}", file.display());
        }
    }
    Cmd::TrimBlocks { puzzle, to, max_len, seconds, fill_timeout, seed, write, engine } => {
        let (p, file) = ctx.load(&puzzle)?;
        let to = pattern::TrimOptions { to, max_len, seconds: Duration::from_secs(seconds), fill_timeout: Duration::from_secs(fill_timeout), seed };
        let r = pattern::trim(&p, &engine.options(), &to)?;
        if engine.json {
            println!("{}", serde_json::to_string_pretty(&r)?);
        } else {
            print!("{}", pattern::render_trim(&r));
        }
        if write && !r.removed.is_empty() {
            let mut q = p.clone();
            edit::set_grid(&mut q, r.grid.clone())?;
            q.save(&file)?;
            eprintln!("Saved {} black squares to {}", r.blocks, file.display());
        }
    }
    }
    Ok(())
}
