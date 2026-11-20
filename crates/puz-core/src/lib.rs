//! puz-core: the crossword puzzle file and everything that works on one file.
//!
//! - `puzzle`: the file format, answer numbering, marks, validation.
//! - `edit`: place words, clues, marks, locks, regions, resize.
//! - `blocks`: black squares with their symmetric partner, and grid shape problems.
//! - `scene`: the steps of a custom ending.
//! - `markup`: italics in clue text.
//! - `formats`: `.jpz` and `.puz` (Across Lite) import and export.
//! - `export`: one call for every export format, and import by file name.
//! - `stats`: grid stats and checks against a paper's published grid rules.
//! - `print`: the grid as a PNG, a one-page PDF and a share card.
//! - `profile`: the constructor's name and contact details, in a file the caller names.
//! - `submission`: a puzzle as a PDF in a paper's submission format.
//! - `files`: file names for new puzzles, and finding a puzzle by name in a folder.
//! - `guides`: how-to texts about the file format.
//! - `swords`: publishing to a Swords solving site, and the links kept in the puzzle file.
//!
//! Nothing here needs a word list, a database or AI. Only `swords` uses the network. It is behind
//! the `swords` feature (on by default).

pub mod blocks;
pub mod edit;
pub mod export;
pub mod files;
pub mod formats;
pub mod guides;
pub mod markup;
pub mod print;
pub mod profile;
pub mod puzzle;
pub mod scene;
pub mod stats;
pub mod submission;
#[cfg(feature = "swords")]
pub mod swords;
