//! `puz update` (get the newest build of puz from the Swords site and put it in place) and the quiet
//! daily check that says when a newer puz is out.
//!
//! The site serves the builds at `/cli/puz-<os>-<arch>` (`puz-windows-x64.exe` for Windows) and the
//! newest version number at `/cli/version`.

use anyhow::{Context as _, Result, bail};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use swords_client::api::{self, Body, Method, Proof};
use swords_client::style::Pen;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often the quiet check asks the site: every 4 hours. Releases can come more than once a day,
/// and the ask runs in the background, so a shorter wait costs nothing.
const EVERY: u64 = 4 * 60 * 60;

/// The site that serves the builds: `$PUZ_UPDATE_SITE`, else the built-in Swords site.
pub fn site() -> String {
    std::env::var("PUZ_UPDATE_SITE").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| swords_client::DEFAULT_SITE.to_string()).trim_end_matches('/').to_string()
}

/// The name of the build for a computer, like `puz-macos-arm64` or `puz-windows-x64.exe`. None when
/// there is no build.
pub fn build_name(os: &str, arch: &str) -> Option<String> {
    let name = match (os, arch) {
        ("macos" | "linux", "aarch64") => format!("puz-{os}-arm64"),
        ("macos" | "linux", "x86_64") => format!("puz-{os}-x64"),
        ("windows", "x86_64") => "puz-windows-x64.exe".to_string(),
        _ => return None,
    };
    Some(name)
}

/// True when the bytes start like a program for macOS, Linux or Windows, not like a web page.
pub fn is_program(bytes: &[u8]) -> bool {
    const STARTS: [&[u8]; 5] = [b"\x7fELF", &[0xcf, 0xfa, 0xed, 0xfe], &[0xca, 0xfe, 0xba, 0xbe], &[0xca, 0xfe, 0xba, 0xbf], b"MZ"];
    bytes.len() > 1000 && STARTS.iter().any(|s| bytes.starts_with(s))
}

/// The file the person ran: the first argument as a path, or found on `$PATH` when it is a bare name.
pub fn invoked_file(argv0: &OsStr, path_var: Option<OsString>) -> Option<PathBuf> {
    let name = Path::new(argv0);
    if name.components().count() > 1 {
        return Some(name.to_path_buf());
    }
    std::env::split_paths(&path_var?).map(|dir| dir.join(name)).find(|file| file.is_file())
}

/// Why this file can't be replaced, or nothing when it can.
pub fn refusal(file: &Path) -> Option<String> {
    let Ok(meta) = fs::symlink_metadata(file) else {
        return Some(format!("Can't find this command's own file ({}).", file.display()));
    };
    if meta.file_type().is_symlink() {
        let target = fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        return Some(format!("This puz ({}) is a link to {}. Update it there instead.", file.display(), target.display()));
    }
    if meta.permissions().readonly() {
        return Some(format!("Can't update: {} can't be written. Check who owns it, or install again.", file.display()));
    }
    None
}

/// The file name with `tail` added: `puz` and `.new` give `puz.new`.
fn with_tail(file: &Path, tail: &str) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(tail);
    PathBuf::from(name)
}

/// Put new bytes in place of a file: write them beside it, make that runnable, let `check` try it,
/// then rename it over the old file. When the check fails, the old file stays and the new one is removed.
pub fn replace(file: &Path, bytes: &[u8], check: &dyn Fn(&Path) -> Result<()>) -> Result<()> {
    // On Windows the new file keeps `.exe` at the end, so it runs for the check.
    let beside = if cfg!(windows) { file.with_extension("new.exe") } else { with_tail(file, ".new") };
    let folder = file.parent().unwrap_or(Path::new("."));
    let cant = |_| anyhow::anyhow!("Can't update: the folder {} can't be written. Check who owns it, or install again.", folder.display());
    fs::write(&beside, bytes).map_err(cant)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&beside, fs::Permissions::from_mode(0o755)).map_err(cant)?;
    }
    if let Err(e) = check(&beside) {
        let _ = fs::remove_file(&beside);
        return Err(e);
    }
    swap(&beside, file).inspect_err(|_| drop(fs::remove_file(&beside))).map_err(cant)
}

/// Rename `new` over `file`. Windows can't replace a program while it runs, but it can rename it: the
/// running one moves to `puz.exe.old` first, and the next update removes that.
fn swap(new: &Path, file: &Path) -> std::io::Result<()> {
    if cfg!(windows) {
        let old = with_tail(file, ".old");
        let _ = fs::remove_file(&old);
        fs::rename(file, &old)?;
        return fs::rename(new, file).inspect_err(|_| drop(fs::rename(&old, file)));
    }
    fs::rename(new, file)
}

/// Does the new build run here, and is it the version the site named? Run it with `--version`.
fn runs(file: &Path, newest: &str) -> Result<()> {
    let out = std::process::Command::new(file).arg("--version").output().context("The new puz did not run on this computer. Nothing changed.")?;
    let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || said != format!("puz {newest}") {
        bail!("The new puz did not run as it should (it said {said:?}). Nothing changed.");
    }
    Ok(())
}

/// A version number as it is sent: short, one word, no markup.
fn plausible(v: &str) -> bool {
    !v.is_empty() && v.len() <= 40 && !v.contains(|c: char| c.is_whitespace() || c == '<')
}

/// `puz update`.
pub fn update(json: bool) -> Result<()> {
    if swords_agent_run() {
        bail!("An AI agent may not update puz. The user runs `puz update` in a terminal.");
    }
    let site = site();
    if let Some(why) = std::env::args_os().next().and_then(|a| invoked_file(&a, std::env::var_os("PATH"))).filter(|f| f.is_symlink()).and_then(|f| refusal(&f)) {
        bail!(why);
    }
    let me = std::env::current_exe().context("Can't find this command's own file.")?;
    if let Some(why) = refusal(&me) {
        bail!(why);
    }
    let Some(name) = build_name(std::env::consts::OS, std::env::consts::ARCH) else {
        bail!("There is no puz build for this computer ({} {}).", std::env::consts::OS, std::env::consts::ARCH);
    };
    let cant_get = || format!("Can't get puz from {site}.");
    let newest = match api::send(Method::Get, &format!("{site}/cli/version"), Proof::default(), Body::None) {
        Ok(reply) if reply.ok() => String::from_utf8_lossy(&reply.body).trim().to_string(),
        _ => bail!(cant_get()),
    };
    if !plausible(&newest) {
        bail!("{site} did not send a puz version.");
    }
    let _ = save_check(&Check { checked: now(), newest: newest.clone() });
    let out = |updated: bool, text: String| {
        if json {
            swords_client::say(&serde_json::to_string_pretty(&serde_json::json!({ "version": newest, "was": VERSION, "updated": updated })).unwrap_or_default());
        } else {
            swords_client::say(&Pen::stdout().ok(&text));
        }
    };
    if !newer(&newest, VERSION) {
        out(false, format!("puz {VERSION} is the newest."));
        return Ok(());
    }
    let build = match api::send(Method::Get, &format!("{site}/cli/{name}"), Proof::default(), Body::None) {
        Ok(reply) if reply.ok() => reply.body,
        Ok(reply) if reply.status == 404 => bail!("{site} has no puz build for this computer ({name})."),
        _ => bail!(cant_get()),
    };
    if !is_program(&build) {
        bail!("{site} did not send a puz program.");
    }
    replace(&me, &build, &|f| runs(f, &newest))?;
    out(true, format!("Updated puz from {VERSION} to {newest}."));
    Ok(())
}

fn swords_agent_run() -> bool {
    puz_core::swords::in_agent_run()
}

/// Is version `a` newer than `b`? Numbers compare as numbers (0.10.0 > 0.9.0); a part that is not a
/// number compares as text.
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| v.trim().trim_start_matches('v').split(['.', '-', '+']).map(str::to_string).collect::<Vec<_>>();
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).map_or("0", |s| s.as_str()), b.get(i).map_or("0", |s| s.as_str()));
        let order = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => x.cmp(y),
        };
        if order != std::cmp::Ordering::Equal {
            return order.is_gt();
        }
    }
    false
}

/// What the last quiet check found. Kept in `update-check.json` beside your puz profile.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Check {
    /// When the site was last asked (seconds since 1970).
    #[serde(default)]
    pub checked: u64,
    /// The newest version it named.
    #[serde(default)]
    pub newest: String,
}

fn check_file() -> PathBuf {
    let profile = puz_core::profile::user_file();
    profile.parent().unwrap_or(Path::new(".")).join("update-check.json")
}

fn load_check() -> Check {
    fs::read(check_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_check(c: &Check) -> Result<()> {
    let file = check_file();
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&file, serde_json::to_vec(c)?)?;
    Ok(())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Is it time to ask the site again? Every 4 hours. A check time in the future (a clock moved back) asks again.
pub fn due(last: &Check, now: u64) -> bool {
    last.checked == 0 || last.checked > now || now - last.checked >= EVERY
}

/// Why the quiet check is off for this run, if it is. `var` reads the environment.
pub fn skip(var: &dyn Fn(&str) -> Option<String>, terminal: bool, json: bool) -> Option<&'static str> {
    let set = |k: &str| var(k).is_some_and(|v| !v.is_empty());
    if var("PUZ_NO_UPDATE_CHECK").is_some_and(|v| !v.is_empty() && v != "0") {
        Some("PUZ_NO_UPDATE_CHECK")
    } else if set("PUZ_SCRATCH") {
        Some("an AI agent's run")
    } else if set("CI") {
        Some("CI")
    } else if json {
        Some("--json")
    } else if !terminal {
        Some("not at a terminal")
    } else {
        None
    }
}

/// The line that says a newer puz is out, or None.
pub fn notice(newest: &str, current: &str) -> Option<String> {
    (plausible(newest) && newer(newest, current)).then(|| format!("puz {newest} is out (you have {current}). Run: puz update"))
}

/// The quiet check for one run. `start` it before the command runs, `finish` it after.
pub struct Quiet {
    last: Check,
    asking: Option<std::sync::mpsc::Receiver<Check>>,
}

impl Quiet {
    /// Start the check: nothing when it is off for this run; else read the last result and, once a
    /// day, ask the site in the background (2 seconds at most).
    pub fn start(json: bool, command_is_update: bool) -> Option<Quiet> {
        use std::io::IsTerminal;
        let var = |k: &str| std::env::var(k).ok();
        if command_is_update || skip(&var, std::io::stderr().is_terminal(), json).is_some() {
            return None;
        }
        let last = load_check();
        let asking = due(&last, now()).then(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            let url = format!("{}/cli/version", site());
            std::thread::spawn(move || {
                // A failed check counts as a check, so a site that is down is not asked on every run.
                let newest = api::get_quick(&url, 2).map(|v| v.trim().to_string()).filter(|v| plausible(v)).unwrap_or_default();
                let c = Check { checked: now(), newest };
                let _ = save_check(&Check { newest: if c.newest.is_empty() { load_check().newest } else { c.newest.clone() }, ..c.clone() });
                let _ = tx.send(c);
            });
            rx
        });
        Some(Quiet { last, asking })
    }

    /// After the command: print the line when a newer puz is out. Waits a moment at most for the site.
    pub fn finish(self) {
        let mut newest = self.last.newest;
        if let Some(rx) = self.asking
            && let Ok(c) = rx.recv_timeout(Duration::from_millis(300))
            && !c.newest.is_empty()
        {
            newest = c.newest;
        }
        // One line, set apart from the command's own output by a blank line.
        if let Some(line) = notice(&newest, VERSION) {
            let pen = Pen::stderr();
            let line = match line.split_once("Run: ") {
                Some((said, cmd)) if pen.color => format!("{}{} {}", pen.dim(said), pen.dim("Run:"), pen.bold(cmd)),
                _ => line,
            };
            eprintln!("\n{}", pen.caution(&line));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("puz-update-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn build_names() {
        assert_eq!(build_name("macos", "aarch64").as_deref(), Some("puz-macos-arm64"));
        assert_eq!(build_name("macos", "x86_64").as_deref(), Some("puz-macos-x64"));
        assert_eq!(build_name("linux", "aarch64").as_deref(), Some("puz-linux-arm64"));
        assert_eq!(build_name("linux", "x86_64").as_deref(), Some("puz-linux-x64"));
        assert_eq!(build_name("windows", "x86_64").as_deref(), Some("puz-windows-x64.exe"));
        assert_eq!(build_name("windows", "aarch64"), None);
        assert_eq!(build_name("freebsd", "x86_64"), None);
        assert!(build_name(std::env::consts::OS, std::env::consts::ARCH).is_some());
    }

    #[test]
    fn programs_and_pages() {
        let mut elf = vec![0x7f, b'E', b'L', b'F'];
        elf.resize(2000, 0);
        let mut macho = vec![0xcf, 0xfa, 0xed, 0xfe];
        macho.resize(2000, 0);
        let mut exe = b"MZ".to_vec();
        exe.resize(2000, 0);
        assert!(is_program(&elf) && is_program(&macho) && is_program(&exe));
        assert!(!is_program(b"<!doctype html>"));
        assert!(!is_program(&vec![b'#'; 2000]));
    }

    #[test]
    fn replace_swaps_only_a_build_that_passes_the_check() {
        let dir = scratch("replace");
        let file = dir.join("puz");
        fs::write(&file, b"old").unwrap();
        assert!(replace(&file, b"bad", &|_| bail!("did not run")).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"old");
        assert!(!dir.join("puz.new").exists());
        replace(&file, b"new", &|f| if fs::read(f)? == b"new" { Ok(()) } else { bail!("wrong") }).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o755);
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_and_a_read_only_file_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("refuse");
        let file = dir.join("real");
        fs::write(&file, b"x").unwrap();
        assert_eq!(refusal(&file), None);
        let link = dir.join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(refusal(&link).unwrap().contains("is a link to"));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o555)).unwrap();
        assert!(refusal(&file).unwrap().contains("can't be written"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn invoked_file_is_the_path_or_found_on_path() {
        let dir = scratch("invoked");
        fs::write(dir.join("puz"), b"x").unwrap();
        assert_eq!(invoked_file(OsStr::new("./bin/puz"), None), Some(PathBuf::from("./bin/puz")));
        let path = std::env::join_paths([PathBuf::from("/no/such/folder"), dir.clone()]).unwrap();
        assert_eq!(invoked_file(OsStr::new("puz"), Some(path)), Some(dir.join("puz")));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(newer("0.2", "0.1.9"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
        assert!(newer("v0.2.0", "0.1.0"));
    }

    #[test]
    fn the_check_runs_at_most_every_4_hours() {
        let day = 4 * 60 * 60;
        assert!(due(&Check::default(), 1_000_000));
        let last = Check { checked: 1_000_000, newest: "0.2.0".into() };
        assert!(!due(&last, 1_000_000));
        assert!(!due(&last, 1_000_000 + day - 1));
        assert!(due(&last, 1_000_000 + day));
        // A clock moved back: ask again.
        assert!(due(&last, 999_000));
    }

    #[test]
    fn the_check_is_off_for_agents_ci_json_pipes_and_on_request() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|p| p.0 == k).map(|p| p.1.to_string());
        assert_eq!(skip(&env(&[]), true, false), None);
        assert_eq!(skip(&env(&[("PUZ_NO_UPDATE_CHECK", "1")]), true, false), Some("PUZ_NO_UPDATE_CHECK"));
        assert_eq!(skip(&env(&[("PUZ_NO_UPDATE_CHECK", "0")]), true, false), None);
        assert_eq!(skip(&env(&[("PUZ_SCRATCH", "/tmp/s")]), true, false), Some("an AI agent's run"));
        assert_eq!(skip(&env(&[("PUZ_SCRATCH", "/tmp/s")]), true, false), Some("an AI agent's run"));
        assert_eq!(skip(&env(&[("CI", "true")]), true, false), Some("CI"));
        assert_eq!(skip(&env(&[("CI", "")]), true, false), None);
        assert_eq!(skip(&env(&[]), true, true), Some("--json"));
        assert_eq!(skip(&env(&[]), false, false), Some("not at a terminal"));
    }

    #[test]
    fn the_notice_names_both_versions() {
        assert_eq!(notice("0.3.0", "0.2.0").as_deref(), Some("puz 0.3.0 is out (you have 0.2.0). Run: puz update"));
        assert_eq!(notice("0.2.0", "0.2.0"), None);
        assert_eq!(notice("", "0.2.0"), None);
        assert_eq!(notice("<html>", "0.2.0"), None);
    }
}
