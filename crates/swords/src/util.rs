//! Small pure helpers: paths, links, times.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The full path of a file name, worked out from the text alone: `.` and `..` are folded, links are
/// not followed. This is the form the state file holds, so the same file is found again.
pub fn resolve(cwd: &Path, name: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for part in cwd.join(name).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The site part of an address: `https://host:port`, with no path.
pub fn origin(url: &str) -> &str {
    let start = url.find("://").map_or(0, |i| i + 3);
    let end = url[start..].find(['/', '?', '#']).map_or(url.len(), |i| start + i);
    &url[..end]
}

fn is_id(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// The puzzle id in a link like `https://site/p/abcd2345/`, when there is one.
pub fn id_in_link(text: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(i) = text[from..].find("/p/") {
        let start = from + i + 3;
        if let Some(id) = text.get(start..start + 8) {
            let after = text[start + 8..].bytes().next();
            let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
            if is_id(id) && !after.is_some_and(word) {
                return Some(id);
            }
        }
        from += i + 1;
    }
    None
}

/// The parts of a claim link `https://site/p/ID/manage#KEY`: the site, the id and the key.
pub fn parse_claim(link: &str) -> Option<(&str, &str, &str)> {
    if !(link.starts_with("http://") || link.starts_with("https://")) {
        return None;
    }
    let site = origin(link);
    if site.ends_with("://") {
        return None;
    }
    let rest = link[site.len()..].strip_prefix("/p/")?;
    let (id, rest) = rest.split_at_checked(8)?;
    let key = rest.strip_prefix("/manage#")?;
    (is_id(id) && !key.is_empty() && !site.contains(['?', '#'])).then_some((site, id, key))
}

/// Seconds as `m:ss`.
pub fn clock(seconds: f64) -> String {
    let s = seconds.round().max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Make text safe to put in an address, as JavaScript's `encodeURIComponent` does.
pub fn encode_component(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The time now as an ISO string in UTC, like `2026-11-18T10:00:00.000Z`.
pub fn now_iso() -> String {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    iso(since.as_secs(), since.subsec_millis())
}

/// Seconds since 1970 as an ISO string in UTC.
pub fn iso(secs: u64, millis: u32) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Days to a calendar date (Howard Hinnant's civil-from-days).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z", rem / 3_600, rem % 3_600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_folds_dots_and_keeps_full_paths() {
        let cwd = Path::new("/home/me/work");
        assert_eq!(resolve(cwd, "a.json"), PathBuf::from("/home/me/work/a.json"));
        assert_eq!(resolve(cwd, "./x/../a.json"), PathBuf::from("/home/me/work/a.json"));
        assert_eq!(resolve(cwd, "../a.json"), PathBuf::from("/home/me/a.json"));
        assert_eq!(resolve(cwd, "/tmp/a.json"), PathBuf::from("/tmp/a.json"));
        assert_eq!(resolve(cwd, "sub/"), PathBuf::from("/home/me/work/sub"));
    }

    #[test]
    fn origin_drops_the_path() {
        assert_eq!(origin("http://localhost:4747/api/puzzles/x?y=1"), "http://localhost:4747");
        assert_eq!(origin("https://swords.cx"), "https://swords.cx");
        assert_eq!(origin("https://swords.cx#frag"), "https://swords.cx");
    }

    #[test]
    fn id_in_link_finds_the_id() {
        assert_eq!(id_in_link("https://s/p/abcd2345/"), Some("abcd2345"));
        assert_eq!(id_in_link("https://s/p/abcd2345"), Some("abcd2345"));
        assert_eq!(id_in_link("https://s/p/abcd2345/manage#KEY"), Some("abcd2345"));
        assert_eq!(id_in_link("https://s/p/abcd23456/"), None);
        assert_eq!(id_in_link("https://s/p/ABCD2345/"), None);
        assert_eq!(id_in_link("abcd2345"), None);
        assert_eq!(id_in_link("/tmp/p/puzzle.json"), None);
        assert_eq!(id_in_link("/p/short/p/abcd2345/"), Some("abcd2345"));
    }

    #[test]
    fn claim_links() {
        assert_eq!(parse_claim("http://localhost:4747/p/abcd2345/manage#K-1_x"), Some(("http://localhost:4747", "abcd2345", "K-1_x")));
        assert_eq!(parse_claim("https://swords.cx/p/abcd2345/manage#k"), Some(("https://swords.cx", "abcd2345", "k")));
        for bad in ["", "abcd2345", "https://s/p/abcd2345/", "https://s/p/abcd2345/manage#", "https://s/p/abcd234/manage#k", "ftp://s/p/abcd2345/manage#k", "https:///p/abcd2345/manage#k", "https://s/x/p/abcd2345/manage#k"] {
            assert_eq!(parse_claim(bad), None, "{bad}");
        }
    }

    #[test]
    fn clock_is_minutes_and_seconds() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(65.0), "1:05");
        assert_eq!(clock(600.0), "10:00");
    }

    #[test]
    fn encode_component_matches_javascript() {
        assert_eq!(encode_component("aZ9-_.!~*'()"), "aZ9-_.!~*'()");
        assert_eq!(encode_component("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
    }

    #[test]
    fn iso_times() {
        assert_eq!(iso(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(951_782_400, 7), "2000-02-29T00:00:00.007Z");
        assert_eq!(iso(1_795_082_096, 789), "2026-11-19T09:54:56.789Z");
        assert_eq!(now_iso().len(), 24);
    }
}
