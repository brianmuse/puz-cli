//! What each `puz swords` command does, and the words and JSON shapes of a publish.

use crate::api::{self, Body, Method, Proof};
use crate::cli::Command;
use crate::json::Json;
use crate::setup;
use crate::share;
use crate::state::{Puzzle, State};
use crate::style::{self, Cell, Look, Pen};
use crate::util;
use crate::{Usage, say};
use anyhow::{Context as _, Result, bail};
use std::path::PathBuf;
use std::process::Stdio;

/// What every command needs to know.
pub struct Ctx {
    /// The site in use, with no slash at the end.
    pub site: String,
    /// True when `--site` was given: `list` then shows that site only.
    pub site_flag: bool,
    /// Print JSON, not words.
    pub json: bool,
    pub state_file: PathBuf,
    /// The folder that file names are read from.
    pub cwd: PathBuf,
    /// The person's name: the author of a puzzle that names none. Empty when none is set.
    /// The program that bundles this library keeps it (puz: in its profile).
    pub name: String,
}

impl Ctx {
    /// Print a result: the data with `--json`, the words without.
    pub fn out(&self, data: &Json, text: &str) {
        if self.json { say(&data.pretty()) } else { say(text) }
    }
    /// How to write the words: plain with `--json` or through a pipe, with marks and colors at a terminal.
    pub fn pen(&self) -> Pen {
        if self.json { Pen::PLAIN } else { Pen::stdout() }
    }
    /// The page on the site that lists your puzzles. It shows the puzzles of the email you are signed in
    /// with, so there is none when nobody is signed in: the page would not know this computer's puzzles.
    pub fn puzzles_url(&self, state: &State) -> Option<String> {
        state.session(&self.site).map(|_| format!("{}/puzzles", self.site))
    }
    pub(crate) fn save(&self, state: &State) -> Result<()> {
        state.save(&self.state_file)
    }
    fn path(&self, name: &str) -> String {
        util::resolve(&self.cwd, name).to_string_lossy().into_owned()
    }
}

/// Run one command that uses the state file.
pub fn run(ctx: &Ctx, state: &mut State, command: Command) -> Result<()> {
    match command {
        Command::List => list(ctx, state),
        Command::Stats { puzzle } => stats(ctx, state, &puzzle),
        Command::Versions { puzzle } => versions(ctx, state, &puzzle),
        Command::Restore { puzzle, version } => restore(ctx, state, &puzzle, version),
        Command::Embed { puzzle } => embed(ctx, state, &puzzle),
        Command::Pin { puzzle, pin, remove, .. } => pin_command(ctx, state, &puzzle, pin.as_deref(), remove),
        Command::Protect { puzzle, .. } => protect_command(ctx, state, &puzzle, true),
        Command::Unprotect { puzzle, .. } => protect_command(ctx, state, &puzzle, false),
        Command::Open { puzzle } => open(ctx, state, &puzzle),
        Command::Manage { puzzle, yes } => manage(ctx, state, &puzzle, yes),
        Command::Unpublish { puzzle, yes, .. } => delete(ctx, state, &puzzle, yes),
        Command::Claim { link } => claim(ctx, state, &link),
        Command::Login { email } => login(ctx, state, &email),
        Command::Logout => logout(ctx, state),
        Command::Whoami => whoami(ctx, state),
        Command::Site { url } => site(ctx, state, url.as_deref()),
    }
}

/// The key and the sign-in to send for a puzzle on a site.
fn proof<'a>(state: &'a State, site: &str, key: &'a str) -> Proof<'a> {
    Proof { key: Some(key), session: state.session(site).map(|s| s.token.as_str()), author: None }
}

/// Find a remembered puzzle by file, id or link. Gives its place in the list; the newest match wins.
pub fn find(state: &State, ctx: &Ctx, name: &str) -> Result<usize> {
    let id = util::id_in_link(name).unwrap_or(name);
    let path = ctx.path(name);
    match state.puzzles.iter().rposition(|p| p.id == id || (p.file == path && p.site == ctx.site)) {
        Some(i) => Ok(i),
        None => bail!("No puzzle {} among the ones you published. See them with: puz swords list", Json::from(name).pretty()),
    }
}

/// The puzzle that a push of this file replaces: the newest one pushed from the same file to the same site.
pub fn pushed_before(state: &State, path: &str, site: &str) -> Option<usize> {
    state.puzzles.iter().rposition(|p| p.file == path && p.site == site)
}

fn manage_link(p: &Puzzle) -> String {
    format!("{}/p/{}/manage#{}", p.site, p.id, p.key)
}

/// One line of stats, like `3 opened · 2 started · 1 solved · median 4:10 · fastest 3:02`.
pub fn stat_line(stats: &Json) -> String {
    let mut line = format!("{} opened · {} started · {} solved", stats.text("opens"), stats.text("starts"), stats.text("solves"));
    let seconds = |key: &str| stats.get(key).and_then(Json::as_f64).unwrap_or(0.0);
    if seconds("median") > 0.0 {
        line += &format!(" · median {} · fastest {}", util::clock(seconds("median")), util::clock(seconds("fastest")));
    }
    line
}

/// A whole number from the site, as text: `7`. Empty when it is not there.
fn count(data: &Json, key: &str) -> u64 {
    data.get(key).and_then(Json::as_f64).unwrap_or(0.0) as u64
}

/// The day of a time from the site: `2026-11-19` from `2026-11-19T01:00:29.169Z`.
fn day(time: &str) -> &str {
    time.get(..10).unwrap_or(time)
}

/// The parts of a puzzle's stats that both forms of `puz swords stats` show. Each is empty when there is nothing to say.
struct Facts {
    solves: String,
    stopped: String,
    /// The numbers of the last days, the day with the most opens, and the opens of each day.
    days: String,
    best: String,
    opens: Vec<u64>,
    from: String,
    countries: String,
    devices: String,
    /// `Last opened DAY` and `first solved DAY`, the ones there are.
    seen: Vec<String>,
    /// The version, then who pushed it, then what the file is: each a short part.
    version: String,
    pushed: String,
    file: Vec<String>,
}

fn facts(live: &Json) -> Facts {
    let stats = live.field("stats");
    // "name 3 · name 2" from a list of objects, or from pairs.
    let join = |pairs: Vec<(String, u64)>| pairs.iter().filter(|(_, n)| *n > 0).map(|(name, n)| format!("{name} {n}")).collect::<Vec<_>>().join(" · ");
    let list = |key: &str, name: &str| stats.get(key).map_or(&[][..], Json::as_array).iter().map(|row| (row.text(name), count(row, "opens"))).collect::<Vec<_>>();

    let (clean, helped) = (count(&stats, "clean"), count(&stats, "with_help"));
    let solves = if clean + helped > 0 { format!("{clean} with no help · {helped} with a check or a reveal") } else { String::new() };
    let stopped = join(stats.get("stopped").map_or(&[][..], Json::as_array).iter().map(|b| (if b.text("from") == b.text("to") { "full grid".to_string() } else { format!("{}–{}%", b.text("from"), b.text("to")) }, count(b, "count"))).collect());
    let all = stats.get("days").map_or(&[][..], Json::as_array);
    let sum = |key: &str| all.iter().map(|d| count(d, key)).sum::<u64>();
    let (mut days, mut best, mut opens) = (String::new(), String::new(), Vec::new());
    if sum("opens") + sum("starts") + sum("solves") > 0 {
        days = format!("{} opened · {} started · {} solved", sum("opens"), sum("starts"), sum("solves"));
        opens = all.iter().map(|d| count(d, "opens")).collect();
        if let Some(day) = all.iter().rev().max_by_key(|d| count(d, "opens")).filter(|d| count(d, "opens") > 0) {
            best = format!("most on {} ({})", day.text("day"), count(day, "opens"));
        }
    }
    let mut from = list("referrers", "host");
    from.push(("direct".to_string(), count(&stats, "direct")));
    let from = if from.iter().any(|(name, n)| name != "direct" && *n > 0) { join(from) } else { String::new() };
    let devices = stats.field("devices");
    let devices = join(["computer", "phone", "tablet"].iter().map(|d| (d.to_string(), count(&devices, d))).chain([("embedded in another site".to_string(), count(&stats, "embedded"))]).collect());
    let seen = [("Last opened", "last_opened_at"), ("first solved", "first_solved_at")].iter().filter_map(|(label, key)| stats.get(key).and_then(Json::as_str).map(|t| format!("{label} {}", day(t)))).collect();
    let pushed = match (live.text("pushed_by").as_str(), live.text("cli_version").as_str()) {
        ("", _) => String::new(),
        ("cli", v) if !v.is_empty() => format!("published from the command line ({v})"),
        ("cli", _) => "published from the command line".to_string(),
        ("web", _) => "published from a browser".to_string(),
        ("restore", _) => "an old version, restored".to_string(),
        (other, _) => format!("published by {other}"),
    };
    let mut file = Vec::new();
    if !live.text("format").is_empty() {
        file.push(format!("a .{} file", live.text("format")));
    }
    if !live.text("answers").is_empty() {
        file.push(format!("{} answers", live.text("answers")));
    }
    let features: Vec<&str> = live.get("features").map_or(&[][..], Json::as_array).iter().filter_map(Json::as_str).collect();
    if !features.is_empty() {
        file.push(features.join(", "));
    }
    Facts { solves, stopped, days, best, opens, from, countries: join(list("countries", "country")), devices, seen, version: live.text("version"), pushed, file }
}

/// All of a puzzle's stats, as a few short lines: the plain form, for a pipe. `live` is the site's reply
/// for the puzzle. A line shows only when there is something to say on it.
pub fn stats_text(live: &Json) -> String {
    let stats = live.field("stats");
    let f = facts(live);
    let mut first = stat_line(&stats);
    if let Some(done) = stats.get("completion").and_then(Json::as_f64) {
        first = first.replacen(" solved", &format!(" solved ({:.0}% of starts)", done * 100.0), 1);
    }
    let mut lines = vec![format!("{} ({}){}", live.text("title"), live.text("size"), lock_mark(live)), live.text("url"), first];
    let mut add = |label: &str, value: &str| {
        if !value.is_empty() {
            lines.push(format!("{label}{value}"));
        }
    };
    add("Solves: ", &f.solves);
    add("Stopped with the grid filled: ", &f.stopped);
    let sep = if f.best.is_empty() { "" } else { " · " };
    add(&format!("Last {} days: ", f.opens.len()), &if f.days.is_empty() { String::new() } else { format!("{}{sep}{}", f.days, f.best) });
    add("From: ", &f.from);
    add("Countries: ", &f.countries);
    add("Devices: ", &f.devices);
    add("", &f.seen.join(" · "));
    if live.get("version").is_some() {
        let parts: Vec<String> = [format!("Version {}", f.version), f.pushed.clone()].into_iter().chain(f.file.iter().cloned()).filter(|p| !p.is_empty()).collect();
        add("", &parts.join(" · "));
    }
    add("Preview picture: ", &live.text("image"));
    lines.join("\n  ")
}

/// All of a puzzle's stats as a card, for a terminal: the title and the link, then a label and a value on each line.
pub fn stats_card(pen: &Pen, live: &Json) -> String {
    let stats = live.field("stats");
    let f = facts(live);
    let about = if is_locked(live) { format!("{} · PIN", live.text("size")) } else { live.text("size") };
    let n = |key: &str| pen.bold(&stats.text(key));
    let mut solvers = format!("{} opened · {} started · {} solved", n("opens"), n("starts"), n("solves"));
    if let Some(done) = stats.get("completion").and_then(Json::as_f64) {
        solvers += &format!(" ({:.0}% of starts)", done * 100.0);
    }
    let days = format!("Last {} days", f.opens.len());
    let mut rows: Vec<(&str, String)> = vec![("Solvers", solvers)];
    let seconds = |key: &str| stats.get(key).and_then(Json::as_f64).unwrap_or(0.0);
    if seconds("median") > 0.0 {
        rows.push(("Times", format!("median {} · fastest {}", util::clock(seconds("median")), util::clock(seconds("fastest")))));
    }
    let mut add = |label: &'static str, value: String| {
        if !value.is_empty() {
            rows.push((label, value));
        }
    };
    add("Solves", f.solves);
    add("Stopped at", f.stopped);
    if !f.days.is_empty() {
        rows.push((&days, f.days));
        // One block for each day's opens: the 30 days the site sends fit on a line. With opens on a day
        // or two only, the bar would be a flat line, so the day with the most says it all.
        if f.opens.len() <= 40 && f.opens.iter().filter(|n| **n > 0).count() >= 3 {
            rows.push(("", format!("{}  {}", style::spark(&f.opens), pen.dim(&f.best)).trim_end().to_string()));
        } else if !f.best.is_empty() {
            rows.push(("", pen.dim(&f.best)));
        }
    }
    let mut add = |label: &'static str, value: String| {
        if !value.is_empty() {
            rows.push((label, value));
        }
    };
    add("From", f.from);
    add("Countries", f.countries);
    add("Devices", f.devices);
    add("Activity", f.seen.join(" · ").to_lowercase());
    if live.get("version").is_some() {
        add("Version", [f.version, f.pushed].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" · "));
        add("Puzzle", f.file.join(" · "));
    }
    add("Picture", pen.url(&live.text("image")));
    let block = pen.pairs(&rows).lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n");
    format!("{}  {}\n{}\n\n{block}", pen.bold(&live.text("title")), pen.dim(&about), pen.link(&live.text("url")))
}

/// One line for an old version: its number, when it was pushed and replaced, its title and size.
pub fn version_line(v: &Json) -> String {
    // The minute, in UTC: two pushes on one day must be told apart.
    let minute = |key: &str| v.text(key).get(..16).map_or(v.text(key), |t| t.replace('T', " "));
    format!("{:>3}  published {}  replaced {} UTC  {} ({})", v.text("version"), minute("pushed"), minute("replaced"), v.text("title"), v.text("size"))
}

/// The old versions as a table, for a terminal. The times are in UTC, to the minute.
pub fn versions_table(pen: &Pen, old: &[Json]) -> String {
    let minute = |v: &Json, key: &str| v.text(key).get(..16).map_or(v.text(key), |t| t.replace('T', " "));
    let heads = [("VERSION", true), ("PUSHED", false), ("REPLACED", false), ("TITLE", false)];
    // What the other columns leave of the width, for the title and its size.
    let room = style::WIDTH - (7 + 2 + 16 + 2 + 16 + 2);
    let rows: Vec<Vec<Cell>> = old
        .iter()
        .map(|v| {
            let size = format!(" ({})", v.text("size"));
            let mut title = Cell::new(style::truncate(&v.text("title"), room.saturating_sub(style::width(&size))), Look::Plain);
            title.tail = size;
            vec![Cell::new(v.text("version"), Look::Bold), Cell::new(minute(v, "pushed"), Look::Plain), Cell::new(minute(v, "replaced"), Look::Plain), title]
        })
        .collect();
    pen.table(&heads, &rows)
}

/// What `puz publish` says. `p` is the site's reply. `tip` is true on a first push by someone who ran no
/// setup; `puzzles` is the link to all your puzzles, when you are signed in.
pub fn pushed_text(pen: &Pen, p: &Json, how: Pushed, tip: bool, puzzles: Option<&str>) -> String {
    let title = pen.title(&p.text("title"));
    let mut lines = Vec::new();
    match how {
        Pushed::New => {
            let who = if is_locked(p) { "Anyone with this link and the PIN" } else { "Anyone with this link" };
            lines.push(pen.ok(&format!("Published {title}. {who} can solve it:")));
            lines.push(pen.link(&p.text("url")));
            match p.get("email").and_then(Json::as_str) {
                Some(email) => lines.push(pen.note(&format!("It is attached to {email}."))),
                None if !pen.rich => lines.push("Its key is saved on this computer only. To keep your puzzles on any device: puz swords login EMAIL".to_string()),
                // One hint, and only to a person: never a question.
                None if tip => lines.extend([pen.note("Its key is saved on this computer only. Add your name and keep your puzzles:"), pen.cmd("puz setup")]),
                None => lines.extend([pen.note("Its key is saved on this computer only. To keep your puzzles on any device:"), pen.cmd("puz swords login EMAIL")]),
            }
        }
        Pushed::Updated | Pushed::Same => {
            let what = if how == Pushed::Same { format!("No change to {title}.") } else { format!("Updated {title}.") };
            lines.push(pen.ok(&format!("{what} The link is the same:")));
            lines.push(pen.link(&p.text("url")));
            if is_locked(p) {
                lines.push(pen.note("Solvers need the PIN to open it."));
            }
        }
    }
    if let Some(url) = puzzles {
        lines.push(pen.labelled_link("All your puzzles", url));
    }
    lines.join("\n")
}

/// What a push did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pushed {
    /// A new puzzle, with a new link.
    New,
    /// The puzzle at the link has the new file.
    Updated,
    /// The file is the one the site has already.
    Same,
}

/// The `--json` result of a first push. `manage` holds the key, on purpose: it is the claim link.
pub fn pushed_json(reply: &Json) -> Json {
    Json::object([
        ("id", reply.field("id")),
        ("title", reply.field("title")),
        ("url", reply.field("url")),
        ("manage", reply.field("manage")),
        ("author", reply.field("author")),
        ("email", reply.field("email")),
        ("locked", is_locked(reply).into()),
        ("image", reply.field("image")),
    ])
}

/// Does the site say this puzzle has a PIN?
fn is_locked(live: &Json) -> bool {
    live.get("locked") == Some(&Json::Bool(true))
}

/// The mark after the title of a puzzle that has a PIN.
fn lock_mark(live: &Json) -> &'static str {
    if is_locked(live) { " [PIN]" } else { "" }
}

/// A PIN as the site takes it: trimmed, 4 to 12 characters.
pub fn valid_pin(text: &str) -> Result<&str> {
    let pin = text.trim();
    if !(4..=12).contains(&pin.chars().count()) {
        return Err(Usage("A PIN is 4 to 12 characters.".into()).into());
    }
    Ok(pin)
}

/// The body of a call that sets a PIN (`{"pin":"4821"}`) or takes it off (`{"pin":null}`).
pub fn pin_body(pin: Option<&str>) -> String {
    serde_json::json!({ "pin": pin }).to_string()
}

/// One row of `list --json`. `live` is what the site says now; none when the puzzle is not there.
pub fn list_row(p: &Puzzle, live: Option<&Json>) -> Json {
    let title = live.and_then(|l| l.get("title")).cloned().unwrap_or_else(|| p.title.as_str().into());
    Json::object([
        ("id", p.id.as_str().into()),
        ("title", title),
        ("url", format!("{}/p/{}/", p.site, p.id).into()),
        ("file", p.file.as_str().into()),
        ("stats", live.map_or(Json::Null, |l| l.field("stats"))),
        ("locked", live.map_or(Json::Null, |l| is_locked(l).into())),
        ("image", live.map_or(Json::Null, |l| l.field("image"))),
        ("gone", live.is_none().into()),
    ])
}

/// The words for one row of `list`.
pub fn list_text(row: &Json) -> String {
    let status = if row.get("gone") == Some(&Json::Bool(true)) { "not on the site (deleted, or the site is down)".to_string() } else { stat_line(&row.field("stats")) };
    let from = match row.text("file") {
        file if file.is_empty() => String::new(),
        file => format!("\n  from {file}"),
    };
    format!("{}{}\n  {}\n  {status}{from}", row.text("title"), lock_mark(row), row.text("url"))
}

/// The rows of `list` as a table, for a terminal: the title, the three counts and the link.
/// A long title is cut so that a line fits the width. A link is never cut.
pub fn list_table(pen: &Pen, rows: &[Json]) -> String {
    let heads = [("TITLE", false), ("OPENED", true), ("STARTED", true), ("SOLVED", true), ("LINK", false)];
    let link = rows.iter().map(|r| style::width(&r.text("url"))).max().unwrap_or(0);
    let room = style::WIDTH.saturating_sub(6 + 7 + 6 + 4 * 2 + link).max(12);
    let mut gone = false;
    let cells: Vec<Vec<Cell>> = rows
        .iter()
        .map(|row| {
            let tail = lock_mark(row);
            let mut title = Cell::new(style::truncate(&row.text("title"), room - style::width(tail)), Look::Bold);
            title.tail = tail.to_string();
            let stats = row.field("stats");
            let number = |key: &str| if row.get("gone") == Some(&Json::Bool(true)) { Cell::new("–", Look::Dim) } else { Cell::new(stats.text(key), Look::Plain) };
            gone |= row.get("gone") == Some(&Json::Bool(true));
            vec![title, number("opens"), number("starts"), number("solves"), Cell::new(row.text("url"), Look::Link)]
        })
        .collect();
    let mut text = pen.table(&heads, &cells);
    if gone {
        text += &format!("\n\n{}", pen.dim("– not on the site (deleted, or the site is down)"));
    }
    text
}

fn list(ctx: &Ctx, state: &mut State) -> Result<()> {
    let site = &ctx.site;
    // Signed in: the puzzles attached to your email on this site come too, even ones pushed elsewhere.
    if state.session(site).is_some() {
        let me = api::try_call(Method::Get, &format!("{site}/api/me"), proof(state, site, ""));
        for p in me.as_ref().and_then(|m| m.get("puzzles")).map_or(&[][..], Json::as_array) {
            let id = p.text("id");
            if state.puzzles.iter().any(|q| q.id == id && &q.site == site) {
                continue;
            }
            state.puzzles.push(Puzzle { id, site: site.clone(), title: p.text("title"), pushed: p.get("created").map(Json::to_value), ..Puzzle::default() });
        }
        ctx.save(state)?;
    }
    let mut rows = Vec::new();
    for p in state.puzzles.iter().filter(|p| !ctx.site_flag || &p.site == site) {
        // `brief`: the counts and the times only. The site does less work than for `puz swords stats`.
        let live = api::try_call(Method::Get, &format!("{}/api/puzzles/{}?brief=1", p.site, p.id), proof(state, &p.site, &p.key));
        rows.push(list_row(p, live.as_ref()));
    }
    if ctx.json {
        say(&Json::Array(rows).pretty());
        return Ok(());
    }
    let pen = ctx.pen();
    if rows.is_empty() {
        say(&if pen.rich { format!("You have published no puzzles.\n{}", pen.cmd("puz publish hello-world.puz")) } else { "You have published no puzzles. Try one with: puz publish hello-world.puz".to_string() });
        return Ok(());
    }
    if pen.rich {
        say(&list_table(&pen, &rows));
    } else {
        for row in &rows {
            say(&list_text(row));
        }
    }
    // The page that shows them in a browser. With no sign-in that page would be empty, so the way to sign in is shown.
    match ctx.puzzles_url(state) {
        Some(url) if pen.rich => say(&format!("\n{}", pen.labelled_link("All your puzzles", &url).trim_start())),
        Some(url) => say(&pen.labelled_link("All your puzzles", &url)),
        None if pen.rich => say(&format!("\n{}\n{}", pen.dim("To see your puzzles in a browser, on any device:"), pen.cmd("puz swords login EMAIL"))),
        None => {}
    }
    Ok(())
}

fn stats(ctx: &Ctx, state: &State, name: &str) -> Result<()> {
    let p = &state.puzzles[find(state, ctx, name)?];
    let live = api::call(Method::Get, &format!("{}/api/puzzles/{}", p.site, p.id), proof(state, &p.site, &p.key), Body::None)?;
    let pen = ctx.pen();
    ctx.out(&live, &if pen.rich { stats_card(&pen, &live) } else { stats_text(&live) });
    Ok(())
}

fn versions(ctx: &Ctx, state: &State, name: &str) -> Result<()> {
    let p = &state.puzzles[find(state, ctx, name)?];
    let live = api::call(Method::Get, &format!("{}/api/puzzles/{}/versions", p.site, p.id), proof(state, &p.site, &p.key), Body::None)?;
    let old = live.get("versions").map_or(&[][..], Json::as_array);
    let pen = ctx.pen();
    let now = format!("{} is at version {}.", pen.title(&p.title), live.text("version"));
    let text = if old.is_empty() && pen.rich {
        format!("{now}\n{}", pen.dim("No old versions are kept: the puzzle was not replaced yet."))
    } else if old.is_empty() {
        format!("{now} No old versions are kept: the puzzle was not replaced yet.")
    } else if pen.rich {
        format!("{now}\n{}\n\n{}\n\n{}", pen.dim("Old versions (the site keeps the last 10). Times are in UTC."), versions_table(&pen, old), pen.cmd(&format!("puz swords restore {name} N")))
    } else {
        let lines: Vec<String> = old.iter().map(version_line).collect();
        format!("{now} Old versions (the site keeps the last 10):\n{}\nGo back to one with: puz swords restore {name} N", lines.join("\n"))
    };
    ctx.out(&live, &text);
    Ok(())
}

/// What `swords restore` says. `live` is the site's reply; `version` is the old version that is back.
pub fn restored_text(pen: &Pen, live: &Json, version: u32) -> String {
    let (title, now, link) = (pen.title(&live.text("title")), live.text("version"), pen.link(&live.text("url")));
    if pen.rich {
        format!("{}\n{}\n{link}", pen.ok(&format!("Restored version {version} of {title}.")), pen.note(&format!("It is now version {now}. The link is the same:")))
    } else {
        format!("{title} is back to version {version}. It is now version {now}. The link is the same:\n{link}")
    }
}

fn restore(ctx: &Ctx, state: &mut State, name: &str, version: u32) -> Result<()> {
    let i = find(state, ctx, name)?;
    let p = &state.puzzles[i];
    let live = api::call(Method::Post, &format!("{}/api/puzzles/{}/versions/{version}", p.site, p.id), proof(state, &p.site, &p.key), Body::None)?;
    if let Some(title) = live.get("title").and_then(Json::as_str) {
        state.puzzles[i].title = title.to_string();
    }
    if let Some(updated) = live.get("updated") {
        state.puzzles[i].updated = Some(updated.to_value());
    }
    ctx.save(state)?;
    let data = Json::object([("id", live.field("id")), ("title", live.field("title")), ("url", live.field("url")), ("restored", live.field("restored")), ("version", live.field("version"))]);
    ctx.out(&data, &restored_text(&ctx.pen(), &live, version));
    Ok(())
}

fn embed(ctx: &Ctx, state: &State, name: &str) -> Result<()> {
    let p = &state.puzzles[find(state, ctx, name)?];
    let live = api::call(Method::Get, &format!("{}/api/puzzles/{}?brief=1", p.site, p.id), proof(state, &p.site, &p.key), Body::None)?;
    let html = live.text("embed");
    if html.is_empty() {
        bail!("This site does not give embed code yet.");
    }
    let data = Json::object([("id", live.field("id")), ("title", live.field("title")), ("embed_url", live.field("embed_url")), ("embed", live.field("embed")), ("locked", is_locked(&live).into())]);
    // The HTML alone on stdout, so it can be piped into a file. The note goes to the person.
    if !ctx.json {
        let pin = if is_locked(&live) { " Solvers type the PIN inside the frame." } else { "" };
        let pen = Pen::stderr();
        crate::warn(pen.note(&format!("Paste this into your page to show {} there.{pin}", if pen.rich { live.text("title") } else { pen.title(&live.text("title")) })).trim_start());
    }
    ctx.out(&data, &html);
    Ok(())
}

/// Read a PIN that was not on the command line: asked for without showing it, or one line from a pipe.
fn read_pin() -> Result<String> {
    use std::io::IsTerminal as _;
    let stdin = std::io::stdin();
    let asked = stdin.is_terminal();
    if asked {
        let pen = Pen::stderr();
        eprint!("{} {}: ", pen.bold("PIN"), pen.dim("(4 to 12 characters; it is not shown)"));
    }
    let hidden = asked && echo(false);
    let mut line = String::new();
    let read = stdin.read_line(&mut line);
    if hidden {
        echo(true);
        eprintln!();
    }
    read.context("Can't read the PIN")?;
    Ok(line)
}

/// Turn the terminal's echo of typed letters on or off. False when this computer can't do it.
fn echo(on: bool) -> bool {
    crate::style::console::echo(on)
}

fn pin_command(ctx: &Ctx, state: &State, name: &str, pin: Option<&str>, remove: bool) -> Result<()> {
    let p = &state.puzzles[find(state, ctx, name)?];
    let typed;
    let pin = match (remove, pin) {
        (true, _) => None,
        (false, Some(pin)) => Some(valid_pin(pin)?),
        (false, None) => {
            typed = read_pin()?;
            Some(valid_pin(&typed)?)
        }
    };
    let live = share::set_pin(state, &p.site, &p.id, pin).map_err(share::Error::into_anyhow)?.reply;
    let data = Json::object([("id", live.field("id")), ("title", live.field("title")), ("url", live.field("url")), ("locked", is_locked(&live).into())]);
    let pen = ctx.pen();
    let title = pen.title(&live.text("title"));
    let text = match pin {
        Some(_) => format!("{title} now needs the PIN. Send the PIN with the link:"),
        None => format!("{title} has no PIN now. Anyone with this link can solve it:"),
    };
    let text = format!("{}\n{}", pen.ok(&text), pen.link(&live.text("url")));
    ctx.out(&data, &text);
    Ok(())
}

fn protect_command(ctx: &Ctx, state: &State, name: &str, on: bool) -> Result<()> {
    let p = &state.puzzles[find(state, ctx, name)?];
    let live = share::set_protected(state, &p.site, &p.id, on).map_err(share::Error::into_anyhow)?;
    let now = matches!(live.get("protected"), Some(Json::Bool(true)));
    let data = Json::object([("id", live.field("id")), ("title", live.field("title")), ("url", live.field("url")), ("protected", now.into())]);
    let pen = ctx.pen();
    let title = pen.title(&live.text("title"));
    let text = if now { format!("{title} is protected from deletion.") } else { format!("{title} is no longer protected. It can be deleted.") };
    ctx.out(&data, &pen.ok(&text));
    Ok(())
}

fn open(ctx: &Ctx, state: &State, name: &str) -> Result<()> {
    let p = &state.puzzles[find(state, ctx, name)?];
    let url = format!("{}/p/{}/", p.site, p.id);
    // $BROWSER names another program to open links with. Windows opens a link through the shell's
    // file handler, which needs no quoting (`cmd /c start` would break a link at its `&`).
    let (program, first): (std::ffi::OsString, Option<&str>) = match std::env::var_os("BROWSER").filter(|b| !b.is_empty()) {
        Some(b) => (b, None),
        None if cfg!(windows) => ("rundll32".into(), Some("url.dll,FileProtocolHandler")),
        None if cfg!(target_os = "macos") => ("open".into(), None),
        None => ("xdg-open".into(), None),
    };
    let started = std::process::Command::new(&program).args(first).arg(&url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    if started.is_err() {
        bail!("Can't start a browser here ({} did not run). Open the link yourself:\n  {url}", program.to_string_lossy());
    }
    ctx.out(&Json::object([("url", url.as_str().into())]), &if ctx.pen().rich { format!("{}\n{}", ctx.pen().ok("Opened in your browser:"), ctx.pen().link(&url)) } else { format!("Opened {url}") });
    Ok(())
}

fn manage(ctx: &Ctx, state: &mut State, name: &str, yes: bool) -> Result<()> {
    let i = find(state, ctx, name)?;
    if state.puzzles[i].key.is_empty() {
        // A puzzle that came from your sign-in has no key here. Make one; the old key stops working.
        let p = &state.puzzles[i];
        if !yes {
            bail!("This computer has no key for \"{}\". To make a new one (the old key and old claim links stop working): puz swords manage {name} --yes", p.title);
        }
        let made = api::call(Method::Post, &format!("{}/api/puzzles/{}/key", p.site, p.id), proof(state, &p.site, ""), Body::None)?;
        state.puzzles[i].key = made.text("key");
        ctx.save(state)?;
    }
    let link = manage_link(&state.puzzles[i]);
    ctx.out(&Json::object([("manage", link.as_str().into())]), &if ctx.pen().rich { format!("{}\n{}", ctx.pen().caution("Anyone who has this link can change the puzzle:"), ctx.pen().link(&link)) } else { format!("{link}\nAnyone who has this link can change the puzzle.") });
    Ok(())
}

fn delete(ctx: &Ctx, state: &mut State, name: &str, yes: bool) -> Result<()> {
    let i = find(state, ctx, name)?;
    let p = &state.puzzles[i];
    if !yes {
        bail!("This takes \"{}\" off the site for good: its link and its stats go. To do it: puz swords unpublish {name} --yes", p.title);
    }
    let (site, id, title) = (p.site.clone(), p.id.clone(), p.title.clone());
    share::delete(state, &ctx.state_file, &site, &id).map_err(share::Error::into_anyhow)?;
    let p = Puzzle { id, title, ..Puzzle::default() };
    ctx.out(&Json::object([("deleted", p.id.as_str().into())]), &ctx.pen().ok(&format!("Took {} off the site. Its link no longer works.", ctx.pen().title(&p.title))));
    Ok(())
}

fn claim(ctx: &Ctx, state: &mut State, link: &str) -> Result<()> {
    let Some((origin, id, key)) = util::parse_claim(link) else {
        return Err(Usage("Usage: puz swords claim LINK   (the claim link, like https://site/p/abcd2345/manage#KEY)".into()).into());
    };
    let live = api::call(Method::Get, &format!("{origin}/api/puzzles/{id}"), proof(state, origin, key), Body::None)?;
    state.puzzles.retain(|p| !(p.id == id && p.site == origin));
    state.puzzles.push(Puzzle { id: id.into(), site: origin.into(), key: key.into(), title: live.text("title"), pushed: Some(util::now_iso().into()), ..Puzzle::default() });
    ctx.save(state)?;
    let data = Json::object([("id", id.into()), ("title", live.field("title")), ("url", live.field("url"))]);
    ctx.out(&data, &ctx.pen().ok(&format!("Claimed {}. It is in your list now.", ctx.pen().title(&live.text("title")))));
    Ok(())
}

pub fn login_body(state: &State, site: &str, email: &str) -> String {
    let keys: serde_json::Map<String, serde_json::Value> = state.puzzles.iter().filter(|p| p.site == site && !p.key.is_empty()).map(|p| (p.id.clone(), p.key.as_str().into())).collect();
    serde_json::json!({ "email": email, "keys": keys, "wait": true }).to_string()
}

fn login(ctx: &Ctx, state: &mut State, email: &str) -> Result<()> {
    // At a terminal, the person types the code from the email. With none, this waits for the link to be opened.
    let mut terminal = setup::Terminal;
    let person: Option<&mut dyn setup::Ask> = if setup::can_ask(ctx) { Some(&mut terminal) } else { None };
    let Some(done) = setup::sign_in(ctx, state, email, person)? else {
        bail!("You are not signed in. Start again with: puz swords login EMAIL");
    };
    ctx.out(&done, &ctx.pen().ok(&setup::signed_in_text(&done)));
    Ok(())
}

fn logout(ctx: &Ctx, state: &mut State) -> Result<()> {
    let site = &ctx.site;
    if state.session(site).is_none() {
        ctx.out(&Json::object([("signed_out", false.into())]), &format!("Nobody is signed in on {site}."));
        return Ok(());
    }
    // Tell the site. If it does not answer, sign out here all the same.
    let _ = api::send(Method::Post, &format!("{site}/api/logout"), proof(state, site, ""), Body::None);
    state.sessions.remove(site);
    state.sessions.remove(util::origin(site));
    ctx.save(state)?;
    ctx.out(&Json::object([("signed_out", true.into())]), &ctx.pen().ok(&format!("Signed out on {site}. The keys on this computer still work.")));
    Ok(())
}

/// The words of `whoami`: the name, the email and the site, one on each line. `puzzles` is the link to
/// all your puzzles, when you are signed in.
pub fn whoami_text(pen: &Pen, name: &str, email: Option<&str>, site: &str, puzzles: Option<&str>) -> String {
    if !pen.rich {
        let name = if name.is_empty() { "not set. Set it with: puz setup" } else { name };
        let email = email.unwrap_or("not signed in. Sign in with: puz swords login EMAIL");
        let puzzles = puzzles.map(|url| format!("\n{}", pen.labelled_link("All your puzzles", url))).unwrap_or_default();
        return format!("Name:  {name}\nEmail: {email}\nSite:  {site}{puzzles}");
    }
    let or = |text: Option<&str>, none: &str| text.map_or(pen.dim(none), str::to_string);
    let mut rows = vec![("Name", or(Some(name).filter(|n| !n.is_empty()), "not set")), ("Email", or(email, "not signed in")), ("Site", site.to_string())];
    if let Some(url) = puzzles {
        rows.push(("Puzzles", pen.url(url)));
    }
    let mut lines = vec![pen.pairs(&rows)];
    // What to run for the parts that are not there.
    let todo: Vec<String> = [(name.is_empty(), "puz setup"), (email.is_none(), "puz swords login EMAIL")].iter().filter(|(missing, _)| *missing).map(|(_, c)| pen.cmd(c)).collect();
    if !todo.is_empty() {
        lines.push(format!("\n{}", todo.join("\n")));
    }
    lines.join("\n")
}

fn whoami(ctx: &Ctx, state: &State) -> Result<()> {
    let site = &ctx.site;
    let email = state.session(site).map(|s| s.email.as_str());
    let name = if ctx.name.is_empty() { Json::Null } else { ctx.name.as_str().into() };
    let puzzles = ctx.puzzles_url(state);
    let data = Json::object([("name", name), ("email", email.into()), ("site", site.as_str().into()), ("puzzles_url", puzzles.as_deref().into())]);
    ctx.out(&data, &whoami_text(&ctx.pen(), &ctx.name, email, site, puzzles.as_deref()));
    Ok(())
}

/// What `swords site` says. `set` is true when the site was just saved.
pub fn site_text(pen: &Pen, site: &str, set: bool) -> String {
    match (pen.rich, set) {
        (false, _) => format!("Puzzles are published to {site}"),
        (true, true) => format!("{}\n{}", pen.ok("Puzzles are published to:"), pen.link(site)),
        (true, false) => format!("Puzzles are published to:\n{}", pen.link(site)),
    }
}

fn site(ctx: &Ctx, state: &mut State, url: Option<&str>) -> Result<()> {
    let now = match url {
        Some(url) => {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(Usage("The site is an address that starts with http:// or https://".into()).into());
            }
            state.site = url.trim_end_matches('/').to_string();
            ctx.save(state)?;
            state.site.clone()
        }
        None => ctx.site.clone(),
    };
    ctx.out(&Json::object([("site", now.as_str().into())]), &site_text(&ctx.pen(), &now, url.is_some()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: &str = "http://localhost:4747";
    /// A terminal with colors off: the marks and the layout, with no escape codes to read around.
    const MARKS: Pen = Pen { rich: true, color: false };

    fn ctx() -> Ctx {
        Ctx { site: SITE.into(), site_flag: false, json: true, state_file: "/nowhere/puzzles.json".into(), cwd: "/work".into(), name: String::new() }
    }
    fn puzzle(id: &str, site: &str, file: &str) -> Puzzle {
        Puzzle { id: id.into(), site: site.into(), key: format!("key-{id}"), file: file.into(), title: format!("T {id}"), ..Puzzle::default() }
    }
    fn state() -> State {
        State {
            puzzles: vec![puzzle("aaaa1111", SITE, "/work/a.json"), puzzle("bbbb2222", "https://other", "/work/a.json"), puzzle("cccc3333", SITE, "/work/a.json"), puzzle("dddd4444", SITE, "")],
            ..State::default()
        }
    }
    fn compact(j: &Json) -> String {
        serde_json::to_string(j).unwrap()
    }

    #[test]
    fn find_by_file_id_or_link() {
        let (s, c) = (state(), ctx());
        // The file: the newest push of it to this site.
        assert_eq!(find(&s, &c, "a.json").unwrap(), 2);
        assert_eq!(find(&s, &c, "./a.json").unwrap(), 2);
        assert_eq!(find(&s, &c, "/work/a.json").unwrap(), 2);
        // The id works for any site.
        assert_eq!(find(&s, &c, "bbbb2222").unwrap(), 1);
        assert_eq!(find(&s, &c, "http://localhost:4747/p/aaaa1111/").unwrap(), 0);
        assert_eq!(find(&s, &c, "http://localhost:4747/p/dddd4444/manage#K").unwrap(), 3);
        let err = find(&s, &c, "nope.json").unwrap_err().to_string();
        assert_eq!(err, "No puzzle \"nope.json\" among the ones you published. See them with: puz swords list");
    }

    #[test]
    fn a_push_replaces_the_newest_puzzle_from_the_same_file_and_site() {
        let s = state();
        assert_eq!(pushed_before(&s, "/work/a.json", SITE), Some(2));
        assert_eq!(pushed_before(&s, "/work/a.json", "https://other"), Some(1));
        assert_eq!(pushed_before(&s, "/work/b.json", SITE), None);
        assert_eq!(pushed_before(&s, "/work/a.json", "https://third"), None);
        // A claimed puzzle has no file, so no push replaces it.
        assert_eq!(pushed_before(&s, "", "https://third"), None);
    }

    #[test]
    fn stat_lines() {
        let s = Json::parse(br#"{"opens":3,"starts":2,"solves":0,"fastest":null,"median":null}"#);
        assert_eq!(stat_line(&s), "3 opened · 2 started · 0 solved");
        let s = Json::parse(br#"{"opens":3,"starts":2,"solves":2,"fastest":182,"median":250}"#);
        assert_eq!(stat_line(&s), "3 opened · 2 started · 2 solved · median 4:10 · fastest 3:02");
    }

    #[test]
    fn full_stats_in_words() {
        // A site from before the full stats: the old three lines.
        let old = Json::parse(br#"{"title":"T","size":"5x5","url":"U","stats":{"opens":3,"starts":2,"solves":0,"fastest":null,"median":null}}"#);
        assert_eq!(stats_text(&old), "T (5x5)\n  U\n  3 opened · 2 started · 0 solved");
        let live = Json::parse(
            br#"{"title":"T","size":"5x5","url":"U","locked":true,"version":3,"format":"puz","pushed_by":"cli","cli_version":"0.3.0","answers":10,"features":["walls","rebus"],"image":"I",
            "stats":{"opens":7,"starts":5,"solves":3,"fastest":100,"median":185,"completion":0.6,"tracked":7,"embedded":1,"clean":2,"with_help":1,
            "last_opened_at":"2026-11-19T01:00:29.169Z","first_solved_at":"2026-11-18T01:00:28.915Z",
            "days":[{"day":"2026-11-18","opens":1,"starts":1,"solves":1},{"day":"2026-11-19","opens":5,"starts":4,"solves":2}],
            "referrers":[{"host":"news.example.com","opens":2}],"direct":4,"countries":[{"country":"US","opens":2},{"country":"DE","opens":1}],
            "devices":{"phone":2,"tablet":0,"computer":3},
            "stopped":[{"from":0,"to":24,"count":1},{"from":25,"to":49,"count":0},{"from":100,"to":100,"count":1}]}}"#,
        );
        assert_eq!(
            stats_text(&live),
            "T (5x5) [PIN]\n  U\n  7 opened · 5 started · 3 solved (60% of starts) · median 3:05 · fastest 1:40\n  \
             Solves: 2 with no help · 1 with a check or a reveal\n  \
             Stopped with the grid filled: 0–24% 1 · full grid 1\n  \
             Last 2 days: 6 opened · 5 started · 3 solved · most on 2026-11-19 (5)\n  \
             From: news.example.com 2 · direct 4\n  \
             Countries: US 2 · DE 1\n  \
             Devices: computer 3 · phone 2 · embedded in another site 1\n  \
             Last opened 2026-11-19 · first solved 2026-11-18\n  \
             Version 3 · published from the command line (0.3.0) · a .puz file · 10 answers · walls, rebus\n  \
             Preview picture: I"
        );
    }

    #[test]
    fn version_lines() {
        let v = Json::parse(br#"{"version":3,"title":"T","size":"5x5","pushed":"2026-11-18T01:00:00.000Z","replaced":"2026-11-19T02:00:00.000Z"}"#);
        assert_eq!(version_line(&v), "  3  published 2026-11-18 01:00  replaced 2026-11-19 02:00 UTC  T (5x5)");
    }

    #[test]
    fn push_json_shape() {
        let reply = Json::parse(br#"{"id":"aaaa1111","title":"T","url":"U","key":"SECRET","manage":"M","email":null,"note":"n"}"#);
        assert_eq!(compact(&pushed_json(&reply)), r#"{"id":"aaaa1111","title":"T","url":"U","manage":"M","author":null,"email":null,"locked":false,"image":null}"#);
        // A site that sends no email field still gives one in the result.
        assert_eq!(compact(&pushed_json(&Json::parse(br#"{"id":"x"}"#))), r#"{"id":"x","title":null,"url":null,"manage":null,"author":null,"email":null,"locked":false,"image":null}"#);
    }

    #[test]
    fn list_json_shape_and_words() {
        let p = puzzle("aaaa1111", SITE, "/work/a.json");
        let live = Json::parse(br#"{"id":"aaaa1111","title":"Live title","stats":{"opens":1,"starts":0,"solves":0,"fastest":null,"median":null},"image":"I"}"#);
        let row = list_row(&p, Some(&live));
        assert_eq!(
            compact(&row),
            r#"{"id":"aaaa1111","title":"Live title","url":"http://localhost:4747/p/aaaa1111/","file":"/work/a.json","stats":{"opens":1,"starts":0,"solves":0,"fastest":null,"median":null},"locked":false,"image":"I","gone":false}"#
        );
        assert_eq!(list_text(&row), "Live title\n  http://localhost:4747/p/aaaa1111/\n  1 opened · 0 started · 0 solved\n  from /work/a.json");

        let gone = list_row(&puzzle("dddd4444", SITE, ""), None);
        assert_eq!(compact(&gone), r#"{"id":"dddd4444","title":"T dddd4444","url":"http://localhost:4747/p/dddd4444/","file":"","stats":null,"locked":null,"image":null,"gone":true}"#);
        assert_eq!(list_text(&gone), "T dddd4444\n  http://localhost:4747/p/dddd4444/\n  not on the site (deleted, or the site is down)");
        // The row never holds the key.
        assert!(!compact(&row).contains("key-"));
    }

    #[test]
    fn a_puzzle_with_a_pin_is_marked() {
        let p = puzzle("aaaa1111", SITE, "");
        let live = Json::parse(br#"{"id":"aaaa1111","title":"Secret","locked":true,"stats":{"opens":0,"starts":0,"solves":0,"fastest":null,"median":null}}"#);
        let row = list_row(&p, Some(&live));
        assert_eq!(row.get("locked"), Some(&Json::Bool(true)));
        assert_eq!(list_text(&row), "Secret [PIN]\n  http://localhost:4747/p/aaaa1111/\n  0 opened · 0 started · 0 solved");
        // A site from before PINs sends no `locked`: the puzzle is not marked.
        assert_eq!(list_row(&p, Some(&Json::parse(br#"{"title":"Old"}"#))).get("locked"), Some(&Json::Bool(false)));
    }

    #[test]
    fn pins_are_trimmed_and_4_to_12_characters() {
        assert_eq!(valid_pin("4821").unwrap(), "4821");
        assert_eq!(valid_pin("  owl-4821\n").unwrap(), "owl-4821");
        assert_eq!(valid_pin("éééé").unwrap(), "éééé");
        assert_eq!(valid_pin("123456789012").unwrap(), "123456789012");
        for bad in ["", "123", " 123 ", "1234567890123"] {
            let err = valid_pin(bad).unwrap_err();
            assert!(err.is::<Usage>(), "{bad}");
            assert_eq!(err.to_string(), "A PIN is 4 to 12 characters.");
        }
        assert_eq!(pin_body(Some("4821")), r#"{"pin":"4821"}"#);
        assert_eq!(pin_body(None), r#"{"pin":null}"#);
    }

    #[test]
    fn login_sends_only_this_sites_keys() {
        let mut s = state();
        s.puzzles[3].key.clear();
        let body: serde_json::Value = serde_json::from_str(&login_body(&s, SITE, "me@example.com")).unwrap();
        assert_eq!(body, serde_json::json!({ "email": "me@example.com", "wait": true, "keys": { "aaaa1111": "key-aaaa1111", "cccc3333": "key-cccc3333" } }));
    }

    #[test]
    fn whoami_in_words() {
        let plain = Pen::PLAIN;
        assert_eq!(whoami_text(&plain, "Ada Lovelace", Some("me@example.com"), SITE, Some("http://localhost:4747/puzzles")), "Name:  Ada Lovelace\nEmail: me@example.com\nSite:  http://localhost:4747\nAll your puzzles: http://localhost:4747/puzzles");
        assert_eq!(whoami_text(&plain, "", None, SITE, None), "Name:  not set. Set it with: puz setup\nEmail: not signed in. Sign in with: puz swords login EMAIL\nSite:  http://localhost:4747");
        // At a terminal: labels in one column, and the commands for what is not there.
        assert_eq!(whoami_text(&MARKS, "Ada", Some("me@example.com"), SITE, Some("http://localhost:4747/puzzles")), "Name     Ada\nEmail    me@example.com\nSite     http://localhost:4747\nPuzzles  http://localhost:4747/puzzles");
        assert_eq!(whoami_text(&MARKS, "", None, SITE, None), "Name   not set\nEmail  not signed in\nSite   http://localhost:4747\n\n  $ puz setup\n  $ puz swords login EMAIL");
    }

    #[test]
    fn a_push_in_plain_words_and_at_a_terminal() {
        let new = Json::parse(br#"{"title":"Hello, World","url":"http://s/p/aaaa1111/","email":null}"#);
        assert_eq!(
            pushed_text(&Pen::PLAIN, &new, Pushed::New, false, None),
            "Published \"Hello, World\". Anyone with this link can solve it:\n  http://s/p/aaaa1111/\nIts key is saved on this computer only. To keep your puzzles on any device: puz swords login EMAIL"
        );
        assert_eq!(
            pushed_text(&MARKS, &new, Pushed::New, false, None),
            "✓ Published Hello, World. Anyone with this link can solve it:\n  http://s/p/aaaa1111/\n  Its key is saved on this computer only. To keep your puzzles on any device:\n  $ puz swords login EMAIL"
        );
        // The first push by someone who ran no setup: one hint, the setup.
        let tip = pushed_text(&MARKS, &new, Pushed::New, true, None);
        assert!(tip.ends_with("\n  Its key is saved on this computer only. Add your name and keep your puzzles:\n  $ puz setup"), "{tip}");
        // Colors: a green mark, a bold title, a cyan link with nothing around it.
        let rich = pushed_text(&Pen::RICH, &new, Pushed::New, false, None);
        assert!(rich.starts_with("\x1b[32m✓\x1b[0m Published \x1b[1mHello, World\x1b[0m. Anyone with this link can solve it:\n  \x1b[36mhttp://s/p/aaaa1111/\x1b[0m\n"), "{rich:?}");

        // Signed in, with a PIN: the email, and the page with all your puzzles.
        let mine = Json::parse(br#"{"title":"T","url":"U","email":"me@example.com","locked":true}"#);
        assert_eq!(
            pushed_text(&Pen::PLAIN, &mine, Pushed::New, false, Some("http://s/puzzles")),
            "Published \"T\". Anyone with this link and the PIN can solve it:\n  U\nIt is attached to me@example.com.\nAll your puzzles: http://s/puzzles"
        );
        assert_eq!(pushed_text(&MARKS, &mine, Pushed::New, false, Some("http://s/puzzles")), "✓ Published T. Anyone with this link and the PIN can solve it:\n  U\n  It is attached to me@example.com.\n  All your puzzles: http://s/puzzles");
        assert_eq!(pushed_text(&Pen::PLAIN, &mine, Pushed::Updated, false, None), "Updated \"T\". The link is the same:\n  U\nSolvers need the PIN to open it.");
        assert_eq!(pushed_text(&MARKS, &new, Pushed::Updated, false, None), "✓ Updated Hello, World. The link is the same:\n  http://s/p/aaaa1111/");
        assert_eq!(pushed_text(&Pen::PLAIN, &new, Pushed::Same, false, None), "No change to \"Hello, World\". The link is the same:\n  http://s/p/aaaa1111/");
        for how in [Pushed::New, Pushed::Updated, Pushed::Same] {
            for line in pushed_text(&MARKS, &mine, how, true, Some("http://s/puzzles")).lines() {
                assert!(style::width(line) <= style::WIDTH, "{line}");
            }
        }
    }

    #[test]
    fn the_list_as_a_table() {
        let live = |title: &str, locked: bool, opens: u32| Json::parse(format!(r#"{{"title":"{title}","locked":{locked},"stats":{{"opens":{opens},"starts":2,"solves":1}}}}"#).as_bytes());
        let rows = [
            list_row(&puzzle("aaaa1111", SITE, ""), Some(&live("Hello, World", false, 120))),
            list_row(&puzzle("bbbb2222", SITE, ""), Some(&live("A title that is much too long to fit in the table", true, 3))),
            list_row(&puzzle("dddd4444", SITE, ""), None),
        ];
        assert_eq!(
            list_table(&MARKS, &rows),
            "TITLE                OPENED  STARTED  SOLVED  LINK\n\
             Hello, World            120        2       1  http://localhost:4747/p/aaaa1111/\n\
             A title that… [PIN]       3        2       1  http://localhost:4747/p/bbbb2222/\n\
             T dddd4444                –        –       –  http://localhost:4747/p/dddd4444/\n\n\
             – not on the site (deleted, or the site is down)"
        );
        for line in list_table(&MARKS, &rows).lines() {
            assert!(style::width(line) <= style::WIDTH, "{line}");
        }
        let rich = list_table(&Pen::RICH, &rows[..1]);
        assert!(rich.ends_with("\n\x1b[1mHello, World\x1b[0m     120        2       1  \x1b[36mhttp://localhost:4747/p/aaaa1111/\x1b[0m"), "{rich:?}");
    }

    #[test]
    fn full_stats_as_a_card() {
        let live = Json::parse(
            br#"{"title":"T","size":"5x5","url":"U","locked":true,"version":3,"format":"puz","pushed_by":"cli","cli_version":"0.3.0","answers":10,"features":["walls","rebus"],"image":"I",
            "stats":{"opens":7,"starts":5,"solves":3,"fastest":100,"median":185,"completion":0.6,"embedded":1,"clean":2,"with_help":1,
            "last_opened_at":"2026-11-19T01:00:29.169Z","first_solved_at":"2026-11-18T01:00:28.915Z",
            "days":[{"day":"2026-11-16","opens":0,"starts":0,"solves":0},{"day":"2026-11-17","opens":1,"starts":0,"solves":0},{"day":"2026-11-18","opens":1,"starts":1,"solves":1},{"day":"2026-11-19","opens":5,"starts":4,"solves":2}],
            "referrers":[{"host":"news.example.com","opens":2}],"direct":4,"countries":[{"country":"US","opens":2}],
            "devices":{"phone":2,"tablet":0,"computer":3},"stopped":[{"from":0,"to":24,"count":1},{"from":100,"to":100,"count":1}]}}"#,
        );
        assert_eq!(
            stats_card(&MARKS, &live),
            "T  5x5 · PIN\n  U\n\n  \
             Solvers      7 opened · 5 started · 3 solved (60% of starts)\n  \
             Times        median 3:05 · fastest 1:40\n  \
             Solves       2 with no help · 1 with a check or a reveal\n  \
             Stopped at   0–24% 1 · full grid 1\n  \
             Last 4 days  7 opened · 5 started · 3 solved\n  \
             \x20            ▁▂▂▇  most on 2026-11-19 (5)\n  \
             From         news.example.com 2 · direct 4\n  \
             Countries    US 2\n  \
             Devices      computer 3 · phone 2 · embedded in another site 1\n  \
             Activity     last opened 2026-11-19 · first solved 2026-11-18\n  \
             Version      3 · published from the command line (0.3.0)\n  \
             Puzzle       a .puz file · 10 answers · walls, rebus\n  \
             Picture      I"
        );
        // A puzzle nobody opened: the title, the link and the counts.
        let quiet = Json::parse(br#"{"title":"T","size":"5x5","url":"U","stats":{"opens":0,"starts":0,"solves":0}}"#);
        assert_eq!(stats_card(&MARKS, &quiet), "T  5x5\n  U\n\n  Solvers  0 opened · 0 started · 0 solved");
        // Opens on one day only: no bar, the day says it.
        let one = Json::parse(br#"{"title":"T","size":"5x5","url":"U","stats":{"opens":2,"starts":0,"solves":0,"days":[{"day":"2026-11-18","opens":0},{"day":"2026-11-19","opens":2}]}}"#);
        assert!(stats_card(&MARKS, &one).ends_with("\n  Last 2 days  2 opened · 0 started · 0 solved\n               most on 2026-11-19 (2)"), "{}", stats_card(&MARKS, &one));
        assert!(stats_card(&Pen::RICH, &quiet).starts_with("\x1b[1mT\x1b[0m  \x1b[2m5x5\x1b[0m\n  \x1b[36mU\x1b[0m\n\n  \x1b[2mSolvers\x1b[0m  \x1b[1m0\x1b[0m opened"));
    }

    #[test]
    fn old_versions_as_a_table() {
        let old = [
            Json::parse(br#"{"version":2,"title":"T","size":"5x5","pushed":"2026-11-18T01:00:00.000Z","replaced":"2026-11-19T02:00:00.000Z"}"#),
            Json::parse(br#"{"version":10,"title":"A title that is much too long to fit in what is left of the line","size":"15x15","pushed":"2026-11-19T02:00:00.000Z","replaced":"2026-11-19T03:00:00.000Z"}"#),
        ];
        let table = versions_table(&MARKS, &old);
        assert_eq!(table.lines().next(), Some("VERSION  PUSHED            REPLACED          TITLE"));
        assert_eq!(table.lines().nth(1), Some("      2  2026-11-18 01:00  2026-11-19 02:00  T (5x5)"));
        assert_eq!(table.lines().nth(2), Some("     10  2026-11-19 02:00  2026-11-19 03:00  A title that is much too l… (15x15)"));
        assert!(table.lines().all(|l| style::width(l) <= style::WIDTH));
    }

    #[test]
    fn a_restore_in_plain_words_and_at_a_terminal() {
        let live = Json::parse(br#"{"title":"T","url":"U","version":4}"#);
        assert_eq!(restored_text(&Pen::PLAIN, &live, 2), "\"T\" is back to version 2. It is now version 4. The link is the same:\n  U");
        assert_eq!(restored_text(&MARKS, &live, 2), "✓ Restored version 2 of T.\n  It is now version 4. The link is the same:\n  U");
    }

    #[test]
    fn the_site_in_plain_words_and_at_a_terminal() {
        assert_eq!(site_text(&Pen::PLAIN, SITE, true), "Puzzles are published to http://localhost:4747");
        assert_eq!(site_text(&MARKS, SITE, true), "✓ Puzzles are published to:\n  http://localhost:4747");
        assert_eq!(site_text(&MARKS, SITE, false), "Puzzles are published to:\n  http://localhost:4747");
    }

    #[test]
    fn manage_link_holds_the_key() {
        assert_eq!(manage_link(&puzzle("aaaa1111", SITE, "")), "http://localhost:4747/p/aaaa1111/manage#key-aaaa1111");
    }
}
