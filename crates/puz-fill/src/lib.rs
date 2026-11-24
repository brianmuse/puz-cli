//! puz-fill: the crossword fill engine behind `puz fill`, `puz autofill`, `puz check` and the other
//! fill commands. Everything here takes a puzzle (and options) in and gives a result back. Nothing
//! reads or writes files, uses the network, runs programs or opens a database, so it also builds
//! for WebAssembly. The host (`puz`, or another program) loads the word list and gives the user's
//! word choices.
//!
//! No AI, no private data. Usage counts from a host's own puzzle history (used by `words` and
//! `green_paint`) come in only through `words::set_source`, and are none unless the host gives them.
//!
//! - `engine`: the fill engine on ingrid_core: word lists, checks, candidates, fill, hotspots.
//! - `autofill`: fill with good words and work on the weak spots.
//! - `pattern`: whole black-square patterns, and trimming black squares.
//! - `suggest`: black squares that make a stuck grid fill.
//! - `live`, `livework`: fillable status and word verdicts, and the steps of a live check.
//! - `words`: word quality and the user's banned and liked words (given by the host).
//! - `collisions`: answers that repeat across a set of puzzles.
//! - `submit`: the NYT submission helper (form answers and checks).
//! - `api`: the web API's JSON for each of these, shared by the server and the WebAssembly build.
//! - `par`: threads where there are threads, one at a time where there are none.

// The puzzle file model, from puz-core. Modules here use it as `crate::puzzle` and so on.
pub use puz_core::{blocks, edit, export, markup, pictures, print, profile, puzzle, stats, submission};

pub mod api;
pub mod autofill;
pub mod collisions;
pub mod crosswordese;
pub mod engine;
pub mod green_paint;
pub mod live;
pub mod livework;
pub mod par;
pub mod pattern;
pub mod submit;
pub mod suggest;
pub mod words;

/// Skip a test that needs the default word list when there is none. Set `PUZ_FILL_WORDLIST` to the
/// list's file (Spread the Wordlist, `word;score` per line) to run it.
#[cfg(test)]
#[macro_export]
macro_rules! need_list {
    () => {
        if !$crate::engine::test_list() {
            eprintln!("skipped: no word list. Set PUZ_FILL_WORDLIST to a word list file to run this test.");
            return;
        }
    };
}
