//! One puzzle file out to another format, and another format in.

use crate::formats;
use crate::print::{self, PrintOptions};
use crate::puzzle::Puzzle;
use anyhow::{bail, Context, Result};

/// The formats `export` writes.
pub const FORMATS: [&str; 5] = ["jpz", "puz", "json", "png", "pdf"];

/// An exported puzzle.
pub struct Exported {
    pub bytes: Vec<u8>,
    /// The media type, for a download.
    pub mime: &'static str,
    /// What the format can't hold and so left out ("circles", "italics in 3 clues").
    pub dropped: Vec<String>,
}

/// The puzzle as `format`: jpz, puz (Across Lite), json, png (the grid) or pdf (one page: title,
/// grid and clues). `answers` shows the letters in a png or pdf; `scale` sizes a png (2 = 80 pixels
/// per square).
pub fn export(p: &Puzzle, format: &str, answers: bool, scale: f32) -> Result<Exported> {
    let o = PrintOptions { answers };
    let (bytes, mime, dropped) = match format {
        "jpz" => (formats::to_jpz(p).into_bytes(), "application/xml", formats::jpz_dropped(p)),
        "puz" => (formats::to_puz(p)?, "application/x-crossword", formats::puz_dropped(p)),
        "json" => ((serde_json::to_string_pretty(p)? + "\n").into_bytes(), "application/json", vec![]),
        "png" => (print::to_png(p, o, scale)?, "image/png", vec![]),
        "pdf" => (print::to_pdf(p, o)?, "application/pdf", vec![]),
        f => bail!("unknown format {f:?} (use {})", FORMATS.join(", ")),
    };
    Ok(Exported { bytes, mime, dropped })
}

/// Read a puzzle from the bytes of a file named `name`: a puzzle `.json`, else an Across Lite
/// `.puz`. A puzzle with no title takes the file's name.
pub fn import(name: &str, bytes: &[u8]) -> Result<Puzzle> {
    let path = std::path::Path::new(name);
    let mut p = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
        let p: Puzzle = serde_json::from_slice(bytes).with_context(|| format!("{name} is not a puzzle file"))?;
        p.validate()?;
        p
    } else {
        formats::from_puz(bytes)?
    };
    if p.title.trim().is_empty() {
        p.title = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_and_imports() {
        let mut p = Puzzle::blank("", 3, 3, "rotational");
        p.grid = vec!["CAT".into(), "ARE".into(), "BEN".into()];
        for f in FORMATS {
            assert!(!export(&p, f, true, 1.0).unwrap().bytes.is_empty(), "{f}");
        }
        assert!(export(&p, "doc", false, 1.0).is_err());
        let back = import("My Grid.puz", &export(&p, "puz", false, 1.0).unwrap().bytes).unwrap();
        assert_eq!((back.grid, back.title.as_str()), (p.grid.clone(), "My Grid"));
        let back = import("three.json", &export(&p, "json", false, 1.0).unwrap().bytes).unwrap();
        assert_eq!((back.grid, back.title.as_str()), (p.grid, "three"));
    }
}
