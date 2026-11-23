//! The constructor's details: name, address, city line and email, with an optional co-constructor.
//! They go on page 1 of a paper's submission PDF (`submission`), and the name is the author of new
//! puzzles and of exports with no author. A profile is one small JSON file; the caller says which.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One person's contact block: name, street address, "city, state ZIP", email.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Contact {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub email: String,
}

impl Contact {
    pub fn is_empty(&self) -> bool {
        self.lines().is_empty()
    }

    /// The block's lines, top to bottom, empty ones left out.
    pub fn lines(&self) -> Vec<&str> {
        [&self.name, &self.address, &self.city, &self.email].iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    #[serde(default)]
    pub me: Contact,
    /// A co-constructor's block, shown to the right of yours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partner: Option<Contact>,
}

impl Profile {
    /// The profile in `file`. A missing file is an empty profile.
    pub fn load(file: &Path) -> Result<Profile> {
        if !file.exists() {
            return Ok(Profile::default());
        }
        let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", file.display()))
    }

    pub fn save(&self, file: &Path) -> Result<()> {
        let mut p = self.clone();
        if p.partner.as_ref().is_some_and(Contact::is_empty) {
            p.partner = None;
        }
        if let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&p)? + "\n")?;
        std::fs::rename(&tmp, file)?;
        Ok(())
    }

    /// What `me` still lacks for a submission: "name", "address", "city line", "email".
    pub fn missing(&self) -> Vec<&'static str> {
        missing(&self.me)
    }

    /// The contact blocks for a puzzle's page 1. Your name falls back to the first name in the
    /// puzzle's author field; with no partner block, a second author gets a block with just a name.
    pub fn blocks(&self, authors: &[String]) -> Vec<Contact> {
        let mut me = self.me.clone();
        if me.name.trim().is_empty() {
            me.name = authors.first().cloned().unwrap_or_default();
        }
        let partner = match &self.partner {
            Some(c) if !c.is_empty() => Some(c.clone()),
            _ => authors.get(1).map(|n| Contact { name: n.clone(), ..Default::default() }),
        };
        let mut out = vec![me];
        out.extend(partner);
        out
    }
}

/// Fill in an empty author with your name, for exports and new puzzles. A puzzle's own author
/// wins. Only the name: address and email go only on a submission PDF.
pub fn default_author(p: &mut crate::puzzle::Puzzle, profile: &Profile) {
    if p.author.trim().is_empty() {
        p.author = profile.me.name.trim().to_string();
    }
}

/// The parts of a contact block that are empty: "name", "address", "city line", "email".
pub fn missing(c: &Contact) -> Vec<&'static str> {
    [("name", &c.name), ("address", &c.address), ("city line", &c.city), ("email", &c.email)].iter().filter(|(_, v)| v.trim().is_empty()).map(|(what, _)| *what).collect()
}

/// The people named in an author field: split on " and ", " & ", ",", " / " and ";".
pub fn authors(author: &str) -> Vec<String> {
    let mut parts = vec![author.to_string()];
    for sep in [" and ", " AND ", " And ", " & ", ",", " / ", ";"] {
        parts = parts.iter().flat_map(|p| p.split(sep).map(str::to_string).collect::<Vec<_>>()).collect();
    }
    // An editor credit ("Ed. Name", "Edited by Name") is not a maker.
    let editor = |p: &str| {
        let l = p.to_ascii_lowercase();
        l.starts_with("ed.") || l.starts_with("edited by")
    };
    parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty() && !editor(p)).collect()
}

/// The file that holds your own profile: `$PUZ_PROFILE` if set; else `profile.json` in
/// `$PUZ_CONFIG_DIR`, `$XDG_CONFIG_HOME/puz` or `~/.config/puz` (`%APPDATA%\puz` on Windows).
pub fn user_file() -> PathBuf {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(f) = var("PUZ_PROFILE") {
        return f;
    }
    let root = || if cfg!(windows) { var("APPDATA").unwrap_or_default() } else { var("HOME").unwrap_or_default().join(".config") };
    let dir = var("PUZ_CONFIG_DIR").or_else(|| var("XDG_CONFIG_HOME").map(|d| d.join("puz"))).unwrap_or_else(|| root().join("puz"));
    dir.join("profile.json")
}

/// A change to the profile: each Some field is set ("" clears it).
#[derive(Debug, Default, Deserialize)]
pub struct Update {
    pub name: Option<String>,
    pub address: Option<String>,
    pub city: Option<String>,
    pub email: Option<String>,
}

impl Update {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.address.is_none() && self.city.is_none() && self.email.is_none()
    }

    pub fn apply(self, c: &mut Contact) {
        let set = |f: &mut String, v: Option<String>| {
            if let Some(v) = v {
                *f = v.trim().to_string();
            }
        };
        set(&mut c.name, self.name);
        set(&mut c.address, self.address);
        set(&mut c.city, self.city);
        set(&mut c.email, self.email);
    }
}

/// The profile as plain text for the CLI.
pub fn render(p: &Profile) -> String {
    let block = |c: &Contact| {
        let show = |s: &str| if s.trim().is_empty() { "(empty)".to_string() } else { s.trim().to_string() };
        format!("  Name     {}\n  Address  {}\n  City     {}\n  Email    {}", show(&c.name), show(&c.address), show(&c.city), show(&c.email))
    };
    let mut o = format!("You\n{}", block(&p.me));
    match &p.partner {
        Some(c) => o.push_str(&format!("\nCo-constructor\n{}", block(c))),
        None => o.push_str("\nCo-constructor: none"),
    }
    o.push_str("\nThese go at the top of page 1 of a submission PDF (puz export --format nyt).");
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_falls_back() {
        let dir = std::env::temp_dir().join(format!("puz-core-profile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("deep/profile.json");
        assert_eq!(Profile::load(&file).unwrap(), Profile::default());
        assert_eq!(Profile::default().missing(), vec!["name", "address", "city line", "email"]);
        let mut p = Profile::default();
        Update { email: Some(" a@b.c ".into()), ..Default::default() }.apply(&mut p.me);
        p.partner = Some(Contact::default());
        p.save(&file).unwrap();
        let q = Profile::load(&file).unwrap();
        assert_eq!(q.missing(), vec!["name", "address", "city line"]);
        assert_eq!(authors("Pat Lee & Sam Roe, ed. Kim Poe"), vec!["Pat Lee", "Sam Roe"]);
        assert_eq!(q.me.email, "a@b.c");
        assert!(q.partner.is_none());
        let blocks = q.blocks(&["Pat Lee".into(), "Sam Roe".into()]);
        assert_eq!(blocks[0].lines(), vec!["Pat Lee", "a@b.c"]);
        assert_eq!(blocks[1].lines(), vec!["Sam Roe"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
