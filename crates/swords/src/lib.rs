//! Swords sharing as a library: publish crosswords to a Swords site and manage the ones you published.
//!
//! There is no `swords` program any more. The `puz` command bundles this library: `puz publish`
//! pushes through [`share`], `puz setup` asks its questions through [`setup`], and the `puz swords`
//! group runs the commands in [`cli::Command`] through [`run`]. The keys stay in the state file
//! ([`state`]), so puzzles pushed with the old `swords` command keep working.

pub mod api;
pub mod cli;
pub mod commands;
pub mod json;
pub mod setup;
pub mod share;
pub mod state;
pub mod style;
pub mod util;

use anyhow::Result;
use cli::Command;
use std::io::Write;
use std::path::PathBuf;

/// This library's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The site to use when nothing else names one. Set `SWORDS_DEFAULT_SITE` at build time to change it.
pub const DEFAULT_SITE: &str = match option_env!("SWORDS_DEFAULT_SITE") {
    Some(site) if !site.is_empty() => site,
    _ => "https://swords.cx",
};

/// A mistake in how a command was used. It ends with exit code 2.
#[derive(Debug)]
pub struct Usage(pub String);

impl std::fmt::Display for Usage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Usage {}

/// Print a line of results. A closed pipe (`puz swords list | head`) is not an error.
pub fn say(text: &str) {
    let _ = writeln!(std::io::stdout(), "{text}");
}

/// Print a line for the person, apart from the results.
pub fn warn(text: &str) {
    let _ = writeln!(std::io::stderr(), "{text}");
}

/// The state file that holds the keys: `$SWORDS_HOME/puzzles.json`, else `~/.config/swords/puzzles.json`
/// (`%APPDATA%\swords\puzzles.json` on Windows).
pub fn state_file() -> Result<PathBuf> {
    state::state_file(std::env::var_os("SWORDS_HOME"), state::config_root()).ok_or_else(|| anyhow::anyhow!("Can't find your home folder. Set SWORDS_HOME to the folder to keep your keys in."))
}

/// The site in use: `flag` (`--site`), then `$SWORDS_SITE`, then the one saved in the state file, then
/// [`DEFAULT_SITE`]. With no slash at the end.
pub fn site(flag: Option<&str>, saved: &state::State) -> String {
    let env_site = std::env::var("SWORDS_SITE").ok();
    state::choose_site(flag, env_site.as_deref(), &saved.site, DEFAULT_SITE)
}

/// What a command needs to know, from this process. `name` is the person's name (the author of a
/// puzzle that names none), kept by the program that calls this.
pub fn context(site_flag: Option<&str>, json: bool, name: &str, state: &state::State) -> Result<commands::Ctx> {
    let cwd = std::env::current_dir().map_err(|_| anyhow::anyhow!("Can't tell which folder this is. Go to a folder that exists and try again."))?;
    Ok(commands::Ctx { site: site(site_flag, state), site_flag: site_flag.is_some_and(|s| !s.is_empty()), json, state_file: state_file()?, cwd, name: name.to_string() })
}

/// Run one `puz swords` command.
pub fn run(ctx: &commands::Ctx, state: &mut state::State, command: Command) -> Result<()> {
    commands::run(ctx, state, command)
}

/// Print a failure the way these commands do, and give its exit code: 2 for a usage mistake, else 1.
pub fn report(e: &anyhow::Error) -> i32 {
    warn(&style::Pen::stderr().error(&format!("{e:#}")));
    if e.is::<Usage>() { 2 } else { 1 }
}
