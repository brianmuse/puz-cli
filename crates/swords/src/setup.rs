//! The first-run setup (`puz setup`), and sign-in with the code from the email.
//!
//! The name is not kept here: the program that runs the setup keeps it (puz keeps it in its profile)
//! and hands it over as [`Names`]. The sign-in is kept in the state file, by site.

use crate::api::{self, Body, Method, Proof};
use crate::commands::{Ctx, login_body};
use crate::json::Json;
use crate::state::{Session, State};
use crate::style::Pen;
use crate::util;
use crate::{Usage, say};
use anyhow::{Result, bail};
use std::io::{IsTerminal as _, Write as _};
use std::time::Duration;

/// The commands that end the setup.
pub const TRY_IT: [&str; 2] = ["puz publish hello-world.puz", "puz --help"];

/// The person's name, kept by the program that runs the setup: the name now, and how to save a new one.
pub struct Names<'a> {
    /// The name now. Empty when none is set.
    pub now: String,
    /// Save a new name (already checked with [`valid_name`]).
    pub save: &'a mut dyn FnMut(&str) -> Result<()>,
}

impl Names<'_> {
    fn set(&mut self, name: String) -> Result<()> {
        (self.save)(&name)?;
        self.now = name;
        Ok(())
    }
}

pub const NAME_HINT: &str = "Shown as the author when a puzzle file names none.";
pub const EMAIL_HINT: &str = "Lets you keep ownership of your puzzles, from any device.";
pub const CODE_HINT: &str = "The 6 digits from the email. Enter alone sends a new one.";

/// The person at the terminal: questions go out, typed lines come back. Tests give the answers themselves.
pub trait Ask {
    /// Ask one thing and read one line, trimmed. `label` names it ("Email (optional)"), `hint` says what it is
    /// for (empty when the question comes again), `old` is the answer kept by Enter (empty when there is none).
    /// None when there is no more input (Ctrl-D).
    fn ask(&mut self, label: &str, hint: &str, old: &str) -> Option<String>;
    /// Show a line.
    fn tell(&mut self, text: &str);
    /// Show the heading that opens the setup, and the line under it.
    fn title(&mut self, text: &str, under: &str) {
        self.tell(&format!("{text}\n{under}"));
    }
    /// Show that a step worked.
    fn done(&mut self, text: &str) {
        self.tell(text);
    }
    /// Show the commands to try next.
    fn next(&mut self, commands: &[&str]) {
        self.tell(&format!("\nTry it:\n  {}", commands.join("\n  ")));
    }
}

/// The real terminal. The marks and the colors come from [`Pen`].
pub struct Terminal;

impl Ask for Terminal {
    fn ask(&mut self, label: &str, hint: &str, old: &str) -> Option<String> {
        let pen = Pen::stdout();
        let mut out = std::io::stdout();
        if !hint.is_empty() {
            let kept = if old.is_empty() { String::new() } else { pen.dim(&format!("  Enter keeps {old}")) };
            let _ = writeln!(out, "\n{}{kept}\n{}", pen.bold(label), pen.dim(hint));
        }
        let _ = write!(out, "{} ", pen.prompt());
        let _ = out.flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(n) if n > 0 => Some(line.trim().to_string()),
            _ => {
                let _ = writeln!(out);
                None
            }
        }
    }
    fn tell(&mut self, text: &str) {
        say(text);
    }
    fn title(&mut self, text: &str, under: &str) {
        let pen = Pen::stdout();
        say(&format!("{} {}\n{}", pen.bold("■"), pen.bold(text), pen.dim(under)));
    }
    fn done(&mut self, text: &str) {
        say(&Pen::stdout().ok(text));
    }
    fn next(&mut self, commands: &[&str]) {
        let pen = Pen::stdout();
        say(&format!("\n{}", pen.bold("Try it")));
        for c in commands {
            say(&pen.cmd(c));
        }
    }
}

/// May this run ask questions? Only with a person at a terminal: never with `--json`, in CI, or through a pipe.
pub fn can_ask(ctx: &Ctx) -> bool {
    !ctx.json && std::env::var_os("CI").is_none_or(|v| v.is_empty()) && std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// A name as it is kept: one line, single spaces, 100 characters at most.
pub fn valid_name(text: &str) -> Result<String> {
    let name = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(Usage("A name is 1 to 100 characters.".into()).into());
    }
    Ok(name)
}

/// Does this look like an email address? The site has the last word.
pub fn looks_like_email(text: &str) -> bool {
    let Some((name, host)) = text.split_once('@') else { return false };
    !name.is_empty() && !text.contains(char::is_whitespace) && !host.contains('@') && host.split_once('.').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
}

/// Text for JSON: null when it is empty.
fn some(text: &str) -> Json {
    if text.is_empty() { Json::Null } else { text.into() }
}

/// `puz setup`: ask for the name and the email, or take them from the flags.
pub fn setup(ctx: &Ctx, state: &mut State, names: &mut Names, name: Option<&str>, email: Option<&str>) -> Result<()> {
    let mut terminal = Terminal;
    let person: Option<&mut dyn Ask> = if can_ask(ctx) { Some(&mut terminal) } else { None };
    match (name.is_some() || email.is_some(), person) {
        (true, person) => with_flags(ctx, state, names, name, email, person),
        (false, Some(person)) => questions(ctx, state, names, person),
        (false, None) => {
            let data = Json::object([
                ("asked", false.into()),
                ("name", some(&names.now)),
                ("email", state.session(&ctx.site).map(|s| s.email.as_str()).into()),
                ("site", ctx.site.as_str().into()),
                ("run", Json::Array(vec!["puz setup --name NAME".into(), "puz swords login EMAIL".into()])),
            ]);
            let text = "Setup asks questions, so it needs a terminal. Here, run these:\n  \
                        puz setup --name NAME     # the author of a puzzle that names none\n  \
                        puz swords login EMAIL    # optional: get your puzzles back on any device";
            ctx.out(&data, text);
            Ok(())
        }
    }
}

/// The setup with `--name` or `--email`: no questions, but for the code when a person is there.
pub fn with_flags(ctx: &Ctx, state: &mut State, names: &mut Names, name: Option<&str>, email: Option<&str>, person: Option<&mut dyn Ask>) -> Result<()> {
    let pen = ctx.pen();
    let mut lines = Vec::new();
    if let Some(name) = name {
        // Saved first: a sign-in that fails keeps the name.
        names.set(valid_name(name)?)?;
        lines.push(pen.ok(&format!("Your name is {}.", names.now)));
    }
    let mut attached = Json::Null;
    if let Some(email) = email {
        let email = email.trim();
        if !looks_like_email(email) {
            return Err(Usage("That is not an email address.".into()).into());
        }
        if state.session(&ctx.site).is_some_and(|s| s.email.eq_ignore_ascii_case(email)) {
            lines.push(pen.ok(&format!("You are signed in as {} on {} already.", email.to_lowercase(), ctx.site)));
        } else {
            let Some(done) = sign_in(ctx, state, email, person)? else { bail!("{NOT_SIGNED_IN}") };
            attached = done.field("attached");
            lines.push(pen.ok(&signed_in_text(&done)));
        }
    }
    let data = Json::object([("name", some(&names.now)), ("email", state.session(&ctx.site).map(|s| s.email.as_str()).into()), ("site", ctx.site.as_str().into()), ("attached", attached)]);
    ctx.out(&data, &lines.join("\n"));
    Ok(())
}

const NOT_SIGNED_IN: &str = "You are not signed in. Sign in later with: puz swords login EMAIL";

/// The setup as questions. Enter keeps what is there. Each answer is saved at once, so Ctrl-C loses nothing.
pub fn questions(ctx: &Ctx, state: &mut State, names: &mut Names, person: &mut dyn Ask) -> Result<()> {
    person.title("Set up puz", "Two questions. Press Enter to skip one.");
    let mut more = true;
    loop {
        let now = names.now.clone();
        let Some(typed) = person.ask("Name", NAME_HINT, &now) else {
            more = false;
            break;
        };
        if typed.is_empty() {
            break;
        }
        match valid_name(&typed) {
            Ok(name) => {
                names.set(name)?;
                break;
            }
            Err(e) => person.tell(&e.to_string()),
        }
    }
    if more {
        loop {
            let now = state.session(&ctx.site).map(|s| s.email.clone()).unwrap_or_default();
            let Some(typed) = person.ask("Email (optional)", EMAIL_HINT, &now) else { break };
            if typed.is_empty() || typed.eq_ignore_ascii_case(&now) {
                break;
            }
            if !looks_like_email(&typed) {
                person.tell("That is not an email address.");
                continue;
            }
            // A sign-in that fails does not end the setup: the name is saved, and the person can sign in later.
            match sign_in(ctx, state, &typed, Some(&mut *person)) {
                Ok(Some(done)) => person.done(&signed_in_text(&done)),
                Ok(None) => person.tell(NOT_SIGNED_IN),
                Err(e) => person.tell(&format!("{e:#}\n{NOT_SIGNED_IN}")),
            }
            break;
        }
    }
    if names.now.is_empty() && state.session(&ctx.site).is_none() {
        person.tell("Nothing is saved. Run puz setup at any time.");
    }
    person.next(&TRY_IT);
    Ok(())
}

/// The line that says a sign-in worked. `done` is what [`sign_in`] gives.
pub fn signed_in_text(done: &Json) -> String {
    let attached = done.get("attached").and_then(Json::as_f64).unwrap_or(0.0);
    let extra = if attached > 0.0 { format!(" {attached} puzzle(s) from this computer are attached to it.") } else { String::new() };
    format!("Signed in as {} on {}.{extra}", done.text("email"), done.text("site"))
}

/// Ask the site to start a sign-in: it emails the code and the link. The keys on this computer go with
/// the request, so those puzzles are attached to the email when the sign-in is done.
fn start(ctx: &Ctx, state: &State, email: &str) -> Result<Json> {
    api::call(Method::Post, &format!("{}/api/login", ctx.site), Proof::default(), Body::Json(&login_body(state, &ctx.site, email)))
}

/// Sign in by email. With a person at the terminal, they type the 6-digit code from the email; with
/// none (or on a site from before codes), this waits for the link in the email to be opened.
/// Gives `{email, site, attached}`, or nothing when the person left the question (Ctrl-D).
pub fn sign_in(ctx: &Ctx, state: &mut State, email: &str, person: Option<&mut dyn Ask>) -> Result<Option<Json>> {
    let mut started = start(ctx, state, email)?;
    let done = match person {
        Some(person) if started.get("digits").is_some() => match by_code(ctx, state, email, &mut started, person) {
            Some(done) => done,
            None => return Ok(None),
        },
        _ => by_link(ctx, email, &started)?,
    };
    let who = done.text("email");
    state.sessions.insert(ctx.site.clone(), Session { email: who.clone(), token: done.text("session"), ..Session::default() });
    ctx.save(state)?;
    Ok(Some(Json::object([("email", who.as_str().into()), ("site", ctx.site.as_str().into()), ("attached", started.field("attach"))])))
}

/// Has the link in the email been opened? The site hands the session over once.
fn link_opened(site: &str, started: &Json) -> Option<Json> {
    let poll = format!("{site}/api/login/poll?code={}", util::encode_component(&started.text("code")));
    api::send(Method::Get, &poll, Proof::default(), Body::None).ok().filter(|r| r.status == 200).map(|r| r.json())
}

/// Wait for the link in the email to be opened. Gives the site's reply: the email and the session.
fn by_link(ctx: &Ctx, email: &str, started: &Json) -> Result<Json> {
    let site = &ctx.site;
    // For the person, apart from the result: this goes to stderr.
    let pen = Pen::stderr();
    match started.get("dev_link").and_then(Json::as_str) {
        Some(link) => crate::warn(&format!("This local site sends no email. Open this link to sign in:\n{}", pen.link(link))),
        None if pen.rich => crate::warn(&format!("A sign-in link is on its way to {email}.\n{}", pen.note("Open it. This command waits, 15 minutes at most…"))),
        None => crate::warn(&format!("A sign-in link is on its way to {email}. Open it; this command waits (15 minutes at most).")),
    }
    let poll = format!("{site}/api/login/poll?code={}", util::encode_component(&started.text("code")));
    loop {
        std::thread::sleep(Duration::from_secs(2));
        // No answer: the site may be back in a moment. 202: the link is not opened yet.
        let Ok(reply) = api::send(Method::Get, &poll, Proof::default(), Body::None) else { continue };
        if reply.status == 202 {
            continue;
        }
        if !reply.ok() {
            bail!("The sign-in ran out of time. Start again: puz swords login EMAIL");
        }
        return Ok(reply.json());
    }
}

/// Say where the code is.
fn code_sent(person: &mut dyn Ask, email: &str, started: &Json) {
    match started.get("dev_code").and_then(Json::as_str) {
        Some(code) => person.tell(&format!("This local site sends no email. Code: {code}")),
        None => person.done(&format!("A 6-digit code is on its way to {email}.")),
    }
}

/// Ask for the code from the email until one works. Gives the site's reply (the email and the session),
/// or nothing when the person left the question.
fn by_code(ctx: &Ctx, state: &State, email: &str, started: &mut Json, person: &mut dyn Ask) -> Option<Json> {
    let site = &ctx.site;
    code_sent(person, email, started);
    let mut hint = CODE_HINT;
    loop {
        let typed = person.ask("Code", hint, "")?;
        hint = "";
        let digits: String = typed.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
        if digits.is_empty() || digits.eq_ignore_ascii_case("r") {
            // The link in the email signs in too. If it was opened, there is nothing to send.
            if let Some(done) = link_opened(site, started) {
                return Some(done);
            }
            match start(ctx, state, email) {
                Ok(new) => {
                    *started = new;
                    code_sent(person, email, started);
                }
                // Such as: one email a minute. The code already sent still works.
                Err(e) => person.tell(&format!("{e:#}")),
            }
            continue;
        }
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            person.tell("The code is 6 digits.");
            continue;
        }
        let body = serde_json::json!({ "email": email, "code": started.text("code"), "digits": digits }).to_string();
        let Ok(reply) = api::send(Method::Post, &format!("{site}/api/login/code"), Proof::default(), Body::Json(&body)) else {
            person.tell(&format!("Can't reach {site}. Try the code again."));
            continue;
        };
        if reply.ok() {
            return Some(reply.json());
        }
        if reply.status == 410
            && let Some(done) = link_opened(site, started)
        {
            return Some(done);
        }
        match reply.json().get("error").and_then(Json::as_str).filter(|e| !e.is_empty()) {
            Some(error) => person.tell(error),
            None => person.tell(&format!("The site did not take the code ({}).", reply.status)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_one_line_of_1_to_100_characters() {
        assert_eq!(valid_name("  Ada   Lovelace\n").unwrap(), "Ada Lovelace");
        assert_eq!(valid_name("Zoë").unwrap(), "Zoë");
        assert_eq!(valid_name(&"x".repeat(100)).unwrap().len(), 100);
        for bad in ["", "  \n", &"x".repeat(101), "a\u{7}b"] {
            let err = valid_name(bad).unwrap_err();
            assert!(err.is::<Usage>(), "{bad:?}");
            assert_eq!(err.to_string(), "A name is 1 to 100 characters.");
        }
    }

    #[test]
    fn emails_are_checked_lightly() {
        for good in ["me@example.com", "a.b+c@mail.example.org"] {
            assert!(looks_like_email(good), "{good}");
        }
        for bad in ["", "me", "me@", "@example.com", "me@example", "me @example.com", "me@@example.com", "me@.com", "me@example."] {
            assert!(!looks_like_email(bad), "{bad}");
        }
    }

    #[test]
    fn the_signed_in_line() {
        let done = Json::parse(br#"{"email":"me@example.com","site":"http://s","attached":2}"#);
        assert_eq!(signed_in_text(&done), "Signed in as me@example.com on http://s. 2 puzzle(s) from this computer are attached to it.");
        let done = Json::parse(br#"{"email":"me@example.com","site":"http://s","attached":0}"#);
        assert_eq!(signed_in_text(&done), "Signed in as me@example.com on http://s.");
    }
}
