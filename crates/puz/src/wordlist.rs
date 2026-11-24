//! The word list for the fill commands. puz has none built in. `puz wordlist install` downloads
//! Spread the Word(list) from its own site into the puz data folder; `puz wordlist use FILE` points
//! puz at a list of your own. A fill command that finds no list asks to download it (at a terminal).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use swords_client::style::Pen;

/// Where Spread the Word(list) is downloaded from. We keep no copy.
pub const URL: &str = "https://www.spreadthewordlist.com/downloads/spreadthewordlist.dict";
/// The question asked before a download.
pub const QUESTION: &str = "Install the Spread the Word(list) word list (about 4 MB, free for non-commercial use)?";
/// Said when a command needs a list and none is installed.
pub const NEEDED: &str = "This needs a word list. Run `puz wordlist install`.";
/// The credit, shown once on install.
pub const CREDIT: &str = "Word list: Spread the Word(list), https://www.spreadthewordlist.com\n\
Licensed CC BY-NC-SA 4.0 (https://creativecommons.org/licenses/by-nc-sa/4.0/): free for non-commercial use, with credit.";

const LIST_FILE: &str = "spreadthewordlist.dict";
const STATE_FILE: &str = "wordlist.json";

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Download Spread the Word(list) (about 4 MB) from spreadthewordlist.com. Asking for it by name
    /// is the yes: it does not ask first.
    Install {
        /// Kept so old scripts still work: install never asks
        #[arg(short, long, hide = true)]
        yes: bool,
        /// Ask, and skip quietly when there is no terminal or the list is already there (for install scripts)
        #[arg(long, hide = true)]
        offer: bool,
        #[arg(long)]
        json: bool,
    },
    /// Download Spread the Word(list) again, to get its newest version
    Update {
        #[arg(long)]
        json: bool,
    },
    /// Show the word list in use: its file, size, number of entries and download date
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Delete the downloaded list and forget a list set with `use`
    Remove {
        #[arg(long)]
        json: bool,
    },
    /// Use your own word list file in place of Spread the Word(list): one `word;score` per line,
    /// score 0 to 100. puz reads the file where it is, so later edits count.
    Use {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

/// What is saved in `wordlist.json` in the data folder.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// When Spread the Word(list) was downloaded (seconds since 1970).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    downloaded: Option<u64>,
    /// A list of your own (`puz wordlist use`), used in place of the download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom: Option<PathBuf>,
}

/// The puz data folder: $PUZ_DATA_DIR, else $XDG_DATA_HOME/puz, else ~/.local/share/puz
/// (%LOCALAPPDATA%\puz on Windows).
pub fn data_dir() -> PathBuf {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(d) = var("PUZ_DATA_DIR") {
        return d;
    }
    if let Some(d) = var("XDG_DATA_HOME") {
        return d.join("puz");
    }
    if cfg!(windows) {
        return var("LOCALAPPDATA").or_else(|| var("APPDATA")).unwrap_or_default().join("puz");
    }
    var("HOME").unwrap_or_default().join(".local/share/puz")
}

fn load_state(dir: &Path) -> State {
    std::fs::read(dir.join(STATE_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_state(dir: &Path, s: &State) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(STATE_FILE), serde_json::to_string_pretty(s)? + "\n")?;
    Ok(())
}

/// The list file in use: your own (`use`), else the download. None when neither is there.
pub fn find() -> Option<PathBuf> {
    let dir = data_dir();
    let state = load_state(&dir);
    if let Some(c) = state.custom.filter(|c| c.is_file()) {
        return Some(c);
    }
    Some(dir.join(LIST_FILE)).filter(|f| f.is_file())
}

/// Load the list into the fill engine. With no list: at a terminal, ask to download it (`yes`
/// downloads without asking); otherwise fail with the `NEEDED` message.
pub fn ensure(yes: bool) -> Result<()> {
    if puz_fill::engine::has_default_wordlist() {
        return Ok(());
    }
    let file = match find() {
        Some(f) => f,
        None => {
            if !(yes || (can_ask() && confirm(QUESTION)?)) {
                bail!(NEEDED);
            }
            download(false)?;
            eprintln!();
            find().context(NEEDED)?
        }
    };
    let text = std::fs::read_to_string(&file).with_context(|| format!("reading the word list {}", file.display()))?;
    puz_fill::engine::set_default_wordlist(text);
    Ok(())
}

/// Can puz ask a question here? A terminal on stdin and stderr, and not an AI agent's run.
fn can_ask() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal() && !puz_core::swords::in_agent_run()
}

/// Ask a yes/no question on stderr; Enter means yes.
fn confirm(question: &str) -> Result<bool> {
    let pen = Pen::stderr();
    if pen.rich {
        eprint!("{} {} {} ", pen.bold(question), pen.dim("[Y/n]"), pen.prompt());
    } else {
        eprint!("{question} [Y/n] ");
    }
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let a = line.trim().to_ascii_lowercase();
    Ok(a.is_empty() || a.starts_with('y'))
}

/// Download Spread the Word(list) into the data folder and print the credit. Returns the file.
fn download(quiet: bool) -> Result<PathBuf> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("making {}", dir.display()))?;
    if !quiet {
        eprintln!("{}", Pen::stderr().dim(&format!("Downloading {URL} …")));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(format!("puz/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut reply = agent.get(URL).call().with_context(|| format!("Can't download the word list from {URL}"))?;
    let text = reply.body_mut().with_config().limit(64 * 1024 * 1024).read_to_string().context("reading the word list download")?;
    let entries = count_entries(&text);
    if entries < 1000 {
        bail!("{URL} did not send a word list (one `word;score` per line).");
    }
    let file = dir.join(LIST_FILE);
    let tmp = dir.join(format!("{LIST_FILE}.part"));
    std::fs::write(&tmp, &text)?;
    std::fs::rename(&tmp, &file)?;
    let mut state = load_state(&dir);
    state.downloaded = Some(now());
    save_state(&dir, &state)?;
    if !quiet {
        let pen = Pen::stderr();
        eprintln!("{}", pen.ok(&format!("Installed {} words.", thousands(entries))));
        eprintln!("{}", pen.note(&file.display().to_string()));
        eprintln!();
        for line in CREDIT.lines() {
            eprintln!("{}", pen.note(line));
        }
    }
    Ok(file)
}

/// Lines that look like `word;score`.
fn count_entries(text: &str) -> usize {
    text.lines().filter(|l| l.split_once(';').is_some_and(|(w, s)| !w.trim().is_empty() && s.trim().parse::<u32>().is_ok())).count()
}

/// How long ago, in days: "today", "yesterday", "12 days ago".
fn ago(secs: u64) -> String {
    match now().saturating_sub(secs) / 86_400 {
        0 => "today".into(),
        1 => "yesterday".into(),
        n => format!("{n} days ago"),
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A date as YYYY-MM-DD (UTC).
fn date(secs: u64) -> String {
    // Days since 1970 to a civil date (Howard Hinnant's algorithm).
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// What `status` reports.
#[derive(Serialize)]
struct Status {
    installed: bool,
    /// "spreadthewordlist" or "custom".
    source: Option<&'static str>,
    path: Option<PathBuf>,
    bytes: Option<u64>,
    entries: Option<usize>,
    downloaded: Option<String>,
    data_dir: PathBuf,
}

fn status() -> Status {
    let dir = data_dir();
    let state = load_state(&dir);
    let file = find();
    let custom = file.is_some() && file == state.custom;
    let text = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok());
    Status {
        installed: file.is_some(),
        source: file.as_ref().map(|_| if custom { "custom" } else { "spreadthewordlist" }),
        bytes: file.as_ref().and_then(|f| std::fs::metadata(f).ok()).map(|m| m.len()),
        entries: text.as_deref().map(count_entries),
        downloaded: if custom { None } else { file.as_ref().and(state.downloaded).map(|t| format!("{} ({})", date(t), ago(t))) },
        path: file,
        data_dir: dir,
    }
}

fn print_status(s: &Status) {
    let pen = Pen::stdout();
    let Some(path) = &s.path else {
        println!("{}", pen.caution("No word list installed."));
        println!("{}", pen.note("The fill commands (fill, autofill, check and more) need one."));
        println!("{}", pen.note("Spread the Word(list) is free for non-commercial use. About 4 MB."));
        println!();
        println!("{}", pen.next("Install it", "puz wordlist install"));
        return;
    };
    let mb = s.bytes.unwrap_or(0) as f64 / 1_000_000.0;
    let name = if s.source == Some("custom") { "Your own list (puz wordlist use)".to_string() } else { "Spread the Word(list), CC BY-NC-SA 4.0".to_string() };
    println!("{}", pen.ok(&pen.bold(&name)));
    let mut rows = vec![("File", path.display().to_string()), ("Size", format!("{mb:.1} MB, {} words", thousands(s.entries.unwrap_or(0))))];
    if let Some(d) = &s.downloaded {
        rows.push(("Downloaded", d.clone()));
    }
    println!("{}", indent(&pen.pairs(&rows)));
}

fn indent(text: &str) -> String {
    text.lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n")
}

/// 304410 as 304,410.
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn json_out(v: serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

/// Offer the download after puz is installed or updated: ask at a terminal when no list is there;
/// otherwise say how to install it later. Never fails.
pub fn offer() {
    if find().is_some() {
        return;
    }
    let later = "Skipped the word list. The fill commands need it; install it later with: puz wordlist install";
    if !can_ask() {
        eprintln!("{later}");
        return;
    }
    match confirm(QUESTION) {
        Ok(true) => {
            if let Err(e) = download(false) {
                eprintln!("{} {e:#}", Pen::stderr().caution("Could not install the word list:"));
                eprintln!("Try again later with: puz wordlist install");
            }
        }
        _ => eprintln!("{later}"),
    }
}

/// Run `puz wordlist ...`.
pub fn run(cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Install { offer: true, .. } => offer(),
        Cmd::Install { yes, json, .. } => {
            let have = find();
            if let Some(f) = have {
                if json {
                    return json_out(serde_json::to_value(status())?);
                }
                println!("A word list is already installed: {}. Get the newest with: puz wordlist update", f.display());
                return Ok(());
            }
            // Asking for it by name is the yes: no question here.
            let _ = yes;
            download(json)?;
            if json {
                json_out(serde_json::to_value(status())?)?;
            }
        }
        Cmd::Update { json } => {
            download(json)?;
            if json {
                return json_out(serde_json::to_value(status())?);
            }
            if load_state(&data_dir()).custom.is_some_and(|c| c.is_file()) {
                println!("Note: your own list (puz wordlist use) is still the one in use. `puz wordlist remove` goes back to Spread the Word(list).");
            }
        }
        Cmd::Status { json } => {
            let s = status();
            if json {
                return json_out(serde_json::to_value(&s)?);
            }
            print_status(&s);
        }
        Cmd::Remove { json } => {
            let dir = data_dir();
            let file = dir.join(LIST_FILE);
            let had = file.is_file();
            if had {
                std::fs::remove_file(&file)?;
            }
            let state = load_state(&dir);
            let had_custom = state.custom.is_some();
            if dir.join(STATE_FILE).is_file() {
                std::fs::remove_file(dir.join(STATE_FILE))?;
            }
            if json {
                return json_out(serde_json::json!({ "removed": had, "forgot_custom": had_custom }));
            }
            match (had, had_custom) {
                (false, false) => println!("No word list was installed."),
                _ => {
                    if had {
                        println!("Deleted {}.", file.display());
                    }
                    if had_custom {
                        println!("Forgot your own list (the file itself is untouched).");
                    }
                }
            }
        }
        Cmd::Use { file, json } => {
            let file = std::path::absolute(&file)?;
            let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            let entries = count_entries(&text);
            if entries == 0 {
                bail!("{} has no `word;score` lines.", file.display());
            }
            let dir = data_dir();
            let mut state = load_state(&dir);
            state.custom = Some(file.clone());
            save_state(&dir, &state)?;
            if json {
                return json_out(serde_json::to_value(status())?);
            }
            println!("Using {} ({entries} entries) as the word list.", file.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_counts() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(86_400 * 365), "1971-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(count_entries("cat;50\ndog;40\n\nnot a line\nbad;x\n"), 2);
    }
}
