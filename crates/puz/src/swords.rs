//! `puz publish` (to Swords), `puz setup` and the `puz swords` group: publish a finished puzzle on a
//! Swords solving site and manage it there.
//!
//! The work is in `puz_core::swords` (the link records kept in the puzzle file, the publish itself)
//! and in the bundled swords library (`crates/swords`, package `swords-client`: the site calls, the state file with the
//! keys, the account commands, the setup questions and the look of the output). This file parses
//! nothing and keeps no state: it joins the two, and it guards what an AI agent may do.

use anyhow::{Context, Result, bail};
use puz_core::profile::{self, Profile};
use puz_core::swords as sw;
use std::path::{Path, PathBuf};
use swords_client::cli::Command;
use swords_client::commands::{self, Pushed};
use swords_client::json::Json;
use swords_client::setup;
use swords_client::state::State;
use swords_client::style::Pen;

pub use sw::in_agent_run;

/// Why an agent may not run a command: these change the account, show a key or open a browser.
const NO_AGENTS: &str = "An AI agent may not run this command: it is for the user to run in a terminal. \
An agent may run `puz publish PUZZLE`, `puz swords pin`, `puz swords protect|unprotect` and `puz swords unpublish` with --confirmed after the user says yes, \
and `puz swords status|list|stats|versions|embed|whoami` at any time.";

/// What the user typed for `puz publish` to Swords.
pub struct Publish {
    pub file: PathBuf,
    pub site: Option<String>,
    pub pin: Option<String>,
    pub no_pin: bool,
    pub new: bool,
    pub author: Option<String>,
    pub confirmed: bool,
    pub json: bool,
}

/// Your name, from your puz profile. A name kept by the old `swords` command (in its state file) moves
/// to the profile once, so there is one name, not two.
pub fn your_name(state: &mut State, state_file: &Path) -> String {
    let file = profile::user_file();
    let mut prof = Profile::load(&file).unwrap_or_default();
    let old = state.name.trim().to_string();
    if !old.is_empty() {
        if prof.me.name.trim().is_empty() {
            prof.me.name = old;
            if prof.save(&file).is_err() {
                return prof.me.name;
            }
        }
        state.name.clear();
        let _ = state.save(state_file);
    }
    prof.me.name.trim().to_string()
}

fn save_name(name: &str) -> Result<()> {
    let file = profile::user_file();
    let mut prof = Profile::load(&file)?;
    prof.me.name = name.to_string();
    prof.save(&file)
}

/// Load the state file and make the context the library's commands need.
fn open(site: Option<&str>, json: bool) -> Result<(State, commands::Ctx)> {
    let state_file = swords_client::state_file()?;
    let mut state = State::load(&state_file)?;
    let name = your_name(&mut state, &state_file);
    let ctx = swords_client::context(site, json, &name, &state)?;
    Ok((state, ctx))
}

/// Run a step and turn its failure into an exit code, printed in the swords style.
fn finish(r: Result<()>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => swords_client::report(&e),
    }
}

/// `puz publish PUZZLE` (to Swords).
pub fn publish(o: Publish) -> i32 {
    finish(publish_inner(o))
}

/// The puzzle a new user publishes first: `puz publish hello-world.puz` works right after install.
const HELLO_NAME: &str = "hello-world.puz";
const HELLO: &[u8] = include_bytes!("../assets/hello-world.puz");

/// When the file is `hello-world.puz` and it is not there, write the built-in one, so the first try just works.
/// Says so on stderr, unless the output is JSON.
pub fn write_hello(file: &std::path::Path, json: bool) -> Result<()> {
    if file.file_name().and_then(|n| n.to_str()) != Some(HELLO_NAME) || file.exists() {
        return Ok(());
    }
    std::fs::write(file, HELLO).with_context(|| format!("could not write {}", file.display()))?;
    if !json {
        eprintln!("Saved {} here: a small puzzle to try.", file.display());
    }
    Ok(())
}

fn publish_inner(o: Publish) -> Result<()> {
    let env = sw::Env::from_process()?;
    if env.agent && !o.confirmed {
        bail!(sw::NOT_CONFIRMED);
    }
    let (state, ctx) = open(o.site.as_deref(), o.json)?;
    let first = state.puzzles.is_empty() && ctx.name.is_empty() && state.session(&ctx.site).is_none();
    let author = match o.author.as_deref() {
        Some(a) => setup::valid_name(a)?,
        None => ctx.name.clone(),
    };
    let opts = sw::PushOptions { site: Some(ctx.site.clone()), pin: o.pin.clone(), no_pin: o.no_pin, new: o.new, confirmed: o.confirmed, author };
    let sent = sw::push(&o.file, &env, &opts)?;
    let state = State::load(&ctx.state_file)?;
    let puzzles = ctx.puzzles_url(&state);
    if o.json {
        let mut data = serde_json::to_value(&sent)?;
        data["path"] = serde_json::json!(o.file.display().to_string());
        data["puzzles_url"] = serde_json::json!(puzzles);
        // The claim link holds the key, on purpose: it is how the owner gets the puzzle on another device.
        // An agent's run never gets it: `puz swords manage` shows it to the user.
        if sent.created && !env.agent {
            data["claim"] = sent.reply.get("manage").map_or(serde_json::Value::Null, Json::to_value);
        }
        swords_client::say(&serde_json::to_string_pretty(&data)?);
        return Ok(());
    }
    let pen = Pen::stdout();
    let mut reply = sent.reply.clone();
    reply.set("title", sent.title.as_str().into());
    if sent.link.pin {
        reply.set("locked", true.into());
    }
    let how = match (sent.created, sent.unchanged && o.pin.is_none() && !o.no_pin) {
        (true, _) => Pushed::New,
        (false, true) => Pushed::Same,
        (false, false) => Pushed::Updated,
    };
    let mut text = commands::pushed_text(&pen, &reply, how, first && setup::can_ask(&ctx), puzzles.as_deref());
    if sent.recorded && (sent.created || sent.adopted) {
        text += &format!("\n{}", pen.note("The link is saved in the puzzle file. Publish it again to update the same link, even after a rename."));
    }
    swords_client::say(&text);
    Ok(())
}

/// `puz setup`: your name (the one `puz profile` keeps) and an optional email to sign in to the Swords site.
pub fn setup_command(name: Option<String>, email: Option<String>, site: Option<String>, json: bool) -> i32 {
    finish((|| {
        if in_agent_run() {
            bail!(NO_AGENTS);
        }
        let (mut state, ctx) = open(site.as_deref(), json)?;
        let mut save = |n: &str| save_name(n);
        let mut names = setup::Names { now: ctx.name.clone(), save: &mut save };
        setup::setup(&ctx, &mut state, &mut names, name.as_deref(), email.as_deref())
    })())
}

/// The commands of the `puz swords` group that puz adds to the library's own.
pub enum Extra {
    /// `puz swords status PUZZLE [--live]`.
    Status { puzzle: String, live: bool },
}

/// May an AI agent run this command? `Err` says why not.
pub fn agent_check(cmd: &Command) -> Result<()> {
    if !in_agent_run() {
        return Ok(());
    }
    match cmd {
        Command::List | Command::Stats { .. } | Command::Versions { .. } | Command::Embed { .. } | Command::Whoami => Ok(()),
        Command::Links { add: None, remove: None, .. } | Command::Links { confirmed: true, .. } => Ok(()),
        Command::Links { .. } => bail!(sw::NOT_CONFIRMED),
        Command::Pin { confirmed: true, .. } | Command::Unpublish { confirmed: true, .. } | Command::Protect { confirmed: true, .. } | Command::Unprotect { confirmed: true, .. } => Ok(()),
        Command::Pin { .. } | Command::Unpublish { .. } | Command::Protect { .. } | Command::Unprotect { .. } => bail!(sw::NOT_CONFIRMED),
        _ => bail!(NO_AGENTS),
    }
}

/// The PUZZLE a command names, as the puzzle's id when it is a file with a link: the record in a puz
/// puzzle (so a renamed file still finds its link), else the earlier publish of that file path.
fn as_id(arg: &mut String, file: Option<PathBuf>, env: &sw::Env, site: Option<&str>) -> Option<(PathBuf, sw::Link)> {
    let file = file?;
    let link = sw::link_for_file(&file, env, site)?;
    *arg = link.id.clone();
    Some((file, link))
}

/// `puz swords COMMAND`. `find` turns a PUZZLE argument into a file, when it names one.
pub fn group(site: Option<String>, json: bool, extra: Option<Extra>, cmd: Option<Command>, find: &dyn Fn(&str) -> Option<PathBuf>) -> i32 {
    finish((|| {
        if let Some(Extra::Status { puzzle, live }) = extra {
            return status(&puzzle, find(&puzzle), site.as_deref(), live, json);
        }
        let mut cmd = cmd.expect("a command");
        agent_check(&cmd)?;
        let env = sw::Env::from_process()?;
        let (mut state, ctx) = open(site.as_deref(), json)?;
        let flag = site.as_deref();
        let found = match &mut cmd {
            Command::Stats { puzzle } | Command::Versions { puzzle } | Command::Restore { puzzle, .. } | Command::Embed { puzzle } | Command::Open { puzzle } | Command::Manage { puzzle, .. } => {
                let f = find(puzzle);
                as_id(puzzle, f, &env, flag)
            }
            Command::Pin { puzzle, .. } | Command::Unpublish { puzzle, .. } | Command::Protect { puzzle, .. } | Command::Unprotect { puzzle, .. } | Command::Links { puzzle, .. } => {
                let f = find(puzzle);
                as_id(puzzle, f, &env, flag)
            }
            _ => None,
        };
        let after = match &cmd {
            Command::Pin { remove, .. } => Some(Some(!*remove)),
            Command::Unpublish { yes: true, .. } => Some(None),
            _ => None,
        };
        swords_client::run(&ctx, &mut state, cmd)?;
        // Keep the puzzle file in step with the site: its PIN mark, or the link gone.
        if let (Some((file, link)), Some(change)) = (found, after) {
            match change {
                Some(pin) => sw::mark_pin(&file, &link.site, &link.id, pin)?,
                None => sw::forget(&file, &link.site, &link.id)?,
            }
        }
        Ok(())
    })())
}

/// `puz swords status PUZZLE`: the links kept in the puzzle file, and whether they are out of date.
fn status(arg: &str, file: Option<PathBuf>, site: Option<&str>, live: bool, json: bool) -> Result<()> {
    let Some(file) = file else { bail!("No puzzle file {arg}") };
    let env = sw::Env::from_process()?;
    let state_file = swords_client::state_file()?;
    let mut state = State::load(&state_file)?;
    let name = your_name(&mut state, &state_file);
    let pen = if json { Pen::PLAIN } else { Pen::stdout() };
    let Ok(p) = puz_core::puzzle::Puzzle::load(&file) else {
        // Another format holds no record: the state file knows it by its path.
        let link = sw::link_for_file(&file, &env, site);
        if json {
            swords_client::say(&serde_json::to_string_pretty(&serde_json::json!({ "path": file.display().to_string(), "site": env.site(site)?, "links": link.map(|l| vec![serde_json::json!({ "site": l.site, "id": l.id, "url": format!("{}/p/{}/", l.site, l.id) })]).unwrap_or_default() }))?);
        } else {
            match link {
                Some(l) => swords_client::say(&format!("{} is on Swords:\n{}", file.display(), pen.link(&format!("{}/p/{}/", l.site, l.id)))),
                None => swords_client::say(&format!("{} is not on Swords. Publish it with: puz publish {}", file.display(), file.display())),
            }
        }
        return Ok(());
    };
    let s = sw::status(&p, &name, &env, site, live)?;
    if json {
        swords_client::say(&serde_json::to_string_pretty(&sw::status_json(&file.display().to_string(), &s))?);
        return Ok(());
    }
    let mut lines = vec![];
    if s.links.is_empty() {
        lines.push(format!("{} is not on Swords.", pen.title(&s.title)));
    } else {
        lines.push(format!("{} is on Swords:", pen.title(&s.title)));
        lines.extend(s.links.iter().map(sw::link_text));
    }
    if !s.problems.is_empty() {
        lines.push(pen.caution(&format!("Not ready to publish: {}.", s.problems.join("; "))));
    } else if s.links.is_empty() {
        lines.push(pen.next("Publish it", &format!("puz publish {}", file.display())));
    }
    lines.push(pen.dim(&format!("New links go to {}.", s.site)));
    swords_client::say(&lines.join("\n"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_may_read_but_not_change_without_a_yes() {
        // SAFETY: tests in this module set the variable only here.
        unsafe { std::env::set_var("PUZ_SCRATCH", "/tmp/x") };
        let s = |t: &str| t.to_string();
        assert!(agent_check(&Command::List).is_ok());
        assert!(agent_check(&Command::Stats { puzzle: s("a") }).is_ok());
        assert!(agent_check(&Command::Whoami).is_ok());
        let pin = Command::Pin { puzzle: s("a"), pin: Some(s("4821")), remove: false, confirmed: false };
        assert!(agent_check(&pin).unwrap_err().to_string().contains("only after the user agrees"));
        assert!(agent_check(&Command::Unpublish { puzzle: s("a"), yes: true, confirmed: true }).is_ok());
        assert!(agent_check(&Command::Links { puzzle: s("a"), add: None, remove: None, confirmed: false }).is_ok());
        let add = Command::Links { puzzle: s("a"), add: Some(s("Sam")), remove: None, confirmed: false };
        assert!(agent_check(&add).unwrap_err().to_string().contains("only after the user agrees"));
        assert!(agent_check(&Command::Links { puzzle: s("a"), add: None, remove: Some(s("Sam")), confirmed: true }).is_ok());
        for refused in [Command::Login { email: s("me@example.com") }, Command::Logout, Command::Claim { link: s("l") }, Command::Site { url: None }, Command::Manage { puzzle: s("a"), yes: false }, Command::Open { puzzle: s("a") }] {
            assert!(agent_check(&refused).unwrap_err().to_string().contains("for the user to run"));
        }
        unsafe { std::env::remove_var("PUZ_SCRATCH") };
        assert!(agent_check(&Command::Logout).is_ok());
    }
}
