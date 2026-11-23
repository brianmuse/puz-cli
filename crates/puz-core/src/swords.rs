//! Publishing on a Swords solving site: push a puzzle file, get a link anyone can solve at, and
//! remember that link in the puzzle file (`puz publish`, `puz swords status`).
//!
//! What is kept where:
//! - The link lives in the puzzle file (`Puzzle::swords`, one record per site): the site, the id,
//!   the address to solve at, the owner's page, when it was first and last pushed, a hash of the
//!   content pushed, and whether it has a PIN. So it follows the puzzle through renames and moves.
//! - The secret key that owns the pushed puzzle stays out of the puzzle file, in the swords state
//!   file (`~/.config/swords/puzzles.json`, or `$SWORDS_HOME`). It is found by site and id, never by
//!   file path. The PIN is never stored at all.
//! - A file that is not a puz puzzle (.puz, .jpz, .ipuz, .swords) can't hold a record. It is sent
//!   as it is, and a second publish finds the earlier one by the file's path in the state file.
//!
//! The calls to the site go through the bundled swords library (`crates/swords`, package `swords-client`, its `share`
//! module). An AI agent may read the status; it publishes or takes down only with `confirmed`.

use crate::puzzle::Puzzle;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use swords_client::api::{self, Body, Method};
use swords_client::share;
use swords_client::state::{self, State};

pub use swords_client::DEFAULT_SITE;

pub const NOT_CONFIRMED: &str = "An AI agent publishes a puzzle to Swords, changes its PIN, or takes it down only after the user agrees. Nothing was sent. \
Tell the user exactly what you will do (the puzzle, the site, a new link or an update of the existing link, PIN yes or no) and ask them to confirm. \
Only after they say yes in their next message, run the same command again with --confirmed. Never add --confirmed on your own.";

/// True inside an AI agent's run: an agent runner sets a scratch folder for the commands it starts.
pub fn in_agent_run() -> bool {
    std::env::var_os("PUZ_SCRATCH").is_some_and(|v| !v.is_empty())
}

/// Where the keys are and which site is the default, for one run.
#[derive(Debug, Clone)]
pub struct Env {
    /// The swords state file. It holds the keys.
    pub state_file: PathBuf,
    /// `$SWORDS_SITE`, if set.
    pub site: Option<String>,
    /// Inside an AI agent's run: a publish or a take-down needs `confirmed` (the user said yes).
    pub agent: bool,
}

impl Env {
    /// From this process: `$SWORDS_HOME` (else `~/.config/swords`), `$SWORDS_SITE`, and the agent marks.
    pub fn from_process() -> Result<Env> {
        let state_file = state::state_file(std::env::var_os("SWORDS_HOME"), state::config_root())
            .context("can't find your home folder. Set SWORDS_HOME to the folder to keep your Swords keys in")?;
        Ok(Env { state_file, site: std::env::var("SWORDS_SITE").ok().filter(|s| !s.is_empty()), agent: in_agent_run() })
    }

    pub fn state(&self) -> Result<State> {
        State::load(&self.state_file)
    }

    /// The site a new publish goes to: `flag`, then `$SWORDS_SITE`, then the one saved with
    /// `puz swords site URL`, then the built-in one.
    pub fn site(&self, flag: Option<&str>) -> Result<String> {
        let saved = self.state().map(|s| s.site).unwrap_or_default();
        let site = state::choose_site(flag, self.site.as_deref(), &saved, DEFAULT_SITE);
        if !(site.starts_with("http://") || site.starts_with("https://")) {
            bail!("the site is an address that starts with http:// or https://, not {site:?}");
        }
        Ok(site)
    }
}

/// One link: the puzzle on one Swords site. Saved in the puzzle file. No key, no PIN.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// The site, with no slash at the end.
    pub site: String,
    /// The puzzle's id on that site.
    pub id: String,
    /// The address to solve it at. This is the link to share.
    pub url: String,
    /// The owner's page on the site (stats, PIN, take down). It needs the key or a sign-in.
    pub manage: String,
    /// When it was first pushed (an ISO time, UTC).
    pub first_pushed: String,
    /// When it was last pushed.
    pub last_pushed: String,
    /// A hash of the content last pushed (`content_hash`), to tell when the pushed copy is out of date.
    pub hash: String,
    /// Solvers need a PIN to open it. The PIN itself is never saved.
    #[serde(default)]
    pub pin: bool,
    /// Fields a newer version wrote: kept as they are.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The links saved in a puzzle. A record this version can't read is skipped (and kept in the file).
pub fn links(p: &Puzzle) -> Vec<Link> {
    p.swords.iter().filter_map(|v| serde_json::from_value::<Link>(v.clone()).ok()).filter(|l| !l.site.is_empty() && !l.id.is_empty()).collect()
}

fn same(v: &Value, site: &str, id: &str) -> bool {
    v["site"] == site && v["id"] == id
}

/// Save a link in the puzzle: it replaces the record for the same site and id, or the record
/// named by `replaces` (an older id on that site), else it is added.
pub fn put_link(p: &mut Puzzle, link: &Link, replaces: Option<&str>) -> Result<()> {
    let v = serde_json::to_value(link)?;
    match p.swords.iter().position(|x| same(x, &link.site, &link.id) || replaces.is_some_and(|id| same(x, &link.site, id))) {
        Some(i) => p.swords[i] = v,
        None => p.swords.push(v),
    }
    Ok(())
}

/// What stops a puzzle from being published: empty squares, answers with no clue. Empty when it is ready.
pub fn problems(p: &Puzzle) -> Vec<String> {
    let mut out = vec![];
    let empty = p.grid.iter().flat_map(|r| r.chars()).filter(|c| *c == '.').count();
    if empty > 0 {
        out.push(format!("{empty} empty square(s)"));
    }
    let unclued: Vec<String> = p.slots().iter().filter(|s| p.picture_for(s).is_none() && p.clue_for(s).is_none_or(|c| crate::markup::plain(c).trim().is_empty())).map(crate::edit::label).collect();
    if !unclued.is_empty() {
        out.push(format!("{} answer(s) with no clue ({})", unclued.len(), unclued.iter().take(6).cloned().collect::<Vec<_>>().join(", ")));
    }
    out
}

/// What is sent: the puzzle with an empty author filled in with `author` (the user's name), and
/// without what is only for the constructor (these links, saved submission answers, own fields,
/// locks, the finished and published marks, the target paper). Picture clues the grid shows go as
/// `images` and `imageAlts`, keyed by slot ("x,y,across"), the way a Swords site reads them; ones no
/// slot shows are not sent.
pub fn payload(p: &Puzzle, author: &str) -> Result<Vec<u8>> {
    let mut c = p.clone();
    if c.author.trim().is_empty() {
        c.author = author.trim().to_string();
    }
    c.swords.clear();
    c.submission.clear();
    c.meta.clear();
    c.locked.clear();
    c.finished = false;
    c.published = None;
    c.target = None;
    crate::pictures::send_bytes(&c)
}

/// A short hash of pushed content (FNV-1a, 64 bits, as hex). It only has to notice a change, and
/// it must give the same value on every computer and in every version.
pub fn content_hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// One link for a list: enough to show "on Swords" and "out of date".
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Brief {
    pub site: String,
    pub url: String,
    pub last_pushed: String,
    pub pin: bool,
    /// The puzzle changed since it was last pushed.
    pub out_of_date: bool,
}

/// The puzzle's links, for a list of puzzles. Empty (and no work done) for a puzzle with none.
pub fn briefs(p: &Puzzle, author: &str) -> Vec<Brief> {
    if p.swords.is_empty() {
        return vec![];
    }
    let now = payload(p, author).map(|b| content_hash(&b)).unwrap_or_default();
    links(p).into_iter().map(|l| Brief { out_of_date: l.hash != now, site: l.site, url: l.url, last_pushed: l.last_pushed, pin: l.pin }).collect()
}

/// What the site says about a link now (`status --live`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Live {
    /// The site has no such puzzle (it was taken down).
    pub gone: bool,
    /// The site did not answer, or said no. None when all is well.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starts: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solves: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LinkStatus {
    #[serde(flatten)]
    pub link: Link,
    /// The puzzle changed since it was last pushed: the copy on the site is old.
    pub out_of_date: bool,
    /// This computer has the key that owns the pushed puzzle (in the swords state file).
    pub key_here: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<Live>,
}

/// Where a puzzle stands on Swords.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub title: String,
    /// The site a publish goes to when none is named.
    pub site: String,
    pub links: Vec<LinkStatus>,
    /// What stops a publish: empty squares, answers with no clue. Empty when it can be published.
    pub problems: Vec<String>,
}

/// Where a puzzle stands on Swords. `author` is the user's name (it is part of what is sent, so of
/// the hash). It asks the site only with `live`, and never in an agent run.
pub fn status(p: &Puzzle, author: &str, env: &Env, site_flag: Option<&str>, live: bool) -> Result<Status> {
    if live && env.agent {
        bail!("An AI agent may not ask the Swords site: leave out --live. The saved links are in `puz swords status PUZZLE`.");
    }
    let st = env.state().unwrap_or_default();
    let now = content_hash(&payload(p, author)?);
    let links = links(p)
        .into_iter()
        .map(|link| {
            let key = key_for(&st, &link.site, &link.id);
            let live = live.then(|| ask_live(&st, &link, key));
            LinkStatus { out_of_date: link.hash != now, key_here: !key.is_empty(), live, link }
        })
        .collect();
    Ok(Status { title: p.title.clone(), site: env.site(site_flag)?, links, problems: problems(p) })
}

fn ask_live(st: &State, link: &Link, key: &str) -> Live {
    let none = Live { gone: false, error: None, opens: None, starts: None, solves: None };
    match api::request(Method::Get, &format!("{}/api/puzzles/{}?brief=1", link.site, link.id), share::proof(st, &link.site, key), Body::None) {
        Err(e) if e.status == Some(404) => Live { gone: true, ..none },
        Err(e) => Live { error: Some(site_words(&share::Error::Site(e))), ..none },
        Ok(v) => {
            let count = |k: &str| v.get("stats").and_then(|s| s.get(k)).and_then(|n| n.as_f64()).map(|n| n as u64);
            Live { opens: count("opens"), starts: count("starts"), solves: count("solves"), ..none }
        }
    }
}

/// The link a command means: the one on `site_flag`, else the only one, else the one on the default site.
pub fn pick(links: &[Link], site_flag: Option<&str>, default_site: &str) -> Result<Link> {
    if links.is_empty() {
        bail!("this puzzle is not on Swords. Publish it with: puz publish PUZZLE");
    }
    let on = |site: &str| links.iter().rev().find(|l| l.site == site.trim_end_matches('/')).cloned();
    match site_flag {
        Some(s) => on(s).with_context(|| format!("this puzzle has no link on {s}. It is on: {}", sites(links))),
        None if links.len() == 1 => Ok(links[0].clone()),
        None => on(default_site).with_context(|| format!("this puzzle is on more than one site ({}). Name one with --site", sites(links))),
    }
}

fn sites(links: &[Link]) -> String {
    links.iter().map(|l| l.site.as_str()).collect::<Vec<_>>().join(", ")
}

/// The key this computer holds for a pushed puzzle. Empty when it has none.
fn key_for<'a>(st: &'a State, site: &str, id: &str) -> &'a str {
    share::key(st, site, id).unwrap_or("")
}

/// A failed call in plain words: the site's own words when it sent any.
pub fn site_words(e: &share::Error) -> String {
    match e {
        share::Error::Site(api::Error { url, status: None, .. }) => format!("can't reach {}. Is the site running?", swords_client::util::origin(url)),
        share::Error::Site(api::Error { status: Some(s), message, .. }) | share::Error::PinNotSet { cause: api::Error { status: Some(s), message, .. }, .. } => message.clone().unwrap_or_else(|| format!("the site answered {s}")),
        other => format!("{other}"),
    }
}

fn no_key(link: &Link) -> String {
    format!(
        "this computer has no key for {} (it was published from another computer, or the swords state file is gone). \
Sign in on that site with `puz swords --site {} login EMAIL`, or claim it with `puz swords claim LINK` (the owner link from the other computer), then publish again",
        link.url, link.site
    )
}

#[derive(Debug, Clone, Default)]
pub struct PushOptions {
    /// The site to publish to [default: `Env::site`].
    pub site: Option<String>,
    /// Set this PIN on the published puzzle (4 to 12 characters).
    pub pin: Option<String>,
    /// Take the PIN off.
    pub no_pin: bool,
    /// Forget the link remembered for this site and make a new one.
    pub new: bool,
    /// The user agreed to this publish (`--confirmed`). An agent run needs it.
    pub confirmed: bool,
    /// The author to use when the puzzle names none. Empty: none.
    pub author: String,
}

/// A publish that reached the site. `remember` saves it in the puzzle file.
#[derive(Debug, Clone, Serialize)]
pub struct Sent {
    #[serde(flatten)]
    pub link: Link,
    pub title: String,
    /// A new link was made (false: the link it had was updated).
    pub created: bool,
    /// The link came from an earlier publish of this same file path (recorded in the state file
    /// only, as the old `swords push` did) and is remembered in the puzzle from now on.
    pub adopted: bool,
    /// The content was the same as the last publish (it was sent all the same).
    pub unchanged: bool,
    /// The link is saved in the puzzle file (a puz puzzle). False for other formats.
    pub recorded: bool,
    /// The site's reply to the publish (and to the PIN call, when there was one). No key in it is shown.
    #[serde(skip)]
    pub reply: swords_client::json::Json,
    /// The id this link takes the place of in the puzzle file (`--new`).
    #[serde(skip)]
    replaces: Option<String>,
}

fn full_path(file: &Path) -> String {
    file.canonicalize().unwrap_or_else(|_| file.to_path_buf()).to_string_lossy().into_owned()
}

/// A puzzle file and what is sent for it.
enum Source {
    /// A puz puzzle (.json): the payload is sent, and the link is kept in the file.
    Puz(Box<Puzzle>),
    /// Any other file the site reads: sent as it is. `title` from the file name.
    Other { title: String },
}

/// Read a file to publish. A `.json` that is a puz puzzle is one, and so is a `.ipuz` puz saved; any other file is sent as it is
/// (the site reads .puz, .jpz, .ipuz and its own .swords). A `.puz` is read here too, to check it is ready.
fn source(file: &Path) -> Result<(Source, Vec<u8>)> {
    if !file.exists() {
        bail!("no file {}", file.display());
    }
    let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    // A `.ipuz` that puz saved (it has puz's own `cx.swords:puzzle` field) is a puz puzzle too, so its link is
    // kept in it. Any other app's .ipuz is sent as it is and left unchanged.
    let ours = ext == "json" || (ext == "ipuz" && std::fs::read_to_string(file).is_ok_and(|t| t.contains("\"cx.swords:puzzle\"")));
    if ours
        && let Ok(p) = Puzzle::load(file)
    {
        return Ok((Source::Puz(Box::new(p)), vec![]));
    }
    let bytes = std::fs::read(file).with_context(|| format!("can't read {}", file.display()))?;
    if ext == "puz" {
        let p = crate::formats::from_puz(&bytes).with_context(|| format!("{} is not a .puz file puz can read", file.display()))?;
        let why = problems(&p);
        if !why.is_empty() {
            bail!("can't publish yet: {}", why.join("; "));
        }
    }
    let title = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Ok((Source::Other { title }, bytes))
}

/// The puzzle in `file` when it can hold Swords links. An Across Lite `.puz` can't (puz never
/// writes one), so its link lives in the state file, like any other app's file.
pub fn load_own(file: &Path) -> Result<Puzzle> {
    if crate::files::is_across_lite(file) {
        bail!("{} is a .puz file: it keeps no links", file.display());
    }
    Puzzle::load(file)
}

/// Publish the puzzle in `file` to a Swords site: update the link it has there, or make one. This
/// saves the key in the swords state file, and nothing in the puzzle file: call `remember` next.
pub fn send(file: &Path, env: &Env, o: &PushOptions) -> Result<Sent> {
    if env.agent && !o.confirmed {
        bail!(NOT_CONFIRMED);
    }
    let pin = match (&o.pin, o.no_pin) {
        (Some(_), true) => bail!("give a PIN or --no-pin, not both"),
        (Some(pin), false) => Some(swords_client::commands::valid_pin(pin).map_err(|e| anyhow::anyhow!("{e}"))?.to_string()),
        (None, _) => None,
    };
    let (src, raw) = source(file)?;
    if let Source::Puz(p) = &src {
        let why = problems(p);
        if !why.is_empty() {
            bail!("can't publish yet: {}", why.join("; "));
        }
    }
    let site = env.site(o.site.as_deref())?;
    let (body, title) = match &src {
        Source::Puz(p) => (payload(p, &o.author)?, p.title.clone()),
        Source::Other { title } => (raw, title.clone()),
    };
    let hash = content_hash(&body);
    let now = swords_client::util::now_iso();
    let mut st = env.state()?;
    let path = full_path(file);

    let remembered = match &src {
        Source::Puz(p) => links(p).into_iter().rev().find(|l| l.site == site),
        Source::Other { .. } => None,
    };
    let replaces = remembered.as_ref().filter(|_| o.new).map(|l| l.id.clone());
    // No link in the puzzle yet: one made earlier from this same file path counts.
    let adopted = match (&remembered, o.new) {
        (None, false) => swords_client::commands::pushed_before(&st, &path, &site).or_else(|| swords_client::commands::pushed_before(&st, &file.to_string_lossy(), &site)).map(|i| {
            let q = &st.puzzles[i];
            let first = q.pushed.as_ref().and_then(Value::as_str).unwrap_or(&now).to_string();
            Link { site: site.clone(), id: q.id.clone(), first_pushed: first, ..Link::default() }
        }),
        _ => None,
    };
    let old = remembered.filter(|_| !o.new).or(adopted.clone());

    let author = Some(o.author.trim()).filter(|a| !a.is_empty());
    let req = share::Push {
        site: &site,
        body: &body,
        file: &path,
        update: old.as_ref().map(|l| l.id.as_str()),
        new: false,
        pin: match (&pin, o.no_pin) {
            (Some(pin), _) => share::Pin::Set(pin),
            (None, true) => share::Pin::Remove,
            (None, false) => share::Pin::Keep,
        },
        author,
    };
    let key_here = old.as_ref().is_some_and(|l| share::key(&st, &site, &l.id).is_some());
    let (pushed, pin_failed) = match share::push(&mut st, &env.state_file, &req) {
        Ok(pushed) => (pushed, None),
        // The publish itself worked: the link is remembered below, then the PIN error is given.
        Err(share::Error::PinNotSet { pushed, cause }) => (*pushed, Some(share::Error::Site(cause))),
        Err(e) => match (&old, e.status()) {
            (Some(link), Some(404)) => bail!("{} is no longer on the site (it was taken down). To make a new link: puz publish PUZZLE --new", if link.url.is_empty() { &link.id } else { &link.url }),
            (Some(link), Some(401 | 403)) if !key_here => bail!("{}", no_key(link)),
            _ => bail!("{}", site_words(&e)),
        },
    };
    let created = pushed.outcome == share::Outcome::Created;
    let mut link = match old {
        Some(link) => link,
        None => Link { site: site.clone(), id: pushed.id.clone(), first_pushed: now.clone(), ..Link::default() },
    };
    link.pin = match (pushed.pin_changed, created) {
        (true, _) => pushed.locked.unwrap_or(pin.is_some()),
        (false, false) => pushed.locked.unwrap_or(link.pin),
        (false, true) => link.pin,
    };
    let unchanged = !created && link.hash == hash;
    link.url = pushed.url.clone();
    link.manage = pushed.manage_url.clone();
    link.last_pushed = now;
    link.hash = hash;
    let title = pushed.reply.get("title").and_then(|t| t.as_str()).filter(|t| !t.is_empty()).map_or(title, str::to_string);
    let reply = pushed.pin_reply.clone().map_or(pushed.reply.clone(), |mut r| {
        // The publish's reply holds the key and the claim link; keep them for the JSON result.
        for k in ["key", "manage", "email", "author", "image"] {
            if let Some(v) = pushed.reply.get(k) {
                r.set(k, v.clone());
            }
        }
        r
    });
    let sent = Sent { title, created, adopted: adopted.is_some(), unchanged, recorded: matches!(src, Source::Puz(_)), reply, replaces, link };
    if let Some(e) = pin_failed {
        remember(file, &sent)?;
        bail!("the puzzle is published ({}), but its PIN is not changed: {}", sent.link.url, site_words(&e));
    }
    Ok(sent)
}

/// Save a publish's link in the puzzle file (a puz puzzle only). The file is read again, so an edit
/// made during the publish stays.
pub fn remember(file: &Path, sent: &Sent) -> Result<()> {
    if !sent.recorded {
        return Ok(());
    }
    let mut p = Puzzle::load(file)?;
    put_link(&mut p, &sent.link, sent.replaces.as_deref())?;
    p.save(file)
}

/// Publish and remember: `puz publish`.
pub fn push(file: &Path, env: &Env, o: &PushOptions) -> Result<Sent> {
    let sent = send(file, env, o)?;
    remember(file, &sent)?;
    Ok(sent)
}

/// The link a file names on a site, for the `puz swords` commands that take a PUZZLE: the record in
/// a puz puzzle, else the earlier publish of this file path in the state file. None when there is none.
pub fn link_for_file(file: &Path, env: &Env, site_flag: Option<&str>) -> Option<Link> {
    if !file.is_file() {
        return None;
    }
    if let Ok(p) = load_own(file) {
        let all = links(&p);
        if !all.is_empty() {
            return pick(&all, site_flag, &env.site(None).ok()?).ok();
        }
    }
    let st = env.state().ok()?;
    let site = env.site(site_flag).ok()?;
    swords_client::commands::pushed_before(&st, &full_path(file), &site).map(|i| Link { site: site.clone(), id: st.puzzles[i].id.clone(), ..Link::default() })
}

/// Set the PIN mark of a link in the puzzle file, after the PIN was changed on the site.
pub fn mark_pin(file: &Path, site: &str, id: &str, pin: bool) -> Result<()> {
    let Ok(mut p) = load_own(file) else { return Ok(()) };
    let mut changed = false;
    for v in p.swords.iter_mut().filter(|v| same(v, site, id)) {
        v["pin"] = json!(pin);
        changed = true;
    }
    if changed { p.save(file) } else { Ok(()) }
}

/// Take the puzzle in `file` off a Swords site for good: the link stops working and its stats go.
/// A link the site no longer has counts as taken down. Call `forget` next. `confirmed`: the user
/// agreed (an agent run needs it).
pub fn take_down(file: &Path, env: &Env, site_flag: Option<&str>, confirmed: bool) -> Result<Link> {
    if env.agent && !confirmed {
        bail!(NOT_CONFIRMED);
    }
    let p = load_own(file)?;
    let link = pick(&links(&p), site_flag, &env.site(None)?)?;
    let mut st = env.state()?;
    let key_here = share::key(&st, &link.site, &link.id).is_some();
    match share::delete(&mut st, &env.state_file, &link.site, &link.id) {
        Ok(_) => {}
        // Not on the site any more: taken down all the same, so the key goes too.
        Err(e) if e.status() == Some(404) => {
            share::forget(&mut st, &env.state_file, &link.site, &link.id).map_err(share::Error::into_anyhow)?;
        }
        Err(e) if matches!(e.status(), Some(401 | 403)) && !key_here => bail!("{}", no_key(&link)),
        Err(e) => bail!("{}", site_words(&e)),
    }
    Ok(link)
}

/// Remove a link from the puzzle file. A file that is not a puz puzzle has none: nothing to do.
pub fn forget(file: &Path, site: &str, id: &str) -> Result<()> {
    let Ok(mut p) = load_own(file) else { return Ok(()) };
    let before = p.swords.len();
    p.swords.retain(|v| !same(v, site, id));
    if p.swords.len() != before { p.save(file) } else { Ok(()) }
}

/// Take down and forget.
pub fn unpublish(file: &Path, env: &Env, site_flag: Option<&str>, confirmed: bool) -> Result<Link> {
    let link = take_down(file, env, site_flag, confirmed)?;
    forget(file, &link.site, &link.id)?;
    Ok(link)
}

/// The words for one link in a status report.
pub fn link_text(l: &LinkStatus) -> String {
    let mut out = format!("  {}{}\n    last published {}{}", l.link.url, if l.link.pin { " [PIN]" } else { "" }, l.link.last_pushed, if l.link.first_pushed != l.link.last_pushed { format!(" (first {})", l.link.first_pushed) } else { String::new() });
    out += if l.out_of_date { "\n    OUT OF DATE: the puzzle changed since then. Update the link with: puz publish PUZZLE" } else { "\n    up to date" };
    if !l.key_here {
        out += "\n    this computer has no key for it";
    }
    match &l.live {
        Some(Live { gone: true, .. }) => out += "\n    not on the site any more (taken down)",
        Some(Live { error: Some(e), .. }) => out += &format!("\n    the site: {e}"),
        Some(v) => out += &format!("\n    {} opened · {} started · {} solved", v.opens.unwrap_or(0), v.starts.unwrap_or(0), v.solves.unwrap_or(0)),
        None => {}
    }
    out
}

/// `status` as JSON, with the puzzle's path.
pub fn status_json(path: &str, s: &Status) -> Value {
    let mut v = serde_json::to_value(s).unwrap_or_default();
    v["path"] = json!(path);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use swords_client::api::Proof;

    fn filled() -> Puzzle {
        let mut p = Puzzle::blank("Tiny", 3, 3, "rotational");
        p.grid = vec!["CAT".into(), "ARE".into(), "BEN".into()];
        for s in p.slots() {
            let key = p.clue_key(&s);
            p.clues.insert(key, "A clue".into());
        }
        p
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("puz-core-swords-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A tiny Swords site: publish, update, PIN, read, take down. It records each call.
    fn fake_site() -> (String, Arc<Mutex<Vec<String>>>) {
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
                reader.read_line(&mut line).unwrap();
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
                log.lock().unwrap().push(format!("{method} {url}"));
                let parts: Vec<&str> = url.trim_start_matches("/api/puzzles").split('/').filter(|s| !s.is_empty()).collect();
                let (status, v) = match (method.as_str(), parts.as_slice()) {
                    ("POST", []) => {
                        n += 1;
                        let id = format!("abcd000{n}");
                        live.insert(id.clone(), (format!("Bearer key-{id}"), false));
                        (201, json!({ "id": id, "title": "Tiny", "url": format!("{origin}/p/{id}/"), "key": format!("key-{id}"), "manage": format!("{origin}/p/{id}/manage#key-{id}"), "locked": false }))
                    }
                    (_, [id, rest @ ..]) => match live.get(*id).cloned() {
                        None => (404, json!({ "error": "No puzzle here." })),
                        Some((key, _)) if key != auth => (403, json!({ "error": "This puzzle is not yours." })),
                        Some((key, locked)) => {
                            let view = |locked: bool| json!({ "id": id, "title": "Tiny", "url": format!("{origin}/p/{id}/"), "locked": locked, "updated": "2026-01-01T00:00:00.000Z", "stats": { "opens": 3, "starts": 2, "solves": 1 } });
                            match (method.as_str(), rest) {
                                ("PUT", []) | ("GET", []) => (200, view(locked)),
                                ("POST", ["pin"]) => {
                                    let on = !serde_json::from_slice::<Value>(&body).unwrap()["pin"].is_null();
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

    #[test]
    fn picture_clues_are_sent_by_slot() {
        let mut p = Puzzle::blank("T", 3, 3, "none");
        p.grid = vec!["CAT".into(), "ARE".into(), "BED".into()];
        p.tidy();
        crate::edit::set_picture(&mut p, "1a", Some("data:image/gif;base64,R0lGODlhAQABAAAAACw="), Some("A cat")).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&payload(&p, "").unwrap()).unwrap();
        assert!(v["images"]["0,0,across"].as_str().unwrap().starts_with("data:image/gif;base64,"));
        assert_eq!(v["imageAlts"]["0,0,across"], "A cat");
        assert!(v.get("pictures").is_none());
    }

    #[test]
    fn hash_is_fixed_and_ignores_what_is_not_sent() {
        // FNV-1a test vectors: the value must never change between versions.
        assert_eq!(content_hash(b""), "cbf29ce484222325");
        assert_eq!(content_hash(b"a"), "af63dc4c8601ec8c");
        let p = filled();
        let h = content_hash(&payload(&p, "").unwrap());
        let mut q = p.clone();
        q.finished = true;
        q.published = Some("2026-01-01".into());
        q.meta.insert("Series".into(), "One".into());
        q.locked.insert("0,0".into());
        q.target = Some("nyt-daily".into());
        q.submission.insert("nyt".into(), json!({ "name": "x" }));
        q.swords.push(json!({ "site": "https://s", "id": "abcd2345" }));
        assert_eq!(content_hash(&payload(&q, "").unwrap()), h);
        let text = String::from_utf8(payload(&q, "").unwrap()).unwrap();
        assert!(!text.contains("swords") && !text.contains("Series") && !text.contains("submission"));
        q.clues.insert("CAT".into(), "Pet".into());
        assert_ne!(content_hash(&payload(&q, "").unwrap()), h);
        // The user's name fills an empty author only.
        assert!(String::from_utf8(payload(&p, "Ada L").unwrap()).unwrap().contains("Ada L"));
        let mut named = p.clone();
        named.author = "Zoë".into();
        assert!(!String::from_utf8(payload(&named, "Ada L").unwrap()).unwrap().contains("Ada L"));
    }

    #[test]
    fn picks_a_link() {
        let l = |site: &str, id: &str| Link { site: site.into(), id: id.into(), ..Link::default() };
        assert!(pick(&[], None, "https://a").unwrap_err().to_string().contains("puz publish PUZZLE"));
        let one = [l("https://a", "aaaa1111")];
        assert_eq!(pick(&one, None, "https://other").unwrap().id, "aaaa1111");
        let two = [l("https://a", "aaaa1111"), l("https://b", "bbbb2222")];
        assert_eq!(pick(&two, None, "https://b").unwrap().id, "bbbb2222");
        assert_eq!(pick(&two, Some("https://a/"), "https://b").unwrap().id, "aaaa1111");
        assert!(pick(&two, None, "https://c").unwrap_err().to_string().contains("more than one site"));
        assert!(pick(&two, Some("https://c"), "https://b").is_err());
    }

    #[test]
    fn a_record_this_version_cannot_read_is_kept() {
        let mut p = filled();
        p.swords = vec![json!("odd"), json!({ "site": "https://a", "id": "aaaa1111", "url": "u", "manage": "m", "first_pushed": "t", "last_pushed": "t", "hash": "h", "later": 1 })];
        assert_eq!(links(&p).len(), 1);
        let mut l = links(&p).remove(0);
        l.hash = "h2".into();
        put_link(&mut p, &l, None).unwrap();
        assert_eq!(p.swords.len(), 2);
        assert_eq!(p.swords[0], json!("odd"));
        assert_eq!((&p.swords[1]["hash"], &p.swords[1]["later"]), (&json!("h2"), &json!(1)));
    }

    #[test]
    fn publish_remembers_the_link_and_updates_it_after_a_rename() {
        let (site, calls) = fake_site();
        let dir = temp("push");
        let env = Env { state_file: dir.join("home/puzzles.json"), site: Some(site.clone()), agent: false };
        let file = dir.join("tiny.json");
        filled().save(&file).unwrap();

        let first = push(&file, &env, &PushOptions::default()).unwrap();
        assert!(first.created && !first.unchanged && !first.adopted && first.recorded);
        assert_eq!(first.link.url, format!("{site}/p/abcd0001/"));
        assert_eq!(first.link.manage, format!("{site}/p/abcd0001/manage"));
        // The puzzle file holds the link and no key; the state file holds the key.
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("abcd0001") && !text.contains("key-abcd0001"));
        assert!(std::fs::read_to_string(&env.state_file).unwrap().contains("key-abcd0001"));
        let s = status(&Puzzle::load(&file).unwrap(), "", &env, None, false).unwrap();
        assert_eq!((s.links.len(), s.links[0].out_of_date, s.links[0].key_here), (1, false, true));

        // The same content again: the same link.
        let again = push(&file, &env, &PushOptions::default()).unwrap();
        assert!(!again.created && again.unchanged);
        assert_eq!(again.link.id, "abcd0001");
        assert_eq!(again.link.first_pushed, first.link.first_pushed);

        // An edit: out of date. A finished mark or the link itself: not.
        let mut p = Puzzle::load(&file).unwrap();
        p.finished = true;
        p.save(&file).unwrap();
        assert!(!status(&Puzzle::load(&file).unwrap(), "", &env, None, false).unwrap().links[0].out_of_date);
        p.clues.insert("CAT".into(), "Pet that purrs".into());
        p.save(&file).unwrap();
        assert!(status(&p, "", &env, None, false).unwrap().links[0].out_of_date);
        assert!(briefs(&p, "")[0].out_of_date);

        // Renamed and moved: the publish still updates the same link, by its id.
        std::fs::create_dir_all(dir.join("My Series")).unwrap();
        let moved = dir.join("My Series/new name.json");
        std::fs::rename(&file, &moved).unwrap();
        assert_eq!(link_for_file(&moved, &env, None).unwrap().id, "abcd0001");
        let third = push(&moved, &env, &PushOptions { pin: Some("4821".into()), ..Default::default() }).unwrap();
        assert!(!third.created && !third.unchanged && third.link.pin);
        assert_eq!(third.link.id, "abcd0001");
        let p = Puzzle::load(&moved).unwrap();
        assert_eq!(links(&p).len(), 1);
        assert!(!status(&p, "", &env, None, false).unwrap().links[0].out_of_date);
        assert!(!std::fs::read_to_string(&moved).unwrap().contains("4821"));
        let live = status(&p, "", &env, None, true).unwrap().links[0].live.clone().unwrap();
        assert_eq!((live.gone, live.opens, live.solves), (false, Some(3), Some(1)));
        // The state file's list follows the file.
        assert!(std::fs::read_to_string(&env.state_file).unwrap().contains("new name.json"));
        mark_pin(&moved, &site, "abcd0001", false).unwrap();
        assert!(!links(&Puzzle::load(&moved).unwrap())[0].pin);

        // An agent may read the saved status. It publishes or takes down only with confirmed.
        let agent = Env { agent: true, ..env.clone() };
        assert_eq!(status(&p, "", &agent, None, false).unwrap().links.len(), 1);
        assert!(status(&p, "", &agent, None, true).is_err());
        let before = calls.lock().unwrap().len();
        assert!(push(&moved, &agent, &PushOptions::default()).unwrap_err().to_string().contains("only after the user agrees"));
        assert!(unpublish(&moved, &agent, None, false).unwrap_err().to_string().contains("only after the user agrees"));
        assert_eq!(calls.lock().unwrap().len(), before);
        push(&moved, &agent, &PushOptions { confirmed: true, ..Default::default() }).unwrap();
        assert!(calls.lock().unwrap().len() > before);

        // Take it down: the link leaves the puzzle file and the key leaves the state file.
        let gone = unpublish(&moved, &env, None, false).unwrap();
        assert_eq!(gone.id, "abcd0001");
        assert!(links(&Puzzle::load(&moved).unwrap()).is_empty());
        assert!(!std::fs::read_to_string(&env.state_file).unwrap().contains("key-abcd0001"));
        assert_eq!(calls.lock().unwrap().iter().filter(|c| c.starts_with("POST /api/puzzles") && !c.ends_with("/pin")).count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn publish_refuses_an_unfinished_puzzle_and_explains_a_missing_key_or_link() {
        let (site, calls) = fake_site();
        let dir = temp("refuse");
        let env = Env { state_file: dir.join("home/puzzles.json"), site: Some(site.clone()), agent: false };
        let file = dir.join("tiny.json");
        let mut p = filled();
        p.grid[0] = "C.T".into();
        p.save(&file).unwrap();
        assert!(push(&file, &env, &PushOptions::default()).unwrap_err().to_string().contains("can't publish yet"));
        assert!(calls.lock().unwrap().is_empty());

        filled().save(&file).unwrap();
        push(&file, &env, &PushOptions::default()).unwrap();
        // Another computer: the file came along, the key did not.
        let other = Env { state_file: dir.join("other/puzzles.json"), ..env.clone() };
        let s = status(&Puzzle::load(&file).unwrap(), "", &other, None, false).unwrap();
        assert!(!s.links[0].key_here);
        assert!(push(&file, &other, &PushOptions::default()).unwrap_err().to_string().contains("has no key"));
        // Taken down on the site: the publish says so, and --new makes a new link in the old one's place.
        api::send(Method::Delete, &format!("{site}/api/puzzles/abcd0001"), Proof { key: Some("key-abcd0001"), session: None, author: None }, Body::None).unwrap();
        assert!(push(&file, &env, &PushOptions::default()).unwrap_err().to_string().contains("--new"));
        let fresh = push(&file, &env, &PushOptions { new: true, ..Default::default() }).unwrap();
        assert!(fresh.created);
        let now = links(&Puzzle::load(&file).unwrap());
        assert_eq!((now.len(), now[0].id.as_str()), (1, "abcd0002"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn other_formats_are_sent_as_they_are_and_found_by_path() {
        let (site, _) = fake_site();
        let dir = temp("other");
        let env = Env { state_file: dir.join("home/puzzles.json"), site: Some(site.clone()), agent: false };
        let file = dir.join("tiny.puz");
        std::fs::write(&file, crate::formats::to_puz(&filled()).unwrap()).unwrap();
        let first = push(&file, &env, &PushOptions::default()).unwrap();
        assert!(first.created && !first.recorded);
        assert_eq!(std::fs::read(&file).unwrap(), crate::formats::to_puz(&filled()).unwrap());
        // The second publish of the same path updates the same link.
        let again = push(&file, &env, &PushOptions::default()).unwrap();
        assert!(!again.created && again.adopted);
        assert_eq!(again.link.id, first.link.id);
        assert_eq!(link_for_file(&file, &env, None).unwrap().id, first.link.id);
        // An unfinished .puz is refused before anything is sent.
        let mut p = filled();
        p.grid[1] = "A.E".into();
        std::fs::write(dir.join("half.puz"), crate::formats::to_puz(&p).unwrap()).unwrap();
        assert!(push(&dir.join("half.puz"), &env, &PushOptions::default()).unwrap_err().to_string().contains("can't publish yet"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_link_made_by_the_old_swords_command_is_taken_over() {
        let (site, _) = fake_site();
        let dir = temp("adopt");
        let env = Env { state_file: dir.join("home/puzzles.json"), site: Some(site.clone()), agent: false };
        let file = dir.join("tiny.json");
        filled().save(&file).unwrap();
        // What `swords push tiny.json` left behind: a key by file path, nothing in the puzzle.
        let made = api::send(Method::Post, &format!("{site}/api/puzzles"), Proof::default(), Body::Bytes(b"{}")).unwrap().json();
        let st = State { puzzles: vec![state::Puzzle { id: made.text("id"), site: site.clone(), key: made.text("key"), file: full_path(&file), pushed: Some("2026-01-02T03:04:05.000Z".into()), ..Default::default() }], ..Default::default() };
        st.save(&env.state_file).unwrap();
        let sent = push(&file, &env, &PushOptions::default()).unwrap();
        assert!(sent.adopted && !sent.created);
        assert_eq!((sent.link.id.as_str(), sent.link.first_pushed.as_str()), ("abcd0001", "2026-01-02T03:04:05.000Z"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
