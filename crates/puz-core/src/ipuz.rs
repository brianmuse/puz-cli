//! The ipuz file format (http://ipuz.org, version 2), the format puzzle files are saved in.
//!
//! Everything ipuz has a field for goes in that field, so other apps can open the file: title,
//! author, copyright, notes, the grid (`puzzle`, `solution`), numbered clues, rebus squares, circles,
//! shading, colors, walls and left-out squares. The rest goes in two extension fields, named the
//! ipuz way (a reverse domain name, a colon, a field name):
//!
//! - `cx.swords:puzzle`: the fields ipuz has no place for (emoji on squares, the ending, locks,
//!   symmetry, user fields, share links, and any field this crate does not know). Not volatile:
//!   other apps should keep it.
//! - `cx.swords:clues`: the clues keyed the puz way (by answer, so a clue follows its word, plus
//!   clues no answer shows now). Volatile on `puzzle`, `solution` and `clues`: an app that changes
//!   those should drop it, and then the numbered clues are read.
//!
//! A picture clue is written as an object, `{"number", "clue", "image", "cx.swords:alt"}`, with
//! the image as a data URL; its text may be empty. Pictures no slot shows go in `cx.swords:puzzle`.
//!
//! A file with no extension is read from the standard fields alone.

use crate::puzzle::{square_key, CluePicture, Dir, Puzzle, BOTTOM, COLORS, RIGHT};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const VERSION: &str = "http://ipuz.org/v2";
pub const KIND: &str = "http://ipuz.org/crossword#1";
/// The extension field for what ipuz has no field for.
pub const EXT: &str = "cx.swords:puzzle";
/// The extension field for the clues keyed by answer.
pub const EXT_CLUES: &str = "cx.swords:clues";
/// The field on a clue object for its picture's description.
pub const ALT: &str = "cx.swords:alt";

/// Puzzle fields that go in standard ipuz fields, so they are left out of the extension.
const STANDARD: [&str; 12] = ["title", "author", "copyright", "notes", "grid", "clues", "pictures", "rebus", "circles", "shaded", "colors", "absent"];
/// Walls also map to standard fields.
const BARS: &str = "bars";

/// Is this JSON an ipuz file (its `version` names ipuz.org)?
pub fn is_ipuz(v: &Value) -> bool {
    v.get("version").and_then(Value::as_str).is_some_and(|s| s.contains("ipuz.org"))
}

/// The puzzle as ipuz, with the Swords extension. Expects a tidied, valid puzzle.
pub fn to_ipuz(p: &Puzzle) -> Result<Value> {
    let (w, h) = (p.width(), p.height());
    let slots = p.slots();
    let numbers: BTreeMap<(usize, usize), usize> = slots.iter().map(|s| ((s.x, s.y), s.number)).collect();
    let mut puzzle = Vec::with_capacity(h);
    let mut solution = Vec::with_capacity(h);
    for y in 0..h {
        let (mut prow, mut srow) = (vec![], vec![]);
        for x in 0..w {
            let k = square_key(x, y);
            let c = p.cell(x, y);
            if p.absent.contains(&k) {
                prow.push(Value::Null);
                srow.push(Value::Null);
                continue;
            }
            let base = if c == '#' { json!("#") } else { json!(numbers.get(&(x, y)).copied().unwrap_or(0)) };
            let mut style = Map::new();
            if p.circles.contains(&k) {
                style.insert("shapebg".into(), json!("circle"));
            }
            if p.shaded.contains(&k) {
                style.insert("highlight".into(), json!(true));
            }
            if let Some(name) = p.colors.get(&k) {
                let hex = COLORS.iter().find(|t| t.0 == name).map(|t| if c == '#' { t.2 } else { t.1 }).ok_or_else(|| anyhow!("unknown color {name}"))?;
                style.insert("color".into(), json!(hex.trim_start_matches('#')));
            }
            let barred: String = [(RIGHT, 'R'), (BOTTOM, 'B')].iter().filter(|(side, _)| p.has_bar(x, y, side)).map(|t| t.1).collect();
            if !barred.is_empty() {
                style.insert("barred".into(), json!(barred));
            }
            prow.push(if style.is_empty() { base } else { json!({ "cell": base, "style": style }) });
            srow.push(match c {
                '#' => json!("#"),
                '.' => Value::Null,
                _ => json!(p.rebus.get(&k).cloned().unwrap_or_else(|| c.to_string())),
            });
        }
        puzzle.push(Value::Array(prow));
        solution.push(Value::Array(srow));
    }
    let (mut across, mut down) = (vec![], vec![]);
    let mut shown_pictures = BTreeSet::new();
    for s in &slots {
        let text = p.clue_for(s);
        let picture = p.picture_key(s).and_then(|k| p.pictures.get_key_value(&k));
        if text.is_none() && picture.is_none() {
            continue;
        }
        let list = if s.dir == Dir::Across { &mut across } else { &mut down };
        match picture {
            Some((k, pic)) => {
                shown_pictures.insert(k.clone());
                let mut o = Map::new();
                o.insert("number".into(), json!(s.number));
                o.insert("clue".into(), json!(text.unwrap_or("")));
                o.insert("image".into(), json!(pic.image));
                if !pic.alt.is_empty() {
                    o.insert(ALT.into(), json!(pic.alt));
                }
                list.push(Value::Object(o));
            }
            None => list.push(json!([s.number, text])),
        }
    }
    let mut out = Map::new();
    out.insert("version".into(), json!(VERSION));
    out.insert("kind".into(), json!([KIND]));
    for (k, v) in [("title", &p.title), ("author", &p.author), ("copyright", &p.copyright), ("notes", &p.notes)] {
        if !v.is_empty() {
            out.insert(k.into(), json!(v));
        }
    }
    out.insert("dimensions".into(), json!({ "width": w, "height": h }));
    out.insert("puzzle".into(), Value::Array(puzzle));
    out.insert("solution".into(), Value::Array(solution));
    out.insert("clues".into(), json!({ "Across": across, "Down": down }));

    // The extension: every puz field that has no standard place, as puz writes it.
    let Value::Object(mut ext) = serde_json::to_value(p)? else { bail!("puzzle is not an object") };
    for k in STANDARD.iter().chain([&BARS]) {
        ext.remove(*k);
    }
    let unshown: BTreeMap<_, _> = p.pictures.iter().filter(|(k, _)| !shown_pictures.contains(*k)).collect();
    if !unshown.is_empty() {
        ext.insert("pictures".into(), serde_json::to_value(unshown)?);
    }
    out.insert("volatile".into(), json!({ EXT: "", EXT_CLUES: "puzzle,solution,clues" }));
    out.insert(EXT.into(), Value::Object(ext));
    out.insert(EXT_CLUES.into(), serde_json::to_value(&p.clues)?);
    Ok(Value::Object(out))
}

/// `to_ipuz`, but only when reading the result back gives the same puzzle. Otherwise an error that
/// says what differs, so the caller can keep the older format.
pub fn to_ipuz_checked(p: &Puzzle) -> Result<Value> {
    let v = to_ipuz(p)?;
    let back = from_ipuz(&v)?;
    if back != *p {
        let (a, b) = (serde_json::to_value(p)?, serde_json::to_value(&back)?);
        let fields: Vec<String> = match (&a, &b) {
            (Value::Object(a), Value::Object(b)) => a.keys().chain(b.keys()).filter(|k| a.get(*k) != b.get(*k)).cloned().collect::<BTreeSet<_>>().into_iter().collect(),
            _ => vec![],
        };
        bail!("ipuz would change {}", if fields.is_empty() { "the puzzle".into() } else { fields.join(", ") });
    }
    Ok(v)
}

/// A puzzle from ipuz JSON: one Swords wrote (with the extension) or one from another app.
/// Not tidied or checked.
pub fn from_ipuz(v: &Value) -> Result<Puzzle> {
    if let Some(kinds) = v.get("kind").and_then(Value::as_array) {
        if !kinds.iter().filter_map(Value::as_str).any(|k| k.contains("ipuz.org/crossword")) {
            bail!("this ipuz file is not a crossword");
        }
    }
    let block = v.get("block").and_then(Value::as_str).unwrap_or("#").to_string();
    let rows = |name: &str| v.get(name).and_then(Value::as_array).cloned().unwrap_or_default();
    let (puzzle, solution) = (rows("puzzle"), rows("solution"));
    let dims = v.get("dimensions");
    let dim = |k: &str| dims.and_then(|d| d.get(k)).and_then(Value::as_u64).map(|n| n as usize);
    let h = dim("height").unwrap_or(puzzle.len().max(solution.len()));
    let w = dim("width").unwrap_or_else(|| puzzle.iter().chain(&solution).filter_map(Value::as_array).map(Vec::len).max().unwrap_or(0));
    if w == 0 || h == 0 {
        bail!("the ipuz file has no grid");
    }
    let at = |rows: &[Value], x: usize, y: usize| rows.get(y).and_then(|r| r.get(x)).cloned().unwrap_or(Value::Null);
    let text = |c: &Value| match c {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };

    let mut grid = vec![];
    let (mut rebus, mut circles, mut shaded, mut colors, mut absent) = (BTreeMap::new(), BTreeSet::new(), BTreeSet::new(), BTreeMap::new(), BTreeSet::new());
    let mut bars: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut wall = |x: usize, y: usize, side: char| {
        let (x, y, side) = match side {
            'R' => (x as isize, y as isize, RIGHT),
            'B' => (x as isize, y as isize, BOTTOM),
            'L' => (x as isize - 1, y as isize, RIGHT),
            'T' => (x as isize, y as isize - 1, BOTTOM),
            _ => return,
        };
        if x < 0 || y < 0 || (side == RIGHT && x as usize + 1 >= w) || (side == BOTTOM && y as usize + 1 >= h) {
            return;
        }
        let sides = bars.entry(square_key(x as usize, y as usize)).or_default();
        if !sides.iter().any(|s| s == side) {
            sides.push(side.into());
            sides.sort();
        }
    };
    for y in 0..h {
        let mut row = String::new();
        for x in 0..w {
            let k = square_key(x, y);
            let pc = at(&puzzle, x, y);
            let (cell, style) = match &pc {
                Value::Object(o) => (o.get("cell").cloned().unwrap_or(json!(0)), o.get("style").cloned()),
                other => (other.clone(), None),
            };
            let sc = match at(&solution, x, y) {
                Value::Object(o) => o.get("value").cloned().unwrap_or(Value::Null),
                other => other,
            };
            let is_block = |c: &Value| text(c).is_some_and(|t| t == block);
            if pc.is_null() && !puzzle.is_empty() {
                absent.insert(k);
                row.push('#');
                continue;
            }
            let black = is_block(&cell) || (puzzle.is_empty() && is_block(&sc));
            if black {
                row.push('#');
            } else {
                let letters: String = text(&sc).unwrap_or_default().trim().to_uppercase();
                let letters = if letters.chars().all(|c| c.is_alphanumeric()) { letters } else { String::new() };
                match letters.chars().next() {
                    Some(first) => {
                        row.push(first);
                        if letters.chars().count() > 1 {
                            rebus.insert(k.clone(), letters);
                        }
                    }
                    None => row.push('.'),
                }
            }
            if let Some(Value::Object(st)) = style {
                if st.get("shapebg").and_then(Value::as_str) == Some("circle") && !black {
                    circles.insert(k.clone());
                }
                if st.get("highlight").and_then(Value::as_bool) == Some(true) {
                    shaded.insert(k.clone());
                }
                if let Some(c) = st.get("color").and_then(|c| text(c)) {
                    let hex = format!("#{}", c.trim_start_matches('#').to_lowercase());
                    match COLORS.iter().find(|t| t.1 == hex || t.2 == hex) {
                        Some(t) => {
                            colors.insert(k.clone(), t.0.to_string());
                        }
                        None if !black => {
                            shaded.insert(k.clone());
                        }
                        None => {}
                    }
                }
                if let Some(b) = st.get("barred").and_then(Value::as_str) {
                    for side in b.chars() {
                        wall(x, y, side.to_ascii_uppercase());
                    }
                }
            }
        }
        grid.push(row);
    }

    // Start from the extension, then put the standard fields on top.
    let mut out = match v.get(EXT) {
        Some(Value::Object(o)) => o.clone(),
        _ => Map::new(),
    };
    for (k, field) in [("title", "title"), ("author", "author"), ("copyright", "copyright"), ("notes", "notes")] {
        out.insert(k.into(), json!(v.get(field).and_then(|f| text(f)).unwrap_or_default()));
    }
    out.insert("grid".into(), json!(grid));
    out.insert("rebus".into(), json!(rebus));
    out.insert("circles".into(), json!(circles));
    out.insert("shaded".into(), json!(shaded));
    out.insert("colors".into(), json!(colors));
    out.insert("absent".into(), json!(absent));
    out.insert(BARS.into(), json!(bars));
    out.insert("clues".into(), json!({}));
    let mut p: Puzzle = serde_json::from_value(Value::Object(out))?;
    p.clues = match v.get(EXT_CLUES) {
        Some(c @ Value::Object(_)) => serde_json::from_value(c.clone())?,
        _ => numbered_clues(&p, v.get("clues")),
    };
    for (s, pic) in numbered_pictures(&p, v.get("clues")) {
        let key = match s.answer() {
            Some(a) if p.clues.contains_key(&a) || !p.clues.contains_key(&s.key) => a,
            _ => s.key.clone(),
        };
        p.pictures.entry(key).or_insert(pic);
    }
    Ok(p)
}

/// The standard numbered clues, keyed by slot ("x,y,across"). Numbers follow the puz rule, so a
/// grid numbered another way may lose a clue.
fn numbered_clues(p: &Puzzle, clues: Option<&Value>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    each_clue(p, clues, |s, clue| {
        let text = match clue {
            Value::Array(a) => a.get(1).and_then(Value::as_str),
            Value::Object(o) => o.get("clue").and_then(Value::as_str),
            _ => None,
        };
        if let Some(t) = text.filter(|t| !t.trim().is_empty()) {
            out.insert(s.key.clone(), t.to_string());
        }
    });
    out
}

/// The pictures on standard clue objects: embedded data URLs only (an http address is skipped).
fn numbered_pictures(p: &Puzzle, clues: Option<&Value>) -> Vec<(crate::puzzle::SlotInfo, CluePicture)> {
    let mut out = vec![];
    each_clue(p, clues, |s, clue| {
        let Value::Object(o) = clue else { return };
        let Some(image) = o.get("image").and_then(Value::as_str).map(str::trim).filter(|i| i.get(..5).is_some_and(|h| h.eq_ignore_ascii_case("data:"))) else { return };
        let alt = o.get(ALT).and_then(Value::as_str).unwrap_or("").trim().to_string();
        out.push((s.clone(), CluePicture { image: image.to_string(), alt }));
    });
    out
}

/// Call `f` with each standard clue and the slot its number names.
fn each_clue(p: &Puzzle, clues: Option<&Value>, mut f: impl FnMut(&crate::puzzle::SlotInfo, &Value)) {
    let Some(Value::Object(dirs)) = clues else { return };
    if p.grid.is_empty() || p.grid.iter().any(|r| r.chars().count() != p.width()) {
        return;
    }
    let slots = p.slots();
    for (name, list) in dirs {
        let dir = match name.split(':').next().unwrap_or("").trim().to_lowercase().as_str() {
            "across" => Dir::Across,
            "down" => Dir::Down,
            _ => continue,
        };
        for clue in list.as_array().into_iter().flatten() {
            let number = match clue {
                Value::Array(a) => a.first(),
                Value::Object(o) => o.get("number"),
                _ => continue,
            };
            let number = number.and_then(|n| n.as_u64().map(|n| n as usize).or_else(|| n.as_str().and_then(|s| s.trim().parse().ok())));
            let Some(n) = number else { continue };
            if let Some(s) = slots.iter().find(|s| s.number == n && s.dir == dir) {
                f(s, clue);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rich() -> Puzzle {
        let text = r##"{
            "title": "Brews", "author": "A. C.", "copyright": "2026", "notes": "Have fun",
            "symmetry": "none", "finished": true,
            "grid": ["POLO#", "IRENA", "LAGER", "STOUT", "#ESPY"],
            "clues": {"POLO": "Collared <i>knit</i> shirt", "PILS": "Lager kind", "4,0,down": "old", "UNUSED": "kept"},
            "rebus": {"1,2": "AL"},
            "circles": ["0,1"], "shaded": ["1,1", "4,0"], "colors": {"2,2": "blue", "0,4": "red"},
            "absent": [], "bars": {"0,2": ["right"], "1,0": ["bottom"]},
            "sprites": {"4,0": {"emoji": "🍺"}},
            "locked": ["0,0"], "target": "nyt-daily", "published": "2026-01-01",
            "submission": {"nyt": {"name": "A"}},
            "ending": {"message": "Cheers!", "effects": ["confetti"], "emoji": ["🍺"], "highlight": ["0,0"]},
            "meta": {"Series": "x"}, "swords": [{"site": "https://s.example", "id": "abc"}],
            "someone_elses_field": {"keep": [1, 2]}
        }"##;
        let mut p: Puzzle = serde_json::from_str(text).unwrap();
        p.tidy();
        p.validate().unwrap();
        p
    }

    #[test]
    fn round_trips_every_field() {
        let p = rich();
        assert_eq!(p.extra["someone_elses_field"], json!({"keep": [1, 2]}));
        let v = to_ipuz_checked(&p).unwrap();
        assert_eq!(v["version"], json!(VERSION));
        assert_eq!(v["solution"][2][1], json!("AL"));
        assert_eq!(v["puzzle"][1][0]["style"]["shapebg"], json!("circle"));
        assert_eq!(v["puzzle"][2][0]["style"]["barred"], json!("R"));
        assert_eq!(v["clues"]["Across"][0], json!([1, "Collared <i>knit</i> shirt"]));
        assert_eq!(v[EXT]["someone_elses_field"], json!({"keep": [1, 2]}));
        assert!(v[EXT].get("grid").is_none());
        let text = serde_json::to_string_pretty(&v).unwrap();
        assert_eq!(Puzzle::from_json(&text).unwrap(), p);
    }

    const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAAAAAAAAA==";

    #[test]
    fn picture_clues_round_trip() {
        let mut p = rich();
        crate::edit::set_picture(&mut p, "1a", Some(PNG), Some("A polo shirt")).unwrap();
        // A picture clue with no text, on an incomplete-free slot with no clue.
        crate::edit::set_clue(&mut p, "2d", "").unwrap();
        crate::edit::set_picture(&mut p, "2d", Some(PNG), None).unwrap();
        // A picture no slot shows now.
        p.pictures.insert("GONE".into(), CluePicture { image: PNG.into(), alt: "kept".into() });
        p.tidy();
        assert!(p.pictures.contains_key("POLO"));
        let v = to_ipuz_checked(&p).unwrap();
        assert_eq!(v["clues"]["Across"][0], json!({"number": 1, "clue": "Collared <i>knit</i> shirt", "image": PNG, ALT: "A polo shirt"}));
        let d2 = v["clues"]["Down"].as_array().unwrap().iter().find(|c| c["number"] == json!(2)).unwrap();
        assert_eq!(d2, &json!({"number": 2, "clue": "", "image": PNG}));
        assert_eq!(v[EXT]["pictures"]["GONE"]["alt"], json!("kept"));
        assert!(v[EXT]["pictures"].get("POLO").is_none());
        assert!(v[EXT_CLUES].as_object().unwrap().values().all(Value::is_string));
        let text = serde_json::to_string_pretty(&v).unwrap();
        let back = Puzzle::from_json(&text).unwrap();
        assert_eq!(back, p);
        // Clues with no picture keep the short form.
        assert!(v["clues"]["Down"].as_array().unwrap().iter().any(Value::is_array));
        crate::edit::remove_picture(&mut p, "1a").unwrap();
        assert_eq!(to_ipuz(&p).unwrap()["clues"]["Across"][0], json!([1, "Collared <i>knit</i> shirt"]));
    }

    #[test]
    fn reads_picture_clues_from_other_apps() {
        let v = json!({
            "version": "http://ipuz.org/v2", "kind": ["http://ipuz.org/crossword#1"],
            "dimensions": {"width": 3, "height": 1},
            "puzzle": [[1, 0, 0]], "solution": [["C", "A", "T"]],
            "clues": {"Across": [{"number": 1, "clue": "", "image": PNG, "cx.swords:alt": "A cat"}]}
        });
        let mut p = from_ipuz(&v).unwrap();
        p.tidy();
        assert_eq!(p.pictures["CAT"], CluePicture { image: PNG.into(), alt: "A cat".into() });
        let mut v2 = v.clone();
        v2["clues"]["Across"][0]["image"] = json!("https://example.com/cat.png");
        assert!(from_ipuz(&v2).unwrap().pictures.is_empty());
    }

    #[test]
    fn left_out_squares_round_trip() {
        let mut p = Puzzle::blank("Shape", 3, 3, "none");
        p.grid = vec!["#AB".into(), "CDE".into(), "FG#".into()];
        p.absent.insert("0,0".into());
        p.tidy();
        let v = to_ipuz_checked(&p).unwrap();
        assert!(v["puzzle"][0][0].is_null());
        assert_eq!(from_ipuz(&v).unwrap(), p);
    }

    #[test]
    fn reads_plain_ipuz_from_other_apps() {
        let v = json!({
            "version": "http://ipuz.org/v2",
            "kind": ["http://ipuz.org/crossword#1"],
            "title": "Plain",
            "dimensions": {"width": 3, "height": 3},
            "puzzle": [[1, 2, {"cell": 3, "style": {"shapebg": "circle"}}], [4, {"cell": 0, "style": {"barred": "L"}}, "#"], [5, 0, null]],
            "solution": [["C", "A", "T"], ["A", "X", "#"], ["B", {"value": "E"}, null]],
            "clues": {"Across": [[1, "Pet"], {"number": 4, "clue": "Tool"}], "Down": [[1, "Taxi"], [2, "Tavern"]]}
        });
        let p = from_ipuz(&v).unwrap();
        assert_eq!(p.grid, vec!["CAT", "AX#", "BE#"]);
        assert!(p.circles.contains("2,0") && p.absent.contains("2,2"));
        assert_eq!(p.bars.get("0,1"), Some(&vec!["right".to_string()]));
        assert_eq!(p.clues.get("0,0,across").map(String::as_str), Some("Pet"));
        assert_eq!(p.clues.get("0,0,down").map(String::as_str), Some("Taxi"));
        assert_eq!(p.clues.get("1,0,down").map(String::as_str), Some("Tavern"));
        assert!(!is_ipuz(&json!({"version": 1, "grid": ["A"]})));
    }

    #[test]
    fn an_old_file_changes_to_ipuz_on_save_and_keeps_a_copy() {
        let dir = std::env::temp_dir().join(format!("ipuz-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("old.json");
        let p = rich();
        let old = serde_json::to_string_pretty(&p).unwrap();
        std::fs::write(&file, &old).unwrap();
        Puzzle::load(&file).unwrap().save(&file).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(is_ipuz(&v));
        assert_eq!(Puzzle::load(&file).unwrap(), p);
        assert_eq!(std::fs::read_to_string(dir.join(".old.json.before-ipuz")).unwrap(), old);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
