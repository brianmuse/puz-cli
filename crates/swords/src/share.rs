//! Push a puzzle to a Swords site, and set its PIN or delete it, as a library: no printing.
//!
//! Each function takes what it needs and gives back a result, or an [`Error`] that holds the HTTP
//! status and the site's own words. The keys stay in the swords state file ([`State`]), exactly as
//! the `swords` command keeps them: a push saves the key and the puzzle's record there itself, so
//! `swords list` shows a puzzle pushed through this API too. The caller loads the state and names
//! the file to save it to.
//!
//! A puzzle is found by its site and id here, never by a file path. `puz publish` finds a file's
//! earlier publish (the link saved in the puzzle, else the file path), then calls [`push`] with that puzzle's id.

use crate::api::{self, Body, Method, Proof};
use crate::json::Json;
use crate::state::{Puzzle, State};
use crate::util;
use std::path::Path;

/// What to do with the PIN after a push.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pin<'a> {
    /// Leave the PIN as it is.
    #[default]
    Keep,
    /// Set this PIN (4 to 12 characters; it is trimmed).
    Set(&'a str),
    /// Take the PIN off.
    Remove,
}

/// One push.
#[derive(Clone, Copy, Debug)]
pub struct Push<'a> {
    /// The site, with no slash at the end.
    pub site: &'a str,
    /// The puzzle file's bytes, in any format the site reads (it tells the format from the bytes).
    pub body: &'a [u8],
    /// The full path of the file the puzzle came from. It is saved with the key, so `swords list`
    /// can show it. Empty: keep the path saved before (none for a new puzzle).
    pub file: &'a str,
    /// The id of the puzzle on this site to update. None: make a new puzzle with a new link.
    pub update: Option<&'a str>,
    /// Make a new puzzle even when `update` names one.
    pub new: bool,
    pub pin: Pin<'a>,
    /// The name to use as the author when the file names none.
    pub author: Option<&'a str>,
}

impl<'a> Push<'a> {
    /// A push of `body` to `site` that makes a new puzzle and touches nothing else.
    pub fn new(site: &'a str, body: &'a [u8]) -> Push<'a> {
        Push { site, body, file: "", update: None, new: false, pin: Pin::Keep, author: None }
    }
}

/// What a push did to the puzzle on the site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A new puzzle, with a new link.
    Created,
    /// The puzzle at the link has the new file.
    Updated,
    /// The site had this file already: it changed nothing.
    Unchanged,
}

/// A push that reached the site.
#[derive(Clone, Debug, PartialEq)]
pub struct Pushed {
    pub id: String,
    /// The site, with no slash at the end.
    pub site: String,
    /// The address to solve it at: the link to share.
    pub url: String,
    /// The owner's page (stats, PIN, delete), with no key in it.
    pub manage_url: String,
    /// The title the site shows. Empty when the site sent none.
    pub title: String,
    pub outcome: Outcome,
    /// The puzzle's version on the site, when it said.
    pub version: Option<u64>,
    /// Solvers need a PIN, as the site says after the push (and after the PIN call, when there was one).
    /// None when the site did not say.
    pub locked: Option<bool>,
    /// A PIN was set or taken off.
    pub pin_changed: bool,
    /// The site's reply to the push. For a new puzzle it holds the key (`key`, and `manage` as the claim link).
    pub reply: Json,
    /// The site's reply to the PIN call, when there was one.
    pub pin_reply: Option<Json>,
}

/// A PIN that was set or taken off.
#[derive(Clone, Debug, PartialEq)]
pub struct PinSet {
    /// Solvers need a PIN now, as the site says. None when the site did not say.
    pub locked: Option<bool>,
    /// The site's reply: the puzzle as it shows it now.
    pub reply: Json,
}

/// Why a call did not work.
#[derive(Debug)]
pub enum Error {
    /// The site did not answer, or said no: the HTTP status and the site's own words are in it.
    Site(api::Error),
    /// An input the site would refuse, such as a PIN that is too short. Nothing was sent.
    Invalid(String),
    /// The state file could not be saved. What the site did stays done.
    State(anyhow::Error),
    /// The push worked and its key is saved, but the PIN call did not.
    PinNotSet { pushed: Box<Pushed>, cause: api::Error },
}

impl Error {
    /// The HTTP status the site answered with, when it answered and said no.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Site(e) | Error::PinNotSet { cause: e, .. } => e.status,
            _ => None,
        }
    }
    /// The site's own words, when it sent any.
    pub fn site_message(&self) -> Option<&str> {
        match self {
            Error::Site(e) | Error::PinNotSet { cause: e, .. } => e.message.as_deref(),
            _ => None,
        }
    }
    /// The site did not answer at all.
    pub fn unreachable(&self) -> bool {
        matches!(self, Error::Site(api::Error { status: None, .. }))
    }
    /// As an `anyhow` error, with its causes kept: an invalid input becomes a usage mistake.
    pub fn into_anyhow(self) -> anyhow::Error {
        match self {
            Error::Site(e) => e.into(),
            Error::Invalid(text) => crate::Usage(text).into(),
            Error::State(e) => e,
            e @ Error::PinNotSet { .. } => anyhow::Error::new(e),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Site(e) => e.fmt(f),
            Error::Invalid(text) => f.write_str(text),
            Error::State(e) => write!(f, "{e:#}"),
            Error::PinNotSet { pushed, cause } => write!(f, "The puzzle is published ({}), but its PIN is not set: {cause}", pushed.url),
        }
    }
}

impl std::error::Error for Error {}

impl From<api::Error> for Error {
    fn from(e: api::Error) -> Error {
        Error::Site(e)
    }
}

/// The key and the sign-in to send for a puzzle on a site.
pub fn proof<'a>(state: &'a State, site: &str, key: &'a str) -> Proof<'a> {
    Proof { key: Some(key), session: state.session(site).map(|s| s.token.as_str()), author: None }
}

/// The place of a puzzle's record in the state file: the newest one for this site and id that has a
/// key, else the newest one for them.
fn place(state: &State, site: &str, id: &str) -> Option<usize> {
    let mine = |p: &Puzzle| p.site == site && p.id == id;
    state.puzzles.iter().rposition(|p| mine(p) && !p.key.is_empty()).or_else(|| state.puzzles.iter().rposition(mine))
}

/// The record this computer keeps for a puzzle on a site, found by its id.
pub fn record<'a>(state: &'a State, site: &str, id: &str) -> Option<&'a Puzzle> {
    place(state, site, id).map(|i| &state.puzzles[i])
}

/// The key this computer holds for a puzzle on a site. None when it has none.
pub fn key<'a>(state: &'a State, site: &str, id: &str) -> Option<&'a str> {
    record(state, site, id).map(|p| p.key.as_str()).filter(|k| !k.is_empty())
}

fn save(state: &State, state_file: &Path) -> Result<(), Error> {
    state.save(state_file).map_err(Error::State)
}

/// Push a puzzle: update the puzzle `update` names, or make a new one. The key of a new puzzle, and
/// the title and the time of an update, are saved in `state` and written to `state_file` before the
/// PIN is sent, so a failed PIN call loses nothing.
pub fn push(state: &mut State, state_file: &Path, req: &Push) -> Result<Pushed, Error> {
    let pin = match req.pin {
        Pin::Keep => None,
        Pin::Set(text) => Some(Some(crate::commands::valid_pin(text).map_err(|e| Error::Invalid(e.to_string()))?)),
        Pin::Remove => Some(None),
    };
    let site = req.site;
    let author = req.author;
    let (reply, outcome, id) = match req.update.filter(|_| !req.new) {
        Some(id) => {
            let at = place(state, site, id);
            let key = at.map(|i| state.puzzles[i].key.clone()).unwrap_or_default();
            let reply = api::request(Method::Put, &format!("{site}/api/puzzles/{id}"), Proof { author, ..proof(state, site, &key) }, Body::Bytes(req.body))?;
            // The site changes nothing for a file it has already: the time of the last change stays, and so
            // does the version (1 for a puzzle that was never replaced).
            let before = at.and_then(|i| state.puzzles[i].updated.clone());
            let same = match (&before, reply.get("updated")) {
                (Some(before), Some(now)) => *before == now.to_value(),
                (None, Some(_)) => reply.get("version").and_then(Json::as_f64) == Some(1.0),
                _ => false,
            };
            if let Some(i) = at {
                let old = &mut state.puzzles[i];
                if let Some(title) = reply.get("title").and_then(Json::as_str) {
                    old.title = title.to_string();
                }
                if let Some(updated) = reply.get("updated") {
                    old.updated = Some(updated.to_value());
                }
                if !req.file.is_empty() {
                    old.file = req.file.to_string();
                }
                save(state, state_file)?;
            }
            (reply, if same { Outcome::Unchanged } else { Outcome::Updated }, id.to_string())
        }
        None => {
            let reply = api::request(Method::Post, &format!("{site}/api/puzzles"), Proof { author, ..proof(state, site, "") }, Body::Bytes(req.body))?;
            let id = reply.text("id");
            if id.is_empty() {
                return Err(Error::Invalid("The site's reply has no puzzle id.".into()));
            }
            // The key first: without it the pushed puzzle can't be changed.
            state.puzzles.push(Puzzle {
                id: id.clone(),
                site: site.to_string(),
                key: reply.text("key"),
                file: req.file.to_string(),
                title: reply.text("title"),
                pushed: Some(util::now_iso().into()),
                ..Puzzle::default()
            });
            save(state, state_file)?;
            (reply, Outcome::Created, id)
        }
    };
    let url = reply.get("url").and_then(Json::as_str).map_or_else(|| format!("{site}/p/{id}/"), str::to_string);
    let mut pushed = Pushed {
        manage_url: format!("{site}/p/{id}/manage"),
        site: site.to_string(),
        url,
        title: reply.text("title"),
        outcome,
        version: reply.get("version").and_then(Json::as_f64).map(|v| v as u64),
        locked: reply.get("locked").and_then(|l| if let Json::Bool(b) = l { Some(*b) } else { None }),
        pin_changed: false,
        reply,
        pin_reply: None,
        id,
    };
    // The PIN goes on straight after the push, before anyone has the link.
    if let Some(pin) = pin {
        match send_pin(state, site, &pushed.id, pin) {
            Ok(done) => {
                pushed.locked = done.locked;
                pushed.pin_changed = true;
                pushed.pin_reply = Some(done.reply);
            }
            Err(cause) => return Err(Error::PinNotSet { pushed: Box::new(pushed), cause }),
        }
    }
    Ok(pushed)
}

fn send_pin(state: &State, site: &str, id: &str, pin: Option<&str>) -> Result<PinSet, api::Error> {
    let key = record(state, site, id).map(|p| p.key.as_str()).unwrap_or("");
    let reply = api::request(Method::Post, &format!("{site}/api/puzzles/{id}/pin"), proof(state, site, key), Body::Json(&crate::commands::pin_body(pin)))?;
    let locked = match reply.get("locked") {
        Some(Json::Bool(b)) => Some(*b),
        _ => None,
    };
    Ok(PinSet { locked, reply })
}

/// Set a puzzle's PIN (`Some`), or take it off (`None`). It uses the key saved for that site and id,
/// and the sign-in on that site. Nothing is saved: a PIN is never kept.
pub fn set_pin(state: &State, site: &str, id: &str, pin: Option<&str>) -> Result<PinSet, Error> {
    let pin = pin.map(crate::commands::valid_pin).transpose().map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(send_pin(state, site, id, pin)?)
}

/// Delete a puzzle from its site for good: its link and its stats go. Then its records leave the state
/// file; they are given back. A puzzle the site no longer has is an error with status 404 and nothing
/// is forgotten: call [`forget`] to drop the records all the same.
pub fn delete(state: &mut State, state_file: &Path, site: &str, id: &str) -> Result<Vec<Puzzle>, Error> {
    let key = record(state, site, id).map(|p| p.key.as_str()).unwrap_or("");
    api::request(Method::Delete, &format!("{site}/api/puzzles/{id}"), proof(state, site, key), Body::None)?;
    forget(state, state_file, site, id)
}

/// Drop a puzzle's records (and so its key) from the state file, without asking the site. Gives the
/// records that were there; the file is written only when there were some.
pub fn forget(state: &mut State, state_file: &Path, site: &str, id: &str) -> Result<Vec<Puzzle>, Error> {
    let (gone, kept): (Vec<Puzzle>, Vec<Puzzle>) = std::mem::take(&mut state.puzzles).into_iter().partition(|p| p.site == site && p.id == id);
    state.puzzles = kept;
    if !gone.is_empty() {
        save(state, state_file)?;
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    /// One call the fake site saw: the method, the path, the key sent, the author sent.
    type Calls = Arc<Mutex<Vec<(String, String, String, String)>>>;

    /// A tiny Swords site on this computer: push, update, PIN, delete. One request per connection.
    fn fake_site() -> (String, Calls) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let site = format!("http://{}", listener.local_addr().unwrap());
        let calls: Calls = Arc::default();
        let (log, origin) = (calls.clone(), site.clone());
        std::thread::spawn(move || {
            // id -> (key, locked, version, updated)
            let mut live: HashMap<String, (String, bool, u64, String)> = HashMap::new();
            let mut n = 0;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut words = line.split_whitespace();
                let (method, path) = (words.next().unwrap_or("").to_string(), words.next().unwrap_or("").to_string());
                let mut headers = HashMap::new();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = h.split_once(':') {
                        headers.insert(name.trim().to_lowercase(), value.trim().to_string());
                    }
                }
                let length: usize = headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let auth = headers.get("authorization").cloned().unwrap_or_default();
                log.lock().unwrap().push((method.clone(), path.clone(), auth.clone(), headers.get("x-swords-author").cloned().unwrap_or_default()));
                let parts: Vec<&str> = path.trim_start_matches("/api/puzzles").split('/').filter(|s| !s.is_empty()).collect();
                let view = |id: &str, locked: bool, version: u64, updated: &str| serde_json::json!({ "id": id, "title": "Tiny", "url": format!("{origin}/p/{id}/"), "locked": locked, "version": version, "updated": updated });
                let (status, reply) = match (method.as_str(), parts.as_slice()) {
                    ("POST", []) if body.is_empty() => (400, serde_json::json!({ "error": "No puzzle in the request." })),
                    ("POST", []) => {
                        n += 1;
                        let id = format!("abcd000{n}");
                        live.insert(id.clone(), (format!("Bearer key-{id}"), false, 1, "t1".into()));
                        (201, serde_json::json!({ "id": id, "title": "Tiny", "url": format!("{origin}/p/{id}/"), "key": format!("key-{id}"), "manage": format!("{origin}/p/{id}/manage#key-{id}"), "locked": false, "version": 1 }))
                    }
                    (_, [id, rest @ ..]) => match live.get(*id).cloned() {
                        None => (404, serde_json::json!({ "error": "No puzzle here." })),
                        Some((key, ..)) if key != auth => (403, serde_json::json!({})),
                        Some((key, locked, version, updated)) => match (method.as_str(), rest) {
                            ("PUT", []) if body == b"same" => (200, view(id, locked, version, &updated)),
                            ("PUT", []) => {
                                let now = format!("t{}", version + 1);
                                live.insert(id.to_string(), (key, locked, version + 1, now.clone()));
                                (200, view(id, locked, version + 1, &now))
                            }
                            ("POST", ["pin"]) if body == br#"{"pin":"0000"}"# => (400, serde_json::json!({ "error": "That PIN is not allowed." })),
                            ("POST", ["pin"]) => {
                                let on = !serde_json::from_slice::<serde_json::Value>(&body).unwrap()["pin"].is_null();
                                live.insert(id.to_string(), (key, on, version, updated.clone()));
                                (200, view(id, on, version, &updated))
                            }
                            ("DELETE", []) => {
                                live.remove(*id);
                                (200, serde_json::json!({ "deleted": id }))
                            }
                            _ => (404, serde_json::json!({ "error": "no route" })),
                        },
                    },
                    _ => (404, serde_json::json!({ "error": "no route" })),
                };
                let text = reply.to_string();
                let _ = write!(stream, "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len());
            }
        });
        (site, calls)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("swords-share-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("puzzles.json")
    }

    #[test]
    fn push_update_pin_and_delete_by_id() {
        let (site, calls) = fake_site();
        let file = scratch("flow");
        let mut state = State::default();

        // A new push: the key and the record are saved, in memory and in the file.
        let req = Push { file: "/work/tiny.json", author: Some("Zoë"), ..Push::new(&site, b"one") };
        let first = push(&mut state, &file, &req).unwrap();
        assert_eq!((first.id.as_str(), first.outcome, first.version, first.locked), ("abcd0001", Outcome::Created, Some(1), Some(false)));
        assert_eq!(first.url, format!("{site}/p/abcd0001/"));
        assert_eq!(first.manage_url, format!("{site}/p/abcd0001/manage"));
        assert_eq!(first.reply.text("key"), "key-abcd0001");
        assert_eq!(key(&state, &site, "abcd0001"), Some("key-abcd0001"));
        assert_eq!(State::load(&file).unwrap(), state);
        assert_eq!(record(&state, &site, "abcd0001").unwrap().file, "/work/tiny.json");
        assert_eq!(calls.lock().unwrap()[0].3, "Zo%C3%AB");

        // The same file again: the site changes nothing. A new file: updated. Both by the id given.
        let again = push(&mut state, &file, &Push { update: Some("abcd0001"), ..Push::new(&site, b"same") }).unwrap();
        assert_eq!((again.outcome, again.version), (Outcome::Unchanged, Some(1)));
        let moved = Push { update: Some("abcd0001"), file: "/work/renamed.json", pin: Pin::Set(" 4821 "), ..Push::new(&site, b"two") };
        let second = push(&mut state, &file, &moved).unwrap();
        assert_eq!((second.outcome, second.version, second.locked, second.pin_changed), (Outcome::Updated, Some(2), Some(true), true));
        assert_eq!(second.pin_reply.as_ref().unwrap().get("locked"), Some(&Json::Bool(true)));
        // The record follows the file, and keeps the time the site gave. The PIN is never saved.
        let saved = std::fs::read_to_string(&file).unwrap();
        assert!(saved.contains("/work/renamed.json") && saved.contains("\"t2\"") && !saved.contains("4821"));
        assert_eq!(state.puzzles.len(), 1);
        // The update and the PIN call sent the key; the new push sent none.
        let sent: Vec<(String, String)> = calls.lock().unwrap().iter().map(|c| (format!("{} {}", c.0, c.1), c.2.clone())).collect();
        assert_eq!(sent[0], ("POST /api/puzzles".into(), String::new()));
        assert!(sent[1..].iter().all(|(_, k)| k == "Bearer key-abcd0001"), "{sent:?}");

        // `new` makes a new puzzle even with an id to update.
        let fresh = push(&mut state, &file, &Push { update: Some("abcd0001"), new: true, ..Push::new(&site, b"one") }).unwrap();
        assert_eq!((fresh.id.as_str(), fresh.outcome), ("abcd0002", Outcome::Created));

        // The PIN alone, then off.
        assert_eq!(set_pin(&state, &site, "abcd0001", None).unwrap().locked, Some(false));
        assert!(matches!(set_pin(&state, &site, "abcd0001", Some("12")), Err(Error::Invalid(_))));

        // Delete: gone from the site and from the state file. Again: a 404 with the site's words.
        let gone = delete(&mut state, &file, &site, "abcd0001").unwrap();
        assert_eq!(gone.len(), 1);
        assert_eq!(key(&state, &site, "abcd0001"), None);
        assert!(!std::fs::read_to_string(&file).unwrap().contains("key-abcd0001"));
        let e = delete(&mut state, &file, &site, "abcd0001").unwrap_err();
        assert_eq!((e.status(), e.site_message()), (Some(404), Some("No puzzle here.")));
        delete(&mut state, &file, &site, "abcd0002").unwrap();
        assert!(state.puzzles.is_empty());
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    #[test]
    fn errors_hold_the_status_and_the_sites_words() {
        let (site, _) = fake_site();
        let file = scratch("errors");
        let mut state = State::default();
        // The site says no, in its own words: nothing is saved.
        let e = push(&mut state, &file, &Push::new(&site, b"")).unwrap_err();
        assert_eq!((e.status(), e.site_message(), e.to_string().as_str()), (Some(400), Some("No puzzle in the request."), "No puzzle in the request."));
        assert!(state.puzzles.is_empty() && !file.exists());
        // No key here for that id: the site says no with no words.
        push(&mut state, &file, &Push::new(&site, b"one")).unwrap();
        let other = State::default();
        let e = set_pin(&other, &site, "abcd0001", Some("4821")).unwrap_err();
        assert_eq!((e.status(), e.site_message()), (Some(403), None));
        assert_eq!(e.to_string(), format!("{site}/api/puzzles/abcd0001/pin: 403"));
        // A bad PIN is refused before anything is sent, and is a usage mistake for the command.
        let e = push(&mut state, &file, &Push { pin: Pin::Set("1"), ..Push::new(&site, b"one") }).unwrap_err();
        assert!(matches!(e, Error::Invalid(_)));
        assert!(e.into_anyhow().is::<crate::Usage>());
        assert_eq!(state.puzzles.len(), 1);
        // A site that does not answer.
        let e = push(&mut state, &file, &Push::new("http://127.0.0.1:9", b"one")).unwrap_err();
        assert!(e.unreachable() && e.status().is_none());
        assert_eq!(e.to_string(), "Can't reach http://127.0.0.1:9. Is the site running? Set another one with: puz swords site URL");
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    #[test]
    fn a_failed_pin_keeps_the_push() {
        let (site, _) = fake_site();
        let file = scratch("pin");
        let mut state = State::default();
        // The site refuses this PIN after the push worked: the push and its key are kept and given back.
        let e = push(&mut state, &file, &Push { pin: Pin::Set("0000"), ..Push::new(&site, b"one") }).unwrap_err();
        assert_eq!((e.status(), e.site_message()), (Some(400), Some("That PIN is not allowed.")));
        assert_eq!(e.to_string(), format!("The puzzle is published ({site}/p/abcd0001/), but its PIN is not set: That PIN is not allowed."));
        let Error::PinNotSet { pushed, .. } = e else { panic!("not a PIN error") };
        assert_eq!((pushed.id.as_str(), pushed.outcome, pushed.pin_changed), ("abcd0001", Outcome::Created, false));
        assert_eq!(key(&State::load(&file).unwrap(), &site, "abcd0001"), Some("key-abcd0001"));
        // Another computer with no key: the update itself is refused, so no PIN call is made.
        let other = scratch("pin-other");
        let e = push(&mut State::default(), &other, &Push { update: Some("abcd0001"), pin: Pin::Set("4821"), ..Push::new(&site, b"two") }).unwrap_err();
        assert!(matches!(e, Error::Site(_)) && e.status() == Some(403));
        assert!(!other.exists());
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }
}
