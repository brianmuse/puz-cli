# puz

`puz` makes and edits crossword puzzle files from the command line.

- A puzzle is one JSON file. Every editing command reads or changes one file. It needs no account and no server.
- `puz` can import and export Across Lite `.puz`, Amuse Labs `.jpz`, PNG and a printable PDF.
- `puz publish` puts a finished puzzle on a Swords solving site and prints a link anyone can solve it at.
- Commands that report results take `--json`, so a script or an AI agent can read them.

## Install

```sh
curl -fsSL https://swords.cx/install.sh | sh
```

This puts `puz` in `~/.local/bin` and runs `puz setup`. `puz update` gets the newest version later.

## Build from source

You need Rust 1.89 or newer.

```sh
git clone https://github.com/brianmuse/puz
cd puz
cargo build --release
./target/release/puz --help
```

Run the tests with `cargo test --release`.

## Main commands

```sh
puz new "My Puzzle" --size 15x15      # write my-puzzle.json
puz block my-puzzle.json 4,0          # add a black square and its symmetric partner
puz place my-puzzle.json 1a ALPS      # write a word
puz clue my-puzzle.json 1a "Swiss peaks"
puz show my-puzzle.json               # print the grid and every clue
puz stats my-puzzle.json              # grid stats and a paper's rules
puz export my-puzzle.json --format pdf
puz publish my-puzzle.json            # put it online and print the link
```

Run `puz --help` for every command. The full guide is in [`crates/puz/README.md`](crates/puz/README.md): the file format, marks (rebus, circles, colors, walls), endings, stats, export and publishing.

## Source layout

This is one Cargo workspace with three packages:

| Folder | Package | What it is |
|---|---|---|
| `crates/puz-core` | `puz-core` | A library: the puzzle file model and everything that works on one file. |
| `crates/puz` | `puz-cli` | The `puz` command. |
| `crates/swords` | `swords-client` | A library: publish to a Swords site and manage what you published. |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you say otherwise, any contribution you submit for inclusion in this project, as defined in the Apache-2.0 license, is licensed as above, with no additional terms or conditions.
