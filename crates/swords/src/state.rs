//! The state file: the puzzles you pushed, their keys, the site, your name, and sign-ins.
//!
//! It is `~/.config/swords/puzzles.json` (or `$SWORDS_HOME/puzzles.json`). The shape is the one the
//! first (Node) `swords` command wrote, so an old file keeps working. Fields this version does not
//! know are kept as they are.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// One puzzle this computer knows about.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Puzzle {
    #[serde(default)]
    pub id: String,
    /// The site it is on, with no slash at the end.
    #[serde(default)]
    pub site: String,
    /// The secret key that owns it. Empty for a puzzle that came from a sign-in.
    #[serde(default)]
    pub key: String,
    /// The full path of the file it was pushed from. Empty for a claimed puzzle.
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub title: String,
    /// When it was pushed (an ISO time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed: Option<Value>,
    /// When it was last replaced, as the site reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A sign-in on one site.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub token: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Everything in the state file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The site set with `swords site URL`. Empty when none is set.
    #[serde(default)]
    pub site: String,
    /// The name the old swords command kept here. puz keeps the name in its profile now and moves this one there once.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default)]
    pub puzzles: Vec<Puzzle>,
    /// Sign-ins, by site.
    #[serde(default)]
    pub sessions: BTreeMap<String, Session>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl State {
    /// Read the state file. No file gives an empty state.
    pub fn load(file: &Path) -> Result<State> {
        if !file.exists() {
            return Ok(State::default());
        }
        let text = fs::read_to_string(file).with_context(|| format!("Can't read {}", file.display()))?;
        serde_json::from_str(&text).with_context(|| format!("{} is not a file this command can read. Fix it or move it away", file.display()))
    }

    /// Write the state file, readable by you only: it holds keys.
    pub fn save(&self, file: &Path) -> Result<()> {
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).with_context(|| format!("Can't make the folder {}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self)? + "\n";
        write_private(file, text.as_bytes()).with_context(|| format!("Can't write {}", file.display()))
    }

    /// The sign-in for a site, or for the site an address is on.
    pub fn session(&self, site: &str) -> Option<&Session> {
        self.sessions.get(site).or_else(|| self.sessions.get(crate::util::origin(site)))
    }
}

#[cfg(unix)]
fn write_private(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(file)?;
    f.write_all(bytes)?;
    // An older file may have been made with wider permissions.
    fs::set_permissions(file, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn write_private(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::write(file, bytes)
}

/// The state file's place: `$SWORDS_HOME/puzzles.json`, or `~/.config/swords/puzzles.json`.
pub fn state_file(swords_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let dir = match swords_home.filter(|s| !s.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(home.filter(|s| !s.is_empty())?).join(".config").join("swords"),
    };
    Some(dir.join("puzzles.json"))
}

/// Pick the site: `--site`, then `$SWORDS_SITE`, then the saved one, then the built-in one.
/// An empty value counts as not set. Slashes at the end are dropped.
pub fn choose_site(flag: Option<&str>, env: Option<&str>, saved: &str, default: &str) -> String {
    [flag, env, Some(saved)]
        .into_iter()
        .flatten()
        .find(|s| !s.is_empty())
        .unwrap_or(default)
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file exactly as the Node command wrote it.
    const NODE_FILE: &str = r#"{
  "site": "",
  "puzzles": [
    {
      "id": "abcd2345",
      "site": "http://localhost:4747",
      "key": "k_secret",
      "file": "/tmp/a.json",
      "title": "Sample",
      "pushed": "2026-11-18T10:00:00.000Z",
      "updated": "2026-11-18T11:00:00.000Z"
    },
    {
      "id": "wxyz6789",
      "site": "http://localhost:4747",
      "key": "",
      "file": "",
      "title": "Claimed",
      "pushed": "2026-11-17T10:00:00.000Z"
    }
  ],
  "sessions": {
    "http://localhost:4747": {
      "email": "a@example.com",
      "token": "t_secret"
    }
  }
}
"#;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("swords-state-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_node_file_round_trips_byte_for_byte() {
        let state: State = serde_json::from_str(NODE_FILE).unwrap();
        assert_eq!(state.puzzles[0].key, "k_secret");
        assert_eq!(state.puzzles[1].updated, None);
        assert_eq!(state.sessions["http://localhost:4747"].email, "a@example.com");
        assert_eq!(serde_json::to_string_pretty(&state).unwrap() + "\n", NODE_FILE);
    }

    #[test]
    fn save_then_load_gives_the_same_state() {
        let file = scratch("roundtrip").join("deep").join("puzzles.json");
        let state: State = serde_json::from_str(NODE_FILE).unwrap();
        state.save(&file).unwrap();
        assert_eq!(State::load(&file).unwrap(), state);
        assert_eq!(fs::read_to_string(&file).unwrap(), NODE_FILE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = fs::remove_dir_all(scratch("roundtrip"));
    }

    #[test]
    fn no_file_is_an_empty_state() {
        let state = State::load(&scratch("none").join("puzzles.json")).unwrap();
        assert_eq!(state, State::default());
        assert_eq!(serde_json::to_string(&state).unwrap(), r#"{"site":"","puzzles":[],"sessions":{}}"#);
    }

    #[test]
    fn the_name_is_kept_and_left_out_when_empty() {
        let text = r#"{"site":"","name":"Ada Lovelace","puzzles":[],"sessions":{}}"#;
        let state: State = serde_json::from_str(text).unwrap();
        assert_eq!(state.name, "Ada Lovelace");
        assert_eq!(serde_json::to_string(&state).unwrap(), text);
    }

    #[test]
    fn a_partial_file_gets_the_missing_parts() {
        let state: State = serde_json::from_str(r#"{"site":"https://a.example"}"#).unwrap();
        assert_eq!(state.site, "https://a.example");
        assert!(state.puzzles.is_empty() && state.sessions.is_empty());
    }

    #[test]
    fn unknown_fields_are_kept() {
        let text = r#"{"site":"","puzzles":[{"id":"abcd2345","site":"s","key":"k","file":"","title":"T","pushed":"p","color":"red"}],"sessions":{},"theme":"dark"}"#;
        let state: State = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_string(&state).unwrap(), text);
    }

    #[test]
    fn a_broken_file_is_an_error_that_names_it() {
        let file = scratch("broken").join("puzzles.json");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "{not json").unwrap();
        let err = State::load(&file).unwrap_err().to_string();
        assert!(err.contains("puzzles.json"), "{err}");
        let _ = fs::remove_dir_all(scratch("broken"));
    }

    #[test]
    fn site_choice_order() {
        let d = "https://swords.cx";
        assert_eq!(choose_site(Some("http://flag/"), Some("http://env"), "http://saved", d), "http://flag");
        assert_eq!(choose_site(None, Some("http://env//"), "http://saved", d), "http://env");
        assert_eq!(choose_site(None, None, "http://saved", d), "http://saved");
        assert_eq!(choose_site(None, None, "", d), d);
        // Empty counts as not set, as it did in the Node command.
        assert_eq!(choose_site(Some(""), Some(""), "", d), d);
        assert_eq!(choose_site(Some(""), Some("http://env"), "", d), "http://env");
    }

    #[test]
    fn state_file_place() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(state_file(os("/x/y"), os("/home/me")), Some(PathBuf::from("/x/y/puzzles.json")));
        assert_eq!(state_file(None, os("/home/me")), Some(PathBuf::from("/home/me/.config/swords/puzzles.json")));
        assert_eq!(state_file(os(""), os("/home/me")), Some(PathBuf::from("/home/me/.config/swords/puzzles.json")));
        assert_eq!(state_file(None, None), None);
    }

    #[test]
    fn session_is_found_by_site_or_by_an_address_on_it() {
        let state: State = serde_json::from_str(NODE_FILE).unwrap();
        assert!(state.session("http://localhost:4747").is_some());
        assert!(state.session("http://localhost:4747/api/puzzles/abcd2345").is_some());
        assert!(state.session("http://other").is_none());
    }
}
