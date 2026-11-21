//! Puzzle edits, shared by every program that changes a puzzle. Each one changes a `Puzzle` in memory; the
//! caller saves it.

use crate::puzzle::{parse_square_key, square_key, Puzzle, SlotInfo, Sprite, BOTTOM, RIGHT, SPRITE_MAX_BYTES};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

fn set_cell(p: &mut Puzzle, x: usize, y: usize, c: char) {
    let row: Vec<char> = p.grid[y].chars().enumerate().map(|(i, old)| if i == x { c } else { old }).collect();
    p.grid[y] = row.into_iter().collect();
}

fn cells(s: &SlotInfo) -> impl Iterator<Item = (usize, usize)> {
    s.cells().into_iter()
}

/// One square of a word given to `place`: a letter, `.`, `_`/`?`, or `[LETTERS]` for a rebus.
enum Token {
    Char(char),
    Rebus(String),
}

fn tokens(word: &str) -> Result<Vec<Token>> {
    let mut out = vec![];
    let mut it = word.chars().filter(|c| !c.is_whitespace());
    while let Some(c) = it.next() {
        if c == '[' {
            let inner: String = it.by_ref().take_while(|&c| c != ']').collect();
            out.push(Token::Rebus(inner.to_ascii_uppercase()));
        } else if c.is_alphabetic() || matches!(c, '.' | '?' | '_') {
            out.push(Token::Char(c));
        } else {
            bail!("invalid character {c:?} (use letters, '.' to clear, '_' to keep, [STAR] for a rebus)");
        }
    }
    Ok(out)
}

/// Write a word into a slot. `.` in the word clears that square; `_` or `?` keeps what is there;
/// `[STAR]` puts a rebus in one square. A single letter over a rebus square replaces the rebus
/// unless it is the rebus's first letter.
/// Fails if it would change a locked square; the same letters over a locked square are fine.
pub fn place(p: &mut Puzzle, slot: &str, word: &str) -> Result<SlotInfo> {
    let s = p.find_slot(slot)?;
    let toks = tokens(word.trim())?;
    if toks.len() != s.len {
        bail!("{} has {} squares but {:?} has {} letters", label(&s), s.len, word, toks.len());
    }
    guarded(p, |p| {
        for ((x, y), t) in cells(&s).collect::<Vec<_>>().into_iter().zip(toks) {
            match t {
                Token::Char('?' | '_') => {}
                Token::Char(c) => set_cell(p, x, y, if c == '.' { '.' } else { c.to_ascii_uppercase() }),
                Token::Rebus(r) => {
                    check_square(p, x, y)?;
                    write_rebus(p, x, y, &r)?
                }
            }
        }
        p.tidy();
        p.find_slot(&s.key)
    })
}

/// What `clear` and `clear_region` did: squares emptied, and locked squares left as they were.
#[derive(Debug, Clone, Serialize)]
pub struct Cleared {
    pub cleared: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept_locked: Vec<String>,
}

impl Cleared {
    pub fn message(&self) -> String {
        let mut m = format!("Cleared {} square(s).", self.cleared);
        if !self.kept_locked.is_empty() {
            m += &format!(" Kept {} locked square(s): {}.", self.kept_locked.len(), self.kept_locked.join(" "));
        }
        m
    }
}

/// Empty these squares, except black and locked ones.
fn clear_cells(p: &mut Puzzle, targets: impl IntoIterator<Item = (usize, usize)>) -> Cleared {
    let mut out = Cleared { cleared: 0, kept_locked: vec![] };
    for (x, y) in targets {
        let c = p.cell(x, y);
        if c == '#' || c == '.' {
            continue;
        }
        if p.locked.contains(&square_key(x, y)) {
            out.kept_locked.push(square_key(x, y));
            continue;
        }
        set_cell(p, x, y, '.');
        out.cleared += 1;
    }
    p.tidy();
    out
}

/// Clear the letters in one slot, or in the whole grid when `slot` is None. Locked squares keep
/// their letters.
pub fn clear(p: &mut Puzzle, slot: Option<&str>) -> Result<Cleared> {
    let targets: Vec<(usize, usize)> = match slot {
        Some(s) => cells(&p.find_slot(s)?).collect(),
        None => (0..p.height()).flat_map(|y| (0..p.width()).map(move |x| (x, y))).collect(),
    };
    Ok(clear_cells(p, targets))
}

/// Set (or, with empty text, remove) a slot's clue. The clue is stored under the slot's answer, so
/// it follows the word; an answer with an empty square stores it under the slot key until the
/// answer is complete (see `Puzzle::clues`). Returns the slot's label, e.g. "41A".
pub fn set_clue(p: &mut Puzzle, slot: &str, text: &str) -> Result<String> {
    let s = p.find_slot(slot)?;
    let text = crate::markup::normalize(text.trim());
    let text = text.trim().to_string();
    if crate::markup::plain(&text).trim().is_empty() {
        // Remove what the slot shows: the answer's clue and any slot-keyed clue behind it.
        if let Some(a) = s.answer() {
            p.clues.remove(&a);
        }
        p.clues.remove(&s.key);
    } else {
        p.clues.insert(p.clue_key(&s), text);
    }
    Ok(label(&s))
}

/// One slot and the clue it shows, for `clues --list`.
#[derive(Debug, Clone, Serialize)]
pub struct SlotClue {
    /// e.g. "41A".
    pub slot: String,
    /// The complete answer, or None while a square is empty.
    pub answer: Option<String>,
    pub clue: Option<String>,
    /// Set when the clue gives the answer away (see `clue_problem`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// Every slot with the clue it shows, in slot order.
pub fn slot_clues(p: &Puzzle) -> Vec<SlotClue> {
    p.slots()
        .iter()
        .map(|s| {
            let (answer, clue) = (s.answer(), p.clue_for(s).map(String::from));
            let problem = answer.as_deref().zip(clue.as_deref()).and_then(|(a, c)| clue_problem(a, c));
            SlotClue { slot: label(s), answer, clue, problem }
        })
        .collect()
}

/// Says why a clue gives its answer away, or None when it is fine. A clue must not hold the answer:
/// as a word ("Teen idol" for TEEN), inside a word ("Sweet sixteen" for TEEN), across words ("Ice
/// cream flavor" for ICECREAM), or as a part of the answer ("Snow figure" for SNOWMAN). Answers of
/// 3 letters or fewer only count as a whole word, since they sit inside many words by chance.
/// `web/src/lib/clue-check.ts` mirrors this.
pub fn clue_problem(answer: &str, clue: &str) -> Option<String> {
    let letters = |s: &str| s.chars().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_uppercase()).collect::<String>();
    let a = letters(answer);
    if a.len() < 2 {
        return None;
    }
    let plain = crate::markup::plain(clue);
    let words: Vec<(&str, String)> =
        plain.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '\u{2019}')).map(|w| (w, letters(w))).filter(|(_, l)| !l.is_empty()).collect();
    for (raw, w) in &words {
        if *w == a {
            return Some(format!("the clue has the answer {a} in it"));
        }
        if a.len() >= 4 && w.contains(&a) {
            return Some(format!("{a} is inside \"{raw}\""));
        }
        if w.len() >= 4 && a.len() > w.len() && (a.starts_with(w.as_str()) || a.ends_with(w.as_str())) {
            return Some(format!("\"{raw}\" is part of the answer {a}"));
        }
    }
    if a.len() >= 4 && words.len() > 1 {
        // The answer spelled across neighboring words.
        for i in 0..words.len() {
            let mut run = String::new();
            for (_, w) in &words[i..] {
                run.push_str(w);
                if run == a {
                    return Some(format!("the clue has the answer {a} in it"));
                }
                if run.len() >= a.len() {
                    break;
                }
            }
        }
    }
    None
}

/// Set many clues at once from `{ "41a": "text", "5,7,down": "text" }`. All slots are checked before
/// anything changes.
/// Set the puzzle note (the constructor's note): plain text, line breaks kept, at most
/// `NOTES_MAX` characters. Empty text removes it. Returns the stored note.
pub fn set_notes(p: &mut Puzzle, text: &str) -> Result<String> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim().to_string();
    let n = text.chars().count();
    if n > crate::puzzle::NOTES_MAX {
        bail!("the note has {n} characters; the limit is {}", crate::puzzle::NOTES_MAX);
    }
    p.notes = text.clone();
    Ok(text)
}

pub fn set_clues(p: &mut Puzzle, clues: &BTreeMap<String, String>) -> Result<Vec<String>> {
    for k in clues.keys() {
        p.find_slot(k)?;
    }
    clues.iter().map(|(k, t)| set_clue(p, k, t)).collect()
}

/// A rectangle of squares, corners inclusive, in any order. Errors if it leaves the grid.
pub fn region(p: &Puzzle, a: (usize, usize), b: (usize, usize)) -> Result<(usize, usize, usize, usize)> {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    if x1 >= p.width() || y1 >= p.height() {
        bail!("region {x0},{y0}..{x1},{y1} is outside the {}x{} grid", p.width(), p.height());
    }
    Ok((x0, y0, x1, y1))
}

/// A new blank puzzle. With `pattern_from`, it takes that puzzle's size, symmetry, black squares and
/// walls in place of `width`, `height` and `symmetry`. An empty title becomes "Untitled".
pub fn new_puzzle(title: &str, width: usize, height: usize, symmetry: &str, pattern_from: Option<&Puzzle>) -> Result<Puzzle> {
    use crate::puzzle::{MAX_SIZE, MIN_SIZE};
    let mut p = match pattern_from {
        Some(s) => {
            let mut p = Puzzle::blank(title, s.width(), s.height(), &s.symmetry);
            p.grid = s.grid.iter().map(|r| r.chars().map(|c| if c == '#' { '#' } else { '.' }).collect()).collect();
            p.bars = s.bars.clone();
            p
        }
        None => {
            if !(MIN_SIZE..=MAX_SIZE).contains(&width) || !(MIN_SIZE..=MAX_SIZE).contains(&height) {
                bail!("size must be between {MIN_SIZE} and {MAX_SIZE}");
            }
            Puzzle::blank(title, width, height, symmetry)
        }
    };
    if p.title.trim().is_empty() {
        p.title = "Untitled".into();
    }
    Ok(p)
}

/// Region as text: one line per row, letters, `#` for black, `.` for empty. Editors can use the
/// same format for copy and paste.
pub fn copy_region(p: &Puzzle, a: (usize, usize), b: (usize, usize)) -> Result<String> {
    let (x0, y0, x1, y1) = region(p, a, b)?;
    Ok((y0..=y1).map(|y| (x0..=x1).map(|x| p.cell(x, y).to_ascii_uppercase()).collect::<String>()).collect::<Vec<_>>().join("\n"))
}

/// Clear the letters in a region. Black squares and locked squares stay.
pub fn clear_region(p: &mut Puzzle, a: (usize, usize), b: (usize, usize)) -> Result<Cleared> {
    let (x0, y0, x1, y1) = region(p, a, b)?;
    Ok(clear_cells(p, (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y)))))
}

/// Paste region text with its top-left at (x, y). `#` adds a black square (and its symmetric
/// partner); a letter or `.` on a black square makes it white again (with its partner); space,
/// `_` and `?` keep the square. Text past the grid edge is ignored. Returns squares changed.
/// Fails if it would change a locked square.
pub fn paste(p: &mut Puzzle, x: usize, y: usize, text: &str) -> Result<usize> {
    if x >= p.width() || y >= p.height() {
        bail!("square {x},{y} is outside the grid");
    }
    guarded(p, |p| paste_unguarded(p, x, y, text))
}

fn paste_unguarded(p: &mut Puzzle, x: usize, y: usize, text: &str) -> Result<usize> {
    let before = p.grid.clone();
    for (dy, line) in text.lines().enumerate() {
        for (dx, c) in line.trim_end_matches('\r').chars().enumerate() {
            let (cx, cy) = (x + dx, y + dy);
            if cx >= p.width() || cy >= p.height() || matches!(c, ' ' | '_' | '?') {
                continue;
            }
            let mirror = crate::blocks::partner(p, cx, cy).filter(|&q| q != (cx, cy));
            match c {
                '#' => {
                    set_cell(p, cx, cy, '#');
                    if let Some((mx, my)) = mirror {
                        set_cell(p, mx, my, '#');
                    }
                }
                '.' | 'a'..='z' | 'A'..='Z' => {
                    if p.cell(cx, cy) == '#'
                        && let Some((mx, my)) = mirror
                        && p.cell(mx, my) == '#'
                    {
                        set_cell(p, mx, my, '.');
                    }
                    set_cell(p, cx, cy, if c == '.' { '.' } else { c.to_ascii_uppercase() });
                }
                _ => bail!("invalid character {c:?} in pasted text (use letters, '#', '.', or '_' to keep)"),
            }
        }
    }
    p.tidy();
    Ok(p.grid.iter().zip(&before).map(|(a, b)| a.chars().zip(b.chars()).filter(|(x, y)| x != y).count()).sum())
}

// ---------------------------------------------------------------- marks

/// A square-level mark that only changes how the puzzle looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Circle,
    Shade,
}

fn check_square(p: &Puzzle, x: usize, y: usize) -> Result<()> {
    if x >= p.width() || y >= p.height() {
        bail!("square {x},{y} is outside the {}x{} grid", p.width(), p.height());
    }
    Ok(())
}

/// Add or remove a circle or shading on one square, or on every square of a rectangle when `to` is
/// given. Circles skip black squares in a rectangle; a single black square is an error. Returns how
/// many squares changed.
pub fn mark(p: &mut Puzzle, from: (usize, usize), to: Option<(usize, usize)>, kind: Mark, on: bool) -> Result<usize> {
    let (x0, y0, x1, y1) = region(p, from, to.unwrap_or(from))?;
    if to.is_none() && on && kind == Mark::Circle && p.cell(x0, y0) == '#' {
        bail!("square {x0},{y0} is black; circles go on white squares");
    }
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if on && kind == Mark::Circle && p.cell(x, y) == '#' {
                continue;
            }
            let set = if kind == Mark::Circle { &mut p.circles } else { &mut p.shaded };
            let k = square_key(x, y);
            let changed = if on { set.insert(k) } else { set.remove(&k) };
            n += changed as usize;
        }
    }
    Ok(n)
}

/// Set one of the user's own fields on a puzzle, or take it off with None. Field names keep their
/// case but match without it ("publishing date" finds "Publishing date"). Returns the name stored.
pub fn set_meta(p: &mut Puzzle, name: &str, value: Option<&str>) -> Result<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > 40 {
        bail!("a field name has 1 to 40 characters");
    }
    let old = p.meta.keys().find(|k| k.eq_ignore_ascii_case(&name)).cloned();
    let key = old.clone().unwrap_or(name);
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => {
            p.meta.insert(key.clone(), v.to_string());
        }
        None => {
            if old.is_none() {
                bail!("the puzzle has no field {key:?}");
            }
            p.meta.remove(&key);
        }
    }
    Ok(key)
}

/// Color squares (a name from `puzzle::COLORS`), or take the color off with None. One square or a
/// rectangle. Returns how many squares changed.
pub fn set_color(p: &mut Puzzle, from: (usize, usize), to: Option<(usize, usize)>, name: Option<&str>) -> Result<usize> {
    let (x0, y0, x1, y1) = region(p, from, to.unwrap_or(from))?;
    if let Some(n) = name
        && crate::puzzle::color(n).is_none()
    {
        bail!("unknown color {n:?}. Use one of: {}", crate::puzzle::color_names());
    }
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let k = square_key(x, y);
            if p.absent.contains(&k) {
                continue;
            }
            let changed = match name {
                Some(c) => p.colors.insert(k, c.to_string()).as_deref() != Some(c),
                None => p.colors.remove(&k).is_some(),
            };
            n += changed as usize;
        }
    }
    Ok(n)
}

/// Leave squares out of the grid (they draw as empty space, so the grid can have any shape), or
/// put them back as black squares. One square or a rectangle. A left-out square is a black square
/// to a filler. Squares with a letter are skipped (a single one fails): clear them first.
/// Returns how many squares changed.
pub fn set_absent(p: &mut Puzzle, from: (usize, usize), to: Option<(usize, usize)>, on: bool) -> Result<usize> {
    let (x0, y0, x1, y1) = region(p, from, to.unwrap_or(from))?;
    if on && to.is_none() && !matches!(p.cell(x0, y0), '#' | '.') {
        bail!("square {x0},{y0} has a letter; clear it before you leave it out of the grid");
    }
    let mut q = p.clone();
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let k = square_key(x, y);
            if !on {
                n += q.absent.remove(&k) as usize;
                continue;
            }
            match q.cell(x, y) {
                '#' => {}
                '.' => {
                    let mut row: Vec<char> = q.grid[y].chars().collect();
                    row[x] = '#';
                    q.grid[y] = row.into_iter().collect();
                }
                _ => continue,
            }
            n += q.absent.insert(k) as usize;
        }
    }
    q.tidy();
    *p = q;
    Ok(n)
}

/// Put a rebus in a square: 2 or more letters A-Z. The grid keeps the first letter. Empty
/// `letters` removes the rebus and leaves its first letter in the square. Fails on a locked square.
pub fn set_rebus(p: &mut Puzzle, x: usize, y: usize, letters: &str) -> Result<()> {
    check_square(p, x, y)?;
    guarded(p, |p| write_rebus(p, x, y, letters))
}

fn write_rebus(p: &mut Puzzle, x: usize, y: usize, letters: &str) -> Result<()> {
    let letters: String = letters.trim().to_ascii_uppercase();
    let k = square_key(x, y);
    if letters.is_empty() {
        p.rebus.remove(&k);
        return Ok(());
    }
    if p.cell(x, y) == '#' {
        bail!("square {x},{y} is black; a rebus goes on a white square");
    }
    if !letters.chars().all(|c| c.is_ascii_uppercase()) {
        bail!("a rebus is letters A-Z only, not {letters:?}");
    }
    if letters.len() < 2 {
        bail!("a rebus needs 2 or more letters; for one letter use place");
    }
    set_cell(p, x, y, letters.chars().next().unwrap());
    p.rebus.insert(k, letters);
    Ok(())
}

/// Add or remove a wall on one side of a square: right, bottom, left or top. A left or top wall is
/// stored as the right or bottom wall of the neighbor. Returns the square and side it is stored on.
pub fn set_wall(p: &mut Puzzle, x: usize, y: usize, side: &str, on: bool) -> Result<(usize, usize, &'static str)> {
    check_square(p, x, y)?;
    let (sx, sy, side) = match side.trim().to_ascii_lowercase().as_str() {
        "right" | "r" => (x, y, RIGHT),
        "bottom" | "b" => (x, y, BOTTOM),
        "left" | "l" if x > 0 => (x - 1, y, RIGHT),
        "top" | "t" if y > 0 => (x, y - 1, BOTTOM),
        "left" | "l" | "top" | "t" => bail!("square {x},{y} is on the grid edge; walls go between two squares"),
        s => bail!("side must be right, bottom, left or top, not {s:?}"),
    };
    let (nx, ny) = if side == RIGHT { (sx + 1, sy) } else { (sx, sy + 1) };
    if nx >= p.width() || ny >= p.height() {
        bail!("square {x},{y} is on the grid edge; walls go between two squares");
    }
    let k = square_key(sx, sy);
    if on {
        if p.cell(sx, sy) == '#' || p.cell(nx, ny) == '#' {
            bail!("a wall goes between two white squares; {sx},{sy} and {nx},{ny} are not both white");
        }
        let sides = p.bars.entry(k).or_default();
        if !sides.iter().any(|s| s == side) {
            sides.push(side.to_string());
            sides.sort();
        }
    } else if let Some(sides) = p.bars.get_mut(&k) {
        sides.retain(|s| s != side);
        if sides.is_empty() {
            p.bars.remove(&k);
        }
    }
    Ok((sx, sy, side))
}

/// Put a picture on a square (white or black), or remove it with `None`.
pub fn set_sprite(p: &mut Puzzle, x: usize, y: usize, sprite: Option<Sprite>) -> Result<()> {
    check_square(p, x, y)?;
    let k = square_key(x, y);
    match sprite {
        Some(s) => {
            let old = p.sprites.insert(k.clone(), s);
            if let Err(e) = p.validate() {
                match old {
                    Some(o) => p.sprites.insert(k, o),
                    None => p.sprites.remove(&k),
                };
                return Err(e);
            }
        }
        None => {
            p.sprites.remove(&k);
        }
    }
    Ok(())
}

/// A sprite from an image file (png, jpg, gif, svg or webp, up to 300 KB), as a data URI.
pub fn sprite_from_file(path: &Path, alt: Option<String>) -> Result<Sprite> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => bail!("use a png, jpg, gif, svg or webp image, not {:?}", path.display().to_string()),
    };
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() > SPRITE_MAX_BYTES {
        bail!("{} is {} KB; the limit is {} KB", path.display(), bytes.len() / 1024, SPRITE_MAX_BYTES / 1024);
    }
    Ok(Sprite { emoji: None, src: Some(format!("data:{mime};base64,{}", base64(&bytes))), alt: alt.filter(|a| !a.trim().is_empty()) })
}

/// A sprite that shows an emoji (or any short text).
pub fn sprite_emoji(emoji: &str, alt: Option<String>) -> Sprite {
    Sprite { emoji: Some(emoji.trim().to_string()), src: None, alt: alt.filter(|a| !a.trim().is_empty()) }
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ---------------------------------------------------------------- locks

/// The letter (or rebus) a square holds, for comparing before and after an edit.
fn content(p: &Puzzle, x: usize, y: usize) -> (char, Option<&str>) {
    (p.cell(x, y).to_ascii_uppercase(), p.rebus_at(x, y))
}

/// Locked squares (in `before`) whose letter or rebus is different in `after`.
pub fn changed_locks(before: &Puzzle, after: &Puzzle) -> Vec<String> {
    if before.width() != after.width() || before.height() != after.height() {
        return before.locked.iter().cloned().collect();
    }
    before
        .locked
        .iter()
        .filter(|k| parse_square_key(k).is_some_and(|(x, y)| x < before.width() && y < before.height() && content(before, x, y) != content(after, x, y)))
        .cloned()
        .collect()
}

/// An error if the edit from `before` to `after` changes a locked square.
pub fn check_locks(before: &Puzzle, after: &Puzzle) -> Result<()> {
    let hit = changed_locks(before, after);
    match hit.len() {
        0 => Ok(()),
        1 => bail!("square {} is locked, so it keeps its letter. Unlock it first (lock PUZZLE {} --remove).", hit[0], hit[0]),
        _ => bail!("squares {} are locked, so they keep their letters. Unlock them first (lock PUZZLE x,y --remove).", hit.join(" ")),
    }
}

/// Run an edit on a copy, and keep it only if no locked square changed.
fn guarded<T>(p: &mut Puzzle, f: impl FnOnce(&mut Puzzle) -> Result<T>) -> Result<T> {
    let mut q = p.clone();
    let out = f(&mut q)?;
    check_locks(p, &q)?;
    *p = q;
    Ok(out)
}

/// Replace the whole grid (a fill, or a black-square suggestion). Fails if a locked square changes.
pub fn set_grid(p: &mut Puzzle, grid: Vec<String>) -> Result<()> {
    guarded(p, |p| {
        p.grid = grid;
        p.validate()?;
        p.tidy();
        Ok(())
    })
}

/// What `lock` works on.
#[derive(Debug, Clone)]
pub enum LockTarget {
    /// An answer, e.g. "9A" or "5,7,across".
    Slot(String),
    /// One square, or a rectangle when the second corner is given.
    Squares((usize, usize), Option<(usize, usize)>),
    /// Every filled white square.
    AllFilled,
    /// Every locked square (unlock only).
    All,
}

impl LockTarget {
    /// "9A", "5,7,across", "x,y", or "x,y" with a second corner "x2,y2".
    pub fn parse(target: &str, to: Option<&str>) -> Result<LockTarget> {
        let t = target.trim();
        if t.matches(',').count() == 1 {
            let to = to.map(parse_square).transpose()?;
            return Ok(LockTarget::Squares(parse_square(t)?, to));
        }
        if to.is_some() {
            bail!("a second corner goes with a square (x,y), not an answer");
        }
        Ok(LockTarget::Slot(t.to_string()))
    }
}

/// What `lock` did.
#[derive(Debug, Clone, Serialize)]
pub struct LockReport {
    pub message: String,
    /// Squares that were locked or unlocked by this call.
    pub changed: Vec<String>,
    /// Empty squares left unlocked (only letters can be locked).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped_empty: Vec<String>,
    /// Every locked square after the call.
    pub locked: Vec<String>,
}

/// Lock squares so they keep their letters, or unlock them (`on` = false). Locking an answer locks
/// its filled squares and skips empty ones. A single empty or black square can't be locked.
pub fn lock(p: &mut Puzzle, target: &LockTarget, on: bool) -> Result<LockReport> {
    let (what, squares): (String, Vec<(usize, usize)>) = match target {
        LockTarget::Slot(s) => {
            let slot = p.find_slot(s)?;
            (label(&slot), slot.cells())
        }
        LockTarget::Squares(a, b) => {
            let (x0, y0, x1, y1) = region(p, *a, b.unwrap_or(*a))?;
            let what = if b.is_some() { format!("{x0},{y0} to {x1},{y1}") } else { format!("{x0},{y0}") };
            (what, (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y))).collect())
        }
        LockTarget::AllFilled => ("the grid".into(), (0..p.height()).flat_map(|y| (0..p.width()).map(move |x| (x, y))).collect()),
        LockTarget::All => {
            if on {
                bail!("use --all-filled to lock every letter");
            }
            ("the grid".into(), p.locked.iter().filter_map(|k| parse_square_key(k)).collect())
        }
    };
    let single = matches!(target, LockTarget::Squares(_, None));
    let mut changed = vec![];
    let mut skipped_empty = vec![];
    let mut filled = 0;
    for (x, y) in squares {
        let k = square_key(x, y);
        if !on {
            if p.locked.remove(&k) {
                changed.push(k);
            }
            continue;
        }
        match p.cell(x, y) {
            '#' if single => bail!("square {k} is black; only letters can be locked"),
            '.' if single => bail!("square {k} is empty; type a letter before you lock it"),
            '#' => {}
            '.' if matches!(target, LockTarget::AllFilled) => {}
            '.' => skipped_empty.push(k),
            _ => {
                filled += 1;
                if p.locked.insert(k.clone()) {
                    changed.push(k);
                }
            }
        }
    }
    if on && filled == 0 && !matches!(target, LockTarget::AllFilled) {
        bail!("{what} has no letters to lock");
    }
    let mut message = if on {
        format!("Locked {} square(s) in {what}.", changed.len())
    } else {
        format!("Unlocked {} square(s) in {what}.", changed.len())
    };
    if !skipped_empty.is_empty() {
        let more = if skipped_empty.len() > 8 { " ..." } else { "" };
        message += &format!(" Skipped {} empty square(s): {}{more}.", skipped_empty.len(), skipped_empty[..skipped_empty.len().min(8)].join(" "));
    }
    Ok(LockReport { message, changed, skipped_empty, locked: p.locked.iter().cloned().collect() })
}

// ---------------------------------------------------------------- resize

/// Rows or columns to add (positive) or remove (negative) on each edge of the grid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edges {
    #[serde(default)]
    pub top: i64,
    #[serde(default)]
    pub bottom: i64,
    #[serde(default)]
    pub left: i64,
    #[serde(default)]
    pub right: i64,
}

/// The anchors `Edges::to_size` takes: the part of the grid that stays in place.
pub const ANCHORS: [&str; 9] = ["center", "top", "bottom", "left", "right", "top-left", "top-right", "bottom-left", "bottom-right"];

impl Edges {
    /// The edges that take this grid to `w` x `h`. `anchor` is the part of the grid that stays in
    /// place (see `ANCHORS`). "center" splits the change between both sides; an odd row or column
    /// goes to the bottom or right.
    pub fn to_size(p: &Puzzle, w: usize, h: usize, anchor: &str) -> Result<Edges> {
        let a = anchor.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        let a = if a.is_empty() { "center".to_string() } else { a };
        if !ANCHORS.contains(&a.as_str()) {
            bail!("anchor must be one of {}, not {anchor:?}", ANCHORS.join(", "));
        }
        // (start side, end side) for a change of `d`, given which side stays in place.
        let split = |d: i64, keep_start: bool, keep_end: bool| match (keep_start, keep_end) {
            (true, _) => (0, d),
            (_, true) => (d, 0),
            _ => (d / 2, d - d / 2),
        };
        let (top, bottom) = split(h as i64 - p.height() as i64, a.starts_with("top"), a.starts_with("bottom"));
        let (left, right) = split(w as i64 - p.width() as i64, a.ends_with("left"), a.ends_with("right"));
        Ok(Edges { top, bottom, left, right })
    }

    /// "1 row added on the top, 2 columns removed on the left", or "no change".
    pub fn describe(&self) -> String {
        let mut parts = vec![];
        for (n, edge, unit) in [(self.top, "top", "row"), (self.bottom, "bottom", "row"), (self.left, "left", "column"), (self.right, "right", "column")] {
            if n != 0 {
                let s = if n.abs() == 1 { "" } else { "s" };
                parts.push(format!("{} {unit}{s} {} the {edge}", n.abs(), if n > 0 { "added on" } else { "removed from" }));
            }
        }
        if parts.is_empty() {
            "no change".into()
        } else {
            parts.join(", ")
        }
    }
}

/// Parse a size like "15x15" (width x height).
pub fn parse_size(s: &str) -> Result<(usize, usize)> {
    let t = s.trim().to_ascii_lowercase().replace('×', "x");
    let (w, h) = t.split_once('x').ok_or_else(|| anyhow::anyhow!("size must look like 15x15 (width x height), not {s:?}"))?;
    Ok((w.trim().parse().with_context(|| format!("bad width in {s:?}"))?, h.trim().parse().with_context(|| format!("bad height in {s:?}"))?))
}

/// The edges for a resize request: a target size ("17x15") with an anchor, or edges, not both.
pub fn plan_resize(p: &Puzzle, size: Option<&str>, anchor: Option<&str>, edges: Edges) -> Result<Edges> {
    match size.map(str::trim).filter(|s| !s.is_empty()) {
        Some(_) if edges != Edges::default() => bail!("give a size or edges (top, bottom, left, right), not both"),
        Some(s) => {
            let (w, h) = parse_size(s)?;
            Edges::to_size(p, w, h, anchor.unwrap_or("center"))
        }
        None if anchor.is_some_and(|a| !a.trim().is_empty()) => bail!("an anchor goes with a size, e.g. 17x17"),
        None if edges == Edges::default() => bail!("give a size (e.g. 17x17) or rows/columns to add or remove on an edge"),
        None => Ok(edges),
    }
}

/// What `resize` did.
#[derive(Debug, Clone, Serialize)]
pub struct ResizeReport {
    pub message: String,
    pub width: usize,
    pub height: usize,
    pub old_width: usize,
    pub old_height: usize,
    pub edges: Edges,
    /// Letters in rows or columns that were removed.
    pub removed_letters: usize,
    /// Marks (rebus, circles, shading, walls, pictures) that were removed with their squares.
    pub removed_marks: usize,
    /// Clues keyed by an answer's start square ("x,y,across") whose answer was removed.
    pub removed_clues: usize,
}

/// Add (positive) or remove (negative) rows and columns on each edge. New squares are empty.
/// Marks, locks and slot-keyed clues move with their squares; ones on removed squares go. Clues
/// keyed by answer don't change. Fails if the size would leave MIN_SIZE..=MAX_SIZE, or if a
/// removed square is locked.
pub fn resize(p: &mut Puzzle, e: Edges) -> Result<ResizeReport> {
    use crate::puzzle::{is_slot_key, Dir, MAX_SIZE, MIN_SIZE};
    let (w, h) = (p.width() as i64, p.height() as i64);
    let (nw, nh) = (w + e.left + e.right, h + e.top + e.bottom);
    if -e.left.min(0) - e.right.min(0) >= w || -e.top.min(0) - e.bottom.min(0) >= h {
        bail!("that removes every row or column of the {w}x{h} grid");
    }
    let range = MIN_SIZE as i64..=MAX_SIZE as i64;
    if !range.contains(&nw) || !range.contains(&nh) {
        bail!("size must be between {MIN_SIZE} and {MAX_SIZE}; that would make {nw}x{nh}");
    }
    // Where an old square goes, if it stays.
    let map = |x: usize, y: usize| {
        let (nx, ny) = (x as i64 + e.left, y as i64 + e.top);
        ((0..nw).contains(&nx) && (0..nh).contains(&ny)).then_some((nx as usize, ny as usize))
    };
    let map_key = |k: &str| parse_square_key(k).and_then(|(x, y)| map(x, y)).map(|(x, y)| square_key(x, y));

    // Refuse to drop a locked square, and say which edges hold them.
    let lost: Vec<&String> = p.locked.iter().filter(|k| map_key(k).is_none()).collect();
    if !lost.is_empty() {
        let mut edges = vec![];
        for (x, y) in lost.iter().filter_map(|k| parse_square_key(k)) {
            let (nx, ny) = (x as i64 + e.left, y as i64 + e.top);
            for (off, edge) in [(ny < 0, "top"), (ny >= nh, "bottom"), (nx < 0, "left"), (nx >= nw, "right")] {
                if off && !edges.contains(&edge) {
                    edges.push(edge);
                }
            }
        }
        let more = if lost.len() > 8 { format!(" and {} more", lost.len() - 8) } else { String::new() };
        let squares = lost.iter().take(8).map(|k| k.as_str()).collect::<Vec<_>>().join(" ") + &more;
        bail!(
            "can't remove from the {}: locked square(s) {squares} would go. Unlock them first (lock PUZZLE x,y --remove).",
            edges.join(" and ")
        );
    }

    let old = p.clone();
    let rows: Vec<Vec<char>> = old.grid.iter().map(|r| r.chars().collect()).collect();
    let removed_letters = rows
        .iter()
        .enumerate()
        .flat_map(|(y, r)| r.iter().enumerate().map(move |(x, c)| (x, y, *c)))
        .filter(|&(x, y, c)| c != '#' && c != '.' && map(x, y).is_none())
        .count();
    p.grid = (0..nh)
        .map(|ny| {
            (0..nw)
                .map(|nx| {
                    let (x, y) = (nx - e.left, ny - e.top);
                    if (0..w).contains(&x) && (0..h).contains(&y) {
                        rows[y as usize][x as usize]
                    } else {
                        '.'
                    }
                })
                .collect()
        })
        .collect();

    fn shift_map<V: Clone>(m: &BTreeMap<String, V>, map_key: &dyn Fn(&str) -> Option<String>) -> BTreeMap<String, V> {
        m.iter().filter_map(|(k, v)| map_key(k).map(|k| (k, v.clone()))).collect()
    }
    p.rebus = shift_map(&old.rebus, &map_key);
    p.sprites = shift_map(&old.sprites, &map_key);
    let mut removed_marks = (old.rebus.len() - p.rebus.len()) + (old.sprites.len() - p.sprites.len());
    let shift_set = |s: &std::collections::BTreeSet<String>| -> std::collections::BTreeSet<String> { s.iter().filter_map(|k| map_key(k)).collect() };
    p.circles = shift_set(&old.circles);
    p.shaded = shift_set(&old.shaded);
    p.locked = shift_set(&old.locked);
    p.absent = shift_set(&old.absent);
    p.colors = shift_map(&old.colors, &map_key);
    removed_marks += (old.circles.len() - p.circles.len()) + (old.shaded.len() - p.shaded.len());
    // Walls: a wall on the new right or bottom edge has no neighbor, so it goes too.
    p.bars = BTreeMap::new();
    for (k, sides) in &old.bars {
        let Some((x, y)) = parse_square_key(k).and_then(|(x, y)| map(x, y)) else {
            removed_marks += sides.len();
            continue;
        };
        let keep: Vec<String> = sides
            .iter()
            .filter(|s| match s.as_str() {
                RIGHT => (x as i64) + 1 < nw,
                BOTTOM => (y as i64) + 1 < nh,
                _ => true,
            })
            .cloned()
            .collect();
        removed_marks += sides.len() - keep.len();
        if !keep.is_empty() {
            p.bars.insert(square_key(x, y), keep);
        }
    }

    // Slot-keyed clues follow their answer: to the new answer through its first square that stays.
    // (New squares can make an answer longer, so it may start somewhere else now.)
    let old_slots = old.slots();
    let new_slots = p.slots();
    let slot_through = |x: usize, y: usize, dir: Dir| new_slots.iter().find(|s| s.dir == dir && s.cells().contains(&(x, y))).map(|s| s.key.clone());
    let mut removed_clues = 0;
    let mut clues = BTreeMap::new();
    let mut moved = vec![];
    for (k, text) in &old.clues {
        if !is_slot_key(k) {
            clues.insert(k.clone(), text.clone());
            continue;
        }
        let target = match old_slots.iter().find(|s| s.key == *k) {
            Some(s) => s.cells().into_iter().find_map(|(x, y)| map(x, y)).and_then(|(x, y)| slot_through(x, y, s.dir)),
            // A clue for an answer that is already gone: move its key with the square.
            None => k.rsplit_once(',').and_then(|(sq, dir)| map_key(sq).map(|sq| format!("{sq},{dir}"))),
        };
        match target {
            Some(t) => moved.push((t, text.clone())),
            None => removed_clues += 1,
        }
    }
    for (k, text) in moved {
        if let std::collections::btree_map::Entry::Vacant(e) = clues.entry(k) {
            e.insert(text);
        } else {
            removed_clues += 1;
        }
    }
    p.clues = clues;
    p.validate()?;
    p.tidy();

    let (ow, oh) = (old.width(), old.height());
    let mut message = if e == Edges::default() { format!("The grid stays {ow}x{oh}.") } else { format!("Resized {ow}x{oh} to {nw}x{nh}: {}.", e.describe()) };
    let mut gone = vec![];
    for (n, what) in [(removed_letters, "letter"), (removed_marks, "mark"), (removed_clues, "clue")] {
        if n > 0 {
            gone.push(format!("{n} {what}{}", if n == 1 { "" } else { "s" }));
        }
    }
    if !gone.is_empty() {
        message += &format!(" Removed {}.", gone.join(", "));
    }
    Ok(ResizeReport { message, width: nw as usize, height: nh as usize, old_width: ow, old_height: oh, edges: e, removed_letters, removed_marks, removed_clues })
}

/// Every mark on the puzzle, for `--json` output after an edit.
#[derive(Debug, Serialize)]
pub struct Marks<'a> {
    pub rebus: &'a BTreeMap<String, String>,
    pub circles: &'a std::collections::BTreeSet<String>,
    pub shaded: &'a std::collections::BTreeSet<String>,
    pub colors: &'a BTreeMap<String, String>,
    pub absent: &'a std::collections::BTreeSet<String>,
    pub bars: &'a BTreeMap<String, Vec<String>>,
    /// Sprite squares only; images are long.
    pub sprites: Vec<&'a str>,
    pub locked: &'a std::collections::BTreeSet<String>,
}

pub fn marks(p: &Puzzle) -> Marks<'_> {
    Marks { rebus: &p.rebus, circles: &p.circles, shaded: &p.shaded, colors: &p.colors, absent: &p.absent, bars: &p.bars, sprites: p.sprites.keys().map(String::as_str).collect(), locked: &p.locked }
}

/// Parse "x,y".
pub fn parse_square(s: &str) -> Result<(usize, usize)> {
    parse_square_key(s).ok_or_else(|| anyhow::anyhow!("square must look like x,y (column,row from 0,0 at the top left), not {s:?}"))
}

pub fn label(s: &SlotInfo) -> String {
    format!("{}{}", s.number, if s.dir == crate::puzzle::Dir::Across { "A" } else { "D" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_keep_lines_and_cap_length() {
        let mut p = Puzzle::blank("t", 3, 3, "rotational");
        assert_eq!(set_notes(&mut p, "  Hi\r\nthere <b>  ").unwrap(), "Hi\nthere <b>");
        assert!(set_notes(&mut p, &"x".repeat(crate::puzzle::NOTES_MAX + 1)).is_err());
        assert_eq!(p.notes, "Hi\nthere <b>");
        set_notes(&mut p, "").unwrap();
        assert!(p.notes.is_empty());
    }

    #[test]
    fn place_and_clear() {
        let mut p = Puzzle::blank("t", 3, 3, "rotational");
        place(&mut p, "1a", "cat").unwrap();
        assert_eq!(p.grid[0], "CAT");
        place(&mut p, "1d", "?.e").unwrap();
        assert_eq!(p.grid[0], "CAT");
        assert_eq!(p.grid[2], "E..");
        assert!(place(&mut p, "1a", "toolong").is_err());
        set_clue(&mut p, "1a", "Pet").unwrap();
        assert_eq!(p.clues["CAT"], "Pet");
        set_clue(&mut p, "1a", "").unwrap();
        assert!(p.clues.is_empty());
        assert_eq!(clear(&mut p, Some("1a")).unwrap().cleared, 3);
        assert_eq!(clear(&mut p, None).unwrap().cleared, 1);
    }

    #[test]
    fn regions() {
        let mut p = Puzzle::blank("t", 4, 4, "rotational");
        assert_eq!(paste(&mut p, 0, 0, "AB#\nC_D").unwrap(), 6);
        assert_eq!(p.grid, ["AB#.", "C.D.", "....", ".#.."]);
        assert_eq!(copy_region(&p, (2, 1), (0, 0)).unwrap(), "AB#\nC.D");
        assert_eq!(clear_region(&mut p, (0, 0), (1, 1)).unwrap().cleared, 3);
        assert_eq!(p.grid[0], "..#.");
        paste(&mut p, 2, 0, "x").unwrap();
        assert_eq!(p.grid, ["..X.", "..D.", "....", "...."]);
        assert!(clear_region(&mut p, (0, 0), (4, 0)).is_err());
    }
}

#[cfg(test)]
mod mark_tests {
    use super::*;

    #[test]
    fn marks_rebus_walls_sprites() {
        let mut p = Puzzle::blank("t", 4, 4, "rotational");
        p.grid[0] = "#...".into();
        assert_eq!(mark(&mut p, (0, 0), Some((1, 1)), Mark::Circle, true).unwrap(), 3); // skips the black square
        assert!(mark(&mut p, (0, 0), None, Mark::Circle, true).is_err());
        assert_eq!(mark(&mut p, (0, 0), None, Mark::Shade, true).unwrap(), 1);
        assert_eq!(mark(&mut p, (1, 1), Some((0, 0)), Mark::Circle, false).unwrap(), 3);

        set_rebus(&mut p, 1, 0, "heart").unwrap();
        assert_eq!(p.grid[0], "#H..");
        assert_eq!(p.find_slot("1a").unwrap().pattern, "[HEART]..");
        assert!(set_rebus(&mut p, 0, 0, "AB").is_err());
        assert!(set_rebus(&mut p, 2, 0, "A").is_err());
        set_rebus(&mut p, 1, 0, "").unwrap();
        assert!(p.rebus.is_empty());
        assert_eq!(p.grid[0], "#H..");

        // place with [..] sets a rebus.
        place(&mut p, "1a", "[STAR]_X").unwrap();
        assert_eq!(p.rebus["1,0"], "STAR");
        assert_eq!(p.grid[0], "#S.X");
        place(&mut p, "1a", "T__").unwrap();
        assert!(p.rebus.is_empty(), "a new first letter replaces the rebus");

        assert_eq!(set_wall(&mut p, 2, 1, "left", true).unwrap(), (1, 1, RIGHT));
        assert_eq!(set_wall(&mut p, 2, 2, "top", true).unwrap(), (2, 1, BOTTOM));
        assert!(set_wall(&mut p, 3, 1, "right", true).is_err());
        assert!(set_wall(&mut p, 0, 1, "top", true).is_err(), "next to a black square");
        assert!(set_wall(&mut p, 0, 1, "diagonal", true).is_err());
        set_wall(&mut p, 1, 1, "right", false).unwrap();
        assert_eq!(p.bars.len(), 1);

        set_sprite(&mut p, 0, 0, Some(sprite_emoji("⭐", None))).unwrap();
        assert!(set_sprite(&mut p, 0, 0, Some(sprite_emoji("", None))).is_err());
        assert_eq!(p.sprites["0,0"].emoji.as_deref(), Some("⭐"));
        set_sprite(&mut p, 0, 0, None).unwrap();
        assert!(p.sprites.is_empty());
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    fn sample() -> Puzzle {
        let mut p = Puzzle::blank("t", 4, 4, "rotational");
        p.grid = vec!["CAT.".into(), "....".into(), "....".into(), "...#".into()];
        p
    }

    #[test]
    fn lock_and_unlock() {
        let mut p = sample();
        // An answer: filled squares lock, empty ones are skipped.
        let r = lock(&mut p, &LockTarget::parse("1a", None).unwrap(), true).unwrap();
        assert_eq!(r.changed, ["0,0", "1,0", "2,0"]);
        assert_eq!(r.skipped_empty, ["3,0"]);
        assert!(r.message.contains("Skipped 1 empty"), "{}", r.message);
        // A single empty or black square is refused.
        assert!(lock(&mut p, &LockTarget::parse("3,0", None).unwrap(), true).is_err());
        assert!(lock(&mut p, &LockTarget::parse("3,3", None).unwrap(), true).is_err());
        // An answer with no letters is refused.
        assert!(lock(&mut p, &LockTarget::parse("0,1,across", None).unwrap(), true).is_err());
        // Unlock one square, then a rectangle.
        let r = lock(&mut p, &LockTarget::parse("1,0", None).unwrap(), false).unwrap();
        assert_eq!(r.changed, ["1,0"]);
        let r = lock(&mut p, &LockTarget::parse("0,0", Some("3,1")).unwrap(), false).unwrap();
        assert_eq!(r.changed, ["0,0", "2,0"]);
        assert!(p.locked.is_empty());
        // All filled, then unlock all.
        assert_eq!(lock(&mut p, &LockTarget::AllFilled, true).unwrap().changed.len(), 3);
        assert_eq!(lock(&mut p, &LockTarget::All, false).unwrap().changed.len(), 3);
        assert!(lock(&mut p, &LockTarget::All, true).is_err());
    }

    #[test]
    fn edits_refuse_locked_squares() {
        let mut p = sample();
        lock(&mut p, &LockTarget::parse("1,0", None).unwrap(), true).unwrap();
        let before = p.clone();
        // place: a different letter fails and changes nothing; the same letters are fine.
        assert!(place(&mut p, "1a", "COTS").unwrap_err().to_string().contains("1,0"));
        assert!(place(&mut p, "1a", "C.TS").is_err());
        assert_eq!(p, before);
        place(&mut p, "1a", "CATS").unwrap();
        assert_eq!(p.grid[0], "CATS");
        // paste over it fails; around it is fine.
        assert!(paste(&mut p, 0, 0, "XY").is_err());
        assert!(paste(&mut p, 1, 0, "#").is_err());
        paste(&mut p, 0, 0, "X_").unwrap();
        assert_eq!(p.grid[0], "XATS");
        // rebus on it fails.
        assert!(set_rebus(&mut p, 1, 0, "AB").is_err());
        // clear keeps it and says so.
        let c = clear(&mut p, Some("1a")).unwrap();
        assert_eq!((c.cleared, c.kept_locked.clone()), (3, vec!["1,0".to_string()]));
        assert!(c.message().contains("Kept 1 locked"));
        assert_eq!(p.grid[0], ".A..");
        let c = clear_region(&mut p, (0, 0), (3, 3)).unwrap();
        assert_eq!(c.kept_locked, ["1,0"]);
        assert_eq!(p.grid[0], ".A..");
        // A new grid (fill --write, suggest-blocks --apply) can't change it.
        let mut g = p.grid.clone();
        g[0] = "BB..".into();
        assert!(set_grid(&mut p, g).is_err());
        let mut g = p.grid.clone();
        g[0] = "CATS".into();
        set_grid(&mut p, g).unwrap();
        assert_eq!(p.grid[0], "CATS");
        // A black square on it fails.
        assert!(crate::blocks::set_block(&p, 1, 0, true).is_err());
        // Once unlocked, edits work again.
        lock(&mut p, &LockTarget::parse("1,0", None).unwrap(), false).unwrap();
        place(&mut p, "1a", "COTS").unwrap();
    }

    #[test]
    fn locks_round_trip_and_tidy() {
        let mut p = sample();
        lock(&mut p, &LockTarget::AllFilled, true).unwrap();
        let back: Puzzle = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.locked, p.locked);
        assert!(serde_json::to_string(&p).unwrap().contains("\"locked\""));
        // A square that became empty loses its lock.
        p.grid[0] = "C.T.".into();
        p.tidy();
        assert!(!p.locked.contains("1,0") && p.locked.contains("0,0"));
        assert!(p.print_marked().contains("Locked"));
    }
}

#[cfg(test)]
mod clue_tests {
    use super::*;

    /// Round-trip through JSON and `tidy`, as a save and load would.
    fn reload(p: &Puzzle) -> Puzzle {
        let mut q: Puzzle = serde_json::from_str(&serde_json::to_string(p).unwrap()).unwrap();
        q.tidy();
        q
    }

    #[test]
    fn clue_follows_moved_word() {
        let mut p = Puzzle::blank("t", 3, 3, "none");
        place(&mut p, "1a", "CAT").unwrap();
        set_clue(&mut p, "1a", "Pet").unwrap();
        // Move the word from the top row to the bottom row.
        clear(&mut p, Some("1a")).unwrap();
        place(&mut p, "0,2,across", "CAT").unwrap();
        let p = reload(&p);
        assert_eq!(p.clue_for(&p.find_slot("0,2,across").unwrap()), Some("Pet"));
        assert_eq!(p.clue_for(&p.find_slot("1a").unwrap()), None);
        // Region copy and paste carries it too.
        let mut q = Puzzle::blank("t", 3, 3, "none");
        let text = copy_region(&p, (0, 2), (2, 2)).unwrap();
        paste(&mut q, 0, 1, &text).unwrap();
        q.clues = p.clues.clone();
        assert_eq!(q.clue_for(&q.find_slot("0,1,across").unwrap()), Some("Pet"));
    }

    #[test]
    fn changed_letters_hide_then_restore_clue() {
        let mut p = Puzzle::blank("t", 3, 3, "none");
        place(&mut p, "1a", "CAT").unwrap();
        set_clue(&mut p, "1a", "Pet").unwrap();
        place(&mut p, "1a", "COT").unwrap();
        let mut p = reload(&p);
        let s = p.find_slot("1a").unwrap();
        assert_eq!(p.clue_for(&s), None);
        assert_eq!(p.clues["CAT"], "Pet", "the old clue stays stored");
        assert_eq!(p.unused_clues(), vec![("CAT".to_string(), "Pet".to_string())]);
        place(&mut p, "1a", "CAT").unwrap();
        assert_eq!(p.clue_for(&p.find_slot("1a").unwrap()), Some("Pet"));
        assert!(p.unused_clues().is_empty());
        // The same answer twice shows the same clue.
        place(&mut p, "0,2,across", "CAT").unwrap();
        assert_eq!(p.clue_for(&p.find_slot("0,2,across").unwrap()), Some("Pet"));
    }

    #[test]
    fn incomplete_slot_keeps_clue_until_complete() {
        let mut p = Puzzle::blank("t", 3, 3, "none");
        place(&mut p, "1a", "C.T").unwrap();
        set_clue(&mut p, "1a", "Pet").unwrap();
        assert_eq!(p.clues["0,0,across"], "Pet");
        assert_eq!(p.clue_for(&p.find_slot("1a").unwrap()), Some("Pet"));
        place(&mut p, "1a", "CAT").unwrap();
        let p = reload(&p);
        assert_eq!(p.clues.get("CAT").map(String::as_str), Some("Pet"));
        assert!(!p.clues.contains_key("0,0,across"));
    }

    #[test]
    fn migrates_slot_keyed_clues() {
        let text = r#"{"grid": ["CAT", "...", "DOG"], "clues": {
            "0,0,across": "Pet", "0,1,across": "Not filled yet", "0,2,across": "Barker",
            "0,0,down": "Also not filled", "9,9,across": "Orphan", "dog": "Other dog clue", "x": ""}}"#;
        let mut p: Puzzle = serde_json::from_str(text).unwrap();
        p.tidy();
        let c = |k: &str| p.clues.get(k).map(String::as_str);
        assert_eq!(c("CAT"), Some("Pet"));
        assert_eq!(c("0,1,across"), Some("Not filled yet"));
        assert_eq!(c("0,0,down"), Some("Also not filled"));
        assert_eq!(c("9,9,across"), Some("Orphan"), "a clue whose slot is gone is kept");
        // "dog" already had a clue: the slot-keyed one stays, so no text is lost.
        assert_eq!(c("DOG"), Some("Other dog clue"));
        assert_eq!(c("0,2,across"), Some("Barker"));
        assert_eq!(c("x"), None, "empty clues are dropped");
        assert_eq!(p.clue_for(&p.find_slot("0,2,across").unwrap()), Some("Other dog clue"));
        let unused: Vec<String> = p.unused_clues().into_iter().map(|(k, _)| k).collect();
        assert_eq!(unused, ["0,2,across", "9,9,across"]);
    }
}

#[cfg(test)]
mod resize_tests {
    use super::*;

    fn edges(top: i64, bottom: i64, left: i64, right: i64) -> Edges {
        Edges { top, bottom, left, right }
    }

    fn sample() -> Puzzle {
        let mut p = Puzzle::blank("t", 4, 4, "rotational");
        p.grid = vec!["CAT.".into(), "....".into(), "....".into(), "...#".into()];
        p
    }

    #[test]
    fn adds_and_removes_each_edge() {
        let mut p = sample();
        let r = resize(&mut p, edges(1, 0, 0, 0)).unwrap();
        assert_eq!((r.width, r.height), (4, 5));
        assert_eq!(p.grid, ["....", "CAT.", "....", "....", "...#"]);
        resize(&mut p, edges(-1, 1, 0, 0)).unwrap();
        assert_eq!(p.grid, ["CAT.", "....", "....", "...#", "...."]);
        resize(&mut p, edges(0, -1, 1, 0)).unwrap();
        assert_eq!(p.grid, [".CAT.", ".....", ".....", "....#"]);
        resize(&mut p, edges(0, 0, -1, 1)).unwrap();
        assert_eq!(p.grid, ["CAT..", ".....", ".....", "...#."]);
        let r = resize(&mut p, edges(0, 0, -1, -1)).unwrap();
        assert_eq!(p.grid, ["AT.", "...", "...", "..#"]);
        assert_eq!(r.removed_letters, 1);
        assert!(r.message.contains("1 column removed from the left"), "{}", r.message);
    }

    #[test]
    fn marks_walls_locks_and_clues_move() {
        let mut p = sample();
        p.rebus.insert("0,0".into(), "CAR".into());
        p.circles.insert("1,1".into());
        p.shaded.insert("3,3".into());
        p.sprites.insert("2,2".into(), Sprite { emoji: Some("⭐".into()), ..Default::default() });
        p.bars.insert("1,2".into(), vec![RIGHT.into(), BOTTOM.into()]);
        p.locked.insert("1,0".into());
        p.clues.insert("0,1,across".into(), "Row two".into());
        p.clues.insert("CAT".into(), "Pet".into());
        resize(&mut p, edges(1, 0, 2, 0)).unwrap();
        assert_eq!(p.rebus["2,1"], "CAR");
        assert!(p.circles.contains("3,2") && p.shaded.contains("5,4") && p.sprites.contains_key("4,3"));
        assert_eq!(p.bars["3,3"], ["bottom", "right"]);
        assert_eq!(p.locked.iter().collect::<Vec<_>>(), ["3,1"]);
        // The across answer grew by two squares on the left, so it starts at 0,2 now.
        assert_eq!(p.clues["0,2,across"], "Row two");
        assert_eq!(p.clues["CAT"], "Pet");

        // A wall on the new last column or row goes; marks on removed squares go.
        let mut q = sample();
        q.bars.insert("2,1".into(), vec![RIGHT.into()]);
        q.bars.insert("1,2".into(), vec![BOTTOM.into()]);
        q.circles.insert("3,1".into());
        let r = resize(&mut q, edges(0, -1, 0, -1)).unwrap();
        assert!(q.bars.is_empty() && q.circles.is_empty(), "{:?}", q.bars);
        assert_eq!(r.removed_marks, 3);
    }

    #[test]
    fn refuses_to_remove_locked_squares() {
        let mut p = sample();
        p.locked.insert("0,0".into());
        let before = p.clone();
        let err = resize(&mut p, edges(-1, 0, 0, 0)).unwrap_err().to_string();
        assert!(err.contains("top") && err.contains("0,0") && err.contains("Unlock"), "{err}");
        let err = resize(&mut p, edges(0, 0, -1, 0)).unwrap_err().to_string();
        assert!(err.contains("left"), "{err}");
        assert_eq!(p, before);
        resize(&mut p, edges(0, -1, 0, -1)).unwrap();
        assert!(p.locked.contains("0,0"));
    }

    #[test]
    fn size_limits_and_targets() {
        let mut p = sample();
        assert!(resize(&mut p, edges(-1, -2, 0, 0)).unwrap_err().to_string().contains("between 2 and 40"));
        assert!(resize(&mut p, edges(0, 0, 0, 37)).is_err());
        assert!(resize(&mut p, edges(0, 0, -5, 7)).is_err(), "removes every column");
        resize(&mut p, edges(0, 0, 0, 36)).unwrap();
        assert_eq!(p.width(), 40);

        let p = sample();
        assert_eq!(Edges::to_size(&p, 7, 4, "center").unwrap(), edges(0, 0, 1, 2));
        assert_eq!(Edges::to_size(&p, 4, 1, "").unwrap(), edges(-1, -2, 0, 0));
        assert_eq!(Edges::to_size(&p, 6, 6, "top-left").unwrap(), edges(0, 2, 0, 2));
        assert_eq!(Edges::to_size(&p, 6, 6, "bottom").unwrap(), edges(2, 0, 1, 1));
        assert!(Edges::to_size(&p, 6, 6, "middle-ish").is_err());
        assert_eq!(plan_resize(&p, Some("5x4"), Some("right"), Edges::default()).unwrap(), edges(0, 0, 1, 0));
        assert!(plan_resize(&p, Some("5x4"), None, edges(1, 0, 0, 0)).is_err());
        assert!(plan_resize(&p, None, Some("top"), edges(1, 0, 0, 0)).is_err());
        assert!(plan_resize(&p, None, None, Edges::default()).is_err());
    }
}

#[cfg(test)]
mod clue_problem_tests {
    use super::clue_problem;

    #[test]
    fn finds_the_answer_in_a_clue() {
        assert!(clue_problem("TEEN", "Sweet sixteen, e.g.").unwrap().contains("sixteen"));
        assert!(clue_problem("TEEN", "<i>Teen</i> idol").is_some());
        assert!(clue_problem("ICECREAM", "Ice cream flavor").is_some());
        assert!(clue_problem("SNOWMAN", "Snow figure").is_some());
        assert!(clue_problem("NASA", "NASA's home").is_some());
        assert!(clue_problem("ERA", "Era of note").is_some());
    }

    #[test]
    fn leaves_fair_clues_alone() {
        assert_eq!(clue_problem("ERA", "Long operation, say"), None);
        assert_eq!(clue_problem("TEEN", "High schooler, usually"), None);
        assert_eq!(clue_problem("ATE", "Created a stir"), None);
        assert_eq!(clue_problem("THEATER", "Heat source"), None);
        assert_eq!(clue_problem("A", "A to Z"), None);
    }
}

#[cfg(test)]
mod gap_tests {
    use super::*;

    #[test]
    fn gaps_are_black_squares_left_out_of_the_grid() {
        let mut p = Puzzle::blank("t", 5, 5, "none");
        assert_eq!(set_absent(&mut p, (0, 0), Some((1, 1)), true).unwrap(), 4);
        assert_eq!(p.grid[0], "##...");
        assert!(p.is_absent(1, 1) && !p.is_absent(2, 2) && p.no_square(-1, 0));
        // The numbering sees black squares.
        assert_eq!(p.slots().iter().filter(|s| s.y == 0 && s.dir == crate::puzzle::Dir::Across).map(|s| s.len).max(), Some(3));
        // A square with a letter can't be a gap; an area skips it.
        place(&mut p, "1a", "CAT").unwrap();
        assert!(set_absent(&mut p, (2, 0), None, true).is_err());
        assert_eq!(set_absent(&mut p, (2, 0), Some((4, 1)), true).unwrap(), 3);
        // Color: not on gaps, and a bad name fails.
        assert_eq!(set_color(&mut p, (0, 0), Some((2, 0)), Some("red")).unwrap(), 1);
        assert_eq!(p.square_fill(2, 0), Some("#f7b5b5"));
        assert!(set_color(&mut p, (2, 0), None, Some("teal")).is_err());
        // Back to black squares; then white again drops the gap on tidy.
        assert_eq!(set_absent(&mut p, (0, 0), None, false).unwrap(), 1);
        assert_eq!(p.cell(0, 0), '#');
        p.grid[1] = "#....".into();
        p.tidy();
        assert!(!p.is_absent(1, 1) && p.is_absent(0, 1));
        // The jpz has void cells; the PDF draws no rectangle border.
        let x = crate::formats::to_jpz(&p);
        assert!(x.contains(r#"<cell x="1" y="2" type="void"/>"#), "{x}");
    }
}

