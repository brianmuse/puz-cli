//! Clue markup. A clue may hold italics as `<i>…</i>`, the convention crossword tools share (Across
//! Lite text, .jpz, Amuse Labs, NYT submissions). No other tags are kept.
//!
//! A tag is `<` + optional `/` + a letter, up to the next `>`. Other `<` characters ("a < b", "<3")
//! are plain text. Stored clues hold the raw text with the tags; exports and counts use the helpers here.

/// One piece of a clue: its text and whether it is in italics.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub italic: bool,
}

/// A tag at the start of `s`: (length in bytes, tag name in lowercase, is a closing tag).
fn tag_at(s: &str) -> Option<(usize, String, bool)> {
    let rest = s.strip_prefix('<')?;
    let (closing, body) = match rest.strip_prefix('/') {
        Some(b) => (true, b),
        None => (false, rest),
    };
    if !body.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let end = body.find(['>', '<'])?;
    if body.as_bytes()[end] != b'>' {
        return None;
    }
    let name: String = body[..end].chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    Some((s.len() - body.len() + end + 1, name, closing))
}

/// Split a clue into runs. `<i>` and `<em>` start italics, `</i>` and `</em>` end them; other
/// tags are dropped. Empty runs are left out; neighbouring runs with the same style are joined.
pub fn runs(s: &str) -> Vec<Run> {
    let mut out: Vec<Run> = vec![];
    let mut italic = false;
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if let Some((len, name, closing)) = tag_at(rest) {
            if name == "i" || name == "em" {
                italic = !closing;
            }
            i += len;
            continue;
        }
        let first = rest.chars().next().map_or(1, char::len_utf8);
        let next = rest[first..].find('<').map(|p| p + first).unwrap_or(rest.len());
        let text = &rest[..next];
        match out.last_mut() {
            Some(r) if r.italic == italic => r.text.push_str(text),
            _ => out.push(Run { text: text.to_string(), italic }),
        }
        i += next;
    }
    out
}

/// The clue with only `<i>` tags left, balanced, with neighbouring italic runs joined. Spaces stay
/// where they are, so the editor's text doesn't move under the caret.
pub fn normalize(s: &str) -> String {
    runs(s).into_iter().map(|r| if r.italic { format!("<i>{}</i>", r.text) } else { r.text }).collect()
}

/// The clue as plain text, with no tags. For .puz, text exports, counts and checks.
pub fn plain(s: &str) -> String {
    runs(s).into_iter().map(|r| r.text).collect()
}

/// The clue as XML content: text escaped, italics as `<i>` elements (for .jpz).
pub fn to_xml(s: &str) -> String {
    runs(&normalize(s))
        .into_iter()
        .map(|r| {
            let t = r.text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            if r.italic {
                format!("<i>{t}</i>")
            } else {
                t
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_italics() {
        assert_eq!(normalize("<i>Moby-Dick</i> captain"), "<i>Moby-Dick</i> captain");
        assert_eq!(normalize("<I>Moby</I> <b>bold</b> <span class=x>x</span>"), "<i>Moby</i> bold x");
        assert_eq!(normalize("<em>Hamlet</em> role"), "<i>Hamlet</i> role");
        assert_eq!(normalize("<i>Open"), "<i>Open</i>");
        assert_eq!(normalize("Stray</i> end"), "Stray end");
        assert_eq!(normalize("<i>A</i><i>B</i>"), "<i>AB</i>");
        assert_eq!(normalize("<i>Two words </i>then"), "<i>Two words </i>then");
        assert_eq!(normalize("<i></i>"), "");
        assert_eq!(normalize("1 < 2, <3 & x>y"), "1 < 2, <3 & x>y");
    }

    #[test]
    fn plain_and_runs() {
        assert_eq!(plain("<i>Jaws</i> shark"), "Jaws shark");
        assert_eq!(plain("a < b"), "a < b");
        assert_eq!(
            runs("See <i>Jaws</i>"),
            vec![Run { text: "See ".into(), italic: false }, Run { text: "Jaws".into(), italic: true }]
        );
    }

    #[test]
    fn xml_keeps_italics_and_escapes_text() {
        assert_eq!(to_xml("<i>Tom & Jerry</i> <b>x</b> 1<2"), "<i>Tom &amp; Jerry</i> x 1&lt;2");
    }
}
