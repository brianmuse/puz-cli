//! Black-square suggestions: try blocks at hotspot squares and rank the ones that make the grid fill.

use crate::engine::{self, FillStatus, Options};
use crate::puzzle::Puzzle;
use anyhow::Result;
pub use puz_core::blocks::{grid_problems, partner, set_block};
use serde::Serialize;
use std::collections::HashSet;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct Suggestion {
    /// Squares that become black (the chosen square plus its symmetric partner).
    pub squares: Vec<(usize, usize)>,
    pub status: FillStatus,
    pub seconds: f32,
    /// The resulting grid with the new blocks (not filled).
    pub grid: Vec<String>,
    /// A sample fill, when one was found.
    pub sample_fill: Option<Vec<String>>,
    pub word_count: usize,
}

#[derive(Debug, Serialize)]
pub struct SuggestReport {
    pub already_fillable: bool,
    pub probe_status: FillStatus,
    pub tried: usize,
    pub skipped_breaks_rules: usize,
    pub suggestions: Vec<Suggestion>,
    pub hotspots: Option<engine::Heat>,
}

pub struct SuggestOptions {
    pub probe_timeout: Duration,
    pub trial_timeout: Duration,
    pub max_squares: usize,
}

pub fn suggest(p: &Puzzle, o: &Options, so: &SuggestOptions) -> Result<SuggestReport> {
    let probe = engine::fill(p, o, so.probe_timeout)?;
    if matches!(probe.status, FillStatus::Filled) {
        return Ok(SuggestReport { already_fillable: true, probe_status: probe.status, tried: 0, skipped_breaks_rules: 0, suggestions: vec![], hotspots: None });
    }
    let heat = probe.hotspots.expect("failed fill has hotspots");

    // Candidate squares: hottest empty squares whose partner is also free.
    let mut seen = HashSet::new();
    let mut trials: Vec<Puzzle> = vec![];
    let mut skipped = 0;
    for c in &heat.cells {
        if trials.len() >= so.max_squares {
            break;
        }
        if p.cell(c.x, c.y) != '.' {
            continue;
        }
        // The smallest square of its orbit names the whole group.
        let key = puz_core::blocks::orbit(p, c.x, c.y).into_iter().fold((c.x, c.y), |a, b| a.min(b));
        if !seen.insert(key) {
            continue;
        }
        let Ok(t) = set_block(p, c.x, c.y, true) else { continue };
        if !grid_problems(&t).is_empty() {
            skipped += 1;
            continue;
        }
        trials.push(t);
    }

    // Run the trials in parallel.
    let results: Vec<Result<Suggestion>> = crate::par::parallel_upto(&trials, usize::MAX, |t| -> Result<Suggestion> {
        let r = engine::fill(t, o, so.trial_timeout)?;
        let squares = (0..p.height())
            .flat_map(|y| (0..p.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| p.cell(x, y) != '#' && t.cell(x, y) == '#')
            .collect();
        Ok(Suggestion { squares, status: r.status, seconds: r.seconds, grid: t.grid.clone(), sample_fill: r.grid, word_count: t.slots().len() })
    });
    let mut results = results.into_iter().collect::<Result<Vec<Suggestion>>>()?;
    let rank = |s: &Suggestion| match s.status {
        FillStatus::Filled => 0,
        FillStatus::Timeout => 1,
        FillStatus::Impossible => 2,
    };
    results.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.seconds.total_cmp(&b.seconds)));
    Ok(SuggestReport { already_fillable: false, probe_status: probe.status, tried: trials.len(), skipped_breaks_rules: skipped, suggestions: results, hotspots: Some(heat) })
}
