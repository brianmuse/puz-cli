//! Black squares: add or remove one with its symmetric partner, and check the grid's shape.

use crate::puzzle::Puzzle;
use anyhow::{bail, Result};
use std::collections::{HashSet, VecDeque};

/// The square that mirrors (x, y) under the puzzle's symmetry, if any.
pub fn partner(p: &Puzzle, x: usize, y: usize) -> Option<(usize, usize)> {
    let (w, h) = (p.width(), p.height());
    match p.symmetry.as_str() {
        "rotational" => Some((w - 1 - x, h - 1 - y)),
        "mirror" | "left-right" => Some((w - 1 - x, y)),
        _ => None,
    }
}

/// Set or clear a black square (and its symmetric partner). Letters in the way are an error.
pub fn set_block(p: &Puzzle, x: usize, y: usize, block: bool) -> Result<Puzzle> {
    if x >= p.width() || y >= p.height() {
        bail!("square {x},{y} is outside the grid");
    }
    let mut cells: Vec<(usize, usize)> = vec![(x, y)];
    if let Some(q) = partner(p, x, y)
        && q != (x, y)
    {
        cells.push(q);
    }
    let mut g: Vec<Vec<char>> = p.grid.iter().map(|r| r.chars().collect()).collect();
    for &(cx, cy) in &cells {
        let c = g[cy][cx];
        if block && p.locked.contains(&crate::puzzle::square_key(cx, cy)) {
            bail!("square {cx},{cy} is locked, so it keeps its letter. Unlock it first (lock PUZZLE {cx},{cy} --remove).");
        }
        if block && c != '.' && c != '#' {
            bail!("square {cx},{cy} has the letter {c}; clear it first");
        }
        g[cy][cx] = if block { '#' } else { if c == '#' { '.' } else { c } };
    }
    let mut out = p.clone();
    out.grid = g.into_iter().map(|r| r.into_iter().collect()).collect();
    out.tidy();
    Ok(out)
}

/// Standard American-style rules: every answer is 3+ letters and all white squares connect.
/// Walls split runs like black squares do. A single square cut off by a wall is allowed (barred
/// grids have unchecked squares), but a 1-square run between black squares is not.
pub fn grid_problems(p: &Puzzle) -> Vec<String> {
    let mut problems = vec![];
    let (w, h) = (p.width(), p.height());
    let white = |x: usize, y: usize| p.cell(x, y) != '#';
    // Short runs.
    for (horiz, outer, inner) in [(true, h, w), (false, w, h)] {
        let side = if horiz { crate::puzzle::RIGHT } else { crate::puzzle::BOTTOM };
        for o in 0..outer {
            let (mut run, mut walled) = (0, false);
            for i in 0..=inner {
                let (x, y) = if horiz { (i, o) } else { (o, i) };
                let is_white = i < inner && white(x, y);
                if is_white {
                    run += 1;
                }
                let wall_here = is_white && p.has_bar(x, y, side);
                if !is_white || wall_here {
                    let walled_end = walled || wall_here;
                    if run == 2 || (run == 1 && !walled_end) {
                        problems.push(format!("{}-letter {} at {} {}", run, if horiz { "across" } else { "down" }, if horiz { "row" } else { "column" }, o + 1));
                    }
                    run = 0;
                    walled = wall_here;
                }
            }
        }
    }
    // Connectivity.
    let whites: Vec<(usize, usize)> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| white(x, y)).collect();
    if let Some(&start) = whites.first() {
        let mut seen = HashSet::from([start]);
        let mut q = VecDeque::from([start]);
        while let Some((x, y)) = q.pop_front() {
            let n = [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)];
            for (nx, ny) in n {
                if nx < w && ny < h && white(nx, ny) && seen.insert((nx, ny)) {
                    q.push_back((nx, ny));
                }
            }
        }
        if seen.len() != whites.len() {
            problems.push("grid splits into separate parts".into());
        }
    }
    problems
}
