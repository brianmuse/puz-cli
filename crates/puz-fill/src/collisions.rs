//! Answers that repeat across the puzzles in a folder. Editors don't want the same answer in two
//! puzzles of one set, so this lists each complete answer that shows up in more than one puzzle.
//! The `collisions` command, `/api/collisions` and the MCP `collisions` tool use it. It takes the
//! puzzles as input; the host reads them from a folder.

use crate::edit;
use crate::puzzle::Puzzle;
use serde::Serialize;
use std::collections::BTreeMap;

/// One place an answer appears.
#[derive(Debug, Clone, Serialize)]
pub struct Use {
    /// Library path of the puzzle.
    pub puzzle: String,
    pub title: String,
    /// Answer label, e.g. 9A.
    pub label: String,
}

/// An answer found in two or more puzzles.
#[derive(Debug, Clone, Serialize)]
pub struct Collision {
    pub answer: String,
    pub uses: Vec<Use>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// The folder checked ("" = the whole library).
    pub folder: String,
    pub recursive: bool,
    /// Puzzles checked.
    pub puzzles: usize,
    /// Shortest answer counted.
    pub min_len: usize,
    /// Most-repeated first, then A to Z.
    pub collisions: Vec<Collision>,
}

/// The complete answers of a puzzle, as (ANSWER, label). Rebus squares count as their letters.
/// Answers with an empty square are left out.
fn answers(p: &Puzzle, min_len: usize) -> Vec<(String, String)> {
    p.slots()
        .iter()
        .filter(|s| !s.pattern.contains('.'))
        .map(|s| (s.pattern.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_uppercase(), edit::label(s)))
        .filter(|(a, _)| a.chars().count() >= min_len)
        .collect()
}

/// Find answers that appear in more than one of the given puzzles: (library path, title, puzzle).
pub fn find(folder: &str, recursive: bool, min_len: usize, puzzles: &[(String, String, Puzzle)]) -> Report {
    let mut seen: BTreeMap<String, Vec<Use>> = BTreeMap::new();
    for (path, title, p) in puzzles {
        for (answer, label) in answers(p, min_len) {
            seen.entry(answer).or_default().push(Use { puzzle: path.clone(), title: title.clone(), label });
        }
    }
    let mut collisions: Vec<Collision> = seen
        .into_iter()
        .filter(|(_, uses)| {
            let mut ps: Vec<&str> = uses.iter().map(|u| u.puzzle.as_str()).collect();
            ps.dedup();
            ps.len() > 1
        })
        .map(|(answer, uses)| Collision { answer, uses })
        .collect();
    collisions.sort_by(|a, b| b.uses.len().cmp(&a.uses.len()).then(a.answer.cmp(&b.answer)));
    Report { folder: folder.to_string(), recursive, puzzles: puzzles.len(), min_len, collisions }
}

/// Plain text for the CLI.
pub fn render(r: &Report) -> String {
    let scope = if r.folder.is_empty() { "the library".to_string() } else { r.folder.clone() };
    let deep = if r.recursive { " and its subfolders" } else { "" };
    if r.collisions.is_empty() {
        return format!("No answer of {}+ letters repeats across the {} puzzles in {scope}{deep}.", r.min_len, r.puzzles);
    }
    let mut out = format!(
        "{} answer(s) of {}+ letters repeat across the {} puzzles in {scope}{deep}:\n",
        r.collisions.len(),
        r.min_len,
        r.puzzles
    );
    for c in &r.collisions {
        let uses: Vec<String> = c.uses.iter().map(|u| format!("{} {}", u.puzzle, u.label)).collect();
        out.push_str(&format!("{:<16} {}\n", c.answer, uses.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn puzzle(rows: &[&str]) -> Puzzle {
        let mut p = Puzzle::blank("t", rows[0].len(), rows.len(), "none");
        p.grid = rows.iter().map(|r| r.to_string()).collect();
        p
    }

    #[test]
    fn finds_answers_in_two_puzzles() {
        let a = puzzle(&["CAT", "ORE", "WED"]);
        let b = puzzle(&["CAT", "...", "..."]);
        let c = puzzle(&["DOG", "...", "..."]);
        let r = find("", false, 3, &[("a.json".into(), "A".into(), a), ("b.json".into(), "B".into(), b), ("c.json".into(), "C".into(), c)]);
        let found: Vec<&str> = r.collisions.iter().map(|c| c.answer.as_str()).collect();
        // CAT is in a and b. Answers with empty squares (b's downs) don't count, and DOG is only in c.
        assert_eq!(found, vec!["CAT"]);
        assert_eq!(r.collisions[0].uses.iter().map(|u| u.label.as_str()).collect::<Vec<_>>(), vec!["1A", "1A"]);
    }

    #[test]
    fn repeats_inside_one_puzzle_are_not_collisions() {
        let a = puzzle(&["ABA", "B.B", "ABA"]);
        let r = find("", false, 3, &[("a.json".into(), "A".into(), a)]);
        assert!(r.collisions.is_empty());
    }
}
