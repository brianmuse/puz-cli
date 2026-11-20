mod swords;
mod update;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use puz_core::puzzle::Puzzle;
use puz_core::profile::{self, Profile};
use puz_core::{blocks, edit, export, files, guides, print, puzzle, scene, stats, submission};
use std::io::IsTerminal;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// puz: make and edit a crossword puzzle file from the command line.
///
/// A puzzle is one JSON file in ipuz, the open crossword standard (http://ipuz.org), with an
/// extension for what ipuz can't hold. Older puz JSON files still read, and change to ipuz on their
/// next save. Every command reads or changes one file and needs nothing else:
/// no account, no word list, no server. PUZZLE is a file path (my-puzzle.json).
///
/// If you keep your puzzles in one folder, name it with --library or $PUZ_LIBRARY. PUZZLE may
/// then also be a name inside that folder: "my-puzzle" or "drafts/my-puzzle.json".
///
/// When a puzzle is done, `puz publish PUZZLE` puts it on a Swords solving site, and
/// `puz publish nyt PUZZLE` writes the PDF the New York Times asks constructors to send.
#[derive(Parser)]
#[command(name = "puz", version, about, after_help = AFTER_HELP)]
struct Cli {
    /// A folder of puzzles, so PUZZLE can be a name inside it [default: $PUZ_LIBRARY]
    #[arg(long, global = true)]
    library: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

/// The end of `puz --help`.
const AFTER_HELP: &str = "Start with `puz setup` to save your name. \
Once a day, at a terminal, puz asks the Swords site whether a newer puz is out, and says so after a command. \
It sends only the puz version. Turn it off with PUZ_NO_UPDATE_CHECK=1. Update with `puz update`.";

#[derive(Subcommand)]
enum Cmd {
    /// Create a blank puzzle file. Prints the file it wrote. Without --out, the file is named after
    /// the title (my-title.json) and goes in the current folder, or in the folder of puzzles.
    New {
        title: String,
        /// Width x height
        #[arg(long, default_value = "15x15")]
        size: String,
        /// rotational, mirror, or none
        #[arg(long, default_value = "rotational")]
        symmetry: String,
        /// A folder to put it in (inside the folder of puzzles, when one is set)
        #[arg(long, default_value = "")]
        folder: String,
        /// Copy the size, symmetry and black squares of this puzzle
        #[arg(long)]
        from: Option<String>,
        /// The author [default: your name in `puz profile`]
        #[arg(long)]
        author: Option<String>,
        /// The file to write
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Print the grid and its slots
    Show {
        puzzle: String,
        #[arg(long)]
        json: bool,
    },
    /// Set puzzle details
    Set {
        puzzle: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        copyright: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        /// rotational, mirror, or none
        #[arg(long)]
        symmetry: Option<String>,
        /// The paper whose rules `stats` checks: nyt-daily, nyt-sunday, nyt-mini, lat-daily,
        /// wsj-daily, basic, or auto (pick from the grid size)
        #[arg(long)]
        target: Option<String>,
    },
    /// Write a word into a slot. '.' clears a square, '_' keeps it (e.g. BE_R).
    Place { puzzle: String, slot: String, word: String },
    /// Clear the letters in a slot, or the whole grid with --all
    Clear {
        puzzle: String,
        slot: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Set one clue. An empty string removes it. The clue is stored under the answer, so it follows the word.
    Clue {
        puzzle: String,
        slot: String,
        text: String,
        #[arg(long)]
        json: bool,
    },
    /// Set many clues from a JSON object {"41a": "text", ...}: a file, "-" for stdin, or the JSON itself.
    /// With no FILE, list every answer and its clue.
    Clues {
        puzzle: String,
        file: Option<PathBuf>,
        /// List stored clues that no answer in the grid shows now (their answer was changed or removed)
        #[arg(long, conflicts_with = "file")]
        unused: bool,
        #[arg(long)]
        json: bool,
    },
    /// Add or remove a black square (and its symmetric partner)
    Block {
        puzzle: String,
        /// Square as x,y (0-based column,row)
        square: String,
        /// Remove the block instead of adding it
        #[arg(long)]
        remove: bool,
    },
    /// Add or remove a circle, shading or a color on a square, or on a rectangle from x,y to x2,y2.
    /// Those are visual only: they don't change the answers. --gap makes squares gaps: left out of
    /// the grid (empty space, for grids in any shape). A gap counts as a black square.
    Mark {
        puzzle: String,
        /// Square as x,y (0-based column,row)
        square: String,
        /// Opposite corner of a rectangle, as x,y
        to: Option<String>,
        /// Circle the squares (circles skip black squares)
        #[arg(long, required_unless_present_any = ["shade", "color", "gap"], conflicts_with_all = ["shade", "color", "gap"])]
        circle: bool,
        /// Shade the squares grey (black squares may be shaded too)
        #[arg(long, conflicts_with_all = ["color", "gap"])]
        shade: bool,
        /// Color the squares: red, orange, yellow, green, blue, purple or pink
        #[arg(long, conflicts_with = "gap")]
        color: Option<String>,
        /// Make the squares gaps (no square there). With --remove they come back as black squares
        #[arg(long, alias = "absent")]
        gap: bool,
        /// Remove the mark instead of adding it
        #[arg(long)]
        remove: bool,
        #[arg(long)]
        json: bool,
    },
    /// Put a rebus (2 or more letters) in one square. The grid keeps the first letter.
    /// An empty LETTERS or --remove takes the rebus out and leaves its first letter.
    Rebus {
        puzzle: String,
        /// Square as x,y
        square: String,
        /// Letters A-Z, e.g. STAR
        #[arg(default_value = "")]
        letters: String,
        #[arg(long)]
        remove: bool,
        #[arg(long)]
        json: bool,
    },
    /// Add or remove a wall (bar) on one side of a square: right, bottom, left or top.
    /// A wall ends answers like a black square does.
    Wall {
        puzzle: String,
        /// Square as x,y
        square: String,
        /// right, bottom, left or top
        side: String,
        #[arg(long)]
        remove: bool,
        #[arg(long)]
        json: bool,
    },
    /// Put a picture on a square (white or black): an emoji, or an image file up to 300 KB.
    Sprite {
        puzzle: String,
        /// Square as x,y
        square: String,
        /// An emoji, e.g. "⭐"
        #[arg(long, conflicts_with_all = ["image", "remove"], required_unless_present_any = ["image", "remove"])]
        emoji: Option<String>,
        /// A png, jpg, gif, svg or webp file
        #[arg(long, conflicts_with = "remove")]
        image: Option<PathBuf>,
        /// Text that describes the picture
        #[arg(long)]
        alt: Option<String>,
        /// Remove the picture
        #[arg(long)]
        remove: bool,
        #[arg(long)]
        json: bool,
    },
    /// Lock squares so they keep their letters: an answer (9A), a square (x,y), or a rectangle
    /// (x,y x2,y2). Edits that would change a locked square fail until you unlock it (--remove).
    /// Locking an answer skips its empty squares.
    Lock {
        puzzle: String,
        /// An answer (9A, 17D, 5,7,across) or a square x,y
        #[arg(required_unless_present_any = ["all_filled", "clear_all"])]
        target: Option<String>,
        /// Opposite corner of a rectangle, as x,y
        to: Option<String>,
        /// Lock every filled square
        #[arg(long, conflicts_with_all = ["target", "clear_all", "remove"])]
        all_filled: bool,
        /// Unlock every square
        #[arg(long, conflicts_with_all = ["target", "all_filled"])]
        clear_all: bool,
        /// Unlock instead of lock
        #[arg(long)]
        remove: bool,
        #[arg(long)]
        json: bool,
    },
    /// Make the grid bigger or smaller. Give a new size (17x15) and an anchor, or rows and columns
    /// to add (positive) or remove (negative) on each edge. New squares are empty. Marks, locks
    /// and clues move with their squares. Removing a locked square fails.
    Resize {
        puzzle: String,
        /// New size, width x height (e.g. 17x15). Between 2 and 40.
        #[arg(conflicts_with_all = ["top", "bottom", "left", "right"])]
        size: Option<String>,
        /// Rows to add on the top (negative removes)
        #[arg(long, allow_negative_numbers = true, default_value_t = 0)]
        top: i64,
        /// Rows to add on the bottom (negative removes)
        #[arg(long, allow_negative_numbers = true, default_value_t = 0)]
        bottom: i64,
        /// Columns to add on the left (negative removes)
        #[arg(long, allow_negative_numbers = true, default_value_t = 0)]
        left: i64,
        /// Columns to add on the right (negative removes)
        #[arg(long, allow_negative_numbers = true, default_value_t = 0)]
        right: i64,
        /// With SIZE: the part of the grid that stays in place. center (default) splits the change
        /// between both sides (an odd row or column goes to the bottom or right); or top, bottom,
        /// left, right, top-left, top-right, bottom-left, bottom-right.
        #[arg(long, requires = "size")]
        anchor: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Print a rectangle of squares as text: one line per row, "#" black, "." empty.
    /// Corners are x,y squares in any order. Pipe it into `paste` to move or repeat a region.
    Copy { puzzle: String, from: String, to: String },
    /// Empty every letter in a rectangle. Black squares stay.
    ClearRegion { puzzle: String, from: String, to: String },
    /// Write text (the `copy` format) into the grid with its top-left at x,y.
    /// "#" adds a black square and its partner. A letter or "." on a black square makes it white.
    /// Space, "_" and "?" keep the square as it is. Reads stdin when FILE is "-".
    Paste {
        puzzle: String,
        at: String,
        /// File with the text, or "-" for stdin
        #[arg(required_unless_present = "text")]
        file: Option<PathBuf>,
        /// The text itself; use "\n" or "/" between rows
        #[arg(long, conflicts_with = "file")]
        text: Option<String>,
    },
    /// Grid stats (answers, black squares, unchecked squares, interlock, symmetry, cheaters,
    /// fill progress) and checks against a paper's submission rules
    Stats {
        puzzle: String,
        /// auto (the puzzle's target, or pick from the grid size), nyt-daily, nyt-sunday,
        /// nyt-mini, lat-daily, wsj-daily or basic
        #[arg(long, default_value = "auto")]
        rules: String,
        #[arg(long)]
        json: bool,
    },
    /// Read an Across Lite .puz, an .ipuz, an older .swords or a puzzle .json and write it as a puzzle file. Without --out, the
    /// file is named after the title and goes in the current folder, or in the folder of puzzles.
    Import {
        file: PathBuf,
        #[arg(long, default_value = "")]
        folder: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Set the celebration a solver sees on finishing the puzzle: effects, squares that light up,
    /// and a scene: steps that play over the grid (squares recolor, letters change, lines, pictures, captions).
    /// Each of --paint, --write, --line, --image, --caption, --play and --erase adds one step; use one per command
    Ending {
        puzzle: String,
        /// Kept in the file but no longer shown
        #[arg(long, hide = true)]
        message: Option<String>,
        /// A short message under the solve time, up to 140 characters
        #[arg(long)]
        note: Option<String>,
        /// Effects to play, separated by spaces or commas: confetti, rain, burst, wave. "" for none
        #[arg(long)]
        effects: Option<String>,
        /// The emoji that rain and burst use, separated by spaces. "" for none
        #[arg(long)]
        emoji: Option<String>,
        /// Squares that light up, as "x,y x,y" (0-based). "" for none
        #[arg(long)]
        highlight: Option<String>,
        /// Kept in the file but no longer shown
        #[arg(long, hide = true, num_args = 2, value_names = ["LABEL", "URL"])]
        link: Option<Vec<String>>,
        /// Remove the whole ending (this removes the scene too)
        #[arg(long)]
        clear: bool,
        /// Replace the scene with the steps in a JSON file ("-" reads stdin): a list of steps, or an ending with a "scene"
        #[arg(long, value_name = "FILE")]
        scene: Option<String>,
        /// Remove the scene
        #[arg(long)]
        scene_clear: bool,
        /// Remove one step of the scene, by its number (the first step is 1)
        #[arg(long, value_name = "N")]
        scene_remove: Option<usize>,
        /// Add a step: recolor squares ("x,y x,y") in a color (red, orange, yellow, green, blue, purple, pink, or hex)
        #[arg(long, num_args = 2, value_names = ["SQUARES", "COLOR"])]
        paint: Option<Vec<String>>,
        /// Add a step: change what squares show. TEXT has one character for each square, or one word for each square
        #[arg(long, num_args = 2, value_names = ["SQUARES", "TEXT"])]
        write: Option<Vec<String>>,
        /// Add a step: draw a line through points ("x,y x,y", 0,0 is the middle of the top left square; fractions are fine)
        #[arg(long, num_args = 1..=2, value_names = ["POINTS", "COLOR"])]
        line: Option<Vec<String>>,
        /// Add a step: show a picture (png, jpg, gif, webp or svg; it is embedded in the puzzle file). Place it with --over
        #[arg(long, value_name = "FILE")]
        image: Option<PathBuf>,
        /// Add a step: show a caption, below the grid unless --over says where
        #[arg(long, value_name = "TEXT")]
        caption: Option<String>,
        /// Add a step: play one effect at this point of the scene (confetti, rain, burst, wave)
        #[arg(long, value_name = "EFFECT")]
        play: Option<String>,
        /// Add a step: remove what earlier steps drew. Step numbers ("1 3"), or "all"
        #[arg(long, value_name = "STEPS")]
        erase: Option<String>,
        /// For the new step: seconds after the solve when it starts. Left out: when the step before it ends
        #[arg(long, value_name = "SECONDS")]
        at: Option<f64>,
        /// For the new step: seconds the change takes (default 0.6)
        #[arg(long = "for", value_name = "SECONDS")]
        dur: Option<f64>,
        /// With --paint or --write: seconds between one square and the next
        #[arg(long, value_name = "SECONDS")]
        stagger: Option<f64>,
        /// With --image or --caption: "grid", a rectangle of squares "x,y,w,h", or (captions) "below"
        #[arg(long)]
        over: Option<String>,
        /// With --image: contain (default), cover or stretch
        #[arg(long)]
        fit: Option<String>,
        /// With --image: 0 to 1
        #[arg(long)]
        opacity: Option<f64>,
        /// With --image: how it comes in: fade (default), grow or none
        #[arg(long)]
        enter: Option<String>,
        /// With --line: the width in squares (default 0.12)
        #[arg(long)]
        width: Option<f64>,
        /// With --line: join the last point to the first
        #[arg(long)]
        closed: bool,
        /// With --caption: the letter height in squares (default 0.6), for a caption over the grid
        #[arg(long)]
        size: Option<f64>,
        /// With --caption: its color
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        json: bool,
        /// With --json: leave the picture data out, so the output stays short
        #[arg(long)]
        no_images: bool,
    },
    /// Write the puzzle in another format: .ipuz (the open standard, with nothing left out), .jpz
    /// (Amuse Labs / PuzzleMe), .puz (Across Lite), .json (the older puz JSON),
    /// a .png of the grid, or a printable one-page .pdf. Without --out, the file goes in the current
    /// folder under the puzzle's file name. A puzzle with no author takes your name (`puz profile`).
    /// It only writes a file: to put a puzzle online, or to make the NYT's submission PDF, use `puz publish`.
    Export {
        puzzle: String,
        /// ipuz, jpz, puz, json, png (the grid) or pdf (one page: title, grid and clues)
        #[arg(long, default_value = "ipuz")]
        format: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// png and pdf: show the letters (an answer key). Default is a blank puzzle.
        #[arg(long)]
        answers: bool,
        /// png: pixels per grid unit (2 = 80 pixels per square)
        #[arg(long, default_value_t = 2.0)]
        scale: f32,
        /// Old: `--format pdf --style nyt` is now `puz publish nyt PUZZLE`. It still works.
        #[arg(long, default_value = "page", hide = true)]
        style: String,
        #[arg(long, hide = true)]
        blank_grid: bool,
        #[arg(long, hide = true)]
        name: Option<String>,
        #[arg(long, hide = true)]
        address: Option<String>,
        #[arg(long, hide = true)]
        city: Option<String>,
        #[arg(long, hide = true)]
        email: Option<String>,
    },
    /// Make a puzzle's share card: a 1200 x 630 PNG with the empty grid, the title and the author,
    /// for link previews (og:image).
    Card {
        puzzle: String,
        /// Where to write the PNG (default: NAME-card.png in this folder)
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// A line for the bottom of the card, like the site's name
        #[arg(long)]
        footer: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Put a finished puzzle on a Swords solving site (the default), or write the PDF the NYT asks
    /// constructors to send (`puz publish nyt PUZZLE`).
    ///
    /// `puz publish PUZZLE` sends the puzzle and prints the link to share. The puzzle needs a full
    /// grid and a clue for every answer. The link is saved in the puzzle file, so publishing it
    /// again updates the same link, even after the file is renamed or moved. PUZZLE may also be a
    /// .puz, .jpz or .ipuz file. A puzzle with no author takes your name (`puz setup`).
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Publish {
        #[command(subcommand)]
        to: Option<PublishTo>,
        #[command(flatten)]
        swords: SwordsPublish,
    },
    /// Manage your puzzles on a Swords solving site: list them, see who solves them, set a PIN,
    /// take one down, sign in. PUZZLE is a puzzle file you published, or the puzzle's id or link.
    #[command(after_help = swords_client::cli::AFTER_HELP)]
    Swords(SwordsArgs),
    /// Set up puz: your name (the author of your puzzles) and, if you like, an email to keep your
    /// Swords puzzles on any device. It asks at a terminal; --name and --email skip the questions.
    Setup {
        #[arg(long)]
        name: Option<String>,
        /// Sign in to the Swords site with this email: it sends a 6-digit code
        #[arg(long)]
        email: Option<String>,
        /// The Swords site to sign in to [default: the one in use]
        #[arg(long)]
        site: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Get the newest puz from the Swords site and put it in place of this one. It tries the new
    /// puz first and changes nothing if it does not run.
    Update {
        #[arg(long)]
        json: bool,
    },
    /// Your details as a constructor: name, address, city line and email. The name is the author of
    /// new puzzles, of exports and of published puzzles with no author (`puz setup` sets it too).
    /// All four go at the top of page 1 of the NYT submission PDF (`puz publish nyt`). With no options, show them. They are kept in one
    /// file: $PUZ_PROFILE, else profile.json in $PUZ_CONFIG_DIR, $XDG_CONFIG_HOME/puz or ~/.config/puz
    /// (%APPDATA%\puz on Windows).
    Profile {
        /// Your name
        #[arg(long)]
        name: Option<String>,
        /// Street address
        #[arg(long)]
        address: Option<String>,
        /// City, state and ZIP, as one line
        #[arg(long)]
        city: Option<String>,
        #[arg(long)]
        email: Option<String>,
        /// A co-constructor's name (their block goes to the right of yours)
        #[arg(long)]
        co_name: Option<String>,
        #[arg(long)]
        co_address: Option<String>,
        #[arg(long)]
        co_city: Option<String>,
        #[arg(long)]
        co_email: Option<String>,
        /// Remove the co-constructor's block
        #[arg(long)]
        no_co: bool,
        #[arg(long)]
        json: bool,
    },
    /// Print a how-to guide about the puzzle file. `guide` lists them; `guide ending` says how to
    /// write an ending and its scene.
    Guide {
        /// The guide to print [default: list the guides]
        topic: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// `puz publish PUZZLE` to Swords: the flags.
#[derive(clap::Args)]
struct SwordsPublish {
    /// The puzzle file
    #[arg(required = true)]
    puzzle: Option<String>,
    /// The Swords site [default: $SWORDS_SITE, else the one set with `puz swords site`]
    #[arg(long)]
    site: Option<String>,
    /// Solvers need this PIN (4 to 12 characters) to open the puzzle
    #[arg(long, conflicts_with = "no_pin")]
    pin: Option<String>,
    /// Take the PIN off
    #[arg(long)]
    no_pin: bool,
    /// Make a new link, not an update of the one saved in the puzzle file
    #[arg(long)]
    new: bool,
    /// The author to show when the puzzle names none [default: your name]
    #[arg(long)]
    author: Option<String>,
    /// For AI agents: the user said yes to this publish. An agent's run needs it.
    #[arg(long)]
    confirmed: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum PublishTo {
    /// Put the puzzle on a Swords solving site (the same as `puz publish PUZZLE`).
    Swords(SwordsPublish),
    /// Write the PDF the New York Times asks constructors to send: your details and the filled grid
    /// on page 1, then the clues with their answers. It asks at a terminal for details that are
    /// missing from your profile (`puz profile`), or takes them from --name, --address, --city and --email.
    Nyt {
        puzzle: String,
        /// Where to write it [default: NAME.pdf in this folder]
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Add a page with the blank, numbered grid after the filled grid
        #[arg(long)]
        blank_grid: bool,
        /// Your name, for this run
        #[arg(long)]
        name: Option<String>,
        /// Your street address, for this run
        #[arg(long)]
        address: Option<String>,
        /// Your city, state and ZIP as one line, for this run
        #[arg(long)]
        city: Option<String>,
        /// Your email, for this run
        #[arg(long)]
        email: Option<String>,
    },
}

#[derive(clap::Args)]
struct SwordsArgs {
    /// The Swords site [default: $SWORDS_SITE, else the one set with `puz swords site`]
    #[arg(long, global = true)]
    site: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: SwordsCmd,
}

#[derive(Subcommand)]
enum SwordsCmd {
    /// Is this puzzle on Swords, and is the published copy out of date? Reads the links saved in the
    /// puzzle file. --live also asks the site for each link's state.
    Status {
        puzzle: String,
        #[arg(long)]
        live: bool,
    },
    #[command(flatten)]
    Other(swords_client::cli::Command),
}

/// Write the NYT submission PDF.
fn write_submission(p: &Puzzle, out: &Path, blank_grid: bool, flags: profile::Update) -> Result<()> {
    let prof = submission_profile(flags)?;
    std::fs::write(out, submission::pdf(submission::Style::Nyt, p, &submission::options(p, &prof, blank_grid))?)?;
    Ok(())
}

fn parse_square(s: &str) -> Result<(usize, usize)> {
    edit::parse_square(s)
}

/// Print a short message, or every mark as JSON with `--json`.
fn print_marks(p: &Puzzle, json: bool, msg: &str) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "message": msg, "marks": edit::marks(p) }))?);
    } else {
        println!("{msg}");
    }
    Ok(())
}

/// An environment variable that is set and not empty.
fn env(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|v| !v.is_empty())
}

struct Ctx {
    /// The folder of puzzles for this run: --library, else $PUZ_LIBRARY. None when neither is set.
    library: Option<PathBuf>,
}

impl Ctx {
    /// The file for a PUZZLE argument: a file path, or a name in the folder of puzzles.
    fn file(&self, arg: &str) -> Result<PathBuf> {
        files::find(arg, self.library.as_deref())
    }

    fn load(&self, arg: &str) -> Result<(Puzzle, PathBuf)> {
        let f = self.file(arg)?;
        Ok((Puzzle::load(&f)?, f))
    }

    /// Where a new puzzle goes: `out` if given; else a free file named after the title, in `folder`
    /// (inside the folder of puzzles when one is set, else inside the current folder).
    fn new_file(&self, out: Option<PathBuf>, folder: &str, title: &str) -> Result<PathBuf> {
        if let Some(o) = out {
            return Ok(o);
        }
        let base = self.library.clone().unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join(folder.trim().trim_matches('/'));
        if !dir.is_dir() {
            bail!("no folder {}", dir.display());
        }
        Ok(files::free_file(&dir, title, &|_| false))
    }

    /// How a new file is named in messages: its name in the folder of puzzles, else its path.
    fn shown(&self, file: &Path) -> String {
        let rel = self.library.as_ref().and_then(|l| file.strip_prefix(l).ok()).unwrap_or(file);
        rel.strip_prefix(".").unwrap_or(rel).display().to_string()
    }
}

/// Your name from your profile: the author for a new puzzle, and for an export of a puzzle with none.
fn default_author() -> Option<String> {
    let name = Profile::load(&profile::user_file()).ok()?.me.name.trim().to_string();
    Some(name).filter(|n| !n.is_empty())
}

/// One line typed at the terminal, trimmed.
fn ask(question: &str) -> Result<String> {
    eprint!("{question}");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// The profile for a submission PDF: the saved one, with this run's flags on top. Details still
/// missing are asked for at a terminal (with an offer to save them); with no terminal, or in an AI
/// agent's run, they are an error that names the flags.
fn submission_profile(flags: profile::Update) -> Result<Profile> {
    let file = profile::user_file();
    let mut prof = Profile::load(&file)?;
    flags.apply(&mut prof.me);
    let missing = prof.missing();
    if missing.is_empty() {
        return Ok(prof);
    }
    if swords::in_agent_run() || !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!(
            "A submission PDF has your details at the top of page 1, and your {} {} missing. Give --name, --address, --city and --email, or save them once: puz profile --name NAME --address ADDRESS --city \"CITY, ST ZIP\" --email EMAIL",
            missing.join(", "),
            if missing.len() == 1 { "is" } else { "are" }
        );
    }
    eprintln!("A submission PDF has your details at the top of page 1. Some are missing:");
    let mut typed = profile::Update::default();
    for what in missing {
        let field = match what {
            "name" => &mut typed.name,
            "address" => &mut typed.address,
            "email" => &mut typed.email,
            _ => &mut typed.city,
        };
        let hint = if what == "city line" { "City, state and ZIP (one line)".to_string() } else { format!("Your {what}") };
        *field = Some(ask(&format!("{hint}: "))?);
    }
    let answer = ask(&format!("Save these in {} for next time? [Y/n] ", file.display()))?;
    if answer.is_empty() || answer.to_ascii_lowercase().starts_with('y') {
        // Save what was typed on top of the saved profile: this run's flags stay one-off.
        let mut saved = Profile::load(&file)?;
        profile::Update { name: typed.name.clone(), address: typed.address.clone(), city: typed.city.clone(), email: typed.email.clone() }.apply(&mut saved.me);
        saved.save(&file)?;
        eprintln!("Saved.");
    }
    typed.apply(&mut prof.me);
    Ok(prof)
}

/// Text from a file, from stdin ("-"), or given inline (an argument that starts with "{").
fn read_json_arg(file: &Path) -> Result<String> {
    if file.to_string_lossy().trim_start().starts_with('{') {
        Ok(file.to_string_lossy().into_owned())
    } else if file.as_os_str() == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        Ok(s)
    } else {
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))
    }
}

fn main() {
    swords_client::api::set_client("puz", update::VERSION);
    let cli = Cli::parse();
    let json = std::env::args_os().any(|a| a == "--json");
    let quiet = update::Quiet::start(json, matches!(cli.cmd, Cmd::Update { .. }));
    let code = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e:?}");
            1
        }
    };
    if let Some(q) = quiet {
        q.finish();
    }
    std::process::exit(code);
}

/// Run one command. The number is the exit code.
fn run(cli: Cli) -> Result<i32> {
    let ctx = Ctx { library: cli.library.or_else(|| env("PUZ_LIBRARY").map(PathBuf::from)) };
    match cli.cmd {
        Cmd::New { title, size, symmetry, folder, from, author, out } => {
            let (w, h) = size.split_once('x').context("size must look like 15x15")?;
            let from = from.map(|f| ctx.load(&f)).transpose()?.map(|(p, _)| p);
            let mut p = edit::new_puzzle(&title, w.trim().parse()?, h.trim().parse()?, &symmetry, from.as_ref())?;
            p.author = author.or_else(default_author).unwrap_or_default().trim().to_string();
            let file = ctx.new_file(out, &folder, &p.title)?;
            p.save(&file)?;
            println!("{}", ctx.shown(&file));
        }
        Cmd::Show { puzzle, json } => {
            let (p, _) = ctx.load(&puzzle)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "puzzle": p, "slots": p.slots(), "slot_clues": edit::slot_clues(&p) }))?);
                return Ok(0);
            }
            println!("{} ({}x{})\n{}", p.title, p.width(), p.height(), p.print_marked());
            for s in p.slots() {
                let dir = if s.dir == puzzle::Dir::Across { "A" } else { "D" };
                println!("{:>4}{} {:<16} {}", s.number, dir, s.pattern, p.clue_for(&s).unwrap_or(""));
            }
            if !p.rebus.is_empty() {
                println!("[STAR] in a pattern is a rebus square.");
            }
        }
        Cmd::Set { puzzle, title, author, copyright, notes, symmetry, target } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            if let Some(t) = target {
                p.target = stats::parse_target(&t)?;
            }
            if let Some(s) = &symmetry
                && !["rotational", "mirror", "none"].contains(&s.as_str())
            {
                bail!("symmetry must be rotational, mirror, or none");
            }
            for (field, v) in [(&mut p.title, title), (&mut p.author, author), (&mut p.copyright, copyright), (&mut p.notes, notes), (&mut p.symmetry, symmetry)] {
                if let Some(v) = v {
                    *field = v;
                }
            }
            p.save(&f)?;
        }
        Cmd::Place { puzzle, slot, word } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            let s = edit::place(&mut p, &slot, &word)?;
            p.save(&f)?;
            println!("{} {}", edit::label(&s), s.pattern);
        }
        Cmd::Clear { puzzle, slot, all } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            if slot.is_none() && !all {
                bail!("give a slot, or --all to clear every letter");
            }
            let c = edit::clear(&mut p, slot.as_deref())?;
            p.save(&f)?;
            println!("{}", c.message());
        }
        Cmd::Clue { puzzle, slot, text, json } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            let l = edit::set_clue(&mut p, &slot, &text)?;
            p.save(&f)?;
            let s = p.find_slot(&slot)?;
            let problem = s.answer().zip(p.clue_for(&s)).and_then(|(a, c)| edit::clue_problem(&a, c));
            if json {
                println!("{}", serde_json::json!({ "slot": l, "answer": s.answer(), "clue": p.clue_for(&s), "problem": problem }));
            } else if s.answer().is_none() && !text.trim().is_empty() {
                println!("{l}: {text}\n(The answer has empty squares, so the clue stays on this slot until it is complete. Then it follows the word.)");
            } else {
                println!("{l}: {text}");
                if let Some(why) = &problem {
                    println!("Warning: {why}. A clue must not give its answer away; write a different clue.");
                }
            }
        }
        Cmd::Clues { puzzle, file: None, unused, json } => {
            let (p, _) = ctx.load(&puzzle)?;
            if unused {
                let list = p.unused_clues();
                if json {
                    let rows: Vec<_> = list.iter().map(|(k, t)| serde_json::json!({ "key": k, "clue": t })).collect();
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else if list.is_empty() {
                    println!("No unused clues.");
                } else {
                    for (k, t) in list {
                        println!("{k:<20} {t}");
                    }
                }
            } else if json {
                println!("{}", serde_json::to_string_pretty(&edit::slot_clues(&p))?);
            } else {
                for c in edit::slot_clues(&p) {
                    let warn = c.problem.as_ref().map(|w| format!("   [warning: {w}]")).unwrap_or_default();
                    println!("{:>5} {:<16} {}{warn}", c.slot, c.answer.as_deref().unwrap_or("(incomplete)"), c.clue.as_deref().unwrap_or(""));
                }
            }
        }
        Cmd::Clues { puzzle, file: Some(file), json, .. } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            let map: BTreeMap<String, String> = serde_json::from_str(&read_json_arg(&file)?).context("expected a JSON object of slot -> clue")?;
            let labels = edit::set_clues(&mut p, &map)?;
            p.save(&f)?;
            let set: std::collections::BTreeSet<&String> = labels.iter().collect();
            let problems: Vec<_> = edit::slot_clues(&p).into_iter().filter(|c| c.problem.is_some() && set.contains(&c.slot)).collect();
            if json {
                println!("{}", serde_json::json!({ "set": labels, "problems": problems }));
            } else {
                println!("Set {} clue(s): {}", labels.len(), labels.join(", "));
                for c in &problems {
                    println!("Warning: {}: {}. A clue must not give its answer away; write a different clue.", c.slot, c.problem.as_deref().unwrap_or(""));
                }
            }
        }
        Cmd::Block { puzzle, square, remove } => {
            let (p, f) = ctx.load(&puzzle)?;
            let (x, y) = parse_square(&square)?;
            let q = blocks::set_block(&p, x, y, !remove)?;
            let problems = blocks::grid_problems(&q);
            q.save(&f)?;
            println!("{}", q.print());
            if !problems.is_empty() {
                println!("Warning: {}", problems.join("; "));
            }
        }
        Cmd::Mark { puzzle, square, to, circle, shade: _, color, gap, remove, json } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            let to = to.as_deref().map(parse_square).transpose()?;
            let from = parse_square(&square)?;
            let msg = if gap {
                let n = edit::set_absent(&mut p, from, to, !remove)?;
                if remove { format!("Turned {n} gap(s) back into black squares.") } else { format!("Made {n} square(s) gaps (left out of the grid).") }
            } else if let Some(c) = color {
                let n = edit::set_color(&mut p, from, to, if remove { None } else { Some(c.as_str()) })?;
                if remove { format!("Removed the color from {n} square(s).") } else { format!("Colored {n} square(s) {c}.") }
            } else {
                let kind = if circle { edit::Mark::Circle } else { edit::Mark::Shade };
                let n = edit::mark(&mut p, from, to, kind, !remove)?;
                format!("{} {} on {n} square(s).", if remove { "Removed" } else { "Added" }, if circle { "circle" } else { "shading" })
            };
            p.save(&f)?;
            print_marks(&p, json, &msg)?;
        }
        Cmd::Rebus { puzzle, square, letters, remove, json } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            let (x, y) = parse_square(&square)?;
            let letters = if remove { String::new() } else { letters };
            edit::set_rebus(&mut p, x, y, &letters)?;
            p.save(&f)?;
            let msg = if letters.trim().is_empty() { format!("Removed the rebus at {x},{y}.") } else { format!("Rebus {} at {x},{y}.", letters.trim().to_uppercase()) };
            print_marks(&p, json, &msg)?;
        }
        Cmd::Wall { puzzle, square, side, remove, json } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            let (x, y) = parse_square(&square)?;
            let (sx, sy, stored) = edit::set_wall(&mut p, x, y, &side, !remove)?;
            p.save(&f)?;
            print_marks(&p, json, &format!("{} the wall on the {stored} of {sx},{sy}.", if remove { "Removed" } else { "Added" }))?;
        }
        Cmd::Sprite { puzzle, square, emoji, image, alt, remove, json } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            let (x, y) = parse_square(&square)?;
            let sprite = match (emoji, image, remove) {
                (_, _, true) => None,
                (Some(e), _, _) => Some(edit::sprite_emoji(&e, alt)),
                (None, Some(img), _) => Some(edit::sprite_from_file(&img, alt)?),
                (None, None, false) => bail!("give --emoji, --image or --remove"),
            };
            let added = sprite.is_some();
            edit::set_sprite(&mut p, x, y, sprite)?;
            p.save(&f)?;
            print_marks(&p, json, &format!("{} the picture at {x},{y}.", if added { "Set" } else { "Removed" }))?;
        }

        Cmd::Lock { puzzle, target, to, all_filled, clear_all, remove, json } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            let t = match (all_filled, clear_all, target) {
                (true, _, _) => edit::LockTarget::AllFilled,
                (_, true, _) => edit::LockTarget::All,
                (_, _, Some(t)) => edit::LockTarget::parse(&t, to.as_deref())?,
                _ => bail!("give an answer, a square, --all-filled or --clear-all"),
            };
            let r = edit::lock(&mut p, &t, !(remove || clear_all))?;
            p.save(&f)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                println!("{}", r.message);
            }
        }
        Cmd::Resize { puzzle, size, top, bottom, left, right, anchor, json } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            let e = edit::plan_resize(&p, size.as_deref(), anchor.as_deref(), edit::Edges { top, bottom, left, right })?;
            let r = edit::resize(&mut p, e)?;
            p.save(&f)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "report": r, "grid": p.grid }))?);
            } else {
                println!("{}\n{}", p.print(), r.message);
            }
        }
        Cmd::Copy { puzzle, from, to } => {
            let (p, _) = ctx.load(&puzzle)?;
            println!("{}", edit::copy_region(&p, parse_square(&from)?, parse_square(&to)?)?);
        }
        Cmd::ClearRegion { puzzle, from, to } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            let c = edit::clear_region(&mut p, parse_square(&from)?, parse_square(&to)?)?;
            p.save(&f)?;
            println!("{}\n{}", p.print(), c.message());
        }
        Cmd::Paste { puzzle, at, file, text } => {
            let (mut p, f) = ctx.load(&puzzle)?;
            let (x, y) = parse_square(&at)?;
            let text = match (text, file) {
                (Some(t), _) => t.replace("\\n", "\n").replace('/', "\n"),
                (None, Some(file)) => read_json_arg(&file)?,
                (None, None) => bail!("give FILE, \"-\" or --text"),
            };
            let n = edit::paste(&mut p, x, y, text.trim_end_matches('\n'))?;
            let problems = blocks::grid_problems(&p);
            p.save(&f)?;
            println!("{}\nChanged {n} square(s).", p.print());
            if !problems.is_empty() {
                println!("Warning: {}", problems.join("; "));
            }
        }

        Cmd::Publish { to, swords } => {
            let o = match to {
                Some(PublishTo::Swords(o)) => o,
                None => swords,
                Some(PublishTo::Nyt { puzzle, out, blank_grid, name, address, city, email }) => {
                    let (p, file) = ctx.load(&puzzle)?;
                    let out = out.unwrap_or_else(|| PathBuf::from(file.file_stem().unwrap()).with_extension("pdf"));
                    write_submission(&p, &out, blank_grid, profile::Update { name, address, city, email })?;
                    println!("Wrote {}", out.display());
                    return Ok(0);
                }
            };
            let file = match o.puzzle.as_deref() {
                Some(arg) => {
                    swords::write_hello(std::path::Path::new(arg), o.json)?;
                    ctx.file(arg)?
                }
                None => bail!("name the puzzle to publish: puz publish PUZZLE"),
            };
            let p = swords::Publish { file, site: o.site, pin: o.pin, no_pin: o.no_pin, new: o.new, author: o.author, confirmed: o.confirmed, json: o.json };
            return Ok(swords::publish(p));
        }
        Cmd::Swords(a) => {
            let find = |arg: &str| ctx.file(arg).ok().filter(|f| f.is_file());
            let code = match a.cmd {
                SwordsCmd::Status { puzzle, live } => swords::group(a.site, a.json, Some(swords::Extra::Status { puzzle, live }), None, &find),
                SwordsCmd::Other(cmd) => swords::group(a.site, a.json, None, Some(cmd), &find),
            };
            return Ok(code);
        }
        Cmd::Setup { name, email, site, json } => return Ok(swords::setup_command(name, email, site, json)),
        Cmd::Update { json } => update::update(json)?,
        Cmd::Stats { puzzle, rules, json } => {
            let (p, _) = ctx.load(&puzzle)?;
            let r = stats::report(&p, &rules)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                println!("{}", stats::render(&r));
            }
        }
        Cmd::Import { file, folder, out } => {
            let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            let p = export::import(&file.to_string_lossy(), &bytes)?;
            let dest = ctx.new_file(out, &folder, &p.title)?;
            p.save(&dest)?;
            println!("Imported {:?} ({}x{}) to {}", p.title, p.width(), p.height(), ctx.shown(&dest));
        }
        Cmd::Ending { puzzle, message, note, effects, emoji, highlight, link, clear, scene, scene_clear, scene_remove, paint, write, line, image, caption, play, erase, at, dur, stagger, over, fit, opacity, enter, width, closed, size, color, json, no_images } => {
            let (mut p, path) = ctx.load(&puzzle)?;
            let words = |s: &str| s.split([' ', ',']).map(str::trim).filter(|w| !w.is_empty()).map(str::to_string).collect::<Vec<_>>();
            let before = p.ending.clone();
            if message.is_some() || link.is_some() {
                eprintln!("note: solving pages no longer show an ending's message or link. They are saved in the file. Use --note for a short line.");
            }
            if clear {
                p.ending = Default::default();
            }
            if let Some(m) = message {
                p.ending.message = m.trim().to_string();
            }
            if let Some(n) = note {
                if n.trim().chars().count() > 140 {
                    bail!("the note is {} characters; keep it to 140", n.trim().chars().count());
                }
                p.ending.note = n.trim().to_string();
            }
            if let Some(e) = effects {
                let list = words(&e.to_ascii_lowercase());
                if let Some(bad) = list.iter().find(|w| !puzzle::EFFECTS.contains(&w.as_str())) {
                    bail!("no effect {bad:?}. The effects are: {}", puzzle::EFFECTS.join(", "));
                }
                p.ending.effects = list;
            }
            if let Some(e) = emoji {
                p.ending.emoji = e.split_whitespace().map(str::to_string).collect();
            }
            if let Some(h) = highlight {
                let (w, hgt) = (p.width(), p.height());
                let mut set = std::collections::BTreeSet::new();
                for sq in h.split_whitespace() {
                    let ok = sq.split_once(',').and_then(|(x, y)| Some((x.parse::<usize>().ok()?, y.parse::<usize>().ok()?))).is_some_and(|(x, y)| x < w && y < hgt);
                    if !ok {
                        bail!("{sq:?} is not a square of this {w}x{hgt} grid. Write squares as x,y from 0,0 at the top left");
                    }
                    set.insert(sq.to_string());
                }
                p.ending.highlight = set;
            }
            if let Some(l) = link {
                let (label, url) = (l[0].trim().to_string(), l[1].trim().to_string());
                if label.is_empty() && url.is_empty() {
                    p.ending.link = None;
                } else if label.is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
                    bail!("a link needs a label and an http(s) address");
                } else {
                    p.ending.link = Some(puzzle::EndingLink { label, url });
                }
            }
            if scene_clear {
                p.ending.scene.clear();
            }
            if let Some(file) = scene {
                let text = if file == "-" { std::io::read_to_string(std::io::stdin())? } else { std::fs::read_to_string(&file).with_context(|| format!("reading {file}"))? };
                let mut v: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("{file} is not JSON"))?;
                // The output of `ending PUZZLE --json` works too: its "scene" is used.
                if let Some(inner) = v.get_mut("scene") {
                    v = inner.take();
                }
                p.ending.scene = scene::from_value(v)?;
            }
            if let Some(n) = scene_remove {
                if n == 0 || n > p.ending.scene.len() {
                    bail!("there is no step {n}: the scene has {} step(s)", p.ending.scene.len());
                }
                p.ending.scene.remove(n - 1);
            }
            let mut new_steps = vec![];
            if let Some(a) = &paint {
                new_steps.push(scene::paint(&a[0], &a[1]));
            }
            if let Some(a) = &write {
                new_steps.push(scene::write(&a[0], &a[1])?);
            }
            if let Some(a) = &line {
                new_steps.push(scene::line(&a[0], a.get(1).map(String::as_str)));
            }
            if let Some(f) = &image {
                new_steps.push(scene::image(f)?);
            }
            if let Some(t) = &caption {
                new_steps.push(scene::caption(t));
            }
            if let Some(e) = &play {
                new_steps.push(scene::effect(e));
            }
            if let Some(s) = &erase {
                new_steps.push(scene::clear(s)?);
            }
            if new_steps.len() > 1 {
                bail!("add one step per command: use one of --paint, --write, --line, --image, --caption, --play, --erase. The steps play in the order you add them");
            }
            let step_flags = at.is_some() || dur.is_some() || stagger.is_some() || over.is_some() || fit.is_some() || opacity.is_some() || enter.is_some() || width.is_some() || closed || size.is_some() || color.is_some();
            match new_steps.pop() {
                Some(mut s) => {
                    s.at = at;
                    s.dur = dur;
                    s.stagger = stagger;
                    s.over = over.as_deref().map(scene::parse_over).transpose()?;
                    s.fit = fit.map(|f| f.to_ascii_lowercase());
                    s.opacity = opacity;
                    s.enter = enter.map(|e| e.to_ascii_lowercase());
                    s.width = width;
                    s.closed = closed;
                    s.size = size;
                    if let Some(c) = color {
                        s.color = Some(c.to_ascii_lowercase());
                    }
                    // A flag the step's kind does not take is an error, not a silent drop.
                    let n = p.ending.scene.len() + 1;
                    scene::from_value(serde_json::json!([serde_json::to_value(&s)?])).map_err(|e| anyhow::anyhow!("{}", e.to_string().replace("scene step 1", &format!("scene step {n}"))))?;
                    p.ending.scene.push(s);
                }
                None if step_flags => bail!("--at, --for, --stagger, --over and the other step flags go with a new step: add --paint, --write, --line, --image, --caption, --play or --erase"),
                None => {}
            }
            if p.ending != before {
                p.save(&path)?;
            }
            if json {
                println!("{}", serde_json::to_string(&if no_images { scene::without_images(&p.ending) } else { p.ending.clone() })?);
            } else if p.ending.is_empty() {
                println!("\"{}\" ends with the plain \"Solved!\" message. Effects: {}.", p.title, puzzle::EFFECTS.join(", "));
            } else {
                let f = &p.ending;
                println!("When a solver finishes \"{}\":", p.title);
                println!("  Message:   {}", if f.message.is_empty() { "Solved!" } else { &f.message });
                if !f.note.is_empty() {
                    println!("  Note:      {}", f.note);
                }
                println!("  Effects:   {}", if f.effects.is_empty() { "none".to_string() } else { f.effects.join(", ") });
                if !f.emoji.is_empty() {
                    println!("  Emoji:     {}", f.emoji.join(" "));
                }
                if !f.highlight.is_empty() {
                    println!("  Lights up: {} square(s)", f.highlight.len());
                }
                if let Some(l) = &f.link {
                    println!("  Link:      {} ({})", l.label, l.url);
                }
                if !f.scene.is_empty() {
                    let end = scene::times(&f.scene).iter().fold(0.0_f64, |m, t| m.max(t.1));
                    println!("  Scene:     {} step(s), {end:.1} seconds", f.scene.len());
                    for l in scene::listing(&f.scene) {
                        println!("    {l}");
                    }
                }
                println!("An ending plays on a solving page, when the grid is right.");
            }
        }
        Cmd::Export { puzzle, format, out, answers, scale, style, blank_grid, name, address, city, email } => {
            let (mut p, file) = ctx.load(&puzzle)?;
            let out = out.unwrap_or_else(|| PathBuf::from(file.file_stem().unwrap()).with_extension(&format));
            let flags = profile::Update { name, address, city, email };
            if style != "page" {
                if format != "pdf" {
                    bail!("--style {style} goes with --format pdf");
                }
                submission::Style::parse(&style)?;
                eprintln!("Note: this is now `puz publish nyt {puzzle}`.");
                write_submission(&p, &out, blank_grid, flags)?;
            } else {
                if blank_grid || !flags.is_empty() {
                    bail!("--blank-grid, --name, --address, --city and --email go with the NYT submission PDF: puz publish nyt PUZZLE");
                }
                if p.author.trim().is_empty() {
                    p.author = default_author().unwrap_or_default();
                }
                let e = export::export(&p, &format, answers, scale)?;
                std::fs::write(&out, e.bytes)?;
                if !e.dropped.is_empty() {
                    eprintln!("Warning: .{format} can't hold {}; left out.", e.dropped.join(", "));
                }
            }
            println!("Wrote {}", out.display());
        }
        Cmd::Profile { name, address, city, email, co_name, co_address, co_city, co_email, no_co, json } => {
            let file = profile::user_file();
            let mut prof = Profile::load(&file)?;
            let me = profile::Update { name, address, city, email };
            let co = profile::Update { name: co_name, address: co_address, city: co_city, email: co_email };
            let changed = !me.is_empty() || !co.is_empty() || no_co;
            me.apply(&mut prof.me);
            if no_co {
                prof.partner = None;
            } else if !co.is_empty() {
                co.apply(prof.partner.get_or_insert_with(Default::default));
            }
            if changed {
                prof.save(&file)?;
                prof = Profile::load(&file)?;
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&prof)?);
            } else {
                println!("{}\nKept in {}", profile::render(&prof), file.display());
            }
        }
        Cmd::Card { puzzle, out, footer, json } => {
            let (p, path) = ctx.load(&puzzle)?;
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "puzzle".into());
            let out = out.unwrap_or_else(|| PathBuf::from(format!("{stem}-card.png")));
            std::fs::write(&out, print::puzzle_card_png(&p, footer.as_deref().unwrap_or(""))?)?;
            if json {
                println!("{}", serde_json::json!({ "out": out.display().to_string(), "width": 1200, "height": 630 }));
            } else {
                println!("Wrote {}", out.display());
            }
        }
        Cmd::Guide { topic, json } => match topic {
            Some(t) => {
                let text = guides::guide(&t).with_context(|| format!("no guide {t:?}. The guides are: {}", guides::GUIDES.iter().map(|g| g.0).collect::<Vec<_>>().join(", ")))?;
                if json {
                    println!("{}", serde_json::json!({ "topic": t, "text": text }));
                } else {
                    print!("{text}");
                }
            }
            None if json => println!("{}", serde_json::json!({ "guides": guides::GUIDES.iter().map(|(n, d, _)| serde_json::json!({ "topic": n, "about": d })).collect::<Vec<_>>() })),
            None => {
                for (n, d, _) in guides::GUIDES {
                    println!("{n:<14} {d}");
                }
            }
        },
    }
    Ok(0)
}
