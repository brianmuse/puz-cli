//! Help to send a puzzle to a paper. For now only the New York Times: the answers for each field of
//! its online form, a guess at the puzzle type, and the problems to fix first. The PDF itself is
//! drawn in `print.rs` (`submission_pdf`). This module never fills in or sends the form.
//!
//! Form answers can be saved in the puzzle file (`submission.nyt`, see `set_nyt`). A saved field
//! wins; a field left out is guessed from the puzzle.

use crate::edit::label;
use puz_core::profile::{Contact, Profile};
pub use puz_core::profile::authors;
use crate::puzzle::{Dir, Puzzle, SlotInfo};
use crate::stats::{self, Level};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Longest name the form takes, in characters.
pub const NAME_MAX: usize = 300;
/// Most words the form's notes field takes.
pub const NOTES_MAX_WORDS: usize = 250;
/// Most collaborators the form lets you pick.
pub const COLLABORATORS_MAX: usize = 2;

/// The puzzle type the form asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Themed,
    Themeless,
    Sunday,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Themed => "Themed",
            Kind::Themeless => "Themeless",
            Kind::Sunday => "Sunday",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "themed" => Some(Kind::Themed),
            "themeless" => Some(Kind::Themeless),
            "sunday" => Some(Kind::Sunday),
            _ => None,
        }
    }
}

/// Where a form answer comes from: saved in the puzzle file, or guessed from the puzzle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Saved,
    Guessed,
}

impl Source {
    fn of(saved: bool) -> Source {
        if saved {
            Source::Saved
        } else {
            Source::Guessed
        }
    }
}

/// Where each form answer comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Sources {
    pub name: Source,
    pub collaborators: Source,
    pub kind: Source,
    pub notes: Source,
}

/// A form field you can save or clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Collaborators,
    Kind,
    Notes,
}

impl Field {
    pub const ALL: [Field; 4] = [Field::Name, Field::Collaborators, Field::Kind, Field::Notes];

    /// A field name, or "all".
    pub fn parse(s: &str) -> Result<Vec<Field>> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "name" => vec![Field::Name],
            "collaborators" => vec![Field::Collaborators],
            "kind" | "type" => vec![Field::Kind],
            "notes" => vec![Field::Notes],
            "all" => Field::ALL.to_vec(),
            _ => bail!("unknown field {s:?} (use name, collaborators, kind, notes or all)"),
        })
    }
}

/// Saved answers for the NYT form. Each field left out is guessed (see `submit::nyt`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NytAnswers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 0, 1 or 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaborators: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl NytAnswers {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.collaborators.is_none() && self.kind.is_none() && self.notes.is_none()
    }
}

/// The key of the NYT answers in the puzzle file's `submission` object.
const NYT_KEY: &str = "nyt";

/// The NYT form answers saved in the puzzle file. Text is trimmed, and empty text counts as not
/// saved. Fails when the file's `submission.nyt` is not in this shape or is over the form's limits.
pub fn saved(p: &Puzzle) -> Result<NytAnswers> {
    let Some(v) = p.submission.get(NYT_KEY) else { return Ok(NytAnswers::default()) };
    let mut a: NytAnswers = serde_json::from_value(v.clone()).map_err(|e| anyhow::anyhow!("submission.nyt in the puzzle file: {e}"))?;
    for t in [&mut a.name, &mut a.notes] {
        *t = t.take().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    }
    if let Some(n) = a.collaborators.filter(|&n| n > COLLABORATORS_MAX) {
        bail!("submission.nyt.collaborators is {n}; the NYT form takes 0 to {COLLABORATORS_MAX}");
    }
    Ok(a)
}

/// Put the NYT form answers in the puzzle file (none saved: the key goes away).
fn store(p: &mut Puzzle, a: &NytAnswers) -> Result<()> {
    if a.is_empty() {
        p.submission.remove(NYT_KEY);
    } else {
        p.submission.insert(NYT_KEY.to_string(), serde_json::to_value(a)?);
    }
    Ok(())
}

/// Changes to the saved NYT form answers. Clears apply first, then sets. Empty text clears too.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NytChanges {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub collaborators: Option<usize>,
    #[serde(default)]
    pub kind: Option<Kind>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(skip)]
    pub clear: Vec<Field>,
}

impl NytChanges {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.collaborators.is_none() && self.kind.is_none() && self.notes.is_none() && self.clear.is_empty()
    }
}

/// Save NYT form answers in the puzzle. Refuses a name over `NAME_MAX` characters, notes over
/// `NOTES_MAX_WORDS` words and more than `COLLABORATORS_MAX` collaborators, the form's limits.
/// Returns true when anything changed.
pub fn set_nyt(p: &mut Puzzle, c: &NytChanges) -> Result<bool> {
    let before = saved(p)?;
    let mut a = before.clone();
    for f in &c.clear {
        match f {
            Field::Name => a.name = None,
            Field::Collaborators => a.collaborators = None,
            Field::Kind => a.kind = None,
            Field::Notes => a.notes = None,
        }
    }
    if let Some(name) = &c.name {
        let name = name.trim();
        let n = name.chars().count();
        if n > NAME_MAX {
            bail!("The name has {n} characters. The NYT form takes {NAME_MAX} at most.");
        }
        a.name = Some(name.to_string()).filter(|s| !s.is_empty());
    }
    if let Some(n) = c.collaborators {
        if n > COLLABORATORS_MAX {
            bail!("The NYT form takes 0 to {COLLABORATORS_MAX} collaborators, not {n}.");
        }
        a.collaborators = Some(n);
    }
    if let Some(k) = c.kind {
        a.kind = Some(k);
    }
    if let Some(notes) = &c.notes {
        let notes = notes.trim();
        let n = word_count(notes);
        if n > NOTES_MAX_WORDS {
            bail!("The notes have {n} words. The NYT form takes {NOTES_MAX_WORDS} at most: cut {}.", plural(n - NOTES_MAX_WORDS, "word", "words"));
        }
        a.notes = Some(notes.to_string()).filter(|s| !s.is_empty());
    }
    let changed = a != before;
    store(p, &a)?;
    Ok(changed)
}

/// How bad a problem is: `block` means fix it before you send; `warn` means look at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckLevel {
    Block,
    Warn,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubmitCheck {
    pub id: &'static str,
    pub level: CheckLevel,
    pub text: String,
    /// Answers at fault, like 9A.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

/// The answers for the NYT form, in its order, and the problems to fix first.
#[derive(Debug, Clone, Serialize)]
pub struct Submission {
    pub to: &'static str,
    /// Field 1: the name of the puzzle (saved, else the title, else the file name).
    pub name: String,
    pub name_max: usize,
    /// Field 2: how many people made it with you.
    pub collaborators: usize,
    pub collaborators_label: &'static str,
    /// The names found in the author field.
    pub authors: Vec<String>,
    /// The contact blocks for page 1 of the PDF (from the library's profile, names falling back
    /// to the author field).
    pub contacts: Vec<Contact>,
    /// Field 3: the file name for the PDF.
    pub pdf_file_name: String,
    /// Field 4: the puzzle type, and why.
    pub kind: Kind,
    pub kind_label: &'static str,
    pub kind_reason: String,
    /// The type was set by the caller, not guessed.
    pub kind_override: bool,
    /// Field 5: the notes (saved, else the puzzle's notes).
    pub notes: String,
    pub notes_words: usize,
    pub notes_max_words: usize,
    /// Where each answer comes from: `saved` in the puzzle file (`submit --set-...`) or `guessed`.
    pub source: Sources,
    /// Problems to fix before you send, `block` first.
    pub checks: Vec<SubmitCheck>,
    pub blocks: usize,
    pub warns: usize,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A file name made from the title: letters, digits, "-" and "_", words joined by "-".
pub fn safe_file_name(title: &str, fallback: &str) -> String {
    let clean = |s: &str| {
        s.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join("-")
    };
    let mut stem = clean(title);
    if stem.is_empty() {
        stem = clean(fallback);
    }
    if stem.is_empty() {
        stem = "puzzle".into();
    }
    let stem: String = stem.chars().take(120).collect();
    format!("{stem}.pdf")
}

/// "a", "a and b", "a, b and c".
fn join_and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

pub fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

/// The answer that sits where `s` would be under rotation, or under a left/right mirror.
fn partner<'a>(s: &SlotInfo, slots: &'a [SlotInfo], w: usize, h: usize) -> Option<&'a SlotInfo> {
    let last = *s.cells().last()?;
    let rotated = (w - 1 - last.0, h - 1 - last.1);
    let mirrored = match s.dir {
        Dir::Across => (w - 1 - last.0, s.y),
        Dir::Down => (w - 1 - s.x, s.y),
    };
    slots
        .iter()
        .find(|o| o.dir == s.dir && o.len == s.len && o.key != s.key && ((o.x, o.y) == rotated || (o.x, o.y) == mirrored))
}

/// Guess the puzzle type from what the file holds. In order:
/// 1. Sunday: 21 wide and tall, or bigger than 15x15 and at least 19 wide.
/// 2. The notes say "themeless" (Themeless) or "theme" / "revealer" (Themed).
/// 3. Circles, shading or rebus squares: these almost always mark a theme.
/// 4. More answers than an NYT themeless allows (72): only a themed puzzle may have that many.
/// 5. A few long answers (2 to 6 of 8+ letters) that come in symmetric pairs: a theme set. A
///    themeless grid has many long answers instead.
/// 6. Else Themeless.
pub fn guess_kind(p: &Puzzle) -> (Kind, String) {
    let (w, h) = (p.width(), p.height());
    if (w >= 21 && h >= 21) || ((w > 15 || h > 15) && w >= 19) {
        return (Kind::Sunday, format!("The grid is {w}x{h}, a Sunday size."));
    }
    let notes = p.notes.to_lowercase();
    if notes.contains("themeless") {
        return (Kind::Themeless, "Your notes call it themeless.".into());
    }
    if notes.contains("theme") || notes.contains("revealer") {
        return (Kind::Themed, "Your notes talk about a theme.".into());
    }
    let marks: Vec<&str> = [
        (!p.circles.is_empty(), "circled squares"),
        (!p.shaded.is_empty() || !p.colors.is_empty(), "shaded squares"),
        (!p.rebus.is_empty(), "rebus squares"),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, what)| *what)
    .collect();
    if !marks.is_empty() {
        return (Kind::Themed, format!("It has {}. These usually mark a theme.", marks.join(" and ")));
    }
    let slots = p.slots();
    let themeless_max = stats::find_preset("nyt-daily").ok().and_then(|r| r.max_answers_themeless).unwrap_or(72);
    if (w, h) == (15, 15) && slots.len() > themeless_max {
        return (Kind::Themed, format!("It has {} answers. An NYT themeless puzzle has {themeless_max} at most.", slots.len()));
    }
    let long: Vec<&SlotInfo> = slots.iter().filter(|s| s.len >= 8).collect();
    let paired = long.iter().filter(|s| partner(s, &slots, w, h).is_some()).count();
    if (2..=6).contains(&long.len()) && paired == long.len() {
        return (Kind::Themed, format!("Its {} long answers come in matching pairs, like theme answers.", long.len()));
    }
    let why = if long.len() > 6 { format!("{} long answers", long.len()) } else { format!("{} answers", slots.len()) };
    (Kind::Themeless, format!("No sign of a theme: no circles, shading, rebus or theme pairs, and {why}."))
}

/// The NYT form answers for a puzzle: saved answers first, then guesses. `file_name` is the
/// fallback name (the file's stem). `kind` overrides the type for this call only. `profile` gives
/// the contact details for the PDF.
pub fn nyt(p: &Puzzle, file_name: &str, kind: Option<Kind>, profile: &Profile) -> Submission {
    let saved = &saved(p).unwrap_or_default();
    let mut checks = vec![];
    let mut add = |id: &'static str, level: CheckLevel, text: String, labels: Vec<String>| checks.push(SubmitCheck { id, level, text, labels });

    // 1. Name.
    let title = p.title.trim();
    let mut name = match &saved.name {
        Some(n) => n.clone(),
        None if title.is_empty() => file_name.trim().to_string(),
        None => title.to_string(),
    };
    if name.chars().count() > NAME_MAX {
        name = name.chars().take(NAME_MAX).collect();
        add("name", CheckLevel::Warn, format!("The title is over {NAME_MAX} characters, so the name is cut short."), vec![]);
    }

    // 2. Collaborators.
    let names = authors(&p.author);
    let contacts = profile.blocks(&names);
    let me = &contacts[0];
    let missing = puz_core::profile::missing(me);
    if !missing.is_empty() {
        add(
            "contact",
            CheckLevel::Block,
            format!(
                "Add your {} in your profile (puz profile). NYT's sample submission has your name, address and email at the top of page 1.",
                join_and(&missing)
            ),
            vec![],
        );
    }
    let others = names.len().saturating_sub(1);
    if others > COLLABORATORS_MAX && saved.collaborators.is_none() {
        add(
            "collaborators",
            CheckLevel::Warn,
            format!("The author field names {} people. The form allows {COLLABORATORS_MAX} collaborators at most.", names.len()),
            vec![],
        );
    }
    let collaborators = saved.collaborators.unwrap_or(others).min(COLLABORATORS_MAX);

    // 4. Type.
    let (guess, reason) = guess_kind(p);
    let (kind, kind_reason, kind_override) = match kind.or(saved.kind) {
        Some(k) if k != guess => (k, format!("You chose {}. The guess was {}: {reason}", k.label(), guess.label()), true),
        Some(k) => (k, reason, true),
        None => (guess, reason, false),
    };

    // 5. Notes.
    let notes = saved.notes.clone().unwrap_or_else(|| p.notes.trim().to_string());
    let notes_words = word_count(&notes);
    if notes_words > NOTES_MAX_WORDS {
        add("notes", CheckLevel::Block, format!("The notes have {notes_words} words. The form takes {NOTES_MAX_WORDS} at most."), vec![]);
    }

    // The grid.
    let slots = p.slots();
    let empty = p.grid.iter().map(|r| r.chars().filter(|&c| c == '.').count()).sum::<usize>();
    if empty > 0 {
        add("unfilled", CheckLevel::Block, format!("{} empty. Fill every square.", plural(empty, "square is", "squares are")), vec![]);
    }
    let unclued: Vec<String> = slots.iter().filter(|s| p.clue_for(s).is_none_or(|c| c.trim().is_empty())).map(label).collect();
    if !unclued.is_empty() {
        add("clues", CheckLevel::Block, format!("{} no clue.", plural(unclued.len(), "answer has", "answers have")), unclued);
    }

    // The paper's rules, from the shared presets.
    let preset = stats::find_preset(if kind == Kind::Sunday { "nyt-sunday" } else { "nyt-daily" }).expect("nyt presets exist");
    let st = stats::stats(p);
    for c in stats::check_rules(&st, preset) {
        let level = match c.level {
            Level::Pass => continue,
            Level::Warn => CheckLevel::Warn,
            Level::Fail => CheckLevel::Block,
        };
        let labels: Vec<String> = c.answers.iter().map(|a| a.label.clone()).collect();
        let squares = if c.squares.is_empty() {
            String::new()
        } else {
            let shown: Vec<String> = c.squares.iter().take(8).map(|[x, y]| format!("{x},{y}")).collect();
            format!(" (squares {}{})", shown.join(" "), if c.squares.len() > 8 { " and more" } else { "" })
        };
        let (id, text) = match c.id {
            "size" => ("size", format!("The grid is {}x{}. The NYT takes 15x15 daily and 21x21 Sunday puzzles.", st.width, st.height)),
            "answers" => ("answers", format!("Too many answers: {}.", c.detail)),
            "short" => ("short", format!("{}. Every answer needs 3 or more letters.", plural(labels.len(), "answer has 2 letters", "answers have 2 letters"))),
            "checked" => ("unchecked", format!("{}{squares}. Every square must be in an across and a down answer.", plural(st.unchecked.len(), "square is unchecked", "squares are unchecked"))),
            "interlock" => ("interlock", format!("The grid is in {} separate parts. It must all connect.", st.parts)),
            "symmetry" => ("symmetry", format!("Symmetry: {}.", c.detail)),
            "blocks" => ("blocks", format!("Many black squares: {}.", c.detail)),
            other => (other, format!("{}: {}", c.label, c.detail)),
        };
        add(id, level, text, labels);
    }
    // A themeless grid has a lower answer limit than the shared check uses.
    if kind == Kind::Themeless
        && let Some(max) = preset.max_answers_themeless.filter(|&m| st.answers.total > m && st.answers.total <= preset.max_answers.unwrap_or(usize::MAX))
    {
        add("answers", CheckLevel::Warn, format!("{} answers. An NYT themeless puzzle has {max} at most.", st.answers.total), vec![]);
    }

    // The same answer twice.
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in &slots {
        if let Some(a) = s.answer() {
            seen.entry(a).or_default().push(label(s));
        }
    }
    let dupes: Vec<String> = seen.iter().filter(|(_, l)| l.len() > 1).map(|(a, l)| format!("{a} ({})", l.join(", "))).collect();
    if !dupes.is_empty() {
        add("duplicates", CheckLevel::Warn, format!("The same answer is in the grid more than once: {}.", dupes.join("; ")), vec![]);
    }

    checks.sort_by_key(|c| c.level);
    let blocks = checks.iter().filter(|c| c.level == CheckLevel::Block).count();
    Submission {
        to: "nyt",
        pdf_file_name: safe_file_name(title, file_name),
        name,
        name_max: NAME_MAX,
        collaborators,
        collaborators_label: ["None", "1 Collaborator", "2 Collaborators"][collaborators],
        authors: names,
        contacts,
        kind,
        kind_label: kind.label(),
        kind_reason,
        kind_override,
        notes,
        notes_words,
        notes_max_words: NOTES_MAX_WORDS,
        source: Sources {
            name: Source::of(saved.name.is_some()),
            collaborators: Source::of(saved.collaborators.is_some()),
            kind: Source::of(saved.kind.is_some()),
            notes: Source::of(saved.notes.is_some()),
        },
        warns: checks.len() - blocks,
        blocks,
        checks,
    }
}

/// The submission as plain text for the CLI: the form's fields in order, then the checks.
pub fn render(s: &Submission, pdf: Option<&str>) -> String {
    let mut o = vec![];
    let tag = |src: Source| if src == Source::Saved { "" } else { " (guessed)" };
    o.push("NYT submission form".to_string());
    o.push(format!("1. Name{}: {}", tag(s.source.name), s.name));
    o.push(format!("2. Collaborators{}: {}", tag(s.source.collaborators), s.collaborators_label));
    o.push(format!("3. PDF: {}", pdf.map_or_else(|| format!("{} (not written; add --pdf FILE)", s.pdf_file_name), |f| format!("{f} (written)"))));
    let contact: Vec<String> = s.contacts.iter().map(|c| c.lines().join(" / ")).filter(|l| !l.is_empty()).collect();
    o.push(format!("   Top of page 1: {}", if contact.is_empty() { "(no details; run puz profile)".to_string() } else { contact.join(" | ") }));
    o.push(format!("4. Type{}: {} ({})", tag(s.source.kind), s.kind_label, s.kind_reason));
    if s.notes.is_empty() {
        o.push("5. Notes: (empty; set them with submit PUZZLE --set-notes TEXT)".into());
    } else {
        o.push(format!("5. Notes{} ({} of {} words):", tag(s.source.notes), s.notes_words, s.notes_max_words));
        o.extend(s.notes.lines().map(|l| format!("   {l}")));
    }
    o.push(String::new());
    if s.checks.is_empty() {
        o.push("Checks: nothing to fix.".into());
    } else {
        o.push(format!("Checks: {} to fix, {} to look at.", s.blocks, s.warns));
        for c in &s.checks {
            let mark = if c.level == CheckLevel::Block { "fix " } else { "look" };
            let more = if c.labels.len() > 20 { format!(" and {} more", c.labels.len() - 20) } else { String::new() };
            let who = if c.labels.is_empty() { String::new() } else { format!(" {}{more}", c.labels[..c.labels.len().min(20)].join(", ")) };
            o.push(format!("  {mark}  {}{who}", c.text));
        }
    }
    o.push(String::new());
    o.push("Guessed answers come from the puzzle. Save your own with --set-name, --set-collaborators, --set-kind and --set-notes.".into());
    o.push("This app does not send the form. Copy each field into it and attach the PDF yourself.".into());
    o.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled() -> Puzzle {
        let mut p = Puzzle::blank("Short Words", 5, 5, "rotational");
        p.grid = vec!["#ABCD".into(), "EFGHI".into(), "JKLMN".into(), "OPQRS".into(), "TUVW#".into()];
        p
    }

    #[test]
    fn form_answers() {
        let mut p = filled();
        p.author = "Pat Lee and Sam Roe & Kim Fox".into();
        let s = nyt(&p, "short-words", None, &Profile::default());
        assert_eq!(s.name, "Short Words");
        assert_eq!(s.pdf_file_name, "Short-Words.pdf");
        assert_eq!(s.collaborators, 2);
        assert_eq!(s.collaborators_label, "2 Collaborators");
        assert_eq!(s.kind, Kind::Themeless);
        // No clues at all: one block naming every answer. 5x5 is not an NYT size.
        let clues = s.checks.iter().find(|c| c.id == "clues").unwrap();
        assert_eq!(clues.level, CheckLevel::Block);
        assert_eq!(clues.labels.len(), 10);
        assert!(s.checks.iter().any(|c| c.id == "size" && c.level == CheckLevel::Warn));
        assert_eq!(s.checks[0].level, CheckLevel::Block);
        assert!(!s.checks.iter().any(|c| c.id == "unfilled"));

        p.author = "A, B, C, D".into();
        let s = nyt(&p, "x", None, &Profile::default());
        assert_eq!(s.collaborators, 2);
        assert!(s.checks.iter().any(|c| c.id == "collaborators"));
        p.author = String::new();
        p.title = String::new();
        let s = nyt(&p, "my file", None, &Profile::default());
        assert_eq!((s.collaborators, s.name.as_str(), s.pdf_file_name.as_str()), (0, "my file", "my-file.pdf"));
        let contact = s.checks.iter().find(|c| c.id == "contact").unwrap();
        assert_eq!(contact.level, CheckLevel::Block);
        assert!(contact.text.contains("name, address, city line and email"), "{}", contact.text);
        // A full profile clears the check; the name comes from the author field.
        p.author = "Pat Lee".into();
        let mut prof = Profile::default();
        prof.me = Contact { name: String::new(), address: "1 Main St".into(), city: "Town, ST 00000".into(), email: "p@example.com".into() };
        let s = nyt(&p, "x", None, &prof);
        assert!(!s.checks.iter().any(|c| c.id == "contact"));
        assert_eq!(s.contacts[0].name, "Pat Lee");
    }

    #[test]
    fn kinds() {
        let mut p = filled();
        p.circles.insert("1,1".into());
        let (k, why) = guess_kind(&p);
        assert_eq!(k, Kind::Themed);
        assert!(why.contains("circled"), "{why}");
        assert_eq!(guess_kind(&Puzzle::blank("t", 21, 21, "rotational")).0, Kind::Sunday);
        let mut q = filled();
        q.notes = "A themeless with fun stacks.".into();
        assert_eq!(guess_kind(&q).0, Kind::Themeless);
        q.notes = "The revealer is at 60A.".into();
        assert_eq!(guess_kind(&q).0, Kind::Themed);
        // The caller can override the guess.
        let s = nyt(&q, "x", Some(Kind::Sunday), &Profile::default());
        assert_eq!(s.kind, Kind::Sunday);
        assert!(s.kind_override);
    }

    #[test]
    fn notes_and_checks() {
        let mut p = filled();
        p.grid[1] = "EF.HI".into();
        p.notes = "word ".repeat(260);
        let s = nyt(&p, "x", None, &Profile::default());
        assert_eq!(s.notes_words, 260);
        assert!(s.checks.iter().any(|c| c.id == "notes" && c.level == CheckLevel::Block));
        assert!(s.checks.iter().any(|c| c.id == "unfilled" && c.text.starts_with("1 square is")));
        assert!(render(&s, None).starts_with("NYT submission form\n1. Name (guessed): Short Words\n2. Collaborators (guessed): None"));
    }

    #[test]
    fn saved_answers() {
        let mut p = filled();
        p.author = "Pat Lee and Sam Roe".into();
        p.notes = "The revealer is at 60A.".into();
        let s = nyt(&p, "x", None, &Profile::default());
        let all_guessed = Sources { name: Source::Guessed, collaborators: Source::Guessed, kind: Source::Guessed, notes: Source::Guessed };
        assert_eq!(s.source, all_guessed);

        let c = NytChanges {
            name: Some("  Odd Jobs ".into()),
            collaborators: Some(0),
            kind: Some(Kind::Themeless),
            notes: Some("Four answers hide tools.".into()),
            clear: vec![],
        };
        assert!(set_nyt(&mut p, &c).unwrap());
        assert!(!set_nyt(&mut p, &c).unwrap(), "saving the same answers again changes nothing");
        let s = nyt(&p, "x", None, &Profile::default());
        assert_eq!((s.name.as_str(), s.collaborators, s.kind, s.notes.as_str()), ("Odd Jobs", 0, Kind::Themeless, "Four answers hide tools."));
        assert!(s.kind_reason.starts_with("You chose Themeless. The guess was Themed"), "{}", s.kind_reason);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["source"], serde_json::json!({ "name": "saved", "collaborators": "saved", "kind": "saved", "notes": "saved" }));
        // The file keeps them, and loads them back.
        let text = serde_json::to_string(&p).unwrap();
        assert!(text.contains(r#""submission":{"nyt":{"collaborators":0,"kind":"themeless","name":"Odd Jobs","notes":"Four answers hide tools."}}"#), "{text}");
        assert_eq!(serde_json::from_str::<Puzzle>(&text).unwrap(), p);

        // Clear one field; an empty name clears too.
        set_nyt(&mut p, &NytChanges { clear: Field::parse("notes").unwrap(), name: Some(" ".into()), ..Default::default() }).unwrap();
        let s = nyt(&p, "x", None, &Profile::default());
        assert_eq!((s.source.notes, s.source.name, s.source.kind), (Source::Guessed, Source::Guessed, Source::Saved));
        assert_eq!((s.name.as_str(), s.notes.as_str()), ("Short Words", "The revealer is at 60A."));
        // Clear all: nothing left in the file.
        set_nyt(&mut p, &NytChanges { clear: Field::parse("all").unwrap(), ..Default::default() }).unwrap();
        assert!(p.submission.is_empty());
        assert!(!serde_json::to_string(&p).unwrap().contains("submission"));
        assert!(Field::parse("title").is_err());
    }

    #[test]
    fn refuses_over_limits() {
        let mut p = filled();
        let e = set_nyt(&mut p, &NytChanges { notes: Some("word ".repeat(251)), ..Default::default() }).unwrap_err();
        assert_eq!(e.to_string(), "The notes have 251 words. The NYT form takes 250 at most: cut 1 word.");
        assert!(set_nyt(&mut p, &NytChanges { notes: Some("word ".repeat(250)), ..Default::default() }).is_ok());
        let e = set_nyt(&mut p, &NytChanges { name: Some("n".repeat(301)), ..Default::default() }).unwrap_err();
        assert!(e.to_string().contains("301 characters"), "{e}");
        assert!(set_nyt(&mut p, &NytChanges { collaborators: Some(3), ..Default::default() }).is_err());
        // A refused change saves nothing.
        assert_eq!(saved(&p).unwrap().name, None);
        assert_eq!(word_count(saved(&p).unwrap().notes.as_deref().unwrap()), 250);
    }

    #[test]
    fn splits_authors() {
        assert_eq!(authors("Ann Bee / Cy Dee; Ed"), vec!["Ann Bee", "Cy Dee", "Ed"]);
        assert_eq!(authors("Ann Bee / Ed. Cy Dee"), vec!["Ann Bee"]);
        assert!(authors("  ").is_empty());
        assert_eq!(safe_file_name("What's Up? (Part 2)", "f"), "What-s-Up-Part-2.pdf");
    }
}
