//! Printable output: a PNG of the grid and a one-page PDF with grid and clues.
//! Both are drawn as SVG first, then converted with resvg (PNG) and svg2pdf (PDF).

use crate::markup::{self, Run};
use crate::puzzle::{square_key, Dir, Puzzle, BOTTOM, RIGHT};
use anyhow::{anyhow, bail, Result};
use resvg::usvg;
use std::fmt::Write;
use std::sync::{Arc, Mutex};

const FONT: &str = "Helvetica, Arial, 'Liberation Sans', 'DejaVu Sans', sans-serif";
const EMOJI_FONT: &str = "'Apple Color Emoji', 'Noto Color Emoji', 'Segoe UI Emoji', 'Twemoji Mozilla', sans-serif";

/// What to draw in the white squares.
#[derive(Clone, Copy, Default)]
pub struct PrintOptions {
    /// Show the letters (an answer key). Off by default: a blank puzzle to solve.
    pub answers: bool,
}

/// Text made safe for SVG.
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Square numbers, [y][x], 0 for none.
fn numbers(p: &Puzzle) -> Vec<Vec<usize>> {
    let mut n = vec![vec![0; p.width()]; p.height()];
    for s in p.slots() {
        n[s.y][s.x] = s.number;
    }
    n
}

/// Draw the grid with its top-left corner at (x0, y0), `cell` units per square.
/// Layers, bottom to top: squares (white, grey shading, black), sprites, circles, numbers,
/// letters, walls, outer border.
fn grid_svg(out: &mut String, p: &Puzzle, x0: f32, y0: f32, cell: f32, o: PrintOptions) {
    grid_svg_styled(out, p, x0, y0, cell, o, &GridStyle { font: FONT, border: 3.0 })
}

/// How a grid looks: the font for numbers and letters, and the outer border's width in thin lines.
pub struct GridStyle {
    pub font: &'static str,
    pub border: f32,
}

pub fn grid_svg_styled(out: &mut String, p: &Puzzle, x0: f32, y0: f32, cell: f32, o: PrintOptions, st: &GridStyle) {
    let font = st.font;
    let nums = numbers(p);
    let (w, h) = (p.width() as f32, p.height() as f32);
    let thin = (cell / 30.0).max(0.5);
    let shaped = !p.absent.is_empty();
    if !shaped {
        writeln!(out, r##"<rect x="{x0}" y="{y0}" width="{}" height="{}" fill="#fff"/>"##, w * cell, h * cell).unwrap();
    }
    for (y, row) in p.grid.iter().enumerate() {
        for (x, ch) in row.chars().enumerate() {
            let (cx, cy) = (x0 + x as f32 * cell, y0 + y as f32 * cell);
            let k = square_key(x, y);
            if p.is_absent(x, y) {
                // Left out of the grid: empty space.
                continue;
            }
            let look = p.square_fill(x, y);
            if ch == '#' {
                // A shaded black square shows dark grey, so the shading is still visible.
                let fill = look.unwrap_or("#111");
                writeln!(out, r##"<rect x="{cx}" y="{cy}" width="{cell}" height="{cell}" fill="{fill}"/>"##).unwrap();
                if let Some(sp) = p.sprites.get(&k) {
                    sprite_svg(out, sp, cx, cy, cell, 1.0, "#fff");
                }
                continue;
            }
            let fill = look.unwrap_or(if shaped { "#fff" } else { "none" });
            writeln!(out, r##"<rect x="{cx}" y="{cy}" width="{cell}" height="{cell}" fill="{fill}" stroke="#111" stroke-width="{thin}"/>"##).unwrap();
            if let Some(sp) = p.sprites.get(&k) {
                // Behind the number and letter, faded so they stay readable.
                sprite_svg(out, sp, cx, cy, cell, 0.55, "#111");
            }
            if p.circles.contains(&k) {
                writeln!(out, r##"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="#111" stroke-width="{thin}"/>"##, cx + cell / 2.0, cy + cell / 2.0, cell * 0.47).unwrap();
            }
            if nums[y][x] > 0 {
                writeln!(
                    out,
                    r##"<text x="{}" y="{}" font-size="{}" font-family="{font}" fill="#111">{}</text>"##,
                    cx + cell * 0.07,
                    cy + cell * 0.3,
                    cell * 0.28,
                    nums[y][x]
                )
                .unwrap();
            }
            if o.answers && ch.is_ascii_alphabetic() {
                let letters = p.rebus_at(x, y).map(str::to_string).unwrap_or_else(|| ch.to_ascii_uppercase().to_string());
                // A rebus gets a smaller font so it fits the square.
                let n = letters.chars().count() as f32;
                let size = (cell * 0.6).min(cell * 0.92 / (0.68 * n));
                writeln!(
                    out,
                    r##"<text x="{}" y="{}" font-size="{}" font-family="{font}" text-anchor="middle" fill="#111">{}</text>"##,
                    cx + cell / 2.0,
                    cy + cell * 0.6 + size * 0.43,
                    size,
                    esc(&letters)
                )
                .unwrap();
            }
        }
    }
    // Walls: a thick line on the right or bottom edge of a square.
    let wall = thin * 4.0;
    for (k, sides) in &p.bars {
        let Some((x, y)) = crate::puzzle::parse_square_key(k) else { continue };
        let (cx, cy) = (x0 + x as f32 * cell, y0 + y as f32 * cell);
        for side in sides {
            let (x1, y1, x2, y2) = match side.as_str() {
                RIGHT => (cx + cell, cy, cx + cell, cy + cell),
                BOTTOM => (cx, cy + cell, cx + cell, cy + cell),
                _ => continue,
            };
            writeln!(out, r##"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="#111" stroke-width="{wall}" stroke-linecap="square"/>"##).unwrap();
        }
    }
    if !shaped {
        writeln!(
            out,
            r##"<rect x="{x0}" y="{y0}" width="{}" height="{}" fill="none" stroke="#111" stroke-width="{}"/>"##,
            w * cell,
            h * cell,
            thin * st.border
        )
        .unwrap();
        return;
    }
    // A shaped grid: the border follows the squares that are there.
    let mut d = String::new();
    for y in 0..p.height() {
        for x in 0..p.width() {
            if p.is_absent(x, y) {
                continue;
            }
            let (xi, yi) = (x as isize, y as isize);
            let (cx, cy) = (x0 + x as f32 * cell, y0 + y as f32 * cell);
            if p.no_square(xi, yi - 1) {
                write!(d, "M{cx} {cy}h{cell}").unwrap();
            }
            if p.no_square(xi, yi + 1) {
                write!(d, "M{cx} {}h{cell}", cy + cell).unwrap();
            }
            if p.no_square(xi - 1, yi) {
                write!(d, "M{cx} {cy}v{cell}").unwrap();
            }
            if p.no_square(xi + 1, yi) {
                write!(d, "M{} {cy}v{cell}", cx + cell).unwrap();
            }
        }
    }
    writeln!(out, r##"<path d="{d}" fill="none" stroke="#111" stroke-width="{}" stroke-linecap="square"/>"##, thin * st.border).unwrap();
}

/// A sprite in one square: an emoji as text, or an image from its data URI.
/// `ink` colors an emoji when no color emoji font is found.
fn sprite_svg(out: &mut String, sp: &crate::puzzle::Sprite, cx: f32, cy: f32, cell: f32, opacity: f32, ink: &str) {
    if let Some(src) = &sp.src {
        let pad = cell * 0.08;
        writeln!(
            out,
            r##"<image x="{}" y="{}" width="{}" height="{}" opacity="{opacity}" preserveAspectRatio="xMidYMid meet" href="{}"/>"##,
            cx + pad,
            cy + pad,
            cell - 2.0 * pad,
            cell - 2.0 * pad,
            esc(src)
        )
        .unwrap();
    } else if let Some(uri) = sp.emoji.as_deref().and_then(emoji_png) {
        // A picture of the emoji: PDF output can't embed color emoji fonts, but it can embed images.
        let (size, pad) = (cell * 0.7, cell * 0.15);
        writeln!(out, r##"<image x="{}" y="{}" width="{size}" height="{size}" opacity="{opacity}" href="{uri}"/>"##, cx + pad, cy + pad).unwrap();
    } else if let Some(e) = &sp.emoji {
        let size = cell * 0.62;
        writeln!(
            out,
            r##"<text x="{}" y="{}" font-size="{size}" font-family="{EMOJI_FONT}" text-anchor="middle" opacity="{opacity}" fill="{ink}">{}</text>"##,
            cx + cell / 2.0,
            cy + cell * 0.5 + size * 0.36,
            esc(e)
        )
        .unwrap();
    }
}

/// The grid alone, as SVG. 40 units per square plus a small margin.
pub fn grid_only_svg(p: &Puzzle, o: PrintOptions) -> String {
    let cell = 40.0;
    let m = 4.0;
    let (w, h) = (p.width() as f32 * cell + 2.0 * m, p.height() as f32 * cell + 2.0 * m);
    let mut s = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"##);
    s.push('\n');
    writeln!(s, r##"<rect width="{w}" height="{h}" fill="#fff"/>"##).unwrap();
    grid_svg(&mut s, p, m, m, cell, o);
    s.push_str("</svg>\n");
    s
}

/// Plain words wrapped to `width`, at most `max` lines; the last line ends in "…" when text is cut.
fn wrap_plain(text: &str, width: f32, size: f32, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = vec![];
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if text_width(&format!("{l} {word}"), size) <= width => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    if lines.len() > max {
        lines.truncate(max);
        if let Some(l) = lines.last_mut() {
            l.push('…');
        }
    }
    lines
}

/// A share card, 1200 x 630 (the size link previews use): the empty grid on the left, and on the
/// right a heading, a few smaller lines and a footer. No answers are drawn.
pub fn card_svg(p: &Puzzle, heading: &str, lines: &[String], footer: &str) -> String {
    let (w, h) = (1200.0_f32, 630.0_f32);
    let mut s = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"##);
    s.push('\n');
    writeln!(s, r##"<rect width="{w}" height="{h}" fill="#f4f4f2"/>"##).unwrap();
    // The grid: fit a 510 box, centred in the left half.
    let side = 510.0;
    let cell = side / p.width().max(p.height()).max(1) as f32;
    let (gw, gh) = (p.width() as f32 * cell, p.height() as f32 * cell);
    grid_svg(&mut s, p, 60.0 + (side - gw) / 2.0, (h - gh) / 2.0, cell, PrintOptions::default());
    // The text: the heading as large as fits in three lines.
    let (tx, tw) = (630.0, 510.0);
    let heading = if heading.trim().is_empty() { "Untitled" } else { heading.trim() };
    // Bold text runs a little wider than `text_width` says.
    let fit = |size: f32| wrap_plain(heading, tw / 1.1, size, 3);
    let size = [76.0, 64.0, 54.0, 46.0].into_iter().find(|s| heading.split_whitespace().all(|word| text_width(word, *s) * 1.1 <= tw) && fit(*s).len() <= 3 && !fit(*s).last().is_some_and(|l| l.ends_with('…'))).unwrap_or(40.0);
    let title = fit(size);
    let block = title.len() as f32 * size * 1.12 + lines.len() as f32 * 44.0 + if lines.is_empty() { 0.0 } else { 14.0 };
    let mut y = (h - block) / 2.0 - 20.0 + size * 0.85;
    for l in &title {
        writeln!(s, r##"<text x="{tx}" y="{y}" font-family="{FONT}" font-size="{size}" font-weight="700" fill="#111">{}</text>"##, esc(l)).unwrap();
        y += size * 1.12;
    }
    y += 14.0 - size * 0.25;
    for l in lines {
        let l = wrap_plain(l, tw, 30.0, 1).into_iter().next().unwrap_or_default();
        writeln!(s, r##"<text x="{tx}" y="{}" font-family="{FONT}" font-size="30" fill="#555">{}</text>"##, y + 30.0, esc(&l)).unwrap();
        y += 44.0;
    }
    if !footer.trim().is_empty() {
        let f = wrap_plain(footer.trim(), tw, 26.0, 1).into_iter().next().unwrap_or_default();
        writeln!(s, r##"<text x="{tx}" y="{}" font-family="{FONT}" font-size="26" fill="#777">{}</text>"##, h - 62.0, esc(&f)).unwrap();
    }
    s.push_str("</svg>\n");
    s
}

/// A puzzle's share card: its title, "By AUTHOR", its size, and `footer` (the site's name).
pub fn puzzle_card_png(p: &Puzzle, footer: &str) -> Result<Vec<u8>> {
    let mut lines = vec![];
    if !p.author.trim().is_empty() {
        lines.push(format!("By {}", p.author.trim()));
    }
    lines.push(format!("{}×{} crossword", p.width(), p.height()));
    svg_to_png(&card_svg(p, &p.title, &lines, footer), 1.0)
}

/// A share card with any heading and lines, as a PNG.
pub fn card_png(p: &Puzzle, heading: &str, lines: &[String], footer: &str) -> Result<Vec<u8>> {
    svg_to_png(&card_svg(p, heading, lines, footer), 1.0)
}

/// Rough text width, for wrapping. Helvetica averages about half an em per character.
pub fn text_width(s: &str, size: f32) -> f32 {
    s.chars().map(|c| if c.is_ascii_uppercase() || c == 'm' || c == 'w' { 0.68 } else if " il.,;:'!|".contains(c) { 0.28 } else { 0.53 }).sum::<f32>() * size
}

/// A clue split into lines that fit `width`. Each line is a list of runs (see `markup`), so italics
/// survive the wrap.
pub fn wrap(text: &str, width: f32, size: f32) -> Vec<Vec<Run>> {
    // Words as lists of runs; a word may change style partway ("<i>Jaws</i>'s").
    let mut words: Vec<Vec<Run>> = vec![];
    let mut in_word = false;
    for r in markup::runs(text) {
        for (i, part) in r.text.split(char::is_whitespace).enumerate() {
            if i > 0 {
                in_word = false;
            }
            if part.is_empty() {
                continue;
            }
            let piece = Run { text: part.to_string(), italic: r.italic };
            match words.last_mut() {
                Some(w) if in_word => w.push(piece),
                _ => words.push(vec![piece]),
            }
            in_word = true;
        }
    }
    let plain = |l: &[Run]| l.iter().map(|r| r.text.as_str()).collect::<String>();
    let mut lines: Vec<Vec<Run>> = vec![];
    let mut cur: Vec<Run> = vec![];
    for word in words {
        let next = if cur.is_empty() { plain(&word) } else { format!("{} {}", plain(&cur), plain(&word)) };
        if !cur.is_empty() && text_width(&next, size) > width {
            lines.push(std::mem::take(&mut cur));
        }
        if let Some(last) = cur.last_mut() {
            // The space between words is italic only inside an italic stretch.
            if last.italic && !word[0].italic {
                cur.push(Run { text: " ".into(), italic: false });
            } else {
                last.text.push(' ');
            }
        }
        for piece in word {
            match cur.last_mut() {
                Some(l) if l.italic == piece.italic => l.text.push_str(&piece.text),
                _ => cur.push(piece),
            }
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// One line of clue text as SVG text content: italic runs in `<tspan font-style="italic">`.
pub fn line_svg(line: &[Run]) -> String {
    line.iter().map(|r| if r.italic { format!(r#"<tspan font-style="italic">{}</tspan>"#, esc(&r.text)) } else { esc(&r.text) }).collect()
}

/// A picture clue's image as an SVG `<image>`, its top-left corner at `x`, `top`, scaled to fit
/// `max_w` x `max_h` with its shape kept. Also gives the height drawn. None if the image can't be
/// read (then show the clue as text, with `Puzzle::clue_text_for`).
pub fn picture_svg(pic: &crate::puzzle::CluePicture, x: f32, top: f32, max_w: f32, max_h: f32) -> Option<(String, f32)> {
    let (mime, bytes) = crate::pictures::decode_url(&pic.image)?;
    if !matches!(mime, "image/png" | "image/jpeg" | "image/gif" | "image/webp") {
        return None;
    }
    let (pw, ph) = crate::pictures::dimensions(&bytes).filter(|&(w, h)| w > 0 && h > 0).unwrap_or((1, 1));
    let k = (max_w / pw as f32).min(max_h / ph as f32);
    let (w, h) = (pw as f32 * k, ph as f32 * k);
    let svg = format!(
        r#"<image x="{x}" y="{top}" width="{w}" height="{h}" preserveAspectRatio="xMinYMin meet" href="data:{mime};base64,{}"/>"#,
        crate::pictures::encode(&bytes)
    );
    Some((svg, h))
}

/// The clue text to draw beside a picture, and the picture: the text alone, and for a picture
/// that can't be drawn, the text with "[Picture] <alt text>".
pub fn clue_and_picture<'a>(p: &'a Puzzle, s: &crate::puzzle::SlotInfo) -> (String, Option<&'a crate::puzzle::CluePicture>) {
    match p.picture_for(s) {
        Some(pic) if crate::pictures::decode_url(&pic.image).is_some_and(|(m, _)| matches!(m, "image/png" | "image/jpeg" | "image/gif" | "image/webp")) => {
            (p.clue_for(s).unwrap_or("").to_string(), Some(pic))
        }
        _ => (p.clue_text_for(s).unwrap_or_default(), None),
    }
}

/// How tall a picture clue's image is on the one-page puzzle: about five lines of clue text.
fn pic_h(size: f32) -> f32 {
    size * 6.0
}

/// One line of the clue flow.
enum Item<'a> {
    Heading(&'static str),
    Clue { number: usize, lines: Vec<Vec<Run>>, picture: Option<&'a crate::puzzle::CluePicture> },
}

/// A one-page US Letter puzzle as SVG (units are points): title, grid, and both clue lists.
pub fn page_svg(p: &Puzzle, o: PrintOptions) -> String {
    let (pw, ph, m) = (612.0f32, 792.0f32, 36.0f32);
    let mut s = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="{pw}" height="{ph}" viewBox="0 0 {pw} {ph}">"##);
    s.push('\n');
    writeln!(s, r##"<rect width="{pw}" height="{ph}" fill="#fff"/>"##).unwrap();

    // Header.
    let title = if p.title.trim().is_empty() { "Untitled" } else { p.title.trim() };
    writeln!(s, r##"<text x="{m}" y="{}" font-size="18" font-weight="bold" font-family="{FONT}" fill="#111">{}</text>"##, m + 14.0, esc(title)).unwrap();
    let byline = [p.author.trim(), p.copyright.trim()].iter().filter(|t| !t.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
    if !byline.is_empty() {
        writeln!(s, r##"<text x="{m}" y="{}" font-size="10" font-family="{FONT}" fill="#555">{}</text>"##, m + 30.0, esc(&byline)).unwrap();
    }
    let top = m + 44.0;

    // Grid at the top right, up to 360pt wide.
    let gw = 360.0f32.min(pw - 2.0 * m);
    let cell = (gw / p.width() as f32).min(380.0 / p.height() as f32);
    let (gwid, ghgt) = (cell * p.width() as f32, cell * p.height() as f32);
    let gx = pw - m - gwid;
    grid_svg(&mut s, p, gx, top, cell, o);

    // Clue columns: one left of the grid (full height), then two below it.
    let gap = 14.0;
    let bottom = ph - m;
    let left_w = gx - gap - m;
    let below = top + ghgt + 18.0;
    let mut cols: Vec<(f32, f32, f32, f32)> = vec![]; // x, width, top, bottom
    if left_w > 100.0 {
        cols.push((m, left_w, top, bottom));
        let bw = (gwid - gap) / 2.0;
        cols.push((gx, bw, below, bottom));
        cols.push((gx + bw + gap, bw, below, bottom));
    } else {
        let bw = (pw - 2.0 * m - 2.0 * gap) / 3.0;
        for i in 0..3 {
            cols.push((m + i as f32 * (bw + gap), bw, below, bottom));
        }
    }

    // Shrink the clue text until everything fits on the page.
    let slots = p.slots();
    let mut size = 9.5f32;
    loop {
        let num_w = size * 1.7;
        let lead = size * 1.2;
        let mut items = vec![];
        for (dir, name) in [(Dir::Across, "ACROSS"), (Dir::Down, "DOWN")] {
            items.push(Item::Heading(name));
            for sl in slots.iter().filter(|z| z.dir == dir) {
                let (text, picture) = clue_and_picture(p, sl);
                let lines = if text.trim().is_empty() && picture.is_some() { vec![] } else { wrap(&text, cols[0].1.min(cols[1].1) - num_w, size) };
                items.push(Item::Clue { number: sl.number, lines, picture });
            }
        }
        // Lay out; restart smaller if it overflows.
        let mut out = String::new();
        let (mut c, mut y) = (0usize, cols[0].2);
        let mut fits = true;
        for it in &items {
            let (height, first_gap) = match it {
                Item::Heading(_) => (lead * 1.3, if y > cols[c].2 { lead * 0.6 } else { 0.0 }),
                Item::Clue { lines, picture, .. } => (lead * lines.len() as f32 + picture.map_or(0.0, |_| pic_h(size) + size * 0.25) + size * 0.25, 0.0),
            };
            if y + first_gap + height > cols[c].3 {
                c += 1;
                if c >= cols.len() {
                    fits = false;
                    break;
                }
                y = cols[c].2;
            } else {
                y += first_gap;
            }
            let x = cols[c].0;
            match it {
                Item::Heading(name) => {
                    writeln!(out, r##"<text x="{x}" y="{}" font-size="{}" font-weight="bold" font-family="{FONT}" fill="#111">{name}</text>"##, y + size, size * 1.05).unwrap();
                }
                Item::Clue { number, lines, picture } => {
                    writeln!(out, r##"<text x="{x}" y="{}" font-size="{size}" font-weight="bold" font-family="{FONT}" fill="#111">{number}</text>"##, y + size).unwrap();
                    for (i, l) in lines.iter().enumerate() {
                        out.push_str(&text_line_svg(x + num_w, y + size + i as f32 * lead, size, &format!(r##"font-size="{size}" font-family="{FONT}" fill="#111""##), l));
                    }
                    if let Some(pic) = picture {
                        let top = y + lead * lines.len() as f32 + size * 0.25;
                        if let Some((img, _)) = picture_svg(pic, x + num_w, top, cols[c].1 - num_w, pic_h(size)) {
                            out.push_str(&img);
                            out.push('\n');
                        }
                    }
                }
            }
            y += height;
        }
        if fits || size <= 5.0 {
            s.push_str(&out);
            break;
        }
        size -= 0.25;
    }
    s.push_str("</svg>\n");
    s
}

/// True for a character that starts an emoji: the PDF's text fonts have no glyph for it, and a
/// line that holds one can come out as empty boxes, so it is drawn as a picture instead.
fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0x2300..=0x23FF)
}

/// True for a character that joins an emoji sequence: variation selectors, the zero-width joiner,
/// skin tones, keycap marks and tag characters.
fn emoji_tail(c: char) -> bool {
    matches!(c as u32, 0xFE0E | 0xFE0F | 0x200D | 0x20E3 | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F) || is_emoji(c)
}

/// A line split into text runs and emoji (each `Err` is one emoji sequence).
fn split_emoji(line: &[Run]) -> Vec<Result<Run, String>> {
    let mut out: Vec<Result<Run, String>> = vec![];
    for r in line {
        let mut chars = r.text.chars().peekable();
        while let Some(c) = chars.next() {
            if is_emoji(c) {
                let mut e = c.to_string();
                while let Some(&n) = chars.peek() {
                    // A joiner pulls in the next emoji; a new emoji with no joiner starts its own.
                    if !emoji_tail(n) || (is_emoji(n) && !e.ends_with('\u{200D}') && !(0x1F3FB..=0x1F3FF).contains(&(n as u32))) {
                        break;
                    }
                    e.push(n);
                    chars.next();
                }
                out.push(Err(e));
            } else {
                match out.last_mut() {
                    Some(Ok(t)) if t.italic == r.italic => t.text.push(c),
                    _ => out.push(Ok(Run { text: c.to_string(), italic: r.italic })),
                }
            }
        }
    }
    out
}

/// A readable stand-in for an emoji no font can draw.
fn emoji_word(e: &str) -> &'static str {
    match e.trim_end_matches(['\u{FE0F}', '\u{FE0E}']) {
        "🔒" | "🔐" => "[Locked]",
        "🔓" => "[Unlocked]",
        "🔑" | "🗝" => "[Key]",
        _ => "",
    }
}

#[cfg(test)]
pub fn emoji_word_for_test(e: &str) -> &'static str {
    emoji_word(e)
}

/// How wide `text` is as drawn, from a laid-out SVG. `attrs` are the text's font attributes.
fn measure(attrs: &str, line: &[Run]) -> Option<f32> {
    let w = |body: String| -> Option<f32> {
        let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="2000" height="100"><text x="0" y="50" {attrs} xml:space="preserve">{body}</text></svg>"#);
        Some(tree(&svg).ok()?.root().abs_bounding_box().width())
    };
    // Bars at both ends, so spaces at the ends count.
    Some(w(format!("|{}|", line_svg(line)))? - w("||".into())?)
}

/// One line of clue text as SVG at `x`, baseline `y`, with `attrs` (font size, family, fill).
/// Emoji are drawn as pictures inline (or as a word like "[Key]" if no font draws them), so the
/// rest of the line always prints.
pub fn text_line_svg(x: f32, y: f32, size: f32, attrs: &str, line: &[Run]) -> String {
    let text = |x: f32, runs: &[Run]| format!(r#"<text x="{x}" y="{y}" {attrs} xml:space="preserve">{}</text>"#, line_svg(runs)) + "\n";
    if !line.iter().any(|r| r.text.chars().any(is_emoji)) {
        return text(x, line);
    }
    let mut out = String::new();
    let mut cx = x;
    for part in split_emoji(line) {
        let runs = match part {
            Ok(r) => vec![r],
            Err(e) => match emoji_png(&e) {
                Some(url) => {
                    let s = size * 1.05;
                    out.push_str(&format!(r#"<image x="{cx}" y="{}" width="{s}" height="{s}" href="{url}"/>"#, y - size * 0.85));
                    out.push('\n');
                    cx += s + size * 0.1;
                    continue;
                }
                None => vec![Run { text: emoji_word(&e).to_string(), italic: false }],
            },
        };
        if runs[0].text.is_empty() {
            continue;
        }
        out.push_str(&text(cx, &runs));
        cx += measure(attrs, &runs).unwrap_or_else(|| text_width(&runs[0].text, size));
    }
    out
}

/// An emoji drawn as a 128px PNG data URI, using the system's color emoji font. None if no font
/// draws it (then the caller draws it as plain text).
fn emoji_png(emoji: &str) -> Option<String> {
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128"><text x="64" y="108" font-size="104" font-family="{EMOJI_FONT}" text-anchor="middle">{}</text></svg>"##,
        esc(emoji)
    );
    let t = tree(&svg).ok()?;
    let mut pix = resvg::tiny_skia::Pixmap::new(128, 128)?;
    resvg::render(&t, resvg::tiny_skia::Transform::identity(), &mut pix.as_mut());
    if pix.pixels().iter().all(|p| p.alpha() == 0) {
        return None;
    }
    Some(format!("data:image/png;base64,{}", crate::edit::base64(&pix.encode_png().ok()?)))
}

/// Load the system fonts now, so the first export doesn't wait for it.
pub fn warm_up() {
    let _ = fonts();
}

/// The fonts in use: the system fonts, plus any added with `add_font`.
static FONTS: Mutex<Option<Arc<usvg::fontdb::Database>>> = Mutex::new(None);

fn fonts() -> Arc<usvg::fontdb::Database> {
    let mut db = FONTS.lock().unwrap_or_else(|e| e.into_inner());
    db.get_or_insert_with(|| {
        let mut d = usvg::fontdb::Database::new();
        d.load_system_fonts();
        Arc::new(d)
    })
    .clone()
}

/// Add a font file (TrueType, OpenType or a collection) to the fonts drawings use. This is for
/// places with no system fonts, such as WebAssembly. A drawing finds it by its family name, so add
/// one of the families the drawings ask for (for example Liberation Sans and Liberation Serif).
/// Returns how many font faces the data held.
pub fn add_font(data: Vec<u8>) -> usize {
    let current = fonts();
    let mut d = (*current).clone();
    let before = d.len();
    d.load_font_data(data);
    let added = d.len() - before;
    *FONTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(d));
    added
}

/// Parse an SVG with the system fonts loaded, ready to draw.
pub fn tree(svg: &str) -> Result<usvg::Tree> {
    let fontdb = fonts();
    if fontdb.is_empty() {
        bail!("no system fonts found, so text can't be drawn");
    }
    let opt = usvg::Options { fontdb, ..Default::default() };
    usvg::Tree::from_str(svg, &opt).map_err(|e| anyhow!("drawing failed: {e}"))
}

/// The grid as a PNG. `scale` 2 gives 80 pixels per square.
pub fn to_png(p: &Puzzle, o: PrintOptions, scale: f32) -> Result<Vec<u8>> {
    svg_to_png(&grid_only_svg(p, o), scale)
}

/// The PDF page as a PNG, for previews. `scale` 1 is 612 x 792 pixels.
pub fn page_png(p: &Puzzle, o: PrintOptions, scale: f32) -> Result<Vec<u8>> {
    svg_to_png(&page_svg(p, o), scale)
}

/// Any SVG as a PNG.
pub fn svg_to_png(svg: &str, scale: f32) -> Result<Vec<u8>> {
    if !(0.1..=8.0).contains(&scale) {
        bail!("scale must be between 0.1 and 8");
    }
    let t = tree(svg)?;
    let size = t.size().to_int_size().scale_by(scale).ok_or_else(|| anyhow!("bad image size"))?;
    let mut pix = resvg::tiny_skia::Pixmap::new(size.width(), size.height()).ok_or_else(|| anyhow!("bad image size"))?;
    resvg::render(&t, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pix.as_mut());
    pix.encode_png().map_err(|e| anyhow!("png: {e}"))
}

/// A one-page PDF with the title, grid and clues.
#[cfg(feature = "pdf")]
pub fn to_pdf(p: &Puzzle, o: PrintOptions) -> Result<Vec<u8>> {
    let t = tree(&page_svg(p, o))?;
    svg2pdf::to_pdf(&t, svg2pdf::ConversionOptions::default(), svg2pdf::PageOptions::default()).map_err(|e| anyhow!("pdf: {e}"))
}

/// What a build without the `pdf` feature says.
#[cfg(not(feature = "pdf"))]
pub const NO_PDF: &str = "This build has no PDF export. Export png or jpz, or build with the `pdf` feature.";

#[cfg(not(feature = "pdf"))]
pub fn to_pdf(_: &Puzzle, _: PrintOptions) -> Result<Vec<u8>> {
    Err(anyhow!(NO_PDF))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_long_text() {
        let lines = wrap("One two three four five six seven eight nine ten", 60.0, 9.0);
        assert!(lines.len() > 1);
        let text: Vec<String> = lines.iter().map(|l| l.iter().map(|r| r.text.as_str()).collect()).collect();
        assert_eq!(text.join(" "), "One two three four five six seven eight nine ten");
        assert_eq!(wrap("", 60.0, 9.0), vec![vec![]]);
    }

    #[test]
    fn wraps_italics() {
        let lines = wrap("Star of <i>The Long Goodbye</i>'s sequel, briefly", 80.0, 9.0);
        let svg: Vec<String> = lines.iter().map(|l| line_svg(l)).collect();
        let all = svg.join("|");
        assert!(all.contains(r#"<tspan font-style="italic">The Long"#), "{all}");
        assert!(all.contains("Goodbye</tspan>'s"), "{all}");
        assert!(!all.contains("&lt;i"), "{all}");
    }

    #[test]
    fn renders_png_and_pdf() {
        let mut p = Puzzle::blank("A & B <test>", 5, 5, "rotational");
        p.grid[0] = "CAT#.".into();
        let png = to_png(&p, PrintOptions { answers: true }, 1.0).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let pdf = to_pdf(&p, PrintOptions::default()).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert!(page_svg(&p, PrintOptions::default()).contains("A &amp; B &lt;test&gt;"));
        p.clues.insert("CAT".into(), "<i>Cats</i> & dogs".into());
        let svg = page_svg(&p, PrintOptions::default());
        assert!(svg.contains(r#"<tspan font-style="italic">Cats</tspan> &amp; dogs"#), "{svg}");
        assert!(to_pdf(&p, PrintOptions::default()).unwrap().starts_with(b"%PDF"));
    }
}
