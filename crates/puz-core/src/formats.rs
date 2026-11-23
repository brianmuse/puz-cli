//! File formats: .jpz export (the plain form Amuse Labs accepts) and .puz export/import.
//!
//! What each format keeps of the puzzle marks:
//! - .jpz: rebus (multi-letter `solution`), circles, shading and walls. Not sprites.
//! - .puz: rebus (GRBS/RTBL sections) and circles (GEXT). Not shading, walls or sprites.
//!
//! Clue italics (`<i>`, see `markup`): .jpz keeps them as `<i>` elements. .puz has no text styles,
//! so its clues are plain text. Import keeps `<i>` if a .puz clue holds it and drops other tags.

use crate::puzzle::{square_key, Dir, Puzzle, BOTTOM, RIGHT};
use anyhow::{bail, Result};
use std::collections::BTreeMap;

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Crossword Compiler style .jpz. No linked (multi-part) words: Amuse Labs rejects them, and links
/// written as "With 4-Across" in clue text are detected by PuzzleMe on its own.
pub fn to_jpz(p: &Puzzle) -> String {
    let slots = p.slots();
    let mut numbers = BTreeMap::new();
    for s in &slots {
        numbers.insert((s.x, s.y), s.number);
    }
    let mut out = vec![
        r#"<?xml version="1.0" encoding="UTF-8"?>"#.to_string(),
        r#"<crossword-compiler-applet xmlns="http://crossword.info/xml/crossword-compiler">"#.into(),
        r##"<applet-settings width="720" height="600" cursor-color="#00b100" selected-cells-color="#80ff80"><completion friendly-submit="false" only-if-correct="true">Congratulations!</completion><actions graphical-buttons="false" wide-buttons="false" buttons-layout="left"><reveal-word label="Reveal Word"/><reveal-letter label="Reveal"/><check label="Check"/><solution label="Solution"/><pencil label="Pencil"/></actions></applet-settings>"##.into(),
        r#"<rectangular-puzzle xmlns="http://crossword.info/xml/rectangular-puzzle" alphabet="ABCDEFGHIJKLMNOPQRSTUVWXYZ">"#.into(),
        format!(
            "<metadata><title>{}</title><creator>{}</creator><copyright>{}</copyright><description>{}</description></metadata>",
            xml_escape(&p.title),
            xml_escape(&p.author),
            xml_escape(&p.copyright),
            xml_escape(&p.notes)
        ),
        "<crossword>".into(),
        format!(r#"<grid width="{}" height="{}">"#, p.width(), p.height()),
        r#"<grid-look numbering-scheme="normal" clue-square-divider-width="0.7"/>"#.into(),
    ];
    for y in 0..p.height() {
        for x in 0..p.width() {
            let c = p.cell(x, y);
            if p.is_absent(x, y) {
                // No square here: jpz readers draw empty space.
                out.push(format!(r#"<cell x="{}" y="{}" type="void"/>"#, x + 1, y + 1));
            } else if c == '#' {
                let color = p.colors.get(&square_key(x, y)).and_then(|n| crate::puzzle::color(n)).map(|(_, strong)| format!(r#" background-color="{}""#, strong.to_ascii_uppercase())).unwrap_or_default();
                out.push(format!(r#"<cell x="{}" y="{}" type="block"{color}/>"#, x + 1, y + 1));
            } else {
                let sol = match (c, p.rebus_at(x, y)) {
                    ('.', _) => String::new(),
                    (_, Some(r)) => format!(r#" solution="{r}""#),
                    (c, None) => format!(r#" solution="{}""#, c.to_ascii_uppercase()),
                };
                let num = numbers.get(&(x, y)).map(|n| format!(r#" number="{n}""#)).unwrap_or_default();
                let k = square_key(x, y);
                let mut look = String::new();
                if p.circles.contains(&k) {
                    look.push_str(r#" background-shape="circle""#);
                }
                if let Some((tint, _)) = p.colors.get(&k).and_then(|n| crate::puzzle::color(n)) {
                    look.push_str(&format!(r#" background-color="{}""#, tint.to_ascii_uppercase()));
                } else if p.shaded.contains(&k) {
                    look.push_str(r##" background-color="#C0C0C0""##);
                }
                // Walls go on the cell after the wall, as left-bar / top-bar: every jpz reader draws those.
                if x > 0 && p.has_bar(x - 1, y, RIGHT) {
                    look.push_str(r#" left-bar="true""#);
                }
                if y > 0 && p.has_bar(x, y - 1, BOTTOM) {
                    look.push_str(r#" top-bar="true""#);
                }
                out.push(format!(r#"<cell x="{}" y="{}"{sol}{num}{look}/>"#, x + 1, y + 1));
            }
        }
    }
    out.push("</grid>".into());
    for (i, s) in slots.iter().enumerate() {
        let range = if s.dir == Dir::Across {
            format!(r#"x="{}-{}" y="{}""#, s.x + 1, s.x + s.len, s.y + 1)
        } else {
            format!(r#"x="{}" y="{}-{}""#, s.x + 1, s.y + 1, s.y + s.len)
        };
        out.push(format!(r#"<word id="{}" {range}/>"#, i + 1));
    }
    for dir in [Dir::Across, Dir::Down] {
        out.push(format!(r#"<clues ordering="normal"><title><b>{}</b></title>"#, if dir == Dir::Across { "Across" } else { "Down" }));
        for (i, s) in slots.iter().enumerate().filter(|(_, s)| s.dir == dir) {
            let text = p.clue_text_for(s).unwrap_or_default();
            let text = text.as_str();
            out.push(format!(r#"<clue word="{}" number="{}" format="{}">{}</clue>"#, i + 1, s.number, s.len, crate::markup::to_xml(text)));
        }
        out.push("</clues>".into());
    }
    out.extend(["</crossword>".into(), "</rectangular-puzzle>".into(), "</crossword-compiler-applet>".into()]);
    out.join("\n")
}

fn ending_dropped(p: &Puzzle) -> String {
    match p.ending.scene.len() {
        0 => "the ending (what plays when it is solved)".to_string(),
        n => format!("the ending (what plays when it is solved, with its scene of {n} step(s))"),
    }
}

/// Marks a .jpz export leaves out, in words. Empty when nothing is lost.
pub fn jpz_dropped(p: &Puzzle) -> Vec<String> {
    let mut out = vec![];
    if !p.sprites.is_empty() {
        out.push(format!("{} sprite(s)", p.sprites.len()));
    }
    if !p.ending.is_empty() {
        out.push(ending_dropped(p));
    }
    out
}

/// Marks a .puz export leaves out, in words. Empty when nothing is lost.
pub fn puz_dropped(p: &Puzzle) -> Vec<String> {
    let mut out = vec![];
    if !p.shaded.is_empty() {
        out.push(format!("shading on {} square(s)", p.shaded.len()));
    }
    if !p.colors.is_empty() {
        out.push(format!("color on {} square(s)", p.colors.len()));
    }
    if !p.absent.is_empty() {
        out.push(format!("the grid's shape ({} left-out square(s) show as black squares)", p.absent.len()));
    }
    if !p.bars.is_empty() {
        // Across Lite numbers answers from the black squares alone, so walls change the numbering.
        out.push(format!("{} wall(s) (Across Lite numbers the grid without them, so clues may not line up)", p.bars.values().map(Vec::len).sum::<usize>()));
    }
    if !p.sprites.is_empty() {
        out.push(format!("{} sprite(s)", p.sprites.len()));
    }
    let italic = p.slots().iter().filter(|s| p.clue_for(s).is_some_and(|t| crate::markup::runs(t).iter().any(|r| r.italic))).count();
    if italic > 0 {
        out.push(format!("italics in {italic} clue(s) (the clues become plain text)"));
    }
    if !p.ending.is_empty() {
        out.push(ending_dropped(p));
    }
    out
}

fn cksum(data: &[u8], mut c: u16) -> u16 {
    for &b in data {
        c = (c >> 1) | ((c & 1) << 15);
        c = c.wrapping_add(b as u16);
    }
    c
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => b'\'',
            '\u{201C}' | '\u{201D}' => b'"',
            '\u{2013}' | '\u{2014}' => b'-',
            c if (c as u32) < 256 => c as u8,
            _ => b'?',
        })
        .collect()
}

/// Across Lite .puz (version 1.3). Empty squares are written as 'X' in the solution.
pub fn to_puz(p: &Puzzle) -> Result<Vec<u8>> {
    let (w, h) = (p.width(), p.height());
    if w > 255 || h > 255 {
        bail!("grid too large for .puz");
    }
    let mut sol = Vec::with_capacity(w * h);
    let mut fill = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let c = p.cell(x, y);
            sol.push(match c {
                '#' => b'.',
                '.' => b'X',
                c => c.to_ascii_uppercase() as u8,
            });
            fill.push(if c == '#' { b'.' } else { b'-' });
        }
    }
    let mut slots = p.slots();
    slots.sort_by_key(|s| (s.number, s.dir != Dir::Across));
    let clues: Vec<Vec<u8>> = slots.iter().map(|s| latin1(&crate::markup::plain(&p.clue_text_for(s).unwrap_or_default()))).collect();
    let (t, a, cp, notes) = (latin1(&p.title), latin1(&p.author), latin1(&p.copyright), latin1(&p.notes));
    let text_ck = |mut c: u16| {
        for s in [&t, &a, &cp] {
            if !s.is_empty() {
                let mut z = s.clone();
                z.push(0);
                c = cksum(&z, c);
            }
        }
        for s in &clues {
            c = cksum(s, c);
        }
        if !notes.is_empty() {
            let mut z = notes.clone();
            z.push(0);
            c = cksum(&z, c);
        }
        c
    };
    let mut cib = vec![w as u8, h as u8];
    cib.extend((clues.len() as u16).to_le_bytes());
    cib.extend(1u16.to_le_bytes());
    cib.extend(0u16.to_le_bytes());
    let c_cib = cksum(&cib, 0);
    let overall = text_ck(cksum(&fill, cksum(&sol, c_cib)));
    let masks = [c_cib, cksum(&sol, 0), cksum(&fill, 0), text_ck(0)];
    let magic = [
        0x49 ^ (masks[0] & 0xFF) as u8, 0x43 ^ (masks[1] & 0xFF) as u8, 0x48 ^ (masks[2] & 0xFF) as u8, 0x45 ^ (masks[3] & 0xFF) as u8,
        0x41 ^ (masks[0] >> 8) as u8, 0x54 ^ (masks[1] >> 8) as u8, 0x45 ^ (masks[2] >> 8) as u8, 0x44 ^ (masks[3] >> 8) as u8,
    ];
    let mut out = vec![];
    out.extend(overall.to_le_bytes());
    out.extend(b"ACROSS&DOWN\0");
    out.extend(c_cib.to_le_bytes());
    out.extend(magic);
    out.extend(b"1.3\0");
    out.extend([0u8; 2 + 2 + 12]);
    out.extend(&cib);
    out.extend(&sol);
    out.extend(&fill);
    for s in [&t, &a, &cp] {
        out.extend(s.iter());
        out.push(0);
    }
    for s in &clues {
        out.extend(s.iter());
        out.push(0);
    }
    out.extend(&notes);
    out.push(0);

    // Extra sections: rebus squares (GRBS + RTBL) and circles (GEXT).
    let section = |out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]| {
        out.extend(name);
        out.extend((data.len() as u16).to_le_bytes());
        out.extend(cksum(data, 0).to_le_bytes());
        out.extend(data);
        out.push(0);
    };
    if !p.rebus.is_empty() {
        let mut table: Vec<&str> = vec![];
        let mut grbs = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                if let Some(r) = p.rebus_at(x, y).filter(|_| p.cell(x, y) != '.') {
                    let i = table.iter().position(|t| *t == r).unwrap_or_else(|| {
                        table.push(r);
                        table.len() - 1
                    });
                    if i >= 254 {
                        bail!("too many different rebus entries for .puz");
                    }
                    grbs[y * w + x] = i as u8 + 1;
                }
            }
        }
        let rtbl: String = table.iter().enumerate().map(|(i, r)| format!("{i:>2}:{r};")).collect();
        section(&mut out, b"GRBS", &grbs);
        section(&mut out, b"RTBL", rtbl.as_bytes());
    }
    if !p.circles.is_empty() {
        let gext: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| if p.circles.contains(&square_key(x, y)) { 0x80 } else { 0 }).collect();
        section(&mut out, b"GEXT", &gext);
    }
    Ok(out)
}

/// Read an Across Lite .puz file, with its rebus squares (GRBS/RTBL) and circles (GEXT).
pub fn from_puz(bytes: &[u8]) -> Result<Puzzle> {
    Ok(from_puz_with_clues(bytes)?.0)
}

/// `from_puz`, and the clue texts as the file lists them: by number, across before down (the order
/// of `Puzzle::slots` sorted by number then direction). This keeps one clue per answer,
/// even when an answer repeats with two clues.
pub fn from_puz_with_clues(bytes: &[u8]) -> Result<(Puzzle, Vec<String>)> {
    if bytes.len() < 0x34 || &bytes[2..14] != b"ACROSS&DOWN\0" {
        bail!("not a .puz file");
    }
    let (w, h) = (bytes[0x2C] as usize, bytes[0x2D] as usize);
    let n_clues = u16::from_le_bytes([bytes[0x2E], bytes[0x2F]]) as usize;
    let mut i = 0x34;
    if bytes.len() < i + 2 * w * h {
        bail!("file is cut short");
    }
    let sol = &bytes[i..i + w * h];
    i += 2 * w * h;
    let mut read_str = || -> String {
        let end = bytes[i..].iter().position(|&b| b == 0).map(|p| i + p).unwrap_or(bytes.len());
        let s: String = bytes[i..end].iter().map(|&b| b as char).collect();
        i = (end + 1).min(bytes.len());
        s
    };
    let title = read_str();
    let author = read_str();
    let copyright = read_str();
    let clue_texts: Vec<String> = (0..n_clues).map(|_| read_str()).collect();
    let notes = read_str();
    let sections = puz_sections(&bytes[i.min(bytes.len())..]);
    let grid: Vec<String> = sol
        .chunks(w)
        .map(|row| row.iter().map(|&b| if b == b'.' { '#' } else if b == b'-' { '.' } else { (b as char).to_ascii_uppercase() }).collect())
        .collect();
    let mut p = Puzzle::blank(&title, w, h, "rotational");
    p.grid = grid;
    p.author = author;
    p.copyright = copyright;
    p.notes = notes;
    let mut slots = p.slots();
    slots.sort_by_key(|s| (s.number, s.dir != Dir::Across));
    // Keyed by slot for now; `tidy` below moves them to answer keys.
    for (s, t) in slots.iter().zip(&clue_texts) {
        p.clues.insert(s.key.clone(), t.clone());
    }
    if let (Some(grbs), Some(rtbl)) = (sections.get("GRBS"), sections.get("RTBL")) {
        // RTBL looks like " 0:STAR; 1:HEART;". GRBS holds 1 + the table index per square.
        let table: BTreeMap<u8, String> = String::from_utf8_lossy(rtbl)
            .split(';')
            .filter_map(|e| e.split_once(':'))
            .filter_map(|(n, r)| Some((n.trim().parse().ok()?, r.trim().to_ascii_uppercase())))
            .collect();
        for (idx, &g) in grbs.iter().enumerate().take(w * h) {
            let (x, y) = (idx % w, idx / w);
            if g == 0 || p.cell(x, y) == '#' {
                continue;
            }
            if let Some(r) = table.get(&(g - 1)).filter(|r| r.len() >= 2 && r.chars().all(|c| c.is_ascii_uppercase())) {
                p.rebus.insert(square_key(x, y), r.clone());
                let row: String = p.grid[y].chars().enumerate().map(|(i, c)| if i == x { r.chars().next().unwrap() } else { c }).collect();
                p.grid[y] = row;
            }
        }
    }
    if let Some(gext) = sections.get("GEXT") {
        for (idx, &g) in gext.iter().enumerate().take(w * h) {
            if g & 0x80 != 0 && p.cell(idx % w, idx / w) != '#' {
                p.circles.insert(square_key(idx % w, idx / w));
            }
        }
    }
    // Guess symmetry from the block pattern.
    let rot = (0..h).all(|y| (0..w).all(|x| (p.cell(x, y) == '#') == (p.cell(w - 1 - x, h - 1 - y) == '#')));
    let mir = (0..h).all(|y| (0..w).all(|x| (p.cell(x, y) == '#') == (p.cell(w - 1 - x, y) == '#')));
    p.symmetry = if rot { "rotational" } else if mir { "mirror" } else { "none" }.into();
    // Clues were read by slot; key them by answer now that the rebus squares are in.
    p.tidy();
    Ok((p, clue_texts))
}

/// The extra sections after the strings of a .puz: name -> data. Stops at the first bad one.
fn puz_sections(mut b: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    while b.len() >= 8 {
        let name = String::from_utf8_lossy(&b[0..4]).to_string();
        let len = u16::from_le_bytes([b[4], b[5]]) as usize;
        if b.len() < 8 + len {
            break;
        }
        out.insert(name, b[8..8 + len].to_vec());
        b = &b[(8 + len + 1).min(b.len())..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked() -> Puzzle {
        let mut p = Puzzle::blank("Marks", 3, 3, "none");
        p.grid = vec!["SAT".into(), "HO#".into(), "EAR".into()];
        p.rebus.insert("0,0".into(), "STAR".into());
        p.rebus.insert("1,0".into(), "AB".into());
        p.circles.insert("1,1".into());
        p.circles.insert("2,2".into());
        p.shaded.insert("0,2".into());
        p.bars.insert("0,1".into(), vec![RIGHT.into()]);
        p.sprites.insert("2,1".into(), crate::puzzle::Sprite { emoji: Some("⭐".into()), ..Default::default() });
        p
    }

    #[test]
    fn puz_round_trips_rebus_and_circles() {
        let p = marked();
        let back = from_puz(&to_puz(&p).unwrap()).unwrap();
        assert_eq!(back.grid, p.grid);
        assert_eq!(back.rebus, p.rebus);
        assert_eq!(back.circles, p.circles);
        assert_eq!(puz_dropped(&p).len(), 3, "{:?}", puz_dropped(&p));
        // A plain puzzle has no extra sections and loses nothing.
        let plain = Puzzle::blank("x", 3, 3, "none");
        let bytes = to_puz(&plain).unwrap();
        assert!(!bytes.windows(4).any(|w| w == b"GRBS" || w == b"GEXT"));
        assert!(puz_dropped(&plain).is_empty());
    }

    #[test]
    fn clue_italics_in_exports() {
        let mut p = Puzzle::blank("x", 3, 3, "none");
        p.grid = vec!["CAT".into(), "...".into(), "...".into()];
        crate::edit::set_clue(&mut p, "1a", "<i>Cats</i> & <b>dogs</b>").unwrap();
        assert_eq!(p.clues["CAT"], "<i>Cats</i> & dogs");
        let x = to_jpz(&p);
        assert!(x.contains(r#"format="3"><i>Cats</i> &amp; dogs</clue>"#), "{x}");
        let bytes = to_puz(&p).unwrap();
        assert!(bytes.windows(11).any(|w| w == b"Cats & dogs"));
        assert!(!bytes.windows(3).any(|w| w == b"<i>"));
        assert_eq!(puz_dropped(&p).len(), 1, "{:?}", puz_dropped(&p));
        // A .puz clue that holds <i> keeps it on import; other tags go.
        let mut q = p.clone();
        q.clues.insert("CAT".into(), "<i>Cats</i> <u>x</u>".into());
        let mut bytes = to_puz(&q).unwrap();
        let at = bytes.windows(6).position(|w| w == b"Cats x").unwrap();
        bytes.splice(at..at + 6, b"<i>Cats</i> <u>x</u>".iter().copied());
        // Checksums are not checked on import.
        let back = from_puz(&bytes).unwrap();
        assert_eq!(back.clues["CAT"], "<i>Cats</i> x");
    }

    #[test]
    fn jpz_has_marks() {
        let p = marked();
        let x = to_jpz(&p);
        assert!(x.contains(r#"<cell x="1" y="1" solution="STAR" number="1"/>"#), "{x}");
        assert!(x.contains(r#"<cell x="2" y="2" solution="O" background-shape="circle" left-bar="true"/>"#), "{x}");
        assert!(x.contains(r##"background-color="#C0C0C0""##));
        assert_eq!(jpz_dropped(&p), vec!["1 sprite(s)".to_string()]);
    }
}
