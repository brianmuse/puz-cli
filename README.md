<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
    <img src="assets/logo-light.svg" alt="PUZ" width="186" height="66">
  </picture>
</p>

<h1 align="center">puz-cli</h1>

<p align="center">
  <b>Make crossword puzzles from the command line. Publish them on <a href="https://swords.cx">swords.cx</a> and get a link anyone can solve.</b>
</p>

<p align="center">
  <a href="https://github.com/brianmuse/puz-cli/actions/workflows/ci.yml"><img src="https://github.com/brianmuse/puz-cli/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License: MIT OR Apache-2.0"></a>
  <a href="https://swords.cx"><img src="https://img.shields.io/badge/publish%20on-swords.cx-black" alt="Publish on swords.cx"></a>
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#try-it">Try it</a> ·
  <a href="#commands">Commands</a> ·
  <a href="crates/puz/README.md">Full guide</a> ·
  <a href="https://swords.cx">swords.cx</a>
</p>

---

```console
$ puz show hello-world.json
Hello, World (5x5)
S L A W #
H O N O R
A G G R O
H E L L O
# S O D S
   1A SLAW             Cabbage side at a cookout
   5A HONOR            Medal of ___
   8A HELLO            With 4-Down, a programmer's first words
   ...
```

- **One puzzle, one file.** A puzzle is a plain JSON file. Every command reads or changes one file. No account, no server.
- **Every common format.** Import Across Lite `.puz`. Export `.puz`, `.jpz`, a PNG, a printable PDF, or a newspaper submission PDF.
- **Publish in one command.** `puz publish` puts a finished puzzle on [swords.cx](https://swords.cx) and prints a link to solve it in the browser, on any device.
- **Built for scripts and AI agents.** Commands that report results take `--json`.

## Install

macOS (Apple silicon):

```sh
curl -fsSL https://swords.cx/install.sh | sh
```

This puts `puz` in `~/.local/bin` and runs `puz setup`. `puz` tells you when a new version is out; `puz update` installs it.

Other systems: [build from source](#build-from-source).

## Try it

`puz` comes with a small puzzle. Publish it and open the link:

```console
$ puz publish hello-world.puz
Saved hello-world.puz here: a small puzzle to try.
✓ Published Hello, World. Anyone with this link can solve it:
  https://swords.cx/p/…/
```

## Commands

```sh
puz new "My Puzzle" --size 15x15        # write my-puzzle.json
puz block my-puzzle.json 4,0            # a black square and its symmetric partner
puz place my-puzzle.json 1a ALPS        # write an answer
puz clue my-puzzle.json 1a "Swiss peaks"
puz show my-puzzle.json                 # the grid and every clue
puz stats my-puzzle.json                # grid stats and a paper's rules
puz export my-puzzle.json --format pdf  # also puz, jpz, png, json
puz import puzzle.puz                   # a .puz to a puzzle file
puz publish my-puzzle.json              # put it on swords.cx and print the link
puz publish nyt my-puzzle.json          # a NYT submission PDF
puz swords list                         # what you published, with solve stats
```

Run `puz --help` for every command. The [full guide](crates/puz/README.md) covers the file format, marks (rebus, circles, colors, walls), endings, stats, export and publishing.

## Publishing on swords.cx

[swords.cx](https://swords.cx) is a free place to share crosswords: push a puzzle, get a link.

- **No account needed.** Your computer keeps a key that owns each puzzle you publish.
- **Publish again to update.** `puz` remembers the link in the puzzle file, so the link stays the same.
- **Your puzzles on any device.** Run `puz swords login EMAIL` to add them to your account.
- **Manage them.** Add a PIN, see solve stats, or take a puzzle down with `puz swords`.

You can also drop a `.puz`, `.jpz` or `.ipuz` file on the swords.cx home page.

## Build from source

You need Rust 1.89 or newer.

```sh
git clone https://github.com/brianmuse/puz-cli
cd puz-cli
cargo build --release
./target/release/puz --help
```

Run the tests with `cargo test --release`.

This is one Cargo workspace with three packages:

| Folder | Package | What it is |
|---|---|---|
| `crates/puz` | `puz-cli` | The `puz` command. |
| `crates/puz-core` | `puz-core` | A library: the puzzle file model and everything that works on one file. |
| `crates/swords` | `swords-client` | A library: publish to swords.cx and manage what you published. |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you say otherwise, any contribution you submit for inclusion in this project, as defined in the Apache-2.0 license, is licensed as above, with no additional terms or conditions.
