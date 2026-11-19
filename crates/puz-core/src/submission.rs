//! Submission PDFs: a puzzle in the format a paper asks constructors to send. Now only the NYT's:
//! page 1 has the contact blocks and the filled grid, then double-spaced clue pages with the
//! answers in a right column, in Times. Drawn as SVG with the grid and text helpers of `print`,
//! then joined into one PDF with pdf-writer. Add a paper as a new `Style`.

use crate::profile::{authors, Contact, Profile};
use anyhow::{anyhow, bail, Result};
use crate::print::{esc, grid_svg_styled, line_svg, svg_to_png, text_width, wrap, GridStyle, PrintOptions};
use crate::puzzle::{Dir, Puzzle, SlotInfo};
use std::fmt::Write;

/// Times, the font of NYT's sample submission.
const SERIF: &str = "'Times New Roman', Times, 'Liberation Serif', 'Nimbus Roman', 'DejaVu Serif', serif";
/// Times is narrower than Helvetica: `text_width` times this is about right for it.
const SERIF_WIDTH: f32 = 0.88;

/// Every size and position on a submission page, in points (72 per inch), in one place, measured
/// from NYT's sample submission PDF (US Letter).
struct SubLayout {
    page_w: f32,
    page_h: f32,
    /// Page 1: contact blocks (bold), their top-left corner, line pitch, and where a
    /// co-constructor's block starts.
    contact_x: f32,
    contact_top: f32,
    contact_size: f32,
    contact_pitch: f32,
    partner_x: f32,
    /// Page 1: the filled grid's width and top edge.
    grid_w: f32,
    grid_top: f32,
    /// The lowest the grid may reach (a tall grid gets smaller squares).
    grid_bottom: f32,
    /// Clue pages: left margin, first baseline on a page, lowest baseline, text size.
    left: f32,
    first_baseline: f32,
    last_baseline: f32,
    size: f32,
    /// From one clue's first line to the next clue's; between a clue's own lines.
    pitch: f32,
    wrap_pitch: f32,
    /// The clue text wraps at this x; the answer column starts at this x.
    clue_right: f32,
    answer_x: f32,
}

const SUB: SubLayout = SubLayout {
    page_w: 612.0,
    page_h: 792.0,
    contact_x: 36.0,
    contact_top: 36.0,
    contact_size: 14.0,
    contact_pitch: 18.0,
    partner_x: 306.0,
    grid_w: 345.6,
    grid_top: 208.8,
    grid_bottom: 756.0,
    left: 43.2,
    first_baseline: 74.0,
    last_baseline: 720.0,
    size: 12.0,
    pitch: 24.0,
    wrap_pitch: 14.0,
    clue_right: 374.4,
    answer_x: 388.8,
};

/// A paper's submission format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Nyt,
}

impl Style {
    /// Every style's name.
    pub const NAMES: [&'static str; 1] = ["nyt"];

    pub fn parse(s: &str) -> Result<Style> {
        match s.trim().to_ascii_lowercase().as_str() {
            "nyt" => Ok(Style::Nyt),
            other => bail!("unknown submission style {other:?} (use {})", Style::NAMES.join(", ")),
        }
    }
}

/// What page 1 shows for this puzzle and profile: the contact blocks (yours, then a
/// co-constructor's; names fall back to the puzzle's author field).
pub fn options(p: &Puzzle, profile: &Profile, blank_grid: bool) -> SubmissionOptions {
    SubmissionOptions { contacts: profile.blocks(&authors(&p.author)), blank_grid }
}

/// The puzzle as a submission PDF in a paper's format.
pub fn pdf(style: Style, p: &Puzzle, o: &SubmissionOptions) -> Result<Vec<u8>> {
    match style {
        Style::Nyt => submission_pdf(p, o),
    }
}

/// What goes in a submission PDF besides the grid and clues.
#[derive(Clone, Default)]
pub struct SubmissionOptions {
    /// Contact blocks for the top of page 1 (yours, then a co-constructor's).
    pub contacts: Vec<Contact>,
    /// Add a page with the blank, numbered grid after the filled grid.
    pub blank_grid: bool,
}

/// The answer as the clue pages show it: capitals, a rebus square in brackets, "_" for an empty square.
fn shown_answer(s: &SlotInfo) -> String {
    s.pattern.replace('.', "_").to_uppercase()
}

fn page_start() -> String {
    let (w, h) = (SUB.page_w, SUB.page_h);
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\n<rect width=\"{w}\" height=\"{h}\" fill=\"#fff\"/>\n")
}

/// A grid page: `contacts` at the top left (page 1 only), the grid centered.
fn grid_page(p: &Puzzle, answers: bool, contacts: &[Contact]) -> String {
    let mut s = page_start();
    for (i, c) in contacts.iter().enumerate() {
        let x = if i == 0 { SUB.contact_x } else { SUB.partner_x };
        for (j, line) in c.lines().iter().enumerate() {
            let y = SUB.contact_top + SUB.contact_size + j as f32 * SUB.contact_pitch;
            writeln!(s, r##"<text x="{x}" y="{y}" font-size="{}" font-weight="bold" font-family="{SERIF}" fill="#000">{}</text>"##, SUB.contact_size, esc(line)).unwrap();
        }
    }
    let (w, h) = (p.width() as f32, p.height() as f32);
    let cell = (SUB.grid_w / w).min((SUB.grid_bottom - SUB.grid_top) / h);
    let x = (SUB.page_w - cell * w) / 2.0;
    grid_svg_styled(&mut s, p, x, SUB.grid_top, cell, PrintOptions { answers }, &GridStyle { font: SERIF, border: 1.0 });
    s.push_str("</svg>\n");
    s
}

/// The clue pages: "ACROSS", then "DOWN" after a blank line. Each clue is "N clue" at the left
/// margin, double-spaced, wrapping under its text; its answer is in the column on the right, on
/// the clue's first line. A clue never splits across pages. A missing clue shows just its number.
fn clue_pages(p: &Puzzle) -> Vec<String> {
    let size = SUB.size;
    let slots = p.slots();
    let mut pages = vec![];
    let mut page = String::new();
    let mut y = SUB.first_baseline;
    let mut new_page = |page: &mut String, y: &mut f32| {
        pages.push(std::mem::take(page));
        *y = SUB.first_baseline;
    };
    for (n, (dir, name)) in [(Dir::Across, "ACROSS"), (Dir::Down, "DOWN")].into_iter().enumerate() {
        if n > 0 {
            y += SUB.pitch; // the blank line before DOWN
        }
        // Keep the heading with its first clue.
        if y + SUB.pitch > SUB.last_baseline && !page.is_empty() {
            new_page(&mut page, &mut y);
        }
        writeln!(page, r##"<text x="{}" y="{y}" font-size="{size}" font-family="{SERIF}" fill="#000">{name}</text>"##, SUB.left).unwrap();
        y += SUB.pitch;
        for sl in slots.iter().filter(|z| z.dir == dir) {
            let number = format!("{} ", sl.number);
            let text_x = SUB.left + text_width(&number, size) * SERIF_WIDTH;
            let lines = wrap(p.clue_for(sl).unwrap_or(""), (SUB.clue_right - text_x) / SERIF_WIDTH, size);
            let height = (lines.len() - 1) as f32 * SUB.wrap_pitch;
            if y + height > SUB.last_baseline && !page.is_empty() {
                new_page(&mut page, &mut y);
            }
            writeln!(page, r##"<text x="{}" y="{y}" font-size="{size}" font-family="{SERIF}" fill="#000">{}</text>"##, SUB.left, sl.number).unwrap();
            for (j, l) in lines.iter().enumerate() {
                writeln!(
                    page,
                    r##"<text x="{text_x}" y="{}" font-size="{size}" font-family="{SERIF}" fill="#000" xml:space="preserve">{}</text>"##,
                    y + j as f32 * SUB.wrap_pitch,
                    line_svg(l)
                )
                .unwrap();
            }
            writeln!(page, r##"<text x="{}" y="{y}" font-size="{size}" font-family="{SERIF}" fill="#000">{}</text>"##, SUB.answer_x, esc(&shown_answer(sl))).unwrap();
            y += height + SUB.pitch;
        }
    }
    if !page.is_empty() {
        pages.push(page);
    }
    pages
        .into_iter()
        .map(|body| {
            let mut s = page_start();
            s.push_str(&body);
            s.push_str("</svg>\n");
            s
        })
        .collect()
}

/// Every page of an NYT-format submission as SVG (units are points): the filled grid with the
/// contact blocks, the blank grid if asked, then the clue pages.
pub fn submission_svgs(p: &Puzzle, o: &SubmissionOptions) -> Vec<String> {
    let mut pages = vec![grid_page(p, true, &o.contacts)];
    if o.blank_grid {
        pages.push(grid_page(p, false, &[]));
    }
    pages.extend(clue_pages(p));
    pages
}

/// The puzzle in the NYT submission format, as one PDF.
pub fn submission_pdf(p: &Puzzle, o: &SubmissionOptions) -> Result<Vec<u8>> {
    let author = o.contacts.first().map(|c| c.name.trim().to_string()).unwrap_or_default();
    svgs_to_pdf(&submission_svgs(p, o), p.title.trim(), &author)
}

/// One page of the submission PDF as a PNG (`page` counts from 1). `scale` 1 is 612 x 792 pixels.
pub fn submission_png(p: &Puzzle, o: &SubmissionOptions, page: usize, scale: f32) -> Result<Vec<u8>> {
    let pages = submission_svgs(p, o);
    let svg = pages.get(page.wrapping_sub(1)).ok_or_else(|| anyhow!("page {page} is out of range (1 to {})", pages.len()))?;
    svg_to_png(svg, scale)
}

/// Several SVG pages as one PDF. svg2pdf converts one SVG at a time, so each page is converted to
/// a form XObject and placed on its own page with pdf-writer.
#[cfg(not(feature = "pdf"))]
fn svgs_to_pdf(_: &[String], _: &str, _: &str) -> Result<Vec<u8>> {
    Err(anyhow!(crate::print::NO_PDF))
}

#[cfg(feature = "pdf")]
fn svgs_to_pdf(svgs: &[String], title: &str, author: &str) -> Result<Vec<u8>> {
    use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, TextStr};
    let mut alloc = Ref::new(1);
    let catalog = alloc.bump();
    let tree_id = alloc.bump();
    let mut pdf = Pdf::new();
    let mut page_ids = vec![];
    for svg in svgs {
        let t = crate::print::tree(svg)?;
        let (w, h) = (t.size().width(), t.size().height());
        let (chunk, root) = svg2pdf::to_chunk(&t, svg2pdf::ConversionOptions::default()).map_err(|e| anyhow!("pdf: {e}"))?;
        let mut map = std::collections::HashMap::new();
        let chunk = chunk.renumber(|old| *map.entry(old).or_insert_with(|| alloc.bump()));
        let svg_id = map[&root];
        let (page_id, content_id) = (alloc.bump(), alloc.bump());
        let name = Name(b"S1");
        let mut page = pdf.page(page_id);
        page.media_box(Rect::new(0.0, 0.0, w, h));
        page.parent(tree_id);
        page.contents(content_id);
        page.resources().x_objects().pair(name, svg_id);
        page.finish();
        let mut content = Content::new();
        content.transform([w, 0.0, 0.0, h, 0.0, 0.0]).x_object(name);
        pdf.stream(content_id, &content.finish());
        pdf.extend(&chunk);
        page_ids.push(page_id);
    }
    pdf.catalog(catalog).pages(tree_id);
    pdf.pages(tree_id).count(page_ids.len() as i32).kids(page_ids);
    let mut info = pdf.document_info(alloc.bump());
    if !title.is_empty() {
        info.title(TextStr(title));
    }
    if !author.is_empty() {
        info.author(TextStr(author));
    }
    info.finish();
    Ok(pdf.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "pdf")]
    #[test]
    fn submission_pdf_has_pages() {
        let mut p = Puzzle::blank("Sample", 15, 15, "rotational");
        for (i, s) in p.slots().iter().enumerate() {
            p.clues.insert(p.clue_key(s), format!("Clue number {i} with <i>some</i> words"));
        }
        let contact = Contact { name: "A. Maker".into(), email: "a@example.com".into(), ..Default::default() };
        let o = SubmissionOptions { contacts: vec![contact], blank_grid: false };
        let svgs = submission_svgs(&p, &o);
        assert!(svgs.len() >= 3, "{} pages", svgs.len());
        assert!(svgs[0].contains(">A. Maker<") && svgs[0].contains(">a@example.com<"));
        assert!(svgs[1].contains(">ACROSS<"));
        // Contact details are on page 1 only.
        assert!(svgs[1..].iter().all(|s| !s.contains("a@example.com")));
        let with_blank = submission_svgs(&p, &SubmissionOptions { blank_grid: true, ..o.clone() });
        assert_eq!(with_blank.len(), svgs.len() + 1);
        let pdf = submission_pdf(&p, &o).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert!(String::from_utf8_lossy(&pdf).contains(&format!("/Count {}", svgs.len())), "page count");
    }
}
