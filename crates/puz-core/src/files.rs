//! Puzzle files on disk: a file name for a new puzzle, and finding a puzzle by name in a folder.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// File-name-safe slug: "Beer: A Tale!" -> "beer-a-tale".
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "untitled".into() } else { out }
}

/// A free file in `dir` for a puzzle with this title: `beer.json`, then `beer-2.json`, and so on.
/// `taken` may refuse a name (without ".json") for the caller's own reasons.
pub fn free_file(dir: &Path, title: &str, taken: &dyn Fn(&str) -> bool) -> PathBuf {
    let base = slug(title);
    let mut n = 1;
    loop {
        let stem = if n == 1 { base.clone() } else { format!("{base}-{n}") };
        let p = dir.join(format!("{stem}.json"));
        if !p.exists() && !taken(&stem) {
            return p;
        }
        n += 1;
    }
}

/// True for an Across Lite `.puz` file (by its extension). puz reads these but never writes them:
/// they can't hold pictures, notes or animations.
pub fn is_across_lite(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("puz"))
}

/// Where edits to an Across Lite `.puz` go: `NAME.ipuz` beside it, or `NAME-2.ipuz`, ... when that
/// file exists. An existing file is never overwritten.
pub fn ipuz_for(path: &Path) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "puzzle".into());
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{stem}.ipuz") } else { format!("{stem}-{n}.ipuz") };
        let p = path.with_file_name(name);
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

/// The note shown when edits to the `.puz` file `from` were saved to `to`.
pub fn converted_note(from: &Path, to: &Path) -> String {
    let name = |p: &Path| p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    format!(
        "note: {} is a .puz file, which can't hold pictures, notes or animations, so edits are saved to {}. Edit {} from now on.",
        name(from),
        to.display(),
        name(to)
    )
}

/// The file for a PUZZLE argument: a file path, or a name inside `folder` ("drafts/beer" or
/// "drafts/beer.json"), when a folder of puzzles is given.
pub fn find(arg: &str, folder: Option<&Path>) -> Result<PathBuf> {
    let p = Path::new(arg);
    if p.is_file() {
        return Ok(p.to_path_buf());
    }
    if let Some(dir) = folder {
        let name = arg.trim().trim_matches('/');
        for f in [dir.join(name), dir.join(format!("{name}.json")), dir.join(format!("{name}.ipuz")), dir.join(format!("{name}.puz"))] {
            if !name.is_empty() && f.is_file() {
                return Ok(f);
            }
        }
        if p.is_dir() || (!name.is_empty() && dir.join(name).is_dir()) {
            bail!("{arg:?} is a folder, not a puzzle");
        }
        bail!("no puzzle file {arg:?}, and no puzzle {arg:?} in {}", dir.display());
    }
    if p.is_dir() {
        bail!("{arg:?} is a folder, not a puzzle");
    }
    bail!("no puzzle file {arg:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_finds_files() {
        assert_eq!(slug("Beer: A Tale!"), "beer-a-tale");
        assert_eq!(slug("  "), "untitled");
        let dir = std::env::temp_dir().join(format!("puz-core-files-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("drafts")).unwrap();
        let a = free_file(&dir, "Beer", &|_| false);
        assert_eq!(a, dir.join("beer.json"));
        std::fs::write(&a, "{}").unwrap();
        assert_eq!(free_file(&dir, "Beer", &|_| false), dir.join("beer-2.json"));
        assert_eq!(free_file(&dir, "Beer", &|s| s == "beer-2"), dir.join("beer-3.json"));
        assert_eq!(find("beer", Some(&dir)).unwrap(), a);
        assert_eq!(find("beer.json", Some(&dir)).unwrap(), a);
        assert_eq!(find(a.to_str().unwrap(), None).unwrap(), a);
        assert!(find("drafts", Some(&dir)).unwrap_err().to_string().contains("folder"));
        assert!(find("nope", Some(&dir)).is_err());
        assert!(find("beer", None).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
