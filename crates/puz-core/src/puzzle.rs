//! The puz puzzle file: a small JSON document that any tool can read and edit.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Puzzle {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub copyright: String,
    /// "rotational", "mirror", or "none". Used when suggesting black squares.
    #[serde(default = "default_symmetry")]
    pub symmetry: String,
    /// One string per row. `#` is a black square, `.` is empty, letters are fill.
    pub grid: Vec<String>,
    /// Clue text keyed by answer: the answer's letters in capitals, with a rebus square spelled out
    /// (`[STAR]AT` is "STARAT"). A clue follows its word: move the word and the clue moves too.
    /// Change the letters and the clue stays stored under the old answer, so it comes back with the
    /// word. An answer with an empty square can't have an answer key, so its clue is keyed by slot,
    /// "x,y,across" or "x,y,down" (0-based start square), until the answer is complete. Older files
    /// keyed every clue by slot; `tidy` moves those to answer keys.
    #[serde(default)]
    pub clues: BTreeMap<String, String>,
    /// Picture clues, keyed like `clues` (by answer, else by slot): an image the solver sees with
    /// the clue, and its description. A picture clue's text may be empty. See `pictures`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pictures: BTreeMap<String, CluePicture>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub finished: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Rebus squares: "x,y" -> the letters in that square (A-Z, 2 or more). The grid holds the first
    /// letter, so a reader that doesn't know rebuses still sees a sensible grid.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rebus: BTreeMap<String, String>,
    /// Circled squares, as "x,y".
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub circles: BTreeSet<String>,
    /// Shaded (grey) squares, as "x,y".
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub shaded: BTreeSet<String>,
    /// Colored squares: "x,y" -> a color name from `COLORS` (red, orange, ...). A color replaces
    /// grey shading on that square. Black squares may be colored too.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, String>,
    /// Squares left out of the grid, as "x,y": they draw as empty space, so the grid can have any
    /// shape. Each one is a black square (`#`) in `grid`, so the numbering, a filler and
    /// readers that don't know this field treat it as black.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub absent: BTreeSet<String>,
    /// Walls (bars) between squares: "x,y" -> ["bottom", "right"]. A wall on the left or top of a
    /// square is stored on its neighbor (the right wall of x-1, or the bottom wall of y-1).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bars: BTreeMap<String, Vec<String>>,
    /// Pictures on squares: "x,y" -> an emoji or an image. They may sit on black squares.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sprites: BTreeMap<String, Sprite>,
    /// Locked squares, as "x,y". A locked square keeps its letter (or rebus): edits that would
    /// change it fail until it is unlocked. Only filled white squares can be locked.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub locked: BTreeSet<String>,
    /// The paper whose rules `stats` checks, e.g. "nyt-daily" (see `stats::PRESETS`). Left out
    /// means "auto": pick the rules from the grid size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Data other tools keep in the file about sending the puzzle to a paper, by paper name. This
    /// crate does not read it: it is kept as it is, so an edit never loses it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub submission: BTreeMap<String, serde_json::Value>,
    /// The date (YYYY-MM-DD) the puzzle is public from. Left out means private. A tool that puts
    /// puzzles online sets and reads it; nothing here does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    /// What a solver sees on finishing the puzzle on the solving page: a message, an effect, squares
    /// that light up, a link (`ending`). Left out means the plain "Solved!" message.
    #[serde(default, skip_serializing_if = "Ending::is_empty")]
    pub ending: Ending,
    /// The user's own fields, name -> value ("Publishing date" -> "2026-12-01"). Tools that
    /// manage many puzzles can sort by them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<String, String>,
    /// Where the puzzle is shared: one record per link on a Swords solving site (the site, the
    /// puzzle's id and address there, when it was pushed). A tool that pushes the puzzle writes
    /// it. This crate does not read it: it is kept as it is, so an edit never loses it. It never
    /// holds a key or a PIN.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub swords: Vec<serde_json::Value>,
    /// Fields this crate does not know, kept as they are, so an edit or a change of file format
    /// never loses them.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// The effects a puzzle may play when it is solved. A solving page draws them.
///   confetti  colored paper falls
///   rain      the puzzle's emoji fall
///   burst     the puzzle's emoji fly out from the middle
///   wave      the squares flash in a wave across the grid
pub const EFFECTS: [&str; 4] = ["confetti", "rain", "burst", "wave"];

/// What happens when a solver finishes the puzzle. Every field is optional. It is data, never code:
/// a solving page plays the effects it knows and skips the rest.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Ending {
    /// The headline, in place of "Solved!".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    /// A line under the headline, after the solve time: a punchline, a thank-you, what the theme was.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Effects from `EFFECTS`, played together.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<String>,
    /// The emoji that "rain" and "burst" use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emoji: Vec<String>,
    /// Squares that light up, as "x,y": a hidden word, a shape.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub highlight: BTreeSet<String>,
    /// A button under the message, e.g. to the next puzzle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<EndingLink>,
    /// Steps that play over the grid after the solve: squares recolor, letters change, lines are
    /// drawn, pictures and captions appear (`scene::Step`). The last state stays on screen.
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::scene::deserialize")]
    pub scene: Vec<crate::scene::Step>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EndingLink {
    pub label: String,
    /// An http or https address.
    pub url: String,
}

impl Ending {
    pub fn is_empty(&self) -> bool {
        *self == Ending::default()
    }
}

/// A picture on a square: exactly one of `emoji` or `src` (a `data:image/...` URI).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Sprite {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    /// Text that describes an image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}

/// A clue's picture: `image` is a `data:image/...;base64,` URL (PNG, JPEG, WebP or GIF).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CluePicture {
    pub image: String,
    /// Text that describes the image, for solvers who cannot see it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alt: String,
}

/// Square colors: name, the tint on a white square, the color of a black square.
/// Editors and solving pages that draw the grid mirror this.
pub const COLORS: [(&str, &str, &str); 7] = [
    ("red", "#f7b5b5", "#d92d20"),
    ("orange", "#fcd5a5", "#e8590c"),
    ("yellow", "#fbec9a", "#c99a06"),
    ("green", "#bfe5c4", "#1e9e4a"),
    ("blue", "#bcd8f6", "#2563eb"),
    ("purple", "#dbcaf4", "#7c3aed"),
    ("pink", "#f8c4de", "#d6297a"),
];

/// A color's (tint, strong) hex values by name.
pub fn color(name: &str) -> Option<(&'static str, &'static str)> {
    COLORS.iter().find(|c| c.0 == name).map(|c| (c.1, c.2))
}

pub fn color_names() -> String {
    COLORS.iter().map(|c| c.0).collect::<Vec<_>>().join(", ")
}

/// Smallest and largest grid width or height (`new`, `resize`).
pub const MIN_SIZE: usize = 2;
pub const MAX_SIZE: usize = 40;
/// The most characters a puzzle note may hold (`edit::set_notes`).
pub const NOTES_MAX: usize = 1000;

/// Largest image a sprite may hold, in bytes of image data.
pub const SPRITE_MAX_BYTES: usize = 300 * 1024;

/// The side of a square a wall is on, as stored in the file.
pub const RIGHT: &str = "right";
pub const BOTTOM: &str = "bottom";

/// Is this clue key a slot key ("x,y,across") rather than an answer ("CAT")?
pub fn is_slot_key(k: &str) -> bool {
    k.contains(',')
}

/// "x,y" for a square.
pub fn square_key(x: usize, y: usize) -> String {
    format!("{x},{y}")
}

/// (x, y) from "x,y".
pub fn parse_square_key(k: &str) -> Option<(usize, usize)> {
    let (x, y) = k.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

impl Puzzle {
    pub fn blank(title: &str, width: usize, height: usize, symmetry: &str) -> Puzzle {
        Puzzle {
            version: 1,
            title: title.into(),
            author: String::new(),
            copyright: String::new(),
            symmetry: symmetry.into(),
            grid: vec![".".repeat(width); height],
            clues: BTreeMap::new(),
            pictures: BTreeMap::new(),
            finished: false,
            notes: String::new(),
            rebus: BTreeMap::new(),
            circles: BTreeSet::new(),
            shaded: BTreeSet::new(),
            colors: BTreeMap::new(),
            absent: BTreeSet::new(),
            bars: BTreeMap::new(),
            sprites: BTreeMap::new(),
            locked: BTreeSet::new(),
            target: None,
            submission: BTreeMap::new(),
            published: None,
            ending: Ending::default(),
            meta: BTreeMap::new(),
            swords: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

fn default_version() -> u32 {
    1
}
fn default_symmetry() -> String {
    "rotational".into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Across,
    Down,
}

impl Dir {
    pub fn name(self) -> &'static str {
        match self {
            Dir::Across => "across",
            Dir::Down => "down",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SlotInfo {
    pub key: String,
    pub number: usize,
    pub dir: Dir,
    pub x: usize,
    pub y: usize,
    pub len: usize,
    /// Current letters, with '.' for empty squares. A rebus square shows as its letters in
    /// brackets, e.g. `ST[STAR]T`, so the pattern can be longer than `len`.
    pub pattern: String,
    /// True when the answer passes through a rebus square. A filler leaves such answers out.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub rebus: bool,
}

impl SlotInfo {
    /// The answer's letters with rebus squares spelled out (`[STAR]AT` gives "STARAT"), or None
    /// while a square is empty. Clues are keyed by this.
    pub fn answer(&self) -> Option<String> {
        if self.pattern.contains('.') {
            return None;
        }
        Some(self.pattern.chars().filter(|c| *c != '[' && *c != ']').collect())
    }

    /// The squares of this answer, in order.
    pub fn cells(&self) -> Vec<(usize, usize)> {
        (0..self.len)
            .map(|i| match self.dir {
                Dir::Across => (self.x + i, self.y),
                Dir::Down => (self.x, self.y + i),
            })
            .collect()
    }
}

impl Puzzle {
    pub fn load(path: &Path) -> Result<Puzzle> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut p = Puzzle::from_json(&text).with_context(|| format!("parsing {}", path.display()))?;
        p.tidy();
        p.validate()?;
        Ok(p)
    }

    /// A puzzle from JSON text: an ipuz file (with or without the Swords extension) or the older
    /// puz JSON. Not tidied or checked.
    pub fn from_json(text: &str) -> Result<Puzzle> {
        let v: serde_json::Value = serde_json::from_str(text)?;
        if crate::ipuz::is_ipuz(&v) {
            crate::ipuz::from_ipuz(&v)
        } else {
            Ok(serde_json::from_value(v)?)
        }
    }

    /// Write the file as ipuz. Marks that no longer fit the grid are dropped first (see `tidy`).
    ///
    /// A file in the older puz JSON changes to ipuz on its first save, keeping its name. Before
    /// that, the puzzle is turned into ipuz and back, and must come back the same. If it does not,
    /// the file is saved in the older JSON and a note goes to stderr. On the first change, a copy
    /// of the old file is kept beside it as `.NAME.before-ipuz` (hidden from listings).
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut p = self.clone();
        p.tidy();
        p.validate()?;
        let text = match crate::ipuz::to_ipuz_checked(&p) {
            Ok(v) => {
                let old = std::fs::read_to_string(path).ok();
                let was_old_json = old.as_deref().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()).is_some_and(|v| !crate::ipuz::is_ipuz(&v));
                if was_old_json {
                    if let (Some(old), Some(name)) = (old, path.file_name()) {
                        let backup = path.with_file_name(format!(".{}.before-ipuz", name.to_string_lossy()));
                        if !backup.exists() {
                            std::fs::write(&backup, old).with_context(|| format!("keeping a copy in {}", backup.display()))?;
                        }
                    }
                }
                serde_json::to_string_pretty(&v)?
            }
            Err(e) => {
                eprintln!("note: {} stays in the older puz JSON format: {e}", path.display());
                serde_json::to_string_pretty(&p)?
            }
        };
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text + "\n")?;
        std::fs::rename(&tmp, path)?; // atomic, so the UI never reads half a file
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.grid.is_empty() {
            bail!("grid is empty");
        }
        let w = self.width();
        for (i, row) in self.grid.iter().enumerate() {
            if row.chars().count() != w {
                bail!("row {} has {} squares, expected {}", i + 1, row.chars().count(), w);
            }
            if let Some(c) = row.chars().find(|c| !(c.is_alphabetic() || *c == '#' || *c == '.')) {
                bail!("row {} has an invalid character {:?}", i + 1, c);
            }
        }
        self.validate_marks()?;
        crate::scene::validate(&self.ending.scene, w, self.height())?;
        Ok(())
    }

    /// Check the rebus, circle, shading, wall and sprite fields.
    fn validate_marks(&self) -> Result<()> {
        let (w, h) = (self.width(), self.height());
        let square = |what: &str, k: &str| -> Result<(usize, usize)> {
            match parse_square_key(k) {
                Some((x, y)) if x < w && y < h => Ok((x, y)),
                Some(_) => bail!("{what} square {k} is outside the {w}x{h} grid"),
                None => bail!("{what} square {k:?} must look like x,y"),
            }
        };
        for (k, letters) in &self.rebus {
            square("rebus", k)?;
            if letters.chars().count() < 2 || !letters.chars().all(|c| c.is_ascii_uppercase()) {
                bail!("rebus at {k} must be 2 or more capital letters A-Z, not {letters:?}");
            }
        }
        for k in &self.circles {
            square("circle", k)?;
        }
        for k in &self.shaded {
            square("shaded", k)?;
        }
        for k in &self.locked {
            square("locked", k)?;
        }
        for k in &self.absent {
            square("absent", k)?;
        }
        for (k, name) in &self.colors {
            square("color", k)?;
            if color(name).is_none() {
                bail!("color at {k} must be one of {}, not {name:?}", color_names());
            }
        }
        for (k, sides) in &self.bars {
            let (x, y) = square("wall", k)?;
            for side in sides {
                match side.as_str() {
                    RIGHT if x + 1 >= w => bail!("wall at {k} is on the right edge of the grid"),
                    BOTTOM if y + 1 >= h => bail!("wall at {k} is on the bottom edge of the grid"),
                    RIGHT | BOTTOM => {}
                    other => bail!("wall at {k} must be \"right\" or \"bottom\", not {other:?} (store left and top walls on the neighbor)"),
                }
            }
        }
        for (k, sp) in &self.sprites {
            square("sprite", k)?;
            match (&sp.emoji, &sp.src) {
                (Some(e), None) if !e.trim().is_empty() && e.chars().count() <= 16 => {}
                (Some(_), None) => bail!("sprite at {k}: emoji must be 1 to 16 characters"),
                (None, Some(src)) => {
                    if !src.starts_with("data:image/") {
                        bail!("sprite at {k}: src must be a data:image/... URI");
                    }
                    // Base64 makes 3 bytes into 4 characters.
                    if src.len() > SPRITE_MAX_BYTES * 4 / 3 + 100 {
                        bail!("sprite at {k}: the image is larger than {} KB", SPRITE_MAX_BYTES / 1024);
                    }
                }
                _ => bail!("sprite at {k} needs exactly one of emoji or src"),
            }
        }
        Ok(())
    }

    /// Drop marks that no longer make sense after the grid changed:
    /// - a rebus on a black or empty square, or whose first letter no longer matches the grid,
    /// - a circle on a black square,
    /// - a wall next to a black square,
    /// - a lock on a black or empty square.
    ///
    /// Shading and sprites may stay on black squares. Marks outside the grid are left for
    /// `validate` to report.
    pub fn tidy(&mut self) {
        let (w, h) = (self.width(), self.height());
        let cell = |p: &Puzzle, k: &str| parse_square_key(k).filter(|&(x, y)| x < w && y < h && p.grid[y].chars().count() == w).map(|(x, y)| p.cell(x, y));
        let snapshot = self.clone();
        for v in self.rebus.values_mut() {
            *v = v.trim().to_ascii_uppercase();
        }
        self.rebus.retain(|k, v| match cell(&snapshot, k) {
            Some(c) => c != '#' && c != '.' && v.chars().next().map(|f| f.eq_ignore_ascii_case(&c)).unwrap_or(false),
            None => true,
        });
        self.circles.retain(|k| cell(&snapshot, k) != Some('#'));
        // Only a black square can be left out of the grid, and a left-out square has no look.
        self.absent.retain(|k| matches!(cell(&snapshot, k), Some('#') | None));
        let absent = self.absent.clone();
        self.shaded.retain(|k| !absent.contains(k));
        self.colors.retain(|k, _| !absent.contains(k));
        self.sprites.retain(|k, _| !absent.contains(k));
        self.locked.retain(|k| !matches!(cell(&snapshot, k), Some('#' | '.')));
        for (k, sides) in self.bars.iter_mut() {
            sides.sort();
            sides.dedup();
            let Some((x, y)) = parse_square_key(k) else { continue };
            if cell(&snapshot, k) == Some('#') {
                sides.clear();
                continue;
            }
            sides.retain(|s| {
                let next = match s.as_str() {
                    RIGHT => square_key(x + 1, y),
                    BOTTOM => square_key(x, y + 1),
                    _ => return true,
                };
                cell(&snapshot, &next) != Some('#')
            });
        }
        self.bars.retain(|_, sides| !sides.is_empty());
        self.tidy_clues();
    }

    /// Clean up the clue keys:
    /// - keep only `<i>` markup in the text (see `markup`), and drop clues with no text,
    /// - put answer keys in capitals,
    /// - move a slot-keyed clue to its answer once the answer is complete. If the answer already
    ///   has a different clue, the slot-keyed one stays, so no clue text is lost.
    fn tidy_clues(&mut self) {
        for t in self.clues.values_mut() {
            if t.contains('<') {
                *t = crate::markup::normalize(t);
            }
        }
        self.clues.retain(|_, t| !crate::markup::plain(t).trim().is_empty());
        let lower: Vec<String> = self.clues.keys().filter(|k| !is_slot_key(k) && k.chars().any(|c| c.is_lowercase())).cloned().collect();
        for k in lower {
            let up = k.to_uppercase();
            if !self.clues.contains_key(&up) {
                let t = self.clues.remove(&k).unwrap();
                self.clues.insert(up, t);
            }
        }
        if self.grid.is_empty() || self.grid.iter().any(|r| r.chars().count() != self.width()) {
            return; // `validate` reports a bad grid
        }
        self.pictures.retain(|_, pic| !pic.image.trim().is_empty());
        let lower: Vec<String> = self.pictures.keys().filter(|k| !is_slot_key(k) && k.chars().any(|c| c.is_lowercase())).cloned().collect();
        for k in lower {
            let up = k.to_uppercase();
            if !self.pictures.contains_key(&up) {
                let pic = self.pictures.remove(&k).unwrap();
                self.pictures.insert(up, pic);
            }
        }
        if self.grid.is_empty() || self.grid.iter().any(|r| r.chars().count() != self.width()) {
            return;
        }
        for s in self.slots() {
            let Some(answer) = s.answer() else { continue };
            let Some(pic) = self.pictures.get(&s.key).cloned() else { continue };
            match self.pictures.get(&answer) {
                None => {
                    self.pictures.remove(&s.key);
                    self.pictures.insert(answer, pic);
                }
                Some(q) if *q == pic => {
                    self.pictures.remove(&s.key);
                }
                Some(_) => {}
            }
        }
        for s in self.slots() {
            let Some(answer) = s.answer() else { continue };
            let Some(text) = self.clues.get(&s.key).cloned() else { continue };
            match self.clues.get(&answer) {
                None => {
                    self.clues.remove(&s.key);
                    self.clues.insert(answer, text);
                }
                Some(t) if *t == text => {
                    self.clues.remove(&s.key);
                }
                Some(_) => {}
            }
        }
    }

    /// The key a slot's clue is stored under: its answer when complete, else the slot key.
    pub fn clue_key(&self, s: &SlotInfo) -> String {
        s.answer().unwrap_or_else(|| s.key.clone())
    }

    /// The clue a slot shows: the clue for its answer, else a clue keyed by the slot itself.
    pub fn clue_for(&self, s: &SlotInfo) -> Option<&str> {
        let get = |k: &str| self.clues.get(k).map(|t| t.trim()).filter(|t| !t.is_empty());
        s.answer().and_then(|a| get(&a)).or_else(|| get(&s.key))
    }

    /// The key of the picture a slot shows (its answer's, else one keyed by the slot), if any.
    pub fn picture_key(&self, s: &SlotInfo) -> Option<String> {
        s.answer().filter(|a| self.pictures.contains_key(a)).or_else(|| self.pictures.contains_key(&s.key).then(|| s.key.clone()))
    }

    /// The picture a slot shows, if any.
    pub fn picture_for(&self, s: &SlotInfo) -> Option<&CluePicture> {
        self.picture_key(s).and_then(|k| self.pictures.get(&k))
    }

    /// Stored clues that no slot shows now: answers no longer in the grid, and slot-keyed clues
    /// whose slot is gone or now shows another clue. Pairs of (key, text).
    pub fn unused_clues(&self) -> Vec<(String, String)> {
        let slots = self.slots();
        let shown: HashSet<String> = slots
            .iter()
            .filter_map(|s| {
                let a = s.answer();
                match &a {
                    Some(a) if self.clues.get(a).is_some_and(|t| !t.trim().is_empty()) => Some(a.clone()),
                    _ => self.clues.contains_key(&s.key).then(|| s.key.clone()),
                }
            })
            .collect();
        self.clues.iter().filter(|(k, _)| !shown.contains(*k)).map(|(k, t)| (k.clone(), t.clone())).collect()
    }

    /// The rebus letters at a square, if it holds a rebus.
    /// True when the square is left out of the grid (drawn as empty space).
    pub fn is_absent(&self, x: usize, y: usize) -> bool {
        !self.absent.is_empty() && self.absent.contains(&square_key(x, y))
    }

    /// True when the square is outside the grid or left out of it.
    pub fn no_square(&self, x: isize, y: isize) -> bool {
        x < 0 || y < 0 || x >= self.width() as isize || y >= self.height() as isize || self.is_absent(x as usize, y as usize)
    }

    /// The fill for a square's background: its color (tint on white, strong on black), else grey
    /// shading, else None.
    pub fn square_fill(&self, x: usize, y: usize) -> Option<&'static str> {
        let k = square_key(x, y);
        let black = self.cell(x, y) == '#';
        if let Some((tint, strong)) = self.colors.get(&k).and_then(|n| color(n)) {
            return Some(if black { strong } else { tint });
        }
        self.shaded.contains(&k).then_some(if black { "#666" } else { "#c8c8c8" })
    }

    pub fn rebus_at(&self, x: usize, y: usize) -> Option<&str> {
        self.rebus.get(&square_key(x, y)).map(String::as_str)
    }

    /// Is there a wall on the right (`RIGHT`) or bottom (`BOTTOM`) of this square?
    pub fn has_bar(&self, x: usize, y: usize, side: &str) -> bool {
        self.bars.get(&square_key(x, y)).is_some_and(|s| s.iter().any(|v| v == side))
    }

    pub fn width(&self) -> usize {
        self.grid[0].chars().count()
    }
    pub fn height(&self) -> usize {
        self.grid.len()
    }

    pub fn cell(&self, x: usize, y: usize) -> char {
        self.grid[y].chars().nth(x).unwrap()
    }

    pub fn is_block(&self, x: isize, y: isize) -> bool {
        x < 0 || y < 0 || x as usize >= self.width() || y as usize >= self.height() || self.cell(x as usize, y as usize) == '#'
    }

    /// All slots (answers of 2 or more squares) with standard numbering. A black square or a wall
    /// ends an answer: an across answer stops at a square with a wall on its right, a down answer at
    /// a square with a wall on its bottom. A run of one square is not an answer.
    pub fn slots(&self) -> Vec<SlotInfo> {
        let (w, h) = (self.width() as isize, self.height() as isize);
        let walls: HashSet<(isize, isize, bool)> = self
            .bars
            .iter()
            .filter_map(|(k, sides)| parse_square_key(k).map(|(x, y)| (x as isize, y as isize, sides)))
            .flat_map(|(x, y, sides)| sides.iter().filter_map(move |s| match s.as_str() {
                RIGHT => Some((x, y, true)),
                BOTTOM => Some((x, y, false)),
                _ => None,
            }))
            .collect();
        // Can an answer step from (x, y) one square in this direction?
        let open = |x: isize, y: isize, across: bool| {
            let (nx, ny) = if across { (x + 1, y) } else { (x, y + 1) };
            !self.is_block(x, y) && !self.is_block(nx, ny) && !walls.contains(&(x, y, across))
        };
        let mut out = vec![];
        let mut number = 0;
        for y in 0..h {
            for x in 0..w {
                if self.is_block(x, y) {
                    continue;
                }
                let starts_across = !open(x - 1, y, true) && open(x, y, true);
                let starts_down = !open(x, y - 1, false) && open(x, y, false);
                if !(starts_across || starts_down) {
                    continue;
                }
                number += 1;
                for (starts, dir) in [(starts_across, Dir::Across), (starts_down, Dir::Down)] {
                    if !starts {
                        continue;
                    }
                    let across = dir == Dir::Across;
                    let (dx, dy) = if across { (1, 0) } else { (0, 1) };
                    let mut pattern = String::new();
                    let mut has_rebus = false;
                    let mut len = 0;
                    let (mut cx, mut cy) = (x, y);
                    loop {
                        let c = self.cell(cx as usize, cy as usize);
                        match self.rebus_at(cx as usize, cy as usize).filter(|_| c != '.' && c != '#') {
                            Some(r) => {
                                has_rebus = true;
                                pattern.push('[');
                                pattern.push_str(r);
                                pattern.push(']');
                            }
                            None => pattern.push(if c == '.' { '.' } else { c.to_ascii_uppercase() }),
                        }
                        len += 1;
                        if !open(cx, cy, across) {
                            break;
                        }
                        cx += dx;
                        cy += dy;
                    }
                    out.push(SlotInfo {
                        key: format!("{},{},{}", x, y, dir.name()),
                        number,
                        dir,
                        x: x as usize,
                        y: y as usize,
                        len,
                        pattern,
                        rebus: has_rebus,
                    });
                }
            }
        }
        out
    }

    pub fn find_slot(&self, key_or_label: &str) -> Result<SlotInfo> {
        let slots = self.slots();
        let k = key_or_label.trim().to_lowercase();
        // Accept "x,y,across", or labels like "41a" / "41-across" / "17d".
        if let Some(s) = slots.iter().find(|s| s.key == k) {
            return Ok(s.clone());
        }
        let digits: String = k.chars().take_while(|c| c.is_ascii_digit()).collect();
        let rest = k[digits.len()..].trim_start_matches(['-', ' ']);
        if let Ok(n) = digits.parse::<usize>() {
            let dir = if rest.starts_with('a') { Some(Dir::Across) } else if rest.starts_with('d') { Some(Dir::Down) } else { None };
            if let Some(s) = slots.iter().find(|s| s.number == n && Some(s.dir) == dir) {
                return Ok(s.clone());
            }
        }
        bail!("no slot matches {:?} (use e.g. 41a, 17d, or 5,7,across)", key_or_label)
    }

    pub fn print(&self) -> String {
        self.grid
            .iter()
            .map(|r| r.chars().map(|c| format!("{} ", c.to_ascii_uppercase())).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The grid as text with its marks, for `show`. A `|` between two squares is a wall. Under a
    /// square, `-` is a wall on its bottom. A legend lists every mark by square.
    pub fn print_marked(&self) -> String {
        let (w, h) = (self.width(), self.height());
        let mut lines = vec![];
        for y in 0..h {
            let mut row = String::new();
            for x in 0..w {
                row.push(self.cell(x, y).to_ascii_uppercase());
                if x + 1 < w {
                    row.push(if self.has_bar(x, y, RIGHT) { '|' } else { ' ' });
                }
            }
            lines.push(row);
            if (0..w).any(|x| self.has_bar(x, y, BOTTOM)) {
                let under: String = (0..w).map(|x| if self.has_bar(x, y, BOTTOM) { "- " } else { "  " }).collect();
                lines.push(under.trim_end().to_string());
            }
        }
        lines.extend(self.marks_legend());
        lines.join("\n")
    }

    /// One line per kind of mark, e.g. "Circles: 3,4 5,0". Empty when the puzzle has none.
    pub fn marks_legend(&self) -> Vec<String> {
        let mut out = vec![];
        let list = |set: &BTreeSet<String>| set.iter().cloned().collect::<Vec<_>>().join(" ");
        if !self.rebus.is_empty() {
            out.push(format!("Rebus: {}", self.rebus.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")));
        }
        if !self.circles.is_empty() {
            out.push(format!("Circles: {}", list(&self.circles)));
        }
        if !self.shaded.is_empty() {
            out.push(format!("Shaded: {}", list(&self.shaded)));
        }
        if !self.colors.is_empty() {
            out.push(format!("Colors: {}", self.colors.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")));
        }
        if !self.absent.is_empty() {
            out.push(format!("Gaps (no square): {}", list(&self.absent)));
        }
        if !self.locked.is_empty() {
            out.push(format!("Locked (keep these letters): {}", list(&self.locked)));
        }
        if !self.bars.is_empty() {
            out.push(format!("Walls: {}", self.bars.iter().flat_map(|(k, s)| s.iter().map(move |v| format!("{k} {v}"))).collect::<Vec<_>>().join(", ")));
        }
        if !self.sprites.is_empty() {
            let desc = |s: &Sprite| match (&s.emoji, &s.alt) {
                (Some(e), _) => e.clone(),
                (None, Some(a)) => format!("image ({a})"),
                (None, None) => "image".into(),
            };
            out.push(format!("Sprites: {}", self.sprites.iter().map(|(k, s)| format!("{k} {}", desc(s))).collect::<Vec<_>>().join(", ")));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(p: &Puzzle) -> Vec<(usize, String, usize)> {
        p.slots().into_iter().map(|s| (s.number, s.key, s.len)).collect()
    }

    #[test]
    fn walls_split_answers_and_numbering() {
        let mut p = Puzzle::blank("t", 4, 3, "none");
        // A wall right of 1,0 splits row 0 into two 2-square answers.
        p.bars.insert("1,0".into(), vec![RIGHT.into()]);
        // A wall under 3,0 leaves column 3 with a 1-square run on top (not an answer) and 2 below.
        p.bars.insert("3,0".into(), vec![BOTTOM.into()]);
        let k = keys(&p);
        assert!(k.contains(&(1, "0,0,across".into(), 2)), "{k:?}");
        assert!(k.contains(&(3, "2,0,across".into(), 2)), "{k:?}");
        assert!(k.contains(&(3, "2,0,down".into(), 3)), "{k:?}");
        assert!(!k.iter().any(|(_, key, _)| key == "3,0,down"), "{k:?}");
        // 1 = 0,0, 2 = 1,0 (down only), 3 = 2,0, then 3,0 gets no number, 4 = 0,1 across, 5 = 3,1 down.
        assert!(k.contains(&(2, "1,0,down".into(), 3)), "{k:?}");
        assert!(k.contains(&(4, "0,1,across".into(), 4)), "{k:?}");
        assert!(k.contains(&(5, "3,1,down".into(), 2)), "{k:?}");
    }

    #[test]
    fn rebus_pattern_and_flag() {
        let mut p = Puzzle::blank("t", 3, 3, "none");
        p.grid[0] = "SAT".into();
        p.rebus.insert("0,0".into(), "STAR".into());
        let s = p.find_slot("1a").unwrap();
        assert_eq!(s.pattern, "[STAR]AT");
        assert_eq!(s.len, 3);
        assert!(s.rebus);
        assert!(!p.find_slot("0,1,across").unwrap().rebus);
    }

    #[test]
    fn json_round_trip_and_old_files() {
        let mut p = Puzzle::blank("t", 3, 3, "none");
        p.grid[0] = "SAT".into();
        p.rebus.insert("0,0".into(), "STAR".into());
        p.circles.insert("1,1".into());
        p.shaded.insert("2,2".into());
        p.locked.insert("1,0".into());
        p.bars.insert("0,1".into(), vec![BOTTOM.into(), RIGHT.into()]);
        p.sprites.insert("2,0".into(), Sprite { emoji: Some("⭐".into()), ..Default::default() });
        p.sprites.insert("2,1".into(), Sprite { src: Some("data:image/png;base64,AAAA".into()), alt: Some("dot".into()), ..Default::default() });
        p.validate().unwrap();
        let text = serde_json::to_string_pretty(&p).unwrap();
        let back: Puzzle = serde_json::from_str(&text).unwrap();
        assert_eq!(back, p);
        // Old files have none of the new fields, and new fields are left out when empty.
        let plain = Puzzle::blank("t", 3, 3, "none");
        let text = serde_json::to_string(&plain).unwrap();
        for f in ["rebus", "circles", "shaded", "bars", "sprites", "locked"] {
            assert!(!text.contains(f), "{f} in {text}");
        }
        let old: Puzzle = serde_json::from_str(r#"{"grid": ["...", "...", "..."]}"#).unwrap();
        assert!(old.rebus.is_empty() && old.bars.is_empty() && old.locked.is_empty());
    }

    #[test]
    fn validation_errors() {
        let base = Puzzle::blank("t", 3, 3, "none");
        let bad = |f: &dyn Fn(&mut Puzzle)| {
            let mut p = base.clone();
            f(&mut p);
            p.validate().is_err()
        };
        assert!(bad(&|p| { p.circles.insert("3,0".into()); }));
        assert!(bad(&|p| { p.shaded.insert("a,b".into()); }));
        assert!(bad(&|p| { p.locked.insert("0,3".into()); }));
        assert!(bad(&|p| { p.rebus.insert("0,0".into(), "S".into()); }));
        assert!(bad(&|p| { p.rebus.insert("0,0".into(), "ST4R".into()); }));
        assert!(bad(&|p| { p.bars.insert("2,0".into(), vec![RIGHT.into()]); }));
        assert!(bad(&|p| { p.bars.insert("0,2".into(), vec![BOTTOM.into()]); }));
        assert!(bad(&|p| { p.bars.insert("0,0".into(), vec!["left".into()]); }));
        assert!(bad(&|p| { p.sprites.insert("0,0".into(), Sprite::default()); }));
        assert!(bad(&|p| { p.sprites.insert("0,0".into(), Sprite { emoji: Some("x".into()), src: Some("data:image/png;base64,".into()), alt: None }); }));
        assert!(bad(&|p| { p.sprites.insert("0,0".into(), Sprite { src: Some("https://example.com/a.png".into()), ..Default::default() }); }));
        assert!(bad(&|p| { p.sprites.insert("0,0".into(), Sprite { src: Some(format!("data:image/png;base64,{}", "A".repeat(500_000))), ..Default::default() }); }));
        assert!(!bad(&|p| { p.bars.insert("1,1".into(), vec![RIGHT.into(), BOTTOM.into()]); }));
    }

    #[test]
    fn tidy_drops_stale_marks() {
        let mut p = Puzzle::blank("t", 3, 3, "none");
        p.grid = vec!["S#.".into(), "...".into(), "...".into()];
        p.rebus.insert("0,0".into(), "star".into());
        p.rebus.insert("2,0".into(), "AB".into()); // empty square
        p.circles.insert("1,0".into()); // black
        p.shaded.insert("1,0".into()); // stays
        p.sprites.insert("1,0".into(), Sprite { emoji: Some("⭐".into()), ..Default::default() }); // stays
        p.bars.insert("0,0".into(), vec![RIGHT.into(), BOTTOM.into()]); // right is next to black
        p.locked.extend(["0,0".to_string(), "1,0".into(), "2,0".into()]); // letter, black, empty
        p.tidy();
        assert_eq!(p.locked.iter().collect::<Vec<_>>(), ["0,0"]);
        assert_eq!(p.rebus.get("0,0").map(String::as_str), Some("STAR"));
        assert!(!p.rebus.contains_key("2,0"));
        assert!(p.circles.is_empty());
        assert!(p.shaded.contains("1,0") && p.sprites.contains_key("1,0"));
        assert_eq!(p.bars["0,0"], vec![BOTTOM.to_string()]);
        // Overwriting the first letter drops the rebus.
        p.grid[0] = "X#.".into();
        p.tidy();
        assert!(p.rebus.is_empty());
    }
}
