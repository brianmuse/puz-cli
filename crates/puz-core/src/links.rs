//! Linked (cross-referenced) clues: a clue such as "See 17-Across" linked to that answer.
//!
//! Links live in `Puzzle::links`, keyed like clues (by answer), and name their target by its clue
//! key, so a link follows the words when the grid renumbers. The clue text keeps the number as
//! plain text ("17-Across"), so every reader shows it; `tidy` rewrites that number when the target
//! moves. ipuz files also get the standard `references` field on each linked clue.

use crate::puzzle::{ref_label, ClueLink, Dir, Puzzle, SlotInfo};
use anyhow::{bail, Result};

/// A cross-reference written in clue text: the number and direction, and where it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRef {
    pub number: usize,
    pub dir: Dir,
    /// Byte range of the reference in the text.
    pub start: usize,
    pub end: usize,
}

/// The cross-references written in clue text: "17-Across", "17 Across", "17A", "17-Down", "17d".
/// "17- and 23-Across" gives both numbers (the direction word after the last one counts for all).
/// `web/src/lib/clue-links.ts` and the solving page mirror this.
pub fn find_refs(text: &str) -> Vec<TextRef> {
    let b = text.as_bytes();
    let mut out = vec![];
    let mut pending: Vec<(usize, usize, usize)> = vec![]; // numbers waiting for a direction word
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() || (i > 0 && (b[i - 1].is_ascii_alphanumeric())) {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let number: usize = text[start..i].parse().unwrap_or(0);
        let mut j = i;
        if j < b.len() && (b[j] == b'-' || b[j] == b' ') {
            j += 1;
        }
        let rest = &text[j..];
        let word = |w: &str| rest.len() >= w.len() && rest[..w.len()].eq_ignore_ascii_case(w) && !rest.as_bytes().get(w.len()).is_some_and(|c| c.is_ascii_alphanumeric());
        let dir = if word("across") {
            Some((Dir::Across, j + 6))
        } else if word("down") {
            Some((Dir::Down, j + 4))
        } else if j == i && word("a") {
            Some((Dir::Across, j + 1))
        } else if j == i && word("d") {
            Some((Dir::Down, j + 1))
        } else {
            None
        };
        match dir {
            Some((dir, end)) => {
                for (n, s, e) in pending.drain(..) {
                    out.push(TextRef { number: n, dir, start: s, end: e });
                }
                out.push(TextRef { number, dir, start, end });
                i = end;
            }
            None if number > 0 && i < b.len() && b[i] == b'-' => pending.push((number, start, i)),
            None if number > 0 && rest.starts_with(',') => pending.push((number, start, i)),
            None => pending.clear(),
        }
    }
    out
}

/// The slots the clue text refers to, in order, without repeats.
pub fn detect(p: &Puzzle, text: &str) -> Vec<SlotInfo> {
    let slots = p.slots();
    let mut out: Vec<SlotInfo> = vec![];
    for r in find_refs(&crate::markup::plain(text)) {
        if let Some(s) = slots.iter().find(|s| s.number == r.number && s.dir == r.dir) {
            if !out.iter().any(|o| o.key == s.key) {
                out.push(s.clone());
            }
        }
    }
    out
}

/// Replace `old` with `new` in clue text where it stands alone ("17-Across" but not "117-Across").
pub fn replace_label(text: &str, old: &str, new: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(old) {
        let before = rest[..i].chars().last();
        let after = rest[i + old.len()..].chars().next();
        let alone = !before.is_some_and(|c| c.is_ascii_alphanumeric()) && !after.is_some_and(|c| c.is_ascii_alphanumeric());
        out.push_str(&rest[..i]);
        out.push_str(if alone { new } else { old });
        rest = &rest[i + old.len()..];
    }
    out.push_str(rest);
    out
}

/// Link a slot's clue to other slots (labels like "17A", "23-Down" or slot keys), replacing its
/// links. An empty list removes them. The clue must have text. Where the text writes a target
/// another way ("17A"), it is rewritten as "17-Across", so it stays right when the grid renumbers.
/// Returns the target labels.
pub fn set_links(p: &mut Puzzle, slot: &str, targets: &[String]) -> Result<Vec<String>> {
    let s = p.find_slot(slot)?;
    let key = if s.answer().is_some_and(|a| p.clues.contains_key(&a)) { s.answer().unwrap() } else { s.key.clone() };
    if let Some(a) = s.answer() {
        p.links.remove(&a);
    }
    p.links.remove(&s.key);
    if targets.is_empty() {
        return Ok(vec![]);
    }
    if !p.clues.contains_key(&key) && p.picture_key(&s).is_none() {
        bail!("{} has no clue yet; write its clue first", crate::edit::label(&s));
    }
    let mut list: Vec<ClueLink> = vec![];
    let mut labels = vec![];
    for t in targets {
        let t = p.find_slot(t)?;
        if t.key == s.key {
            bail!("a clue can't link to itself");
        }
        let to = p.clue_key(&t);
        if list.iter().any(|l| l.to == to) {
            continue;
        }
        let full = ref_label(&t);
        let mut label = String::new();
        if let Some(text) = p.clues.get_mut(&key) {
            let plain = crate::markup::plain(text);
            let hit = find_refs(&plain).into_iter().find(|r| r.number == t.number && r.dir == t.dir);
            if let Some(r) = hit {
                let written = &plain[r.start..r.end];
                if written != full && written.ends_with(|c: char| c.is_ascii_alphabetic()) {
                    *text = if plain == *text { format!("{}{full}{}", &plain[..r.start], &plain[r.end..]) } else { replace_label(text, written, &full) };
                }
                if text.contains(&full) {
                    label = full.clone();
                }
            }
        }
        labels.push(crate::edit::label(&t));
        list.push(ClueLink { to, label });
    }
    p.links.insert(key, list);
    Ok(labels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Puzzle {
        let mut p = Puzzle::blank("t", 3, 3, "none");
        p.grid = vec!["CAT".into(), "ARE".into(), "BEE".into()];
        p
    }

    #[test]
    fn finds_refs_in_text() {
        let r = find_refs("See 17-Across, 4D and 23 down");
        let got: Vec<_> = r.iter().map(|r| (r.number, r.dir)).collect();
        assert_eq!(got, vec![(17, Dir::Across), (4, Dir::Down), (23, Dir::Down)]);
        let r = find_refs("With 17- and 23-Across, a phrase");
        assert_eq!(r.iter().map(|r| r.number).collect::<Vec<_>>(), vec![17, 23]);
        assert!(find_refs("Route 66 or A1 sauce").is_empty());
    }

    #[test]
    fn link_follows_renumbering_and_round_trips() {
        let mut p = grid();
        crate::edit::set_clue(&mut p, "1a", "Pet").unwrap();
        crate::edit::set_clue(&mut p, "5a", "Buzzer; see 1a").unwrap();
        assert_eq!(set_links(&mut p, "5a", &["1a".into()]).unwrap(), vec!["1A"]);
        assert_eq!(p.clues["BEE"], "Buzzer; see 1-Across");
        assert_eq!(p.links["BEE"], vec![ClueLink { to: "CAT".into(), label: "1-Across".into() }]);
        // ipuz writes the standard references and reads them back.
        let v = crate::ipuz::to_ipuz(&p).unwrap();
        let bee = v["clues"]["Across"].as_array().unwrap().iter().find(|c| c["number"] == 5).unwrap().clone();
        assert_eq!(bee["references"], serde_json::json!([{"number": 1, "direction": "Across"}]));
        assert_eq!(crate::ipuz::from_ipuz(&v).unwrap(), p);
        // A plain ipuz with only `references` gives the same links.
        let mut plain = v.clone();
        plain.as_object_mut().unwrap().remove(crate::ipuz::EXT_CLUES);
        plain[crate::ipuz::EXT].as_object_mut().unwrap().remove("links");
        let mut q = crate::ipuz::from_ipuz(&plain).unwrap();
        q.tidy();
        assert_eq!(q.links, p.links);
        // Move CAT down a row: it is now 4-Across, and the clue text follows.
        p.grid = vec!["BEE".into(), "ARE".into(), "CAT".into()];
        p.tidy();
        assert_eq!(p.clues["BEE"], "Buzzer; see 5-Across");
        assert_eq!(p.links["BEE"][0].label, "5-Across");
        // Removing the clue drops its links.
        crate::edit::set_clue(&mut p, "1a", "").unwrap();
        p.tidy();
        assert!(p.links.is_empty());
    }
}
