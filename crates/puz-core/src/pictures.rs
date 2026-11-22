//! Picture clues: an image a solver sees with a clue (`Puzzle::pictures`).
//!
//! The rules match what a Swords solving site takes:
//! - The image is embedded as a `data:` URL. A remote http(s) address is not taken.
//! - It is a PNG, JPEG, WebP or GIF, told by its first bytes (the type a data URL names is not trusted).
//! - Each image is at most `MAX_IMAGE_BYTES`, and a puzzle has at most `MAX_IMAGES` of them.

use anyhow::{bail, Context, Result};
use std::path::Path;

pub const MAX_IMAGE_BYTES: usize = 500_000;
pub const MAX_IMAGES: usize = 20;

/// The image type its first bytes show, or None.
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    let at = |i: usize, bytes: &[u8]| b.get(i..i + bytes.len()) == Some(bytes);
    if at(0, &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if at(0, &[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if at(0, b"RIFF") && at(8, b"WEBP") {
        Some("image/webp")
    } else if at(0, b"GIF8") {
        Some("image/gif")
    } else {
        None
    }
}

const BAD: &str = "this is not an image a picture clue can show. Use a PNG, JPEG, WebP or GIF";

/// Bytes into a data URL, after checking the type and size.
pub fn data_url(bytes: &[u8]) -> Result<String> {
    let Some(kind) = sniff(bytes) else { bail!("{BAD}") };
    check_size(bytes.len())?;
    Ok(format!("data:{kind};base64,{}", encode(bytes)))
}

/// An image file as a data URL.
pub fn from_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    data_url(&bytes).with_context(|| path.display().to_string())
}

fn check_size(n: usize) -> Result<()> {
    if n > MAX_IMAGE_BYTES {
        bail!(
            "the picture is too large: {} KB. The limit is {} KB for each picture. Make it smaller first, \
             for example 600 pixels wide: `sips -Z 600 IN --out OUT.jpg` on a Mac, or `magick IN -resize 600x600 OUT.jpg`",
            n.div_ceil(1000),
            MAX_IMAGE_BYTES / 1000
        );
    }
    Ok(())
}

/// A picture's size above this many bytes earns a warning: it loads slowly on a phone.
pub const LARGE_IMAGE_BYTES: usize = 250_000;
/// A picture wider or taller than this many pixels earns a warning: solvers see it at most about 640 pixels wide.
pub const LARGE_IMAGE_PIXELS: u32 = 1200;

/// Width and height of a PNG, GIF or JPEG from its bytes, if they can be read.
pub fn dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| b.get(i..i + 2).map(|x| u16::from_be_bytes([x[0], x[1]]) as u32);
    let le16 = |i: usize| b.get(i..i + 2).map(|x| u16::from_le_bytes([x[0], x[1]]) as u32);
    let be32 = |i: usize| b.get(i..i + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]));
    match sniff(b)? {
        "image/png" => Some((be32(16)?, be32(20)?)),
        "image/gif" => Some((le16(6)?, le16(8)?)),
        "image/jpeg" => {
            let mut i = 2;
            while i + 9 < b.len() {
                if b[i] != 0xff {
                    return None;
                }
                let m = b[i + 1];
                if m == 0xd8 || (0xd0..=0xd7).contains(&m) || m == 0x01 || m == 0xff {
                    i += if m == 0xff { 1 } else { 2 };
                    continue;
                }
                // Start-of-frame markers hold the size: C0-CF, except C4 (tables), C8 and CC.
                if (0xc0..=0xcf).contains(&m) && m != 0xc4 && m != 0xc8 && m != 0xcc {
                    return Some((be16(i + 7)?, be16(i + 5)?));
                }
                i += 2 + be16(i + 2)? as usize;
            }
            None
        }
        _ => None,
    }
}

/// Warnings for a picture that fits the limits but is close to them, or bigger than solvers need.
pub fn warnings(bytes: &[u8]) -> Vec<String> {
    let mut w = Vec::new();
    let kb = bytes.len().div_ceil(1000);
    if bytes.len() > LARGE_IMAGE_BYTES {
        w.push(format!("the picture is large: {kb} KB of the {} KB limit. It loads slowly on a phone; about 600 pixels wide is enough", MAX_IMAGE_BYTES / 1000));
    }
    if let Some((x, y)) = dimensions(bytes) {
        if x > LARGE_IMAGE_PIXELS || y > LARGE_IMAGE_PIXELS {
            w.push(format!("the picture is {x}x{y} pixels. Solvers see it at most about 640 pixels wide, so a smaller copy looks the same and loads faster"));
        }
    }
    w
}

/// Check a data URL and give it back in its standard form (`data:TYPE;base64,...`, with the type
/// its bytes show). Fails on anything else, such as an http address.
pub fn check(url: &str) -> Result<String> {
    let url = url.trim();
    let Some(rest) = url.strip_prefix("data:").or_else(|| url.get(..5).filter(|p| p.eq_ignore_ascii_case("data:")).map(|_| &url[5..])) else {
        bail!("a picture clue must be an embedded image (a data: URL), not a link");
    };
    let Some((head, body)) = rest.split_once(',') else { bail!("{BAD}") };
    if !head.to_ascii_lowercase().ends_with(";base64") {
        bail!("{BAD}");
    }
    let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    if body.len() / 4 * 3 > MAX_IMAGE_BYTES + 3 {
        check_size(body.len() / 4 * 3)?;
    }
    let bytes = decode(&body).context(BAD)?;
    data_url(&bytes)
}

/// Bytes and type of a data URL (no checks beyond decoding).
pub fn decode_url(url: &str) -> Option<(&'static str, Vec<u8>)> {
    let (_, body) = url.trim().split_once(',')?;
    let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = decode(&body).ok()?;
    Some((sniff(&bytes)?, bytes))
}

/// The JSON a Swords site gets for a puzzle (`swords::payload` after it clears the private fields).
/// With no pictures: the puzzle as it is. With pictures: `pictures` is replaced by `images` and
/// `imageAlts`, keyed by slot ("x,y,across"), the way the site reads them; ones no slot shows are
/// left out.
pub fn send_bytes(p: &crate::puzzle::Puzzle) -> Result<Vec<u8>> {
    if p.pictures.is_empty() {
        return Ok(serde_json::to_vec(p)?);
    }
    let (mut images, mut alts) = (serde_json::Map::new(), serde_json::Map::new());
    for s in p.slots() {
        let Some(pic) = p.picture_for(&s) else { continue };
        images.insert(s.key.clone(), pic.image.clone().into());
        if !pic.alt.is_empty() {
            alts.insert(s.key.clone(), pic.alt.clone().into());
        }
    }
    let mut c = p.clone();
    c.pictures.clear();
    let mut v = serde_json::to_value(&c)?;
    if !images.is_empty() {
        v["images"] = images.into();
    }
    if !alts.is_empty() {
        v["imageAlts"] = alts.into();
    }
    Ok(serde_json::to_vec(&v)?)
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for chunk in b.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn decode(s: &str) -> Result<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => bail!("bad base64"),
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for b in [&b""[..], b"a", b"ab", b"abc", b"abcd", &[0xff, 0x00, 0x10, 0x80]] {
            assert_eq!(decode(&encode(b)).unwrap(), b);
        }
        assert_eq!(encode(b"Man"), "TWFu");
    }

    #[test]
    fn checks_type_and_size() {
        let png = [&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a][..], &[0; 8]].concat();
        let url = data_url(&png).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        // The named type is not trusted: the bytes decide.
        assert_eq!(check(&url.replace("image/png", "image/gif")).unwrap(), url);
        assert!(check("https://example.com/a.png").is_err());
        assert!(data_url(b"hello").is_err());
        let mut big = png.clone();
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        assert!(data_url(&big).unwrap_err().to_string().contains("too large"));
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn png_dimensions_and_warnings() {
        let mut png = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        png.extend_from_slice(&2000u32.to_be_bytes());
        png.extend_from_slice(&900u32.to_be_bytes());
        assert_eq!(dimensions(&png), Some((2000, 900)));
        assert_eq!(warnings(&png).len(), 1);
        png.resize(300_000, 0);
        assert_eq!(warnings(&png).len(), 2);
    }

    #[test]
    fn jpeg_dimensions() {
        // SOI, an APP0 of length 4, then SOF0 with height 300 and width 500.
        let jpg = [0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 0x01, 0x2c, 0x01, 0xf4, 3, 0, 0, 0];
        assert_eq!(dimensions(&jpg), Some((500, 300)));
        assert!(warnings(&jpg).is_empty());
    }

    #[test]
    fn too_large_says_how_to_shrink() {
        let e = check_size(600_000).unwrap_err().to_string();
        assert!(e.contains("600 KB") && e.contains("sips"));
    }
}
