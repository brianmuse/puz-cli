//! How-to guides about the puzzle file, for a person or an AI agent: `puz guide TOPIC`.

/// Name, one line about it, text.
pub const GUIDES: &[(&str, &str, &str)] = &[("ending", "How to give a puzzle its own ending: a message, effects, and a scene that plays over the grid", include_str!("ending.md"))];

/// The text of one guide.
pub fn guide(topic: &str) -> Option<&'static str> {
    let t = topic.trim().to_lowercase();
    GUIDES.iter().find(|(n, _, _)| *n == t).map(|(_, _, text)| *text)
}
