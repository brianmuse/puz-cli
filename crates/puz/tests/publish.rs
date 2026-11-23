//! `puz publish`, `puz swords`, `puz setup` and `puz export`, run as a person runs them.
//! Each test has its own scratch folder for the keys ($SWORDS_HOME), the profile and the puzzles.
//! The Swords site is a tiny fake one in this test process, or a port nothing listens on.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// A port on this computer that nothing listens on.
const DEAD: &str = "http://127.0.0.1:9";

/// A scratch folder for one test.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let dir = std::env::temp_dir().join(format!("puz-publish-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("keys")).unwrap();
        fs::create_dir_all(dir.join("config")).unwrap();
        Home(dir)
    }

    fn cmd(&self, words: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_puz"));
        c.args(words)
            .env("SWORDS_HOME", self.0.join("keys"))
            .env("HOME", &self.0)
            .env("PUZ_CONFIG_DIR", self.0.join("config"))
            .env("PUZ_UPDATE_SITE", DEAD)
            .env("BROWSER", "true")
            .env_remove("SWORDS_SITE")
            .env_remove("PUZ_PROFILE")
            .env_remove("PUZ_SCRATCH")
            .env_remove("PUZ_LIBRARY")
            .current_dir(&self.0)
            .stdin(Stdio::null());
        c
    }

    fn run(&self, words: &[&str]) -> Output {
        self.cmd(words).output().unwrap()
    }

    /// Run with the site given in $SWORDS_SITE.
    fn at(&self, site: &str, words: &[&str]) -> Output {
        self.cmd(words).env("SWORDS_SITE", site).output().unwrap()
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn state(&self) -> String {
        fs::read_to_string(self.0.join("keys/puzzles.json")).unwrap_or_default()
    }

    /// A finished 3x3 puzzle: a full grid and every clue.
    fn puzzle(&self, name: &str) -> String {
        let file = self.file(name);
        let s = file.to_str().unwrap().to_string();
        assert_eq!(self.run(&["new", "Tiny", "--size", "3x3", "-o", &s]).status.code(), Some(0));
        for (slot, word) in [("1A", "CAT"), ("4A", "ARE"), ("5A", "TEN")] {
            assert_eq!(self.run(&["place", &s, slot, word]).status.code(), Some(0));
        }
        for slot in ["1A", "4A", "5A", "1D", "2D", "3D"] {
            assert_eq!(self.run(&["clue", &s, slot, "A clue"]).status.code(), Some(0));
        }
        s
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}
fn json_of(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {}{}", out(o), err(o)))
}
fn links(file: &str) -> Vec<Value> {
    // Saved files are ipuz: the links sit in the Swords extension.
    let v: Value = serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
    let v = if v.get(puz_core::ipuz::EXT).is_some() { v[puz_core::ipuz::EXT].clone() } else { v };
    v["swords"].as_array().cloned().unwrap_or_default()
}

/// One call to the fake site: the method and path, and the headers that matter here.
#[derive(Clone, Debug)]
struct Call {
    what: String,
    client: String,
    body: String,
}

/// A tiny Swords site: publish, update, PIN, read, take down. It records each call.
fn fake_site() -> (String, Arc<Mutex<Vec<Call>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let site = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(Mutex::new(vec![]));
    let (log, origin) = (calls.clone(), site.clone());
    std::thread::spawn(move || {
        let mut live: HashMap<String, (String, bool)> = HashMap::new();
        let mut n = 0;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut words = line.split_whitespace();
            let (method, url) = (words.next().unwrap_or("").to_string(), words.next().unwrap_or("").split('?').next().unwrap_or("").to_string());
            let mut headers = HashMap::new();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                let h = h.trim_end();
                if h.is_empty() {
                    break;
                }
                if let Some((k, v)) = h.split_once(':') {
                    headers.insert(k.trim().to_lowercase(), v.trim().to_string());
                }
            }
            let length: usize = headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let auth = headers.get("authorization").cloned().unwrap_or_default();
            let body = String::from_utf8_lossy(&body).into_owned();
            log.lock().unwrap().push(Call { what: format!("{method} {url}"), client: headers.get("x-swords-cli").cloned().unwrap_or_default(), body: body.clone() });
            let parts: Vec<&str> = url.trim_start_matches("/api/puzzles").split('/').filter(|s| !s.is_empty()).collect();
            let (status, v) = match (method.as_str(), parts.as_slice()) {
                _ if !url.starts_with("/api/puzzles") => (404, json!({ "error": "no route" })),
                ("POST", []) => {
                    n += 1;
                    let id = format!("abcd000{n}");
                    live.insert(id.clone(), (format!("Bearer key-{id}"), false));
                    (201, json!({ "id": id, "title": "Tiny", "url": format!("{origin}/p/{id}/"), "key": format!("key-{id}"), "manage": format!("{origin}/p/{id}/manage#key-{id}"), "email": null, "locked": false }))
                }
                (_, [id, rest @ ..]) => match live.get(*id).cloned() {
                    None => (404, json!({ "error": "No puzzle here." })),
                    Some((key, _)) if key != auth => (403, json!({ "error": "This puzzle is not yours." })),
                    Some((key, locked)) => {
                        let view = |locked: bool| json!({ "id": id, "title": "Tiny", "url": format!("{origin}/p/{id}/"), "locked": locked, "updated": "2026-01-01T00:00:00.000Z", "stats": { "opens": 3, "starts": 2, "solves": 1 } });
                        match (method.as_str(), rest) {
                            ("PUT", []) | ("GET", []) => (200, view(locked)),
                            ("POST", ["pin"]) => {
                                let on = !serde_json::from_str::<Value>(&body).unwrap()["pin"].is_null();
                                live.insert(id.to_string(), (key, on));
                                (200, view(on))
                            }
                            ("DELETE", []) => {
                                live.remove(*id);
                                (200, json!({ "deleted": id }))
                            }
                            _ => (404, json!({ "error": "no route" })),
                        }
                    }
                },
                _ => (404, json!({ "error": "no route" })),
            };
            let text = v.to_string();
            let _ = write!(stream, "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len());
        }
    });
    (site, calls)
}

fn whats(calls: &Arc<Mutex<Vec<Call>>>) -> Vec<String> {
    calls.lock().unwrap().iter().map(|c| c.what.clone()).collect()
}

#[test]
fn publish_goes_to_swords_by_default_and_follows_a_rename() {
    let home = Home::new("publish");
    let (site, calls) = fake_site();
    let file = home.puzzle("tiny.json");

    // `puz publish FILE` is Swords: a new link, saved in the puzzle file.
    let o = home.at(&site, &["publish", &file]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(out(&o).starts_with("Published \"Tiny\". Anyone with this link can solve it:\n"), "{}", out(&o));
    assert!(out(&o).contains(&format!("{site}/p/abcd0001/")) && out(&o).contains("The link is saved in the puzzle file"));
    assert_eq!(whats(&calls), ["POST /api/puzzles"]);
    let l = links(&file);
    assert_eq!((l.len(), l[0]["id"].as_str(), l[0]["site"].as_str()), (1, Some("abcd0001"), Some(site.as_str())));
    // The site learns it was puz, and which version.
    assert_eq!(calls.lock().unwrap()[0].client, env!("CARGO_PKG_VERSION"));
    // The key stays in the state file, never in the puzzle.
    assert!(!fs::read_to_string(&file).unwrap().contains("key-abcd0001"));
    assert!(home.state().contains("key-abcd0001"));

    // Again, unchanged: the same link.
    let o = home.at(&site, &["publish", "swords", &file]);
    assert!(out(&o).starts_with("No change to \"Tiny\". The link is the same:"), "{}", out(&o));

    // Renamed and changed: still the same link, now an update.
    let moved = home.file("renamed.json");
    fs::rename(&file, &moved).unwrap();
    let moved = moved.to_str().unwrap();
    assert_eq!(home.run(&["clue", moved, "1A", "A new clue"]).status.code(), Some(0));
    let o = home.at(&site, &["publish", moved, "--json"]);
    let v = json_of(&o);
    assert_eq!((v["id"].as_str(), v["created"].as_bool(), v["unchanged"].as_bool()), (Some("abcd0001"), Some(false), Some(false)));
    assert_eq!(links(moved).len(), 1);

    // A PIN: the record says so, never the PIN itself.
    let o = home.at(&site, &["publish", moved, "--pin", "4821"]);
    assert!(out(&o).contains("Solvers need the PIN"), "{}", out(&o));
    assert_eq!(links(moved)[0]["pin"], json!(true));
    assert!(!fs::read_to_string(moved).unwrap().contains("4821"));
    assert_eq!(whats(&calls).iter().filter(|w| *w == "POST /api/puzzles").count(), 1, "only the first publish made a link");

    // Status reads the file; the rename did not lose the link.
    let v = json_of(&home.at(&site, &["swords", "status", moved, "--json"]));
    assert_eq!(v["links"][0]["id"], json!("abcd0001"));
    assert_eq!(v["links"][0]["out_of_date"], json!(false));
    // Stats by file name: the file's link is the puzzle.
    let o = home.at(&site, &["swords", "stats", moved]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(whats(&calls).last().unwrap().starts_with("GET /api/puzzles/abcd0001"));

    // Unpublish by file: the site, the key and the record all go.
    let o = home.at(&site, &["swords", "unpublish", moved]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("--yes"));
    let o = home.at(&site, &["swords", "unpublish", moved, "--yes"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(whats(&calls).last().unwrap(), "DELETE /api/puzzles/abcd0001");
    assert!(links(moved).is_empty());
    assert!(!home.state().contains("abcd0001"));

    // --new makes a second link. A person gets its claim link.
    home.at(&site, &["publish", moved]);
    let o = home.at(&site, &["publish", moved, "--new", "--json"]);
    assert_eq!(json_of(&o)["id"], json!("abcd0003"));
    assert_eq!(json_of(&o)["claim"], json!(format!("{site}/p/abcd0003/manage#key-abcd0003")));
    assert_eq!(links(moved).len(), 1);
}

#[test]
fn a_puzzle_must_be_ready_and_export_never_sends() {
    let home = Home::new("ready");
    let (site, calls) = fake_site();
    let file = home.puzzle("tiny.json");
    let half = home.file("half.json");
    let half = half.to_str().unwrap();
    assert_eq!(home.run(&["new", "Half", "--size", "3x3", "-o", half]).status.code(), Some(0));
    let o = home.at(&site, &["publish", half]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("not ready") || err(&o).contains("empty"), "{}", err(&o));

    // Export writes files only.
    for format in ["json", "jpz", "puz"] {
        let o = home.at(&site, &["export", &file, "--format", format, "-o", home.file(&format!("x.{format}")).to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    }
    assert!(whats(&calls).is_empty(), "export or a refused publish called the site: {:?}", whats(&calls));
    assert_eq!(home.state(), "");
}

#[test]
fn other_formats_are_found_by_their_path() {
    let home = Home::new("formats");
    let (site, calls) = fake_site();
    let file = home.puzzle("tiny.json");
    let puz = home.file("tiny.puz");
    let puz = puz.to_str().unwrap();
    assert_eq!(home.run(&["export", &file, "--format", "puz", "-o", puz]).status.code(), Some(0));
    assert_eq!(home.at(&site, &["publish", puz]).status.code(), Some(0));
    let o = home.at(&site, &["publish", puz, "--json"]);
    assert_eq!(json_of(&o)["id"], json!("abcd0001"));
    let o = home.at(&site, &["swords", "status", puz]);
    assert!(out(&o).contains("abcd0001"), "{}", out(&o));
    assert_eq!(home.at(&site, &["swords", "unpublish", puz, "--yes"]).status.code(), Some(0));
    assert_eq!(whats(&calls).last().unwrap(), "DELETE /api/puzzles/abcd0001");
}

#[test]
fn agents_need_a_yes_and_may_not_touch_the_account() {
    let home = Home::new("agent");
    let (site, calls) = fake_site();
    let file = home.puzzle("tiny.json");
    let at = |words: &[&str]| home.cmd(words).env("PUZ_SCRATCH", &home.0).env("SWORDS_SITE", &site).output().unwrap();

    let o = at(&["publish", &file]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("only after the user agrees") && err(&o).contains("--confirmed"), "{}", err(&o));
    assert!(whats(&calls).is_empty());
    assert!(links(&file).is_empty());

    let o = at(&["publish", &file, "--confirmed", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    // The claim link holds the key: an agent never gets it.
    assert!(json_of(&o).get("claim").is_none() && !out(&o).contains("key-abcd0001"), "{}", out(&o));
    assert_eq!(links(&file).len(), 1);

    // Reading is fine; --live asks the site, so it is not.
    assert_eq!(at(&["swords", "status", &file]).status.code(), Some(0));
    assert_eq!(at(&["swords", "list", "--json"]).status.code(), Some(0));
    assert_eq!(at(&["swords", "stats", &file]).status.code(), Some(0));
    assert_eq!(at(&["swords", "status", &file, "--live"]).status.code(), Some(1));

    for words in [&["swords", "pin", file.as_str(), "4821"][..], &["swords", "unpublish", &file, "--yes"]] {
        let o = at(words);
        assert_eq!(o.status.code(), Some(1), "{words:?}");
        assert!(err(&o).contains("--confirmed"), "{words:?}: {}", err(&o));
    }
    for words in [
        &["swords", "login", "a@example.com"][..],
        &["swords", "logout"],
        &["swords", "claim", "http://x/p/abcd0001/manage#k"],
        &["swords", "site", "https://a.example"],
        &["swords", "manage", &file],
        &["swords", "open", &file],
        &["setup", "--name", "Agent"],
        &["update"],
    ] {
        let o = at(words);
        assert_eq!(o.status.code(), Some(1), "{words:?}: {}", out(&o));
        assert!(err(&o).contains("may not"), "{words:?}: {}", err(&o));
    }
    let before = whats(&calls).len();
    let o = at(&["swords", "unpublish", &file, "--yes", "--confirmed"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(whats(&calls)[before..], ["DELETE /api/puzzles/abcd0001"]);
    assert!(links(&file).is_empty());
    // The old name of the mark works too.
    let o = home.cmd(&["publish", &file]).env("PUZ_SCRATCH", &home.0).env("SWORDS_SITE", &site).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn setup_and_profile_keep_one_name() {
    let home = Home::new("name");
    let o = home.run(&["setup", "--name", "  Ada   Lovelace "]);
    assert_eq!((o.status.code(), out(&o)), (Some(0), "Your name is Ada Lovelace.\n".to_string()), "{}", err(&o));
    assert_eq!(json_of(&home.run(&["profile", "--json"]))["me"]["name"], json!("Ada Lovelace"));
    assert_eq!(home.run(&["profile", "--name", "Zoë B"]).status.code(), Some(0));
    assert_eq!(json_of(&home.run(&["swords", "whoami", "--json", "--site", DEAD]))["name"], json!("Zoë B"));
    // The state file holds no name.
    assert!(!home.state().contains("Zo"));

    // A published puzzle with no author takes that name.
    let (site, calls) = fake_site();
    let file = home.puzzle("tiny.json");
    let mut v: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    v["author"] = json!("");
    fs::write(&file, v.to_string()).unwrap();
    assert_eq!(home.at(&site, &["publish", &file]).status.code(), Some(0));
    assert!(calls.lock().unwrap()[0].body.contains("Zoë B"));

    for words in [&["setup", "--email", "not-an-email"][..], &["setup", "--name", ""]] {
        let o = home.run(words);
        assert_eq!(o.status.code(), Some(2), "{words:?}: {}", err(&o));
    }
}

#[test]
fn a_name_kept_by_the_old_swords_command_moves_to_the_profile() {
    let home = Home::new("move-name");
    fs::write(home.0.join("keys/puzzles.json"), r#"{"site":"","name":"Old Name","puzzles":[],"sessions":{}}"#).unwrap();
    assert_eq!(json_of(&home.run(&["swords", "whoami", "--json"]))["name"], json!("Old Name"));
    assert_eq!(json_of(&home.run(&["profile", "--json"]))["me"]["name"], json!("Old Name"));
    assert!(!home.state().contains("Old Name"));
    // A name in the profile wins over an old one.
    fs::write(home.0.join("keys/puzzles.json"), r#"{"site":"","name":"Older","puzzles":[],"sessions":{}}"#).unwrap();
    assert_eq!(json_of(&home.run(&["swords", "whoami", "--json"]))["name"], json!("Old Name"));
}

#[test]
fn setup_asks_nothing_without_a_terminal() {
    let home = Home::new("setup-pipe");
    let o = home.run(&["setup"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(out(&o).starts_with("Setup asks questions, so it needs a terminal."), "{}", out(&o));
    assert!(out(&o).contains("puz setup --name NAME") && out(&o).contains("puz swords login EMAIL"));
    assert!(!home.0.join("keys/puzzles.json").exists());
}

#[test]
fn site_is_shown_set_and_overridden() {
    let home = Home::new("site");
    assert_eq!(json_of(&home.run(&["swords", "site", "--json"])), json!({ "site": swords_client::DEFAULT_SITE }));
    let o = home.run(&["swords", "site", "https://a.example///"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(out(&o).contains("https://a.example") && !out(&o).contains("a.example/"), "{}", out(&o));
    assert_eq!(json_of(&home.run(&["swords", "site", "--json"])), json!({ "site": "https://a.example" }));
    // --site wins over $SWORDS_SITE, which wins over the saved site.
    assert_eq!(json_of(&home.at("https://env.example/", &["swords", "site", "--json"])), json!({ "site": "https://env.example" }));
    assert_eq!(json_of(&home.at("https://env.example", &["swords", "site", "--json", "--site", "https://flag.example"])), json!({ "site": "https://flag.example" }));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(home.0.join("keys/puzzles.json")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let o = home.run(&["swords", "site", "swords.quest"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(err(&o).contains("starts with http:// or https://"));
}

/// A state file from the old swords command, with one published puzzle and one sign-in.
fn old_state() -> String {
    format!(
        r#"{{"site":"{DEAD}","puzzles":[{{"id":"abcd2345","site":"{DEAD}","key":"SECRETKEY","file":"/somewhere/sample.json","title":"Sample","pushed":"2026-11-18T10:00:00.000Z"}}],"sessions":{{"{DEAD}":{{"email":"me@example.com","token":"SECRETTOKEN"}}}}}}"#
    )
}

#[test]
fn an_old_state_file_keeps_working_and_keys_stay_secret() {
    let home = Home::new("old");
    fs::write(home.0.join("keys/puzzles.json"), old_state()).unwrap();
    let before = home.state();
    assert_eq!(json_of(&home.run(&["swords", "whoami", "--json"]))["email"], json!("me@example.com"));
    let v = json_of(&home.run(&["swords", "list", "--json"]));
    assert_eq!((v[0]["id"].as_str(), v[0]["gone"].as_bool()), (Some("abcd2345"), Some(true)));
    for words in [&["swords", "list"][..], &["swords", "list", "--json"], &["swords", "whoami"], &["swords", "site"], &["swords", "unpublish", "abcd2345"], &["swords", "stats", "abcd2345"]] {
        let o = home.run(words);
        let all = out(&o) + &err(&o);
        assert!(!all.contains("SECRETKEY") && !all.contains("SECRETTOKEN"), "{words:?} printed a secret");
    }
    // The claim link holds the key, on purpose.
    assert_eq!(json_of(&home.run(&["swords", "manage", "abcd2345", "--json"])), json!({ "manage": format!("{DEAD}/p/abcd2345/manage#SECRETKEY") }));
    assert_eq!(json_of(&home.run(&["swords", "open", &format!("{DEAD}/p/abcd2345/"), "--json"])), json!({ "url": format!("{DEAD}/p/abcd2345/") }));
    // Unpublish needs --yes; a PIN is checked before the site is called; the site being down is exit 1.
    let o = home.run(&["swords", "unpublish", "abcd2345"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("puz swords unpublish abcd2345 --yes"), "{}", err(&o));
    let o = home.run(&["swords", "pin", "abcd2345", "123"]);
    assert_eq!((o.status.code(), err(&o)), (Some(2), "A PIN is 4 to 12 characters.\n".to_string()));
    let o = home.run(&["swords", "unpublish", "abcd2345", "--yes"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains(&format!("Can't reach {DEAD}")), "{}", err(&o));
    let o = home.run(&["swords", "stats", "nope"]);
    assert_eq!((o.status.code(), err(&o)), (Some(1), "No puzzle \"nope\" among the ones you published. See them with: puz swords list\n".to_string()));
    let parse = |t: &str| serde_json::from_str::<Value>(t).unwrap();
    assert_eq!(parse(&home.state()), parse(&before), "reading and failing must not change what the state file holds");
}

#[test]
fn the_nyt_pdf_needs_no_terminal_when_the_details_are_given() {
    let home = Home::new("nyt");
    let file = home.puzzle("tiny.json");
    let me = ["--name", "A B", "--address", "1 Main St", "--city", "Town, ST 00000", "--email", "a@example.com"];
    let pdf = home.file("out.pdf");
    let pdf = pdf.to_str().unwrap();
    let o = home.run(&["export", &file, "--format", "nyt", "-o", pdf]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("--name, --address, --city and --email"), "{}", err(&o));
    let o = home.run(&[&["export", &file, "--format", "nyt", "-o", pdf, "--font", "sans", "--word-count"][..], &me].concat());
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(fs::read(pdf).unwrap().starts_with(b"%PDF"));
    // A folder for --out: the file takes the name the NYT asks for, LastName_PuzzleTitle.pdf.
    let dir = home.file("subs");
    fs::create_dir_all(&dir).unwrap();
    let o = home.run(&[&["export", &file, "--format", "nyt", "-o", dir.to_str().unwrap()][..], &me].concat());
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(fs::read(dir.join("B_Tiny.pdf")).unwrap().starts_with(b"%PDF"));
    // No --out: the same name, in this folder.
    let o = home.run(&[&["export", &file, "--format", "nyt"][..], &me].concat());
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(home.file("B_Tiny.pdf").is_file());
    // The look and detail options go only with --format nyt.
    let o = home.run(&["export", &file, "--format", "pdf", "--font", "sans"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err(&o).contains("go with --format nyt"), "{}", err(&o));
    // `puz publish` is only for Swords now.
    assert_ne!(home.run(&[&["publish", "nyt", &file][..], &me].concat()).status.code(), Some(0));
    // Help shows the new way only.
    assert!(!out(&home.run(&["publish", "--help"])).contains("\n  nyt "));
    assert!(out(&home.run(&["export", "--help"])).contains("--blank-grid"));
}

#[test]
fn help_names_the_new_commands_and_no_old_ones() {
    let home = Home::new("help");
    let o = home.run(&["--help"]);
    for name in ["publish", "swords", "setup", "update", "export", "profile"] {
        assert!(out(&o).contains(&format!("\n  {name} ")), "--help does not list {name}");
    }
    assert!(out(&o).contains("PUZ_NO_UPDATE_CHECK"));
    assert!(!out(&o).contains("link-swords"));
    let o = home.run(&["swords", "--help"]);
    for name in ["status", "list", "stats", "pin", "unpublish", "login", "whoami", "site"] {
        assert!(out(&o).contains(&format!("\n  {name} ")), "swords --help does not list {name}");
    }
    for gone in ["push", "delete", "convert", "example", "completions", "name"] {
        assert!(!out(&o).contains(&format!("\n  {gone} ")), "swords --help lists {gone}");
    }
    // The old `swords delete` still works as a word for unpublish.
    assert_eq!(home.run(&["swords", "delete", "x", "--site", DEAD]).status.code(), Some(1));
}

#[test]
fn no_update_check_through_a_pipe() {
    let home = Home::new("quiet");
    let o = home.run(&["--version"]);
    assert_eq!(out(&o), format!("puz {}\n", env!("CARGO_PKG_VERSION")));
    home.run(&["profile"]);
    assert!(!home.0.join("config/update-check.json").exists());
    assert!(!err(&o).contains("is out"));
}
