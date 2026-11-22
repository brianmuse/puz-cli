//! The `puz swords` commands and their flags, as clap types. The program that bundles this library
//! (puz) puts them under its own command and adds `--site` and `--json`.

use clap::Subcommand;

/// The help after the list of `puz swords` commands.
pub const AFTER_HELP: &str = "\
PUZZLE is the puzzle file you published, the puzzle's id, or its link.
The site is, in order: --site, $SWORDS_SITE, the one set with `puz swords site URL`, the one puz was built for.
Your keys are kept in ~/.config/swords/puzzles.json, or %APPDATA%\\swords\\puzzles.json on Windows
($SWORDS_HOME moves the folder).
Publish a puzzle with: puz publish FILE. Your name and sign-in: puz setup";

/// The commands for your puzzles on a Swords site and your account there: the `puz swords` group.
/// Publishing itself is `puz publish`; the program that bundles this library adds its own commands.
#[derive(Subcommand, Debug, PartialEq)]
pub enum Command {
    /// List your puzzles on Swords, with their links and stats
    #[command(after_help = "Example:\n  puz swords list\n  puz swords list --json\n  puz swords list --site https://swords.cx    # only the puzzles on one site")]
    List,

    /// Show one puzzle's stats: opens, starts, solves, times, and where solvers came from
    #[command(after_help = "Example:\n  puz swords stats puzzle.json\n  puz swords stats abcd2345 --json    # everything, with the counts for each of the last 30 days")]
    Stats {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
    },

    /// List a puzzle's old versions. The site keeps the last 10 files a publish replaced
    #[command(after_help = "Example:\n  puz swords versions puzzle.json\n  puz swords restore puzzle.json 3     # go back to version 3")]
    Versions {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
    },

    /// Go back to an old version of a puzzle. The link and the stats stay
    #[command(after_help = "Example:\n  puz swords versions puzzle.json      # see the versions\n  puz swords restore puzzle.json 3\n\nThe old file becomes a new version. The file it takes the place of is kept too, so you can go back again.\nThe file on this computer is not changed.")]
    Restore {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
        /// The version's number, from: puz swords versions PUZZLE
        version: u32,
    },

    /// Print the HTML that puts a puzzle in a page of your own site
    #[command(after_help = "Example:\n  puz swords embed puzzle.json\n  puz swords embed puzzle.json --json\n\nPaste the HTML into your page. The frame holds the solver and nothing else.\nThe short script makes the frame as tall as the puzzle; the frame works without it.")]
    Embed {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
    },

    /// Set, change or remove a puzzle's PIN. Solvers must type the PIN to open the puzzle
    #[command(
        after_help = "Example:\n  puz swords pin puzzle.json 4821      # set or change it\n  puz swords pin puzzle.json           # type it at a prompt (or pipe one line in)\n  puz swords pin puzzle.json --remove  # anyone with the link can solve it again\n\nA PIN is 4 to 12 characters. You never need it yourself. A new PIN makes everyone type it again."
    )]
    Pin {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
        /// The PIN: 4 to 12 characters. Leave it out to be asked for it
        #[arg(conflicts_with = "remove")]
        pin: Option<String>,
        /// Take the PIN off
        #[arg(long)]
        remove: bool,
        /// For AI agent runs only: the user said yes to this change
        #[arg(long)]
        confirmed: bool,
    },

    /// Protect a puzzle from deletion. While it is on, the site refuses to take the puzzle down
    #[command(after_help = "Example:\n  puz swords protect puzzle.json\n  puz swords unprotect puzzle.json     # it can be deleted again\n\nReplacing the file, old versions and the PIN still work while a puzzle is protected.")]
    Protect {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
        /// For AI agent runs only: the user said yes to this change
        #[arg(long)]
        confirmed: bool,
    },

    /// Turn off a puzzle's deletion protection, so it can be taken down again
    #[command(after_help = "Example:\n  puz swords unprotect puzzle.json\n  puz swords unpublish puzzle.json --yes")]
    Unprotect {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
        /// For AI agent runs only: the user said yes to this change
        #[arg(long)]
        confirmed: bool,
    },

    /// Show or change a published puzzle's title, constructor's note and solve note, in place on the site
    /// (no new version: the stats and solvers' progress stay). Your local file is not changed
    #[command(after_help = "Example:\n  puz swords details puzzle.json\n  puz swords details puzzle.json --title \"Sunday Best\" --solve-note \"Nice solve!\"\n  puz swords details puzzle.json --notes \"\"     # take the note off\n\nLimits: title 200 characters, notes 1000, solve note 140.")]
    Details {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
        /// The new title
        #[arg(long)]
        title: Option<String>,
        /// The constructor's note, shown under the grid while solving
        #[arg(long)]
        notes: Option<String>,
        /// The line in the "Solved" box
        #[arg(long)]
        solve_note: Option<String>,
        /// For AI agent runs only: the user said yes to this change
        #[arg(long)]
        confirmed: bool,
    },

    /// Give a puzzle to a new owner by email. They get a link to accept; when they do, the puzzle is theirs,
    /// it gets a new key, and you lose it
    #[command(after_help = "Example:\n  puz swords transfer puzzle.json sam@example.com\n  puz swords transfer puzzle.json --cancel\n  puz swords transfer puzzle.json              # the pending transfer, if any\n\nThe link works once, for 7 days. A new transfer replaces the pending one.")]
    Transfer {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
        /// The new owner's email
        #[arg(conflicts_with = "cancel")]
        email: Option<String>,
        /// Cancel the pending transfer
        #[arg(long)]
        cancel: bool,
        /// For AI agent runs only: the user said yes to this change
        #[arg(long)]
        confirmed: bool,
    },

    /// List, add or remove links for people: the puzzle's link with a code, so you see that person's solve
    #[command(
        after_help = "Example:\n  puz swords links puzzle.json                  # each link, and how that person's solve went\n  puz swords links puzzle.json --add \"Sam\"      # a new link for Sam\n  puz swords links puzzle.json --remove Sam     # by name or by code\n\nThe link is the puzzle's link plus ?s=CODE. The person sees the normal puzzle; the solve page takes the code out\nof the address bar. The names are only shown to you. Your owner page shows each person's solve under Solving."
    )]
    Links {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
        /// Make a link for this person (a name only you see)
        #[arg(long, value_name = "NAME", conflicts_with = "remove")]
        add: Option<String>,
        /// Delete a link, by its name or its code. The link then opens the puzzle like the plain one
        #[arg(long, value_name = "NAME|CODE")]
        remove: Option<String>,
        /// For AI agent runs only: the user said yes to this change
        #[arg(long)]
        confirmed: bool,
    },

    /// Open a puzzle's solve page in the browser
    #[command(after_help = "Example:\n  puz swords open puzzle.json")]
    Open {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
    },

    /// Print a puzzle's claim link (the owner's page). The link holds the key
    #[command(after_help = "Example:\n  puz swords manage puzzle.json\n  puz swords manage abcd2345 --yes    # signed in, with no key here: make a new key")]
    Manage {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
        /// Make a new key when this computer has none. The old key stops working
        #[arg(long)]
        yes: bool,
    },

    /// Take a puzzle off the site for good (needs --yes)
    #[command(alias = "delete", after_help = "Example:\n  puz swords unpublish puzzle.json --yes\n\nThe link stops working and the stats go. The puzzle file on this computer stays.")]
    Unpublish {
        /// The file you published, the puzzle's id, or its link
        puzzle: String,
        /// Yes, take it down: the link stops working and the stats go
        #[arg(long)]
        yes: bool,
        /// For AI agent runs only: the user said yes to this change
        #[arg(long)]
        confirmed: bool,
    },

    /// Add a puzzle from its claim link (published from somewhere else)
    #[command(after_help = "Example:\n  puz swords claim 'https://swords.cx/p/abcd2345/manage#KEY'")]
    Claim {
        /// The claim link, like https://site/p/abcd2345/manage#KEY
        link: String,
    },

    /// Sign in by email (no password). Your puzzles then follow you to any device
    #[command(
        after_help = "Example:\n  puz swords login me@example.com\n\nThe site emails a 6-digit code and a link. Type the code here, or open the link.\nBoth work once, for 15 minutes. With no terminal (a script), this command waits for the link to be opened."
    )]
    Login {
        /// Your email address
        email: String,
    },

    /// Sign out on this computer
    #[command(after_help = "Example:\n  puz swords logout")]
    Logout,

    /// Show your name, who is signed in, and the site
    #[command(after_help = "Example:\n  puz swords whoami")]
    Whoami,

    /// Show or set the site to publish to
    #[command(after_help = "Example:\n  puz swords site                     # show it\n  puz swords site https://swords.cx   # set it")]
    Site {
        /// The site's address. It starts with http:// or https://
        url: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;
    use clap::{CommandFactory, Parser};

    /// The group as puz puts it under its own command: with `--site` and `--json`.
    #[derive(Parser, Debug)]
    #[command(name = "swords")]
    struct Cli {
        #[arg(long, global = true)]
        site: Option<String>,
        #[arg(long, global = true)]
        json: bool,
        #[command(subcommand)]
        command: Command,
    }

    fn parse(words: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("swords").chain(words.iter().copied()))
    }
    fn command(words: &[&str]) -> Command {
        parse(words).unwrap().command
    }
    fn s(text: &str) -> String {
        text.to_string()
    }

    #[test]
    fn the_command_tree_is_sound() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_command_parses() {
        assert_eq!(command(&["links", "a.puz"]), Command::Links { puzzle: s("a.puz"), add: None, remove: None, confirmed: false });
        assert_eq!(command(&["links", "a.puz", "--add", "Sam"]), Command::Links { puzzle: s("a.puz"), add: Some(s("Sam")), remove: None, confirmed: false });
        assert_eq!(command(&["pin", "a.puz"]), Command::Pin { puzzle: s("a.puz"), pin: None, remove: false, confirmed: false });
        assert_eq!(command(&["pin", "a.puz", "4821"]), Command::Pin { puzzle: s("a.puz"), pin: Some(s("4821")), remove: false, confirmed: false });
        assert_eq!(command(&["pin", "--remove", "a.puz", "--confirmed"]), Command::Pin { puzzle: s("a.puz"), pin: None, remove: true, confirmed: true });
        assert_eq!(command(&["list"]), Command::List);
        assert_eq!(command(&["stats", "abcd2345"]), Command::Stats { puzzle: s("abcd2345") });
        assert_eq!(command(&["versions", "a.puz"]), Command::Versions { puzzle: s("a.puz") });
        assert_eq!(command(&["restore", "a.puz", "3"]), Command::Restore { puzzle: s("a.puz"), version: 3 });
        assert_eq!(command(&["embed", "abcd2345"]), Command::Embed { puzzle: s("abcd2345") });
        assert_eq!(command(&["open", "a.puz"]), Command::Open { puzzle: s("a.puz") });
        assert_eq!(command(&["manage", "a.puz"]), Command::Manage { puzzle: s("a.puz"), yes: false });
        assert_eq!(command(&["manage", "a.puz", "--yes"]), Command::Manage { puzzle: s("a.puz"), yes: true });
        assert_eq!(command(&["unpublish", "a.puz"]), Command::Unpublish { puzzle: s("a.puz"), yes: false, confirmed: false });
        // The old name still works.
        assert_eq!(command(&["delete", "--yes", "a.puz"]), Command::Unpublish { puzzle: s("a.puz"), yes: true, confirmed: false });
        assert_eq!(command(&["claim", "https://s/p/abcd2345/manage#K"]), Command::Claim { link: s("https://s/p/abcd2345/manage#K") });
        assert_eq!(command(&["login", "me@example.com"]), Command::Login { email: s("me@example.com") });
        assert_eq!(command(&["logout"]), Command::Logout);
        assert_eq!(command(&["whoami"]), Command::Whoami);
        assert_eq!(command(&["site"]), Command::Site { url: None });
        assert_eq!(command(&["site", "https://s"]), Command::Site { url: Some(s("https://s")) });
    }

    #[test]
    fn site_and_json_work_before_or_after_the_command() {
        for words in [&["--site", "http://x", "--json", "list"][..], &["list", "--site", "http://x", "--json"], &["--json", "list", "--site=http://x"]] {
            let cli = parse(words).unwrap();
            assert_eq!((cli.site.as_deref(), cli.json, &cli.command), (Some("http://x"), true, &Command::List));
        }
    }

    #[test]
    fn usage_errors() {
        let kind = |words: &[&str]| parse(words).unwrap_err().kind();
        assert_eq!(kind(&["nope"]), ErrorKind::InvalidSubcommand);
        assert_eq!(kind(&["pin"]), ErrorKind::MissingRequiredArgument);
        assert_eq!(kind(&["links", "a.puz", "--add", "Sam", "--remove", "Al"]), ErrorKind::ArgumentConflict);
        assert_eq!(kind(&["restore", "a.puz"]), ErrorKind::MissingRequiredArgument);
        assert_eq!(kind(&["restore", "a.puz", "three"]), ErrorKind::ValueValidation);
        assert_eq!(kind(&["pin", "a.puz", "4821", "--remove"]), ErrorKind::ArgumentConflict);
        assert_eq!(kind(&["list", "--new"]), ErrorKind::UnknownArgument);
        // Gone: publishing is `puz publish`, the name is `puz setup`.
        for gone in ["push", "name", "setup", "example", "convert", "update", "completions"] {
            assert_eq!(kind(&[gone]), ErrorKind::InvalidSubcommand, "{gone}");
        }
    }

    #[test]
    fn every_command_has_a_description_and_an_example() {
        for sub in Cli::command().get_subcommands() {
            assert!(sub.get_about().is_some(), "{} has no description", sub.get_name());
            let after = sub.get_after_help().map(|h| h.to_string()).unwrap_or_default();
            assert!(after.contains("Example:\n  puz swords "), "{} has no example", sub.get_name());
        }
    }
}
