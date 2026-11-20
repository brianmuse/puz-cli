# Endings and scenes

An ending is the celebration a solver sees on the solving page when the grid is right. It is part of the puzzle file (`ending`). It is data, never code: there are no scripts, no HTML and no web addresses in it. After the celebration, the solving page shows its own "Solved!" popup; the puzzle does not change it.

Read it first: `puz ending PUZZLE --json --no-images` (`--no-images` keeps picture data out of your context).

## The simple parts

- `--effects "confetti rain"`: canned effects (confetti, rain, burst, wave). `--emoji "A B"`: the emoji that rain and burst use.
- `--highlight "x,y x,y"`: squares that light up.
- `--note TEXT`: one short line under the solve time in the popup, up to 140 characters (a punchline, a thank-you).
- Older files can hold `message` and `link`. They are kept in the file. Solving pages do not show the link, and show `message` only as the line when there is no note.

## The scene

A scene is a list of steps that play over the grid, one after another, right after the solve. The last state stays on screen. Use it when the puzzle's idea should show on the grid itself: theme squares change color, letters change into other letters, a line joins squares, a picture or a caption appears.

Squares are `x,y`: column then row, from `0,0` at the top left. Black squares and left-out squares count, and a step may paint or write on them.

Add one step per command. Steps play in the order you add them.

| Command | What the step does |
| --- | --- |
| `--paint "x,y x,y" COLOR` | Recolors squares. COLOR is red, orange, yellow, green, blue, purple, pink, or hex (`#1e9e4a`). |
| `--write "x,y x,y" TEXT` | Changes what the squares show. TEXT has one character for each square, or one word for each square. |
| `--line "x,y x,y ..." [COLOR]` | Draws a line through the middle of each square. `2.5,0` is on the border between two squares. `--closed` joins the end to the start. `--width` is in squares. |
| `--image FILE --over "x,y,w,h"` | Shows a picture over a rectangle of squares (or `--over grid`). `--fit contain|cover|stretch`, `--opacity 0..1`, `--enter fade|grow|none`. |
| `--caption TEXT` | Shows a caption below the grid. With `--over grid` or `--over "x,y,w,h"` it goes over the grid (`--size` in squares, `--color`). |
| `--play EFFECT` | Plays one canned effect at this point. |
| `--erase "1 3"` or `--erase all` | Removes what earlier steps drew, by step number. |

Timing, for the step you add:

- `--at SECONDS`: when it starts, counted from the solve. Left out, it starts when the step before it ends. Give two steps the same `--at` to play them together.
- `--for SECONDS`: how long the change takes (0.6 if left out).
- `--stagger SECONDS` (paint and write): the squares change one after another, in the order you list them.
- `--enter fade|sweep|none` (paint): how each square takes its new color. `fade` (the default) blends it, `sweep` fills the square from its left edge, `none` swaps it at once.

Other commands: `--scene FILE.json` replaces the whole scene from a JSON list of steps (`-` reads stdin), `--scene-remove N` removes step N, `--scene-clear` removes the scene. `puz ending PUZZLE` lists the steps with their numbers and start times.

## A picture can be anything

For a reveal that paint, write and line cannot make, draw it as a picture. Write an SVG file sized to the squares it covers (for a 15x15 grid, a `viewBox` of `0 0 15 15` makes one unit one square), then add it with `--image FILE --over grid`. An SVG may animate itself with CSS or SMIL. It cannot run scripts or load other files. All the pictures of one scene together may hold 750 KB.

## Limits

At most 40 steps. Every step ends within 60 seconds of the solve. At most 2,000 squares or points in a step. A caption has at most 200 characters; a square shows at most 10.

## Good practice

- Keep it short: most scenes are 2 to 6 steps and under 10 seconds. The solver waits for it before the popup appears (a tap skips it).
- The scene must make sense as a still picture: solvers who ask for less motion, and solvers who come back later, see only the last state.
- Do not hide the answers the solver just filled in unless that is the point.
- Check the squares against `puz show PUZZLE` before you add a step, and read the result back with `puz ending PUZZLE`.
- Tell the user to look at it on a solving page before they share the puzzle.
