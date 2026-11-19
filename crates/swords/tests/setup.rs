//! The setup questions and the sign-in by code, run in this process against a small stand-in for the site.
//! A list of answers stands in for the person at the terminal. Nothing here touches the real ~/.config/swords.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use swords_client::commands::Ctx;
use std::cell::RefCell;
use swords_client::setup::{self, Ask, Names};
use swords_client::state::{Puzzle, State};

/// The person: answers to give, in order, and everything they were shown.
struct Person {
    answers: VecDeque<&'static str>,
    shown: Vec<String>,
}

impl Person {
    fn new(answers: &[&'static str]) -> Person {
        Person { answers: answers.iter().copied().collect(), shown: Vec::new() }
    }
    fn saw(&self, text: &str) -> bool {
        self.shown.iter().any(|line| line.contains(text))
    }
}

impl Ask for Person {
    /// No answer left is the end of the input, as Ctrl-D is.
    fn ask(&mut self, label: &str, _hint: &str, old: &str) -> Option<String> {
        self.shown.push(if old.is_empty() { label.to_string() } else { format!("{label} [{old}]") });
        self.answers.pop_front().map(|a| a.trim().to_string())
    }
    fn tell(&mut self, text: &str) {
        self.shown.push(text.to_string());
    }
}

/// What the stand-in site does. The right code is 123456.
#[derive(Clone, Copy, Default)]
struct Site {
    /// A site from before codes: its reply to a sign-in names no `digits`.
    old: bool,
    /// The link in the email was opened: the poll hands the session over.
    link_opened: bool,
    /// Only one email goes out: the second request is refused.
    one_email: bool,
}

type Calls = Arc<Mutex<Vec<String>>>;

/// Start the stand-in site. Gives its address and the list of calls it got: `METHOD PATH BODY`.
fn site(does: Site) -> (String, Calls) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header.trim().is_empty() {
                    break;
                }
                if let Some(n) = header.to_lowercase().strip_prefix("content-length:") {
                    length = n.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();
            let call = format!("{} {}", first.split(" HTTP").next().unwrap(), body);
            let emails = seen.lock().unwrap().iter().filter(|c| c.starts_with("POST /api/login {")).count();
            let (status, reply) = match call.as_str() {
                c if c.starts_with("POST /api/login {") && does.one_email && emails > 0 => (429, r#"{"error":"An email went to that address a moment ago. Check your email, or try again in a minute."}"#.to_string()),
                c if c.starts_with("POST /api/login {") && does.old => (200, r#"{"sent":true,"attach":1,"code":"POLL"}"#.to_string()),
                c if c.starts_with("POST /api/login {") => (200, format!(r#"{{"sent":true,"attach":1,"code":"POLL{}","digits":6}}"#, emails + 1)),
                c if c.starts_with("POST /api/login/code ") && c.contains(r#""digits":"123456""#) => (200, r#"{"email":"me@example.com","session":"SESSIONTOKEN"}"#.to_string()),
                c if c.starts_with("POST /api/login/code ") => (401, r#"{"error":"That code is not right. You have 4 tries left."}"#.to_string()),
                c if c.starts_with("GET /api/login/poll") && does.link_opened => (200, r#"{"email":"me@example.com","session":"LINKSESSION"}"#.to_string()),
                c if c.starts_with("GET /api/login/poll") => (202, r#"{"waiting":true}"#.to_string()),
                _ => (404, r#"{"error":"No such call."}"#.to_string()),
            };
            seen.lock().unwrap().push(call);
            let _ = write!(stream, "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}", reply.len());
        }
    });
    (address, calls)
}

/// A scratch folder for one test's state file.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let dir = std::env::temp_dir().join(format!("swords-setup-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Home(dir)
    }
    fn ctx(&self, site: &str) -> Ctx {
        Ctx { site: site.into(), site_flag: false, json: false, state_file: self.0.join("puzzles.json"), cwd: self.0.clone(), name: String::new() }
    }
    fn saved(&self) -> State {
        State::load(&self.0.join("puzzles.json")).unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The setup questions, with the name kept in `name` (puz keeps it in its profile).
fn questions(ctx: &Ctx, state: &mut State, name: &RefCell<String>, person: &mut Person) {
    let now = name.borrow().clone();
    let mut save = |n: &str| {
        *name.borrow_mut() = n.to_string();
        Ok(())
    };
    setup::questions(ctx, state, &mut Names { now, save: &mut save }, person).unwrap();
}

/// A state with one puzzle pushed to the site and one pushed somewhere else.
fn state_with_keys(site: &str) -> State {
    let puzzle = |id: &str, site: &str| Puzzle { id: id.into(), site: site.into(), key: format!("key-{id}"), ..Puzzle::default() };
    State { puzzles: vec![puzzle("aaaa1111", site), puzzle("bbbb2222", "https://other.example")], ..State::default() }
}

#[test]
fn the_questions_save_the_name_and_sign_in_with_the_code() {
    let name = RefCell::new(String::new());
    let (address, calls) = site(Site { one_email: true, ..Site::default() });
    let home = Home::new("questions");
    let ctx = home.ctx(&address);
    let mut state = state_with_keys(&address);
    // A name; an email; a code that is too short; a wrong code; Enter alone (a new code: refused, one email a minute); the right code.
    let mut person = Person::new(&["  Ada   Lovelace ", "me@example.com", "12345", "000000", "", "123 456"]);
    questions(&ctx, &mut state, &name, &mut person);

    assert_eq!(person.shown[0], "Set up puz\nTwo questions. Press Enter to skip one.");
    assert_eq!(person.shown[1], "Name");
    assert_eq!(person.shown[2], "Email (optional)");
    assert_eq!(person.shown[3], "A 6-digit code is on its way to me@example.com.");
    assert_eq!(person.shown[4], "Code");
    assert_eq!(person.shown[5], "The code is 6 digits.");
    assert_eq!(person.shown[7], "That code is not right. You have 4 tries left.");
    assert_eq!(person.shown[9], "An email went to that address a moment ago. Check your email, or try again in a minute.");
    assert_eq!(person.shown[11], format!("Signed in as me@example.com on {address}. 1 puzzle(s) from this computer are attached to it."));
    assert_eq!(person.shown[12], "\nTry it:\n  puz publish hello-world.puz\n  puz --help");
    assert_eq!(person.shown.len(), 13);

    let saved = home.saved();
    assert_eq!(saved, state);
    assert_eq!(*name.borrow(), "Ada Lovelace");
    assert_eq!((saved.sessions[&address].email.as_str(), saved.sessions[&address].token.as_str()), ("me@example.com", "SESSIONTOKEN"));
    assert_eq!(saved.puzzles.len(), 2);

    let calls = calls.lock().unwrap();
    // The sign-in carries this site's keys only, so those puzzles are attached.
    let first: serde_json::Value = serde_json::from_str(calls[0].strip_prefix("POST /api/login ").unwrap()).unwrap();
    assert_eq!(first, serde_json::json!({ "email": "me@example.com", "wait": true, "keys": { "aaaa1111": "key-aaaa1111" } }));
    assert_eq!(calls[1], r#"POST /api/login/code {"code":"POLL1","digits":"000000","email":"me@example.com"}"#);
    assert!(calls[2].starts_with("GET /api/login/poll?code=POLL1"), "{}", calls[2]);
    assert!(calls[3].starts_with("POST /api/login {"));
    // The refused request changed nothing: the first code still names the sign-in.
    assert_eq!(calls[4], r#"POST /api/login/code {"code":"POLL1","digits":"123456","email":"me@example.com"}"#);
    assert_eq!(calls.len(), 5);
    // Nothing shown holds the session.
    assert!(!person.saw("SESSIONTOKEN") && !person.saw("key-"));
}

#[test]
fn enter_skips_every_question_and_saves_nothing() {
    let name = RefCell::new(String::new());
    let (address, calls) = site(Site::default());
    let home = Home::new("skip");
    let mut state = State::default();
    let mut person = Person::new(&["", ""]);
    questions(&home.ctx(&address), &mut state, &name, &mut person);
    assert_eq!(state, State::default());
    assert!(!home.0.join("puzzles.json").exists());
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(person.shown[3..], ["Nothing is saved. Run puz setup at any time.", "\nTry it:\n  puz publish hello-world.puz\n  puz --help"]);
}

#[test]
fn leaving_at_the_code_keeps_the_name_and_does_not_sign_in() {
    let name = RefCell::new(String::new());
    let (address, _calls) = site(Site::default());
    let home = Home::new("leave");
    let mut state = State::default();
    // The answers run out at the code, as when the person presses Ctrl-D. (Ctrl-C ends the program there: the name is saved before.)
    let mut person = Person::new(&["Ada", "me@example.com"]);
    questions(&home.ctx(&address), &mut state, &name, &mut person);
    assert!(person.saw("You are not signed in. Sign in later with: puz swords login EMAIL"));
    let saved = home.saved();
    assert_eq!(*name.borrow(), "Ada");
    assert!(saved.sessions.is_empty());
}

#[test]
fn a_second_run_shows_what_is_set_and_enter_keeps_it() {
    let name = RefCell::new(String::new());
    let (address, calls) = site(Site::default());
    let home = Home::new("again");
    let ctx = home.ctx(&address);
    let mut state = State::default();
    questions(&ctx, &mut state, &name, &mut Person::new(&["Ada", "me@example.com", "123456"]));
    let before = home.saved();
    let asked = calls.lock().unwrap().len();

    let mut person = Person::new(&["", ""]);
    questions(&ctx, &mut state, &name, &mut person);
    assert_eq!(person.shown[1], "Name [Ada]");
    assert_eq!(person.shown[2], "Email (optional) [me@example.com]");
    assert_eq!(person.shown.len(), 4, "{:?}", person.shown);
    assert_eq!(home.saved(), before);
    // The same email typed again is no new sign-in. A name that is not one is asked for again.
    let mut person = Person::new(&["\u{7}", "Zoë", "ME@example.com"]);
    questions(&ctx, &mut state, &name, &mut person);
    assert_eq!(person.shown[2], "A name is 1 to 100 characters.");
    assert_eq!(*name.borrow(), "Zoë");
    assert_eq!(calls.lock().unwrap().len(), asked);
}

#[test]
fn a_site_that_is_down_does_not_end_the_setup() {
    let name = RefCell::new(String::new());
    let home = Home::new("down");
    let mut state = State::default();
    let mut person = Person::new(&["Ada", "not an email", "me@example.com"]);
    questions(&home.ctx("http://127.0.0.1:9"), &mut state, &name, &mut person);
    assert!(person.saw("That is not an email address."));
    assert!(person.saw("Can't reach http://127.0.0.1:9.") && person.saw("You are not signed in."));
    assert!(person.shown.last().unwrap().contains("Try it:"));
    assert_eq!(*name.borrow(), "Ada");
}

#[test]
fn flags_ask_for_the_code_only() {
    let (address, _calls) = site(Site::default());
    let home = Home::new("flags");
    let mut state = State::default();
    let mut person = Person::new(&["123456"]);
    let mut name = String::new();
    let mut save = |n: &str| {
        name = n.to_string();
        Ok(())
    };
    setup::with_flags(&home.ctx(&address), &mut state, &mut Names { now: String::new(), save: &mut save }, Some("Ada"), Some("me@example.com"), Some(&mut person)).unwrap();
    assert_eq!(person.shown, ["A 6-digit code is on its way to me@example.com.", "Code"]);
    assert_eq!((name.as_str(), home.saved().sessions[&address].token.as_str()), ("Ada", "SESSIONTOKEN"));
}

#[test]
fn the_link_in_the_email_signs_in_too() {
    // The person opened the link, then pressed Enter: no new email is asked for.
    let (address, calls) = site(Site { link_opened: true, ..Site::default() });
    let home = Home::new("link");
    let mut state = State::default();
    let done = setup::sign_in(&home.ctx(&address), &mut state, "me@example.com", Some(&mut Person::new(&[""]))).unwrap().unwrap();
    assert_eq!(serde_json::to_string(&done).unwrap(), format!(r#"{{"email":"me@example.com","site":"{address}","attached":1}}"#));
    assert_eq!(home.saved().sessions[&address].token, "LINKSESSION");
    assert_eq!(calls.lock().unwrap().iter().filter(|c| c.starts_with("POST /api/login {")).count(), 1);
}

#[test]
fn with_no_terminal_or_an_old_site_the_command_waits_for_the_link() {
    for (does, person) in [(Site { link_opened: true, ..Site::default() }, false), (Site { old: true, link_opened: true, ..Site::default() }, true)] {
        let (address, calls) = site(does);
        let home = Home::new(if person { "old-site" } else { "no-terminal" });
        let mut state = State::default();
        let mut nobody = Person::new(&[]);
        let done = setup::sign_in(&home.ctx(&address), &mut state, "me@example.com", if person { Some(&mut nobody) } else { None }).unwrap().unwrap();
        assert_eq!(done.text("email"), "me@example.com");
        assert_eq!(home.saved().sessions[&address].token, "LINKSESSION");
        assert!(nobody.shown.is_empty(), "no question is asked: {:?}", nobody.shown);
        assert!(calls.lock().unwrap().iter().all(|c| !c.starts_with("POST /api/login/code")));
    }
}
