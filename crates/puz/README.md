# puz

`puz` makes and edits a crossword puzzle file from the command line.

- A puzzle is one JSON file.
- Every editing command reads or changes one file. It needs nothing else: no account and no server.
- `puz publish` puts a finished puzzle on a Swords solving site and gives you a link to share.
- Commands that report results take `--json`, so a script can read them.

The command is `puz`. The package is `puz-cli`.

## Install

macOS and Linux:

```sh
curl -fsSL https://swords.cx/install.sh | sh   # puts puz in ~/.local/bin and runs puz setup
```

Windows (PowerShell):

```powershell
irm https://swords.cx/install.ps1 | iex   # puts puz in %LOCALAPPDATA%\Programs\puz, adds it to your PATH, runs puz setup
```

There are builds for macOS (Apple Silicon and Intel), Linux (x64 and arm64) and Windows (x64).

Or build it:

```sh
cargo build --release -p puz-cli
alias puz=target/release/puz   # or put target/release on your PATH
```

`puz setup` asks for your name (the author of your puzzles) and, if you like, an email to keep your Swords puzzles on any device.

`puz update` gets the newest puz. Once a day, at a terminal, puz asks the Swords site for the newest version number and prints one line after a command when a newer puz is out. It sends only its own version. It does not check in scripts, in CI, in an AI agent's run, or with `--json`. Turn it off with `PUZ_NO_UPDATE_CHECK=1`.

## Quick start

```sh
puz new "My Puzzle" --size 15x15      # writes my-puzzle.json in the current folder
puz block my-puzzle.json 4,0          # a black square and its symmetric partner
puz place my-puzzle.json 1a ALPS      # write a word
puz clue my-puzzle.json 1a "Swiss peaks"
puz show my-puzzle.json               # the grid and every slot with its clue
puz stats my-puzzle.json              # grid stats and a paper's rules
puz export my-puzzle.json --format pdf
puz publish my-puzzle.json            # put it online and print the link
```

## The PUZZLE argument

`PUZZLE` is a file path: `my-puzzle.json`.

If you keep your puzzles in one folder, name the folder with `--library DIR` or the `PUZ_LIBRARY` environment variable. `PUZZLE` may then also be a name inside that folder:

```sh
export PUZ_LIBRARY=~/crosswords/puzzles
puz show my-puzzle                  # ~/crosswords/puzzles/my-puzzle.json
puz show drafts/my-puzzle.json      # a puzzle in a subfolder
```

`puz new` and `puz import` name the new file after the title (`<slug-of-title>.json`). The file goes:

1. to `-o FILE`, if you give one;
2. else into the folder of puzzles, if one is set (`--folder F` picks a subfolder);
3. else into the current folder.

## Commands

Slots can be `41a`, `17d`, or `5,7,across`. A square is `x,y`: column, then row, from `0,0` at the top left.

**Create and look**

| Command | What it does |
|---|---|
| `puz new "Title" [--size 15x15] [--symmetry rotational] [--folder F] [--from PUZZLE] [--author A] [-o FILE]` | Create a blank puzzle file and print the file name. `--from` copies the size, symmetry and black squares of another puzzle. The author is your name in `puz profile` unless you give one. |
| `puz import FILE [--folder F] [-o FILE]` | Read an Across Lite `.puz`, an `.ipuz`, an older `.swords` or a puzzle `.json` and write it as a puzzle file. |
| `puz show PUZZLE [--json]` | Print the grid and every slot with its clue. |
| `puz stats PUZZLE [--rules auto\|nyt-daily\|...] [--json]` | Grid stats and checks against a paper's rules. See [Stats and paper rules](#stats-and-paper-rules). |
| `puz guide [TOPIC] [--json]` | Print a built-in how-to guide about the puzzle file. `puz guide` lists them; `puz guide ending` says how to write an ending and its scene. |

**Edit**

| Command | What it does |
|---|---|
| `puz set PUZZLE [--title] [--author] [--copyright] [--notes TEXT\|-] [--symmetry] [--target PAPER] [--json]` | Set details. `--notes` is the constructor's note: plain text, line breaks kept, up to 1,000 characters, optional; `-` reads stdin and `""` removes it. It goes in the .puz notepad and the .jpz description, and Swords shows it under the grid. `--target` names the paper whose rules `puz stats` checks (`auto` clears it). |
| `puz place PUZZLE 41a WORD` | Write a word. `.` clears a square, `_` keeps it (`BE_R`), `[STAR]` puts a rebus in one square (`[STAR]FISH`). |
| `puz clear PUZZLE 41a` / `puz clear PUZZLE --all` | Clear letters. |
| `puz clue PUZZLE 41a "Text" [--json]` | Set a clue. `""` removes it. The clue is stored under the answer, so it follows the word. |
| `puz clues PUZZLE FILE [--json]` | Set many clues from `{"41a": "Text", ...}`: a file, `-` for stdin, or the JSON itself. |
| `puz clues PUZZLE [--unused] [--json]` | List every answer and its clue. `--unused` lists stored clues no answer shows now (the word was changed or removed). |
| `puz resize PUZZLE [WxH] [--top N] [--bottom N] [--left N] [--right N] [--anchor A] [--json]` | Make the grid bigger or smaller (2 to 40 a side). Give a new size with an anchor (`center` by default; `top-left`, `bottom`, ...), or rows and columns to add on each edge (a negative number removes them). New squares are empty. Marks, locks and clues move with their squares. Removing a locked square fails. |
| `puz block PUZZLE x,y [--remove]` | Add or remove a black square and its symmetric partner. Warns if the grid breaks the rules. |
| `puz copy PUZZLE x0,y0 x1,y1` | Print a rectangle as text: one line per row, `#` black, `.` empty. |
| `puz clear-region PUZZLE x0,y0 x1,y1` | Empty every letter in a rectangle. Black squares stay. |
| `puz paste PUZZLE x,y FILE` / `--text 'CAT/A#E'` | Write region text with its top-left at x,y. `#` adds a black square and its partner. A letter or `.` on a black square makes it white. Space, `_` and `?` keep the square. Use `-` for stdin. |
| `puz mark PUZZLE x,y [x2,y2] --circle\|--shade\|--color COLOR\|--gap [--remove] [--json]` | Circle, shade or color a square, or every square in a rectangle. Circles skip black squares. `--gap` leaves the squares out of the grid, for grids in any shape. |
| `puz rebus PUZZLE x,y LETTERS [--remove] [--json]` | Put a rebus (2 or more letters) in a square. `""` or `--remove` takes it out and keeps the first letter. |
| `puz wall PUZZLE x,y right\|bottom\|left\|top [--remove] [--json]` | Add or remove a wall (bar) on one side of a square. A wall ends answers. |
| `puz sprite PUZZLE x,y --emoji "⭐" \| --image FILE [--alt TEXT] \| --remove [--json]` | Put a picture on a square, white or black. Images: png, jpg, gif, svg or webp, up to 300 KB. |
| `puz lock PUZZLE 9A\|x,y [x2,y2] [--remove] [--json]`, `puz lock PUZZLE --all-filled\|--clear-all` | Lock squares so they keep their letters. Edits that would change a locked square fail; `puz clear` skips them. Locking an answer skips its empty squares. |
| `puz ending PUZZLE ...` | Set the celebration a solver sees on finishing the puzzle. See [Endings](#endings). |

To move a region: `puz copy my-puzzle.json 0,0 4,2 | puz paste my-puzzle.json 10,12 -`.

**Export and share**

| Command | What it does |
|---|---|
| `puz export PUZZLE [--format ipuz\|jpz\|puz\|json\|png\|pdf] [--answers] [--scale N] [-o FILE]` | Write the puzzle in another format. See [Export](#export). |
| `puz publish nyt PUZZLE [-o FILE] [--blank-grid] [--name N] [--address A] [--city C] [--email E]` | Write the PDF the NYT asks constructors to send. See [The NYT submission PDF](#the-nyt-submission-pdf). |
| `puz profile [--name N] [--address A] [--city C] [--email E] [--co-name N ...] [--no-co] [--json]` | Show or set your details as a constructor. See [Your details](#your-details). |
| `puz card PUZZLE [-o FILE] [--footer TEXT] [--json]` | Make a share card: a 1200 x 630 PNG with the empty grid, the title and the author, for link previews. |
| `puz publish PUZZLE [--pin PIN\|--no-pin] [--new] [--site URL] [--json]` | Put a finished puzzle on a Swords solving site and print its link. See [Publish on Swords](#publish-on-swords). |
| `puz swords COMMAND` | Manage your puzzles on Swords: `status`, `list`, `stats`, `pin`, `protect`, `unpublish`, `login` and more. |
| `puz setup [--name N] [--email E]` | Save your name, and sign in to Swords by email if you like. |
| `puz update` | Get the newest puz. |

## Puzzle files

`puz` saves puzzles as [ipuz](http://ipuz.org), the open JSON crossword standard, so the files open in other crossword apps. What ipuz has no field for (emoji on squares, the ending, locks, share links, and clues keyed by answer) goes in two extension fields, `cx.swords:puzzle` and `cx.swords:clues`. `puz` reads ipuz files from other apps, with or without these fields.

Older files in the puz JSON below still read. A file changes to ipuz on its next save and keeps its name. Before it saves, `puz` turns the puzzle into ipuz and back and checks that nothing changed; if anything would change, it saves the older JSON and prints a note. On that first change it keeps a copy of the old file beside it, as `.NAME.before-ipuz`.

The fields, as `puz` reads them in memory and as the older JSON stores them:

```json
{
  "title": "Corner to Fill",
  "symmetry": "rotational",
  "grid": ["ALP##MOORS#BASS", "..."],
  "clues": { "ALP": "Swiss peak" }
}
```

- `#` is a black square, `.` is an empty square, letters are fill.
- Clues are keyed by answer (`ALP`; a rebus square is spelled out, so `[STAR]FISH` is `STARFISH`). A clue follows its word: move the word and the clue moves with it. Change the letters and the clue hides, but stays in the file and comes back with the word. The same answer twice in a grid shows the same clue.
- A clue may use italics: wrap the text in `<i>…</i>` (`puz clue my-puzzle.json 17a "<i>Jaws</i> setting"`). This is the markup other crossword tools use. Other tags are dropped. `puz clues --json` returns the text with its tags. `.jpz` keeps the italics, the PDF prints them, and `.puz` (which has no text styles) gets plain text.
- An answer with an empty square has no answer yet, so its clue is keyed by slot start square and direction (`x,y,across`). When the answer is complete, the clue moves to the answer key.
- Older files keyed every clue by slot. Loading one moves each clue of a complete answer to its answer key. If that answer already has a different clue, the slot-keyed clue stays, so no text is lost.
- Other fields: `author`, `copyright`, `notes`, `target` (the paper whose rules `puz stats` checks), `locked` (locked squares) and `ending` (see [Endings](#endings)).
- Optional fields hold rebus squares, circles, shading, colors, gaps, walls and sprites. See the next section.
- A file may also hold `finished`, `published`, `meta`, `submission` and `swords`. `puz` does not use them. It keeps them as they are, so an edit never loses them.

### Rebus, circles, shading, colors, gaps, walls and sprites

A puzzle file can hold these optional fields. Each one is left out when empty, so older files load unchanged. A square is `x,y`: column, then row, from `0,0` at the top left.

```json
{
  "rebus":   { "3,4": "STAR" },
  "circles": ["3,4", "5,0"],
  "shaded":  ["3,4"],
  "colors":  { "6,6": "yellow" },
  "absent":  ["0,0", "14,14"],
  "bars":    { "3,4": ["bottom", "right"] },
  "sprites": { "0,6": { "emoji": "⭐" }, "2,2": { "src": "data:image/png;base64,...", "alt": "star" } }
}
```

- **Rebus:** 2 or more letters A-Z in one square. The grid holds the first letter, so a tool that doesn't know rebuses still sees a sensible grid. Patterns in `puz show` print the square as `[STAR]`.
- **Circles and shading:** visual only.
- **Colors:** red, orange, yellow, green, blue, purple or pink. Visual only. A color replaces grey shading on that square.
- **Gaps (`absent`):** squares left out of the grid, so the grid can have any shape. A gap is `#` in `grid`, so numbering and tools that don't know the field see a black square. Drawings leave it empty.
- **Walls (`bars`):** a wall on the `right` or `bottom` edge of a square. The `puz wall` command stores a left or top wall on the neighbor. A wall ends an answer like a black square does, and numbering follows. A run of one square is not an answer, so barred grids may have unchecked squares.
- **Sprites:** a picture on a square: exactly one of `emoji` or `src` (a `data:image/...` URI, up to 300 KB). Sprites may sit on black squares.

Edits drop marks that no longer make sense: a rebus on a black or empty square or whose first letter changed, a circle on a black square, and a wall next to a black square. Shading and sprites stay on black squares.

## Endings

A puzzle can have its own ending: the celebration a solver sees on the solving page when the grid is right. After it, the solving page shows its own "Solved!" popup.

```
puz ending PUZZLE --note "That one was on the house." --effects "confetti rain" --emoji "🍺 🍻" --highlight "0,0 1,1 2,2"
puz ending PUZZLE            # show it
puz ending PUZZLE --clear    # remove the ending
```

- Effects: `confetti` (colored paper falls), `rain` (the emoji fall), `burst` (the emoji fly out of the grid), `wave` (the squares flash in a wave). They play together.
- `--highlight` lights up squares (`x,y`, from `0,0` at the top left): a hidden word or a shape.
- The ending is the `ending` field of the puzzle file. It is data, never code: a solving page plays the effects it knows and skips the rest. `.jpz` and `.puz` exports leave it out.
- Solvers who ask their device for less motion get the popup without the effects.
- `--note` is one short line under the solve time in the popup, up to 140 characters.
- `message` and `link` from older files are kept in the file. Solving pages do not show the link, and show `message` only when there is no note.
- `puz guide ending` prints a longer how-to.

### Scenes

A scene is a custom reveal for one puzzle. It is a list of steps that play over the grid right after the solve: squares change color, letters change, a line is drawn, a picture or a caption appears. The last state stays on the grid, so a solver who comes back later still sees it.

Each of these commands adds one step. Steps play in the order you add them.

```
puz ending PUZZLE --paint "0,0 1,1 2,2" yellow --stagger 0.1    # recolor squares, one after another
puz ending PUZZLE --paint "0,1 1,1 2,1" pink --stagger 0.08 --enter sweep # each square fills from its left edge
puz ending PUZZLE --write "0,0 1,1 2,2" SUN                     # change what squares show
puz ending PUZZLE --line "0,0 2,2 4,0" blue --for 1.5           # draw a line through square middles
puz ending PUZZLE --image sun.svg --over "5,5,4,4" --enter grow # a picture over a rectangle of squares
puz ending PUZZLE --caption "Here comes the sun"                # a caption below the grid
puz ending PUZZLE --play confetti                               # one effect, at this point
puz ending PUZZLE --erase "1 2"                                 # remove what steps 1 and 2 drew ("all" for everything)
puz ending PUZZLE                                               # list the steps, with numbers and start times
puz ending PUZZLE --json --no-images                            # the ending as JSON, without the picture data
puz ending PUZZLE --scene scene.json                            # replace the scene from a file ("-" reads stdin)
puz ending PUZZLE --scene-remove 3                              # remove step 3
puz ending PUZZLE --scene-clear                                 # remove the scene
```

- Squares are `x,y` from `0,0` at the top left. A step may paint or write on black squares and left-out squares too.
- Timing: `--at SECONDS` says when the new step starts, counted from the solve. Left out, the step starts when the step before it ends. `--for SECONDS` says how long the change takes (0.6 by default). `--stagger SECONDS` makes the squares of a paint or write step change one after another.
- Colors: red, orange, yellow, green, blue, purple, pink, or hex (`#1e9e4a`, `#1e9e4a80`).
- `--write` takes one character for each square (`SUN`; a space empties a square), or one word for each square (`"STAR MOON"`).
- A line point is the middle of a square. `2.5,0` is on the border between squares 2 and 3. `--width` is in squares (0.12 by default). `--closed` joins the last point to the first.
- `--image` takes a png, jpg, gif, webp or svg file and embeds it in the puzzle file. `--over` is `grid` or a rectangle of squares `x,y,w,h` (fractions are fine). `--fit` is contain, cover or stretch. `--opacity` is 0 to 1. `--enter` is fade, grow or none.
- For anything the steps cannot do, draw it as an SVG picture and place it over the grid. An SVG may animate itself. It cannot run scripts or load other files.
- `--caption` goes below the grid. With `--over grid` or `--over "x,y,w,h"` it goes over the grid (`--size` in squares, `--color`).
- The solver sees the scene first, then the message. A tap on the grid skips to the end. "Replay" plays it again.
- Solvers who ask for less motion see the last state at once.
- Limits: 40 steps; every step ends within 60 seconds; 2,000 squares or points in a step; 200 characters in a caption; 750 KB of pictures in one scene (750,000 bytes of image data, all pictures together).

In the puzzle file a scene is `ending.scene`, a list of steps. Each step has a `kind`, may have `at` and `for`, and has the fields of its kind:

| `kind` | Fields |
| --- | --- |
| `paint` | `squares`, `color`, `stagger`, `enter` (`fade`, `sweep` or `none`) |
| `write` | `squares`, `text` (one string, or a list with a string for each square), `stagger` |
| `line` | `through` (points), `color`, `width`, `closed` |
| `image` | `src` (a `data:image/...;base64,` URL), `over` (`"grid"` or `{"x": 5, "y": 5, "w": 4, "h": 4}`), `fit`, `opacity`, `enter` |
| `text` | `text`, `over` (`"below"`, `"grid"` or a rectangle), `size`, `color` |
| `clear` | `steps` (step numbers, from 1; left out means all) |
| `effect` | `effect` (no `for`) |

`puz` refuses a step kind or a field it does not know, and names the step. The solving page skips a step it does not understand.

## Stats and paper rules

`puz stats PUZZLE` reports:

1. Size, answers (across, down, total), black squares (count and %), average answer length, and answers by length.
2. 3-letter answers, the longest answers, and 2-letter answers.
3. Unchecked squares: white squares in fewer than two answers. Barred grids have them on purpose; only the rule checks judge them.
4. Interlock: whether answers link every white square, and how many parts there are if not.
5. The symmetry the black squares and walls have (rotational, mirror left/right, mirror up/down), and whether it meets the `symmetry` setting.
6. Cheater squares: black squares that don't change the answer count.
7. Fill progress: filled squares, complete answers and clued answers.

It then checks the grid against a paper's rules. Each check passes, warns or fails.

| Rules | Size | Max answers | Notes |
|---|---|---|---|
| `nyt-daily` | 15x15 | 78 themed, 72 themeless | 3+ letters, every square checked, all-over interlock, rotational symmetry (mirror warns), black squares about 1/6 at most (warns). |
| `nyt-sunday` | 21x21 | 140 | Same as the daily. |
| `nyt-mini` | 5x5 | none | Light: short answers and unchecked squares only warn. No symmetry check. |
| `lat-daily` | 15x15 | 78 themed, 72 themeless | Under 43 black squares preferred (warns). |
| `wsj-daily` | 15x15 | 78 | Standard American rules. |
| `basic` | any | none | 3+ letters, interlock, symmetric. |

`auto` (the default) uses the puzzle's `target` (`puz set PUZZLE --target lat-daily`), or picks from the grid size: 5x5 Mini, 15x15 NYT daily, 21x21 NYT Sunday, any other size `basic`. These presets are common guidelines from constructor guides, not the papers' own spec sheets. Check the paper's current spec sheet before you submit.

## Export

`puz export PUZZLE --format FORMAT` writes the puzzle in another format. Without `-o FILE`, the file goes in the current folder under the puzzle's file name. A puzzle with no author takes your name from `puz profile`.

| Format | What it is | Keeps | Leaves out (with a warning) |
|---|---|---|---|
| `ipuz` (the default) | the open standard, with the extension fields | everything | |
| `jpz` | Amuse Labs / PuzzleMe. No linked words; Amuse Labs rejects them. | rebus (multi-letter `solution`), circles (`background-shape="circle"`), shading and colors (`background-color`), walls (`left-bar` / `top-bar` on the next square), gaps (`type="void"`), clue italics (`<i>`) | sprites |
| `puz` | Across Lite | rebus (`GRBS` / `RTBL`), circles (`GEXT`). `puz import` reads both. | shading, walls, sprites, clue italics (clues become plain text). Across Lite numbers the grid without walls, so clues may not line up. |
| `json` | the older puz JSON | everything | |
| `png` | the grid | everything: grey shading, colors, circles, thick walls, rebus letters in a smaller font (answer key only), sprites | |
| `pdf` | one printable page: title, grid and clues | the same as `png`, and clue italics | |

- `png` and `pdf` are blank unless you add `--answers` (an answer key).
- `--scale` sets the PNG size (default 2 = 80 pixels per square).

### The NYT submission PDF

```sh
puz publish nyt PUZZLE [--blank-grid] [-o FILE]
```

This writes the PDF the NYT asks constructors to send:

1. Page 1: your details and the filled grid.
2. Then the clues, double-spaced, with each answer in a column on the right.

- `--blank-grid` adds a page with the blank, numbered grid after the filled grid.
- Your details come from `puz profile`. At a terminal, `puz` asks for any that are missing. In a script, give them with `--name`, `--address`, `--city` and `--email`; those apply to this run only.
- The old form, `puz export PUZZLE --format pdf --style nyt`, still works.

Check the paper's current spec sheet before you submit.

### Your details

`puz profile` holds your details as a constructor: name, street address, city line and email.

```sh
puz profile                                  # show them
puz profile --name "Your Name" --address "1 Main St" --city "Town, ST 00000" --email you@example.com
puz profile --co-name "Their Name" --co-email them@example.com   # a co-constructor
puz profile --no-co                          # remove the co-constructor
```

- The name is the author of new puzzles and of exports with no author.
- All four go at the top of page 1 of a submission PDF. A co-constructor's block goes to the right of yours.
- They are kept in one file. `puz` picks it in this order:
  1. `$PUZ_PROFILE`
  2. `profile.json` in `$PUZ_CONFIG_DIR`
  3. `profile.json` in `$XDG_CONFIG_HOME/puz`
  4. `~/.config/puz/profile.json`

## Publish on Swords

`puz publish` puts a finished puzzle on a Swords solving site and prints a link to share. Anyone with the link can solve it in a browser.

```sh
puz publish my-puzzle.json             # publish, or update the same link
puz publish my-puzzle.json --pin 4821  # solvers need the PIN to open it
puz swords status my-puzzle.json       # is it on Swords? is the published copy out of date?
puz swords list                        # every puzzle you published, with stats
puz swords protect my-puzzle.json       # no one can delete it until you run: puz swords unprotect
puz swords unpublish my-puzzle.json --yes
puz swords --help                      # every swords command
```

- The puzzle needs a full grid and a clue for every answer.
- A protected puzzle cannot be taken down: `unpublish` fails with the site's message until you run `puz swords unprotect`. Updates, old versions and the PIN still work.
- The link is saved in the puzzle file (`swords`). Publish again to update the same link, even after you rename or move the file. `--new` makes a second link.
- The key that owns each puzzle stays on your computer, in `~/.config/swords/puzzles.json` (`$SWORDS_HOME` moves the folder). It is never written to the puzzle file.
- PUZZLE may also be a `.puz`, `.jpz` or `.ipuz` file. Its link is found by the file's path.
- A puzzle with no author takes your name (`puz setup` or `puz profile --name`).
- `puz swords login EMAIL` keeps your puzzles with your email, so you can manage them from any device.
- The site is `--site`, else `$SWORDS_SITE`, else the one set with `puz swords site URL`, else the one puz was built for.
- In an AI agent's run (`$PUZ_SCRATCH` is set), `puz publish`, `puz swords pin`, `puz swords protect`, `puz swords unprotect` and `puz swords unpublish` need `--confirmed`, which the agent adds only after you say yes. Sign-in and account commands do not run there at all.

The Swords code is the `swords-client` library in `crates/swords`.

## Source layout

- `crates/puz-core`: the library. The puzzle file model and everything that works on one puzzle file: edits, scenes, `.jpz` and `.puz`, export, stats, PNG and PDF, the profile and the built-in guides.
- `crates/puz`: the `puz` command. It parses input, calls `puz-core` and prints the result.
- `crates/swords` (package `swords-client`): the Swords library: the site calls, the keys, sign-in and setup.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT), at your option.
