//! The scene of an ending (`ending.scene`): steps that play over the grid when the puzzle is solved.
//!
//! A scene is data, never code. A step recolors squares, changes what they show, draws a line,
//! shows a picture or a caption, removes earlier drawings, or plays a canned effect. Pictures are
//! embedded `data:` URLs only, so a solving page never fetches anything for a scene.
//! A solving page plays it; the swords site checks the same rules.

use crate::puzzle::{self, Ending};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The step kinds.
///   paint   recolor squares            write   change what squares show
///   line    draw a line through points image   show a picture over the grid
///   text    show a caption             clear   remove what earlier steps drew
///   effect  play one canned effect (`puzzle::EFFECTS`)
pub const KINDS: [&str; 7] = ["paint", "write", "line", "image", "text", "clear", "effect"];
/// Most steps in one scene.
pub const MAX_STEPS: usize = 40;
/// Most image data in one scene, in bytes after decoding (750 KB), all pictures together.
pub const MAX_IMAGE_BYTES: usize = 750_000;
/// Most squares or points in one step.
pub const MAX_POINTS: usize = 2000;
/// Most characters in a caption.
pub const MAX_TEXT: usize = 200;
/// Most characters a `write` step puts in one square.
pub const MAX_SQUARE_TEXT: usize = 10;
/// Every step starts and ends within this many seconds of the solve.
pub const MAX_SECONDS: f64 = 60.0;
/// How long a step takes when it does not say (`for`).
pub const DEFAULT_FOR: f64 = 0.6;
pub const FITS: [&str; 3] = ["contain", "cover", "stretch"];
pub const ENTERS: [&str; 3] = ["fade", "grow", "none"];
/// The image types a scene may embed: (mime type, file extensions).
pub const IMAGE_TYPES: [(&str, &str); 5] = [("image/png", "png"), ("image/jpeg", "jpg"), ("image/gif", "gif"), ("image/webp", "webp"), ("image/svg+xml", "svg")];

/// One step of a scene. `kind` says which fields it uses (see `fields`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Step {
    pub kind: String,
    /// Seconds after the solve when the step starts. Left out: when the step before it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    /// Seconds the change takes. Left out: 0.6.
    #[serde(default, rename = "for", skip_serializing_if = "Option::is_none")]
    pub dur: Option<f64>,
    /// paint, write: the squares, as "x,y", in the order they change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub squares: Vec<String>,
    /// paint, line, text: a color name from `puzzle::COLORS`, or hex (#rgb, #rrggbb, #rrggbbaa).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// paint, write: seconds between one square and the next, so the change flows along the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagger: Option<f64>,
    /// write: one string with a character for each square, or a list with a string for each square.
    /// text: the caption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<Text>,
    /// line: the points, as "x,y". A whole number is the middle of a square; "2.5,0" is on a border.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub through: Vec<String>,
    /// line: the width in squares. Left out: 0.12.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// line: join the last point to the first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
    /// image: the picture, as a `data:image/...;base64,` URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    /// image, text: where it goes. "grid", a rectangle of squares, or (text only) "below".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub over: Option<Over>,
    /// image: contain (default), cover or stretch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<String>,
    /// image: 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// image: how it comes in: fade (default), grow or none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enter: Option<String>,
    /// text: the letter height in squares, for a caption over the grid. Left out: 0.6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// clear: the numbers of the steps to remove (the first step is 1). Left out: all of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<usize>,
    /// effect: a name from `puzzle::EFFECTS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Text {
    One(String),
    Each(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Over {
    Name(String),
    Rect(Rect),
}

/// A rectangle of squares: `x`,`y` is its top left square, `w` and `h` its size. Fractions are fine.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// The fields a step of this kind may hold, besides `kind` and `at`.
pub fn fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "paint" => &["squares", "color", "stagger", "for"],
        "write" => &["squares", "text", "stagger", "for"],
        "line" => &["through", "color", "width", "closed", "for"],
        "image" => &["src", "over", "fit", "opacity", "enter", "for"],
        "text" => &["text", "over", "size", "color", "for"],
        "clear" => &["steps", "for"],
        _ => &["effect"],
    }
}

/// Read a scene from JSON: a list of steps. An unknown kind or field is an error that names the step.
pub fn from_value(v: serde_json::Value) -> Result<Vec<Step>> {
    let serde_json::Value::Array(list) = v else { bail!("a scene is a list of steps, like [{{\"kind\": \"paint\", ...}}]") };
    list.into_iter().enumerate().map(|(i, v)| step_from_value(i + 1, v)).collect()
}

fn step_from_value(n: usize, v: serde_json::Value) -> Result<Step> {
    let Some(map) = v.as_object() else { bail!("scene step {n} must be an object, like {{\"kind\": \"paint\", ...}}") };
    let kind = match map.get("kind").and_then(|k| k.as_str()) {
        Some(k) if KINDS.contains(&k) => k.to_string(),
        Some(k) => bail!("scene step {n}: there is no step kind {k:?}. The kinds are: {}", KINDS.join(", ")),
        None => bail!("scene step {n} needs a \"kind\": {}", KINDS.join(", ")),
    };
    let allowed = fields(&kind);
    if let Some(bad) = map.keys().find(|k| !(*k == "kind" || *k == "at" || allowed.contains(&k.as_str()))) {
        bail!("scene step {n} ({kind}): a {kind} step has no field {bad:?}. It takes: at, {}", allowed.join(", "));
    }
    serde_json::from_value(v).map_err(|e| anyhow::anyhow!("scene step {n} ({kind}): {e}"))
}

/// For `#[serde(deserialize_with)]` on `Ending.scene`.
pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Vec<Step>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    from_value(v).map_err(|e| serde::de::Error::custom(format!("ending.scene: {e}")))
}

/// Is this a color a scene may use: a name from `puzzle::COLORS`, or #rgb, #rrggbb, #rrggbbaa?
pub fn color_ok(c: &str) -> bool {
    puzzle::color(c).is_some() || c.strip_prefix('#').is_some_and(|h| matches!(h.len(), 3 | 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The image type and decoded size of a `data:image/...;base64,` URL a scene may hold.
pub fn image_info(src: &str) -> Result<(&'static str, usize)> {
    let types = || IMAGE_TYPES.map(|t| t.1).join(", ");
    let Some((head, data)) = src.strip_prefix("data:").and_then(|s| s.split_once(";base64,")) else {
        bail!("src must be an embedded picture (data:image/png;base64,...), never an address. Use `ending --image FILE` to embed a file")
    };
    let Some(mime) = IMAGE_TYPES.iter().find(|t| t.0 == head).map(|t| t.0) else { bail!("the picture type {head:?} is not allowed. Use one of: {}", types()) };
    let body = data.trim_end_matches('=');
    if body.is_empty() || data.len() - body.len() > 2 || !body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/') {
        bail!("src is not valid base64 image data");
    }
    Ok((mime, body.len() * 3 / 4))
}

fn point(k: &str) -> Option<(f64, f64)> {
    let (x, y) = k.split_once(',')?;
    let num = |s: &str| s.trim().parse::<f64>().ok().filter(|v| v.is_finite());
    Some((num(x)?, num(y)?))
}

fn plain(t: &str) -> bool {
    !t.chars().any(|c| c.is_control() && c != '\n')
}

impl Step {
    /// Seconds the step takes, with its stagger.
    fn length(&self) -> f64 {
        let each = if self.kind == "effect" { 0.0 } else { self.dur.unwrap_or(DEFAULT_FOR) };
        each + self.stagger.unwrap_or(0.0) * self.squares.len().saturating_sub(1) as f64
    }

    /// Check one step against a grid. `n` is its number (from 1), for `clear`.
    fn check(&self, n: usize, w: usize, h: usize) -> Result<()> {
        let in_range = |name: &str, v: Option<f64>, lo: f64, hi: f64| match v {
            Some(v) if !(v >= lo && v <= hi) => bail!("{name} must be from {lo} to {hi}, not {v}"),
            _ => Ok(()),
        };
        in_range("at", self.at, 0.0, MAX_SECONDS)?;
        in_range("for", self.dur, 0.0, MAX_SECONDS)?;
        in_range("stagger", self.stagger, 0.0, MAX_SECONDS)?;
        let color = |c: &Option<String>| match c {
            Some(c) if !color_ok(c) => bail!("color must be one of {}, or hex like #1e9e4a, not {c:?}", puzzle::color_names()),
            _ => Ok(()),
        };
        let squares = || -> Result<()> {
            if self.squares.is_empty() {
                bail!("it needs \"squares\": a list like [\"0,0\", \"1,0\"]");
            }
            if self.squares.len() > MAX_POINTS {
                bail!("it has {} squares; the limit is {MAX_POINTS}", self.squares.len());
            }
            for k in &self.squares {
                let ok = k.split_once(',').and_then(|(x, y)| Some((x.parse::<usize>().ok()?, y.parse::<usize>().ok()?))).is_some_and(|(x, y)| x < w && y < h);
                if !ok {
                    bail!("{k:?} is not a square of this {w}x{h} grid. Write squares as x,y from 0,0 at the top left");
                }
            }
            Ok(())
        };
        let over = |names: &[&str]| -> Result<()> {
            match &self.over {
                None => {}
                Some(Over::Name(s)) if names.contains(&s.as_str()) => {}
                Some(Over::Name(s)) => bail!("over must be {} or a rectangle of squares {{\"x\": 0, \"y\": 0, \"w\": 2, \"h\": 2}}, not {s:?}", names.iter().map(|n| format!("{n:?}")).collect::<Vec<_>>().join(", ")),
                Some(Over::Rect(r)) => {
                    let ok = [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()) && r.x >= 0.0 && r.y >= 0.0 && r.w > 0.0 && r.h > 0.0 && r.x + r.w <= w as f64 && r.y + r.h <= h as f64;
                    if !ok {
                        bail!("the over rectangle (x {}, y {}, w {}, h {}) must have a size and stay inside the {w}x{h} grid", r.x, r.y, r.w, r.h);
                    }
                }
            }
            Ok(())
        };
        match self.kind.as_str() {
            "paint" => {
                squares()?;
                if self.color.is_none() {
                    bail!("it needs a \"color\": {}, or hex like #1e9e4a", puzzle::color_names());
                }
                color(&self.color)?;
            }
            "write" => {
                squares()?;
                let count = self.squares.len();
                match &self.text {
                    None => bail!("it needs \"text\": one string with a character for each square, or a list with a string for each square"),
                    Some(Text::One(t)) => {
                        if t.chars().count() != count || !plain(t) || t.contains('\n') {
                            bail!("text has {} character(s) for {count} square(s). Give one character for each square (a space leaves a square empty), or a list of strings", t.chars().count());
                        }
                    }
                    Some(Text::Each(list)) => {
                        if list.len() != count {
                            bail!("text has {} string(s) for {count} square(s)", list.len());
                        }
                        if let Some(t) = list.iter().find(|t| t.chars().count() > MAX_SQUARE_TEXT || !plain(t) || t.contains('\n')) {
                            bail!("{t:?} is too long for one square; the limit is {MAX_SQUARE_TEXT} characters");
                        }
                    }
                }
            }
            "line" => {
                if self.through.len() < 2 {
                    bail!("it needs \"through\": two or more points like [\"0,0\", \"4,4\"]");
                }
                if self.through.len() > MAX_POINTS {
                    bail!("it has {} points; the limit is {MAX_POINTS}", self.through.len());
                }
                for k in &self.through {
                    // A point may sit on the outer border: half a square past the first or last middle.
                    let ok = point(k).is_some_and(|(x, y)| x >= -0.5 && y >= -0.5 && x <= w as f64 - 0.5 && y <= h as f64 - 0.5);
                    if !ok {
                        bail!("{k:?} is not a point of this {w}x{h} grid. Write points as x,y: 0,0 is the middle of the top left square, and 0.5,0 is on its right border");
                    }
                }
                color(&self.color)?;
                in_range("width", self.width, 0.01, 1.0)?;
            }
            "image" => {
                let Some(src) = &self.src else { bail!("it needs \"src\": an embedded picture. Use `ending --image FILE`") };
                image_info(src)?;
                over(&["grid"])?;
                if let Some(f) = self.fit.as_deref().filter(|f| !FITS.contains(f)) {
                    bail!("fit must be one of {}, not {f:?}", FITS.join(", "));
                }
                if let Some(e) = self.enter.as_deref().filter(|e| !ENTERS.contains(e)) {
                    bail!("enter must be one of {}, not {e:?}", ENTERS.join(", "));
                }
                in_range("opacity", self.opacity, 0.0, 1.0)?;
            }
            "text" => {
                match &self.text {
                    Some(Text::One(t)) if !t.trim().is_empty() && t.chars().count() <= MAX_TEXT && plain(t) => {}
                    Some(Text::One(t)) if t.chars().count() > MAX_TEXT => bail!("the caption has {} characters; the limit is {MAX_TEXT}", t.chars().count()),
                    _ => bail!("it needs \"text\": the caption, as one string"),
                }
                over(&["below", "grid"])?;
                color(&self.color)?;
                in_range("size", self.size, 0.2, 5.0)?;
            }
            "clear" => {
                if let Some(bad) = self.steps.iter().find(|&&s| s == 0 || s >= n) {
                    bail!("it can only remove steps before it (1 to {}), not step {bad}", n.saturating_sub(1));
                }
                if self.steps.len() > MAX_STEPS {
                    bail!("\"steps\" lists more than {MAX_STEPS} steps");
                }
            }
            "effect" => match self.effect.as_deref() {
                Some(e) if puzzle::EFFECTS.contains(&e) => {}
                Some(e) => bail!("there is no effect {e:?}. The effects are: {}", puzzle::EFFECTS.join(", ")),
                None => bail!("it needs an \"effect\": {}", puzzle::EFFECTS.join(", ")),
            },
            other => bail!("there is no step kind {other:?}. The kinds are: {}", KINDS.join(", ")),
        }
        Ok(())
    }
}

/// When each step starts and ends, in seconds after the solve.
pub fn times(steps: &[Step]) -> Vec<(f64, f64)> {
    let mut t = 0.0;
    steps
        .iter()
        .map(|s| {
            let start = s.at.unwrap_or(t);
            t = start + s.length();
            (start, t)
        })
        .collect()
}

/// Check a scene against a `w` by `h` grid. Errors name the step by its number, from 1.
pub fn validate(steps: &[Step], w: usize, h: usize) -> Result<()> {
    if steps.len() > MAX_STEPS {
        bail!("the ending's scene has {} steps; the limit is {MAX_STEPS}", steps.len());
    }
    let when = times(steps);
    let mut bytes = 0;
    for (i, s) in steps.iter().enumerate() {
        let n = i + 1;
        s.check(n, w, h).map_err(|e| anyhow::anyhow!("ending scene step {n} ({}): {e}", s.kind))?;
        if when[i].1 > MAX_SECONDS + 1e-9 {
            bail!("ending scene step {n} ({}) ends {:.1} seconds after the solve; a scene must end within {MAX_SECONDS} seconds", s.kind, when[i].1);
        }
        if let Some(src) = &s.src {
            bytes += image_info(src).map(|i| i.1).unwrap_or(0);
        }
    }
    if bytes > MAX_IMAGE_BYTES {
        bail!("the ending's scene holds {} KB of pictures; the limit is {} KB for all of them together", bytes / 1000, MAX_IMAGE_BYTES / 1000);
    }
    Ok(())
}

// ---- Building steps (the `ending` command) ----

/// "x,y x,y" as a list. Spaces or semicolons go between squares.
pub fn parse_list(s: &str) -> Vec<String> {
    s.split([' ', ';', '\n', '\t']).map(str::trim).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

pub fn paint(squares: &str, color: &str) -> Step {
    Step { kind: "paint".into(), squares: parse_list(squares), color: Some(color.trim().to_ascii_lowercase()), ..Default::default() }
}

/// `text` is one character for each square, or one word for each square.
pub fn write(squares: &str, text: &str) -> Result<Step> {
    let squares = parse_list(squares);
    let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    let text = if text.chars().count() == squares.len() {
        Text::One(text.to_string())
    } else if words.len() == squares.len() {
        Text::Each(words)
    } else {
        bail!("{} square(s) need {} character(s), or {} word(s) with spaces between; the text has {} character(s) and {} word(s)", squares.len(), squares.len(), squares.len(), text.chars().count(), words.len());
    };
    Ok(Step { kind: "write".into(), squares, text: Some(text), ..Default::default() })
}

pub fn line(points: &str, color: Option<&str>) -> Step {
    Step { kind: "line".into(), through: parse_list(points), color: color.map(|c| c.trim().to_ascii_lowercase()), ..Default::default() }
}

pub fn caption(text: &str) -> Step {
    Step { kind: "text".into(), text: Some(Text::One(text.trim().to_string())), ..Default::default() }
}

pub fn effect(name: &str) -> Step {
    Step { kind: "effect".into(), effect: Some(name.trim().to_ascii_lowercase()), ..Default::default() }
}

/// `steps` is "all" or step numbers ("1 3").
pub fn clear(steps: &str) -> Result<Step> {
    let mut out = vec![];
    if !steps.trim().eq_ignore_ascii_case("all") {
        for s in steps.split([' ', ',']).filter(|s| !s.is_empty()) {
            out.push(s.parse::<usize>().ok().with_context(|| format!("{s:?} is not a step number. Give step numbers (\"1 3\") or \"all\""))?);
        }
    }
    Ok(Step { kind: "clear".into(), steps: out, ..Default::default() })
}

/// "grid", "below", or "x,y,w,h" (a rectangle of squares).
pub fn parse_over(s: &str) -> Result<Over> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("grid") || s.eq_ignore_ascii_case("below") {
        return Ok(Over::Name(s.to_ascii_lowercase()));
    }
    let nums: Vec<f64> = s.split([',', ' ']).filter(|p| !p.is_empty()).map(|p| p.parse::<f64>()).collect::<std::result::Result<_, _>>().ok().filter(|v: &Vec<f64>| v.len() == 4).with_context(|| {
        format!("--over takes \"grid\", \"below\" (captions only) or a rectangle of squares \"x,y,w,h\", not {s:?}")
    })?;
    Ok(Over::Rect(Rect { x: nums[0], y: nums[1], w: nums[2], h: nums[3] }))
}

/// The image type of a file, from its first bytes (never from its name).
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    // SVG is text: it starts with a tag, and has an <svg> element near the top.
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with('<') && head.contains("<svg") {
        return Some("image/svg+xml");
    }
    None
}

/// An image step from a file: the file is embedded as a data URL.
pub fn image(path: &Path) -> Result<Step> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let Some(mime) = sniff(&bytes) else { bail!("{} is not a png, jpg, gif, webp or svg picture", path.display()) };
    if bytes.len() > MAX_IMAGE_BYTES {
        bail!("{} is {} KB; a scene holds at most {} KB of pictures", path.display(), bytes.len() / 1000, MAX_IMAGE_BYTES / 1000);
    }
    Ok(Step { kind: "image".into(), src: Some(format!("data:{mime};base64,{}", crate::edit::base64(&bytes))), ..Default::default() })
}

/// The ending with each picture's data left out, for printing. The result is for reading only:
/// it does not pass `validate`.
pub fn without_images(e: &Ending) -> Ending {
    let mut out = e.clone();
    for s in &mut out.scene {
        if let Some(src) = &s.src {
            let (head, bytes) = match image_info(src) {
                Ok((mime, n)) => (format!("data:{mime};base64,"), n),
                Err(_) => (String::new(), src.len()),
            };
            s.src = Some(format!("{head}...({bytes} bytes left out)"));
        }
    }
    out
}

/// One line that says what a step does.
pub fn describe(s: &Step) -> String {
    let count = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    let place = |o: &Option<Over>, default: &str| match o {
        None => default.to_string(),
        Some(Over::Name(n)) if n == "grid" => "over the grid".to_string(),
        Some(Over::Name(n)) => n.clone(),
        Some(Over::Rect(r)) => format!("over {}x{} squares at {},{}", r.w, r.h, r.x, r.y),
    };
    match s.kind.as_str() {
        "paint" => format!("paint {} {}", count(s.squares.len(), "square"), s.color.as_deref().unwrap_or("")),
        "write" => {
            let text = match &s.text {
                Some(Text::One(t)) => t.clone(),
                Some(Text::Each(l)) => l.join(" "),
                None => String::new(),
            };
            format!("write {text:?} in {}", count(s.squares.len(), "square"))
        }
        "line" => format!("draw a {}{} line through {}", if s.closed { "closed " } else { "" }, s.color.as_deref().unwrap_or("red"), count(s.through.len(), "point")),
        "image" => {
            let info = s.src.as_deref().and_then(|src| image_info(src).ok());
            format!("show a picture ({}) {}", info.map(|(m, n)| format!("{}, {}", m.trim_start_matches("image/"), if n < 1000 { format!("{n} bytes") } else { format!("{} KB", n / 1000) })).unwrap_or_default(), place(&s.over, "over the grid"))
        }
        "text" => format!("caption {:?} {}", match &s.text { Some(Text::One(t)) => t.as_str(), _ => "" }, place(&s.over, "below")),
        "clear" if s.steps.is_empty() => "remove everything drawn so far".to_string(),
        "clear" => format!("remove step {}", s.steps.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")),
        _ => format!("play {}", s.effect.as_deref().unwrap_or("")),
    }
}

/// The scene as numbered lines, with each step's start time.
pub fn listing(steps: &[Step]) -> Vec<String> {
    times(steps).iter().zip(steps).enumerate().map(|(i, (t, s))| format!("{:>2}. {:>5.1}s  {}", i + 1, t.0, describe(s))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::puzzle::Puzzle;
    use serde_json::json;

    const PNG: &str = "data:image/png;base64,iVBORw0KGgo=";

    fn scene(v: serde_json::Value) -> Result<Vec<Step>> {
        let steps = from_value(v)?;
        validate(&steps, 5, 5)?;
        Ok(steps)
    }

    fn err(v: serde_json::Value) -> String {
        format!("{:#}", scene(v).unwrap_err())
    }

    #[test]
    fn a_scene_with_every_kind_is_valid_and_round_trips() {
        let v = json!([
            { "kind": "paint", "squares": ["0,0", "1,0", "2,0"], "color": "yellow", "stagger": 0.1 },
            { "kind": "paint", "squares": ["4,4"], "color": "#1E9E4A80", "at": 0, "for": 0 },
            { "kind": "write", "squares": ["0,0", "1,0"], "text": "A " },
            { "kind": "write", "squares": ["0,1", "1,1"], "text": ["AB", ""] },
            { "kind": "line", "through": ["0,0", "2.5,2", "-0.5,4.5"], "color": "red", "width": 0.2, "closed": true },
            { "kind": "image", "src": PNG, "over": { "x": 1, "y": 1.5, "w": 2, "h": 2 }, "fit": "cover", "opacity": 0.5, "enter": "grow" },
            { "kind": "image", "src": PNG },
            { "kind": "text", "text": "Well done", "over": "grid", "size": 1, "color": "blue" },
            { "kind": "clear", "steps": [1, 5], "for": 0.2 },
            { "kind": "effect", "effect": "confetti", "at": 3 },
            { "kind": "clear" }
        ]);
        let steps = scene(v.clone()).unwrap();
        // Whole numbers come back as 1.0, so compare the steps, not the text.
        assert_eq!(from_value(serde_json::to_value(&steps).unwrap()).unwrap(), steps);
        assert_eq!(steps.len(), v.as_array().unwrap().len());
        // In a puzzle file.
        let mut p = Puzzle::blank("t", 5, 5, "none");
        p.ending.scene = steps;
        p.validate().unwrap();
        let text = serde_json::to_string(&p).unwrap();
        let back: Puzzle = serde_json::from_str(&text).unwrap();
        assert_eq!(back, p);
        // An ending without a scene writes no "scene".
        p.ending.scene.clear();
        p.ending.message = "Hi".into();
        assert!(!serde_json::to_string(&p).unwrap().contains("scene"));
    }

    #[test]
    fn times_run_one_after_another_unless_at_is_set() {
        let steps = scene(json!([
            { "kind": "paint", "squares": ["0,0", "1,0", "2,0"], "color": "red", "stagger": 0.5 },
            { "kind": "effect", "effect": "wave" },
            { "kind": "line", "through": ["0,0", "1,1"], "for": 2 },
            { "kind": "text", "text": "x", "at": 1 }
        ]))
        .unwrap();
        let t = times(&steps);
        assert_eq!(t[0], (0.0, 1.6));
        assert_eq!(t[1], (1.6, 1.6));
        assert_eq!(t[2], (1.6, 3.6));
        assert_eq!(t[3], (1.0, 1.6));
    }

    #[test]
    fn bad_steps_are_named_by_number() {
        let ok = json!({ "kind": "paint", "squares": ["0,0"], "color": "red" });
        let cases = [
            (json!({ "kind": "script", "code": "x" }), "step 2: there is no step kind \"script\""),
            (json!({ "squares": ["0,0"] }), "step 2 needs a \"kind\""),
            (json!("paint"), "step 2 must be an object"),
            (json!({ "kind": "paint", "squares": ["0,0"], "color": "red", "html": "<b>" }), "step 2 (paint): a paint step has no field \"html\""),
            (json!({ "kind": "effect", "effect": "wave", "for": 1 }), "has no field \"for\""),
            (json!({ "kind": "paint", "squares": ["0,0"], "color": "red", "at": "soon" }), "step 2 (paint)"),
            (json!({ "kind": "paint", "squares": ["5,0"], "color": "red" }), "step 2 (paint): \"5,0\" is not a square"),
            (json!({ "kind": "paint", "squares": ["0.5,0"], "color": "red" }), "is not a square"),
            (json!({ "kind": "paint", "squares": [], "color": "red" }), "needs \"squares\""),
            (json!({ "kind": "paint", "squares": ["0,0"] }), "needs a \"color\""),
            (json!({ "kind": "paint", "squares": ["0,0"], "color": "url(x)" }), "color must be one of"),
            (json!({ "kind": "paint", "squares": ["0,0"], "color": "#12345" }), "color must be one of"),
            (json!({ "kind": "paint", "squares": ["0,0"], "color": "red", "at": 61 }), "at must be from 0 to 60"),
            (json!({ "kind": "paint", "squares": ["0,0"], "color": "red", "at": -1 }), "at must be from 0 to 60"),
            (json!({ "kind": "paint", "squares": ["0,0"], "color": "red", "at": 59, "for": 2 }), "must end within 60 seconds"),
            (json!({ "kind": "write", "squares": ["0,0", "1,0"], "text": "ABC" }), "3 character(s) for 2 square(s)"),
            (json!({ "kind": "write", "squares": ["0,0", "1,0"], "text": ["A"] }), "1 string(s) for 2 square(s)"),
            (json!({ "kind": "write", "squares": ["0,0"], "text": ["ABCDEFGHIJK"] }), "too long for one square"),
            (json!({ "kind": "write", "squares": ["0,0"] }), "needs \"text\""),
            (json!({ "kind": "line", "through": ["0,0"] }), "two or more points"),
            (json!({ "kind": "line", "through": ["0,0", "4.6,0"] }), "\"4.6,0\" is not a point"),
            (json!({ "kind": "line", "through": ["0,0", "1,1"], "width": 3 }), "width must be from"),
            (json!({ "kind": "image", "src": "https://example.com/a.png" }), "never an address"),
            (json!({ "kind": "image", "src": "data:text/html;base64,AAAA" }), "is not allowed"),
            (json!({ "kind": "image", "src": "data:image/png;base64,<svg onload=x>" }), "not valid base64"),
            (json!({ "kind": "image" }), "needs \"src\""),
            (json!({ "kind": "image", "src": PNG, "over": "below" }), "over must be \"grid\""),
            (json!({ "kind": "image", "src": PNG, "over": { "x": 4, "y": 0, "w": 2, "h": 1 } }), "stay inside the 5x5 grid"),
            (json!({ "kind": "image", "src": PNG, "over": { "x": 0, "y": 0, "w": 1 } }), "step 2 (image)"),
            (json!({ "kind": "image", "src": PNG, "fit": "tile" }), "fit must be one of"),
            (json!({ "kind": "image", "src": PNG, "enter": "spin" }), "enter must be one of"),
            (json!({ "kind": "image", "src": PNG, "opacity": 2 }), "opacity must be from 0 to 1"),
            (json!({ "kind": "text", "text": "" }), "needs \"text\""),
            (json!({ "kind": "text", "text": ["a"] }), "needs \"text\""),
            (json!({ "kind": "text", "text": "x".repeat(201) }), "the limit is 200"),
            (json!({ "kind": "text", "text": "x", "size": 9 }), "size must be from"),
            (json!({ "kind": "clear", "steps": [2] }), "only remove steps before it (1 to 1), not step 2"),
            (json!({ "kind": "clear", "steps": [0] }), "not step 0"),
            (json!({ "kind": "effect", "effect": "fireworks" }), "there is no effect \"fireworks\""),
            (json!({ "kind": "effect" }), "needs an \"effect\""),
        ];
        for (bad, want) in cases {
            let msg = err(json!([ok, bad]));
            assert!(msg.contains(want) && msg.contains("step 2"), "{bad} gave {msg:?}, wanted {want:?}");
        }
        assert!(format!("{:#}", from_value(json!({ "kind": "paint" })).unwrap_err()).contains("a list of steps"));
    }

    #[test]
    fn limits() {
        let paint = json!({ "kind": "paint", "squares": ["0,0"], "color": "red", "for": 0 });
        assert!(scene(json!(vec![paint.clone(); MAX_STEPS])).is_ok());
        assert!(err(json!(vec![paint.clone(); MAX_STEPS + 1])).contains("the limit is 40"));
        // Squares and points in one step.
        let many = |n: usize| vec!["0,0"; n];
        assert!(scene(json!([{ "kind": "paint", "squares": many(MAX_POINTS), "color": "red" }])).is_ok());
        assert!(err(json!([{ "kind": "paint", "squares": many(MAX_POINTS + 1), "color": "red" }])).contains("the limit is 2000"));
        assert!(err(json!([{ "kind": "line", "through": many(MAX_POINTS + 1) }])).contains("the limit is 2000"));
        // A stagger counts toward the 60 seconds.
        assert!(err(json!([{ "kind": "paint", "squares": many(200), "color": "red", "stagger": 0.5 }])).contains("within 60 seconds"));
        // Pictures: 750 KB decoded, all together.
        let img = |bytes: usize| json!({ "kind": "image", "src": format!("data:image/png;base64,{}", "A".repeat(bytes / 3 * 4)), "for": 0 });
        assert!(scene(json!([img(MAX_IMAGE_BYTES)])).is_ok());
        assert!(scene(json!([img(MAX_IMAGE_BYTES / 2), img(MAX_IMAGE_BYTES / 2)])).is_ok());
        assert!(err(json!([img(MAX_IMAGE_BYTES / 2), img(MAX_IMAGE_BYTES / 2), img(3000)])).contains("the limit is 750 KB"));
    }

    #[test]
    fn a_puzzle_file_with_a_bad_scene_does_not_load() {
        let text = r#"{"grid": ["...", "...", "..."], "ending": {"message": "Hi", "scene": [{"kind": "paint", "squares": ["0,0"], "color": "red"}, {"kind": "run", "js": "alert(1)"}]}}"#;
        let e = serde_json::from_str::<Puzzle>(text).unwrap_err().to_string();
        assert!(e.contains("ending.scene: scene step 2: there is no step kind \"run\""), "{e}");
        // Old files and endings without a scene still load.
        let old: Puzzle = serde_json::from_str(r#"{"grid": ["..."], "ending": {"message": "Hi", "highlight": ["0,0"]}}"#).unwrap();
        assert!(old.ending.scene.is_empty() && old.ending.message == "Hi");
        // A scene that does not fit the grid fails `validate`.
        let mut p = Puzzle::blank("t", 3, 3, "none");
        p.ending.scene = vec![paint("3,3", "red")];
        assert!(format!("{:#}", p.validate().unwrap_err()).contains("ending scene step 1 (paint)"));
    }

    #[test]
    fn builders() {
        assert_eq!(serde_json::to_value(paint("0,0 1,0", "Yellow")).unwrap(), json!({ "kind": "paint", "squares": ["0,0", "1,0"], "color": "yellow" }));
        assert_eq!(serde_json::to_value(write("0,0 1,0", "HI").unwrap()).unwrap(), json!({ "kind": "write", "squares": ["0,0", "1,0"], "text": "HI" }));
        assert_eq!(serde_json::to_value(write("0,0 1,0", "STAR SUN").unwrap()).unwrap(), json!({ "kind": "write", "squares": ["0,0", "1,0"], "text": ["STAR", "SUN"] }));
        assert!(write("0,0 1,0", "ABC").is_err());
        assert_eq!(serde_json::to_value(line("0,0 4,4", None)).unwrap(), json!({ "kind": "line", "through": ["0,0", "4,4"] }));
        assert_eq!(serde_json::to_value(caption(" Hi ")).unwrap(), json!({ "kind": "text", "text": "Hi" }));
        assert_eq!(serde_json::to_value(effect("Wave")).unwrap(), json!({ "kind": "effect", "effect": "wave" }));
        assert_eq!(clear("all").unwrap().steps, Vec::<usize>::new());
        assert_eq!(clear("1, 3").unwrap().steps, vec![1, 3]);
        assert!(clear("first").is_err());
        assert_eq!(parse_over("grid").unwrap(), Over::Name("grid".into()));
        assert_eq!(parse_over("1,2,3.5,4").unwrap(), Over::Rect(Rect { x: 1.0, y: 2.0, w: 3.5, h: 4.0 }));
        assert!(parse_over("1,2,3").is_err());
        // Built steps pass the checks.
        let steps = vec![paint("0,0 1,0", "yellow"), write("0,0 1,0", "HI").unwrap(), line("0,0 4,4", Some("blue")), caption("Hi"), clear("1").unwrap(), effect("wave")];
        validate(&steps, 5, 5).unwrap();
        let lines = listing(&steps);
        assert!(lines[0].contains("1.") && lines[0].contains("paint 2 squares yellow"), "{lines:?}");
        assert!(lines[4].contains("remove step 1"));
    }

    #[test]
    fn images_are_typed_by_their_bytes() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a..."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>"), Some("image/svg+xml"));
        assert_eq!(sniff(b"  <svg xmlns=\"http://www.w3.org/2000/svg\"/>"), Some("image/svg+xml"));
        assert_eq!(sniff(b"<html><script>alert(1)</script></html>"), None);
        assert_eq!(sniff(b"hello"), None);
        // A file named .png that is not a picture is refused; a real one is embedded.
        let dir = std::env::temp_dir().join(format!("puz-scene-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake.png");
        std::fs::write(&fake, b"not a picture").unwrap();
        assert!(image(&fake).is_err());
        let real = dir.join("real.bin");
        std::fs::write(&real, b"\x89PNG\r\n\x1a\n").unwrap();
        let step = image(&real).unwrap();
        assert_eq!(step.src.as_deref(), Some(PNG));
        // --no-images keeps the type and the size, not the data.
        let e = Ending { scene: vec![step], ..Default::default() };
        let short = without_images(&e);
        assert_eq!(short.scene[0].src.as_deref(), Some("data:image/png;base64,...(8 bytes left out)"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
