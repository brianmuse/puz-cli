//! How the words look: one place for the marks, the colors and the few shapes every command uses.
//!
//! A [`Pen`] writes for one stream. At a terminal it is *rich*: a mark opens a result (`✓`, `!`, `✗`),
//! titles are bold, links are cyan, notes are dim, and a command to run next is shown as `  $ command`.
//! Through a pipe it is plain: the same words, with no marks and no colors, so scripts can read them.
//! `NO_COLOR` and `TERM=dumb` turn the colors off and keep the marks and the layout.

use std::io::IsTerminal as _;

const BOLD: &str = "1";
const DIM: &str = "2";
const RED: &str = "31";
const GREEN: &str = "32";
const YELLOW: &str = "33";
const CYAN: &str = "36";

/// The widest line a command makes on its own. A long link can still pass it: a link is never cut.
pub const WIDTH: usize = 80;

/// How to write for one stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pen {
    /// Marks and the terminal layout: on when the stream is a terminal.
    pub rich: bool,
    /// Colors, bold and dim: on at a terminal, off with `NO_COLOR` set or `TERM=dumb`.
    pub color: bool,
}

/// How one cell of a table looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Plain,
    Bold,
    Dim,
    Link,
}

/// One cell of a table: its text, how it looks, and a dim note after it (such as ` [PIN]`).
#[derive(Clone, Debug)]
pub struct Cell {
    pub text: String,
    pub look: Look,
    pub tail: String,
}

impl Cell {
    pub fn new(text: impl Into<String>, look: Look) -> Cell {
        Cell { text: text.into(), look, tail: String::new() }
    }
    fn width(&self) -> usize {
        width(&self.text) + width(&self.tail)
    }
}

/// How many columns a text takes. One for each character: right for the letters and marks used here.
pub fn width(text: &str) -> usize {
    text.chars().count()
}

/// A text cut to `max` columns, with `…` in place of what is left out.
pub fn truncate(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// One line of text as lines of `max` columns at most, broken at spaces. Each line after the first
/// starts with `indent`. A word is never cut, so a link stays whole.
pub fn wrap(text: &str, max: usize, indent: &str) -> String {
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let line = lines.last_mut().unwrap();
        if !line.is_empty() && width(line) + 1 + width(word) > max {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines.join(&format!("\n{indent}"))
}

/// A tiny bar chart, one block for each number. A zero is the lowest block.
pub fn spark(values: &[u64]) -> String {
    const BLOCKS: [char; 5] = ['▁', '▂', '▃', '▅', '▇'];
    let most = values.iter().copied().max().unwrap_or(0);
    values.iter().map(|&v| if v == 0 { BLOCKS[0] } else { BLOCKS[((v * 4).div_ceil(most) as usize).clamp(1, 4)] }).collect()
}

impl Pen {
    /// No marks and no colors: for a pipe, a file, and tests.
    pub const PLAIN: Pen = Pen { rich: false, color: false };
    /// Marks and colors: a terminal.
    pub const RICH: Pen = Pen { rich: true, color: true };

    /// The pen for results (stdout).
    pub fn stdout() -> Pen {
        Pen::for_terminal(std::io::stdout().is_terminal(), console::STDOUT)
    }
    /// The pen for notes and errors (stderr).
    pub fn stderr() -> Pen {
        Pen::for_terminal(std::io::stderr().is_terminal(), console::STDERR)
    }
    fn for_terminal(rich: bool, stream: u32) -> Pen {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) || std::env::var("TERM").is_ok_and(|t| t == "dumb");
        Pen { rich, color: rich && !no_color && console::colors(stream) }
    }

    fn paint(&self, style: &str, text: &str) -> String {
        if self.color && !text.is_empty() { format!("\x1b[{style}m{text}\x1b[0m") } else { text.to_string() }
    }
    pub fn bold(&self, text: &str) -> String {
        self.paint(BOLD, text)
    }
    pub fn dim(&self, text: &str) -> String {
        self.paint(DIM, text)
    }
    pub fn red(&self, text: &str) -> String {
        self.paint(RED, text)
    }
    pub fn yellow(&self, text: &str) -> String {
        self.paint(YELLOW, text)
    }
    /// A link inside a line. Nothing is put around it, so it stays easy to click and to copy.
    pub fn url(&self, url: &str) -> String {
        self.paint(CYAN, url)
    }
    /// The mark that opens a question: `›`.
    pub fn prompt(&self) -> String {
        self.paint(CYAN, "›")
    }

    fn marked(&self, color: &str, mark: &str, text: &str) -> String {
        if self.rich { format!("{} {text}", self.paint(color, mark)) } else { text.to_string() }
    }
    /// A step that worked: `✓ text`.
    pub fn ok(&self, text: &str) -> String {
        self.marked(GREEN, "✓", text)
    }
    /// Something to take care with: `! text`.
    pub fn caution(&self, text: &str) -> String {
        self.marked(YELLOW, "!", text)
    }
    /// A failure: `✗ text`.
    pub fn fail(&self, text: &str) -> String {
        self.marked(RED, "✗", text)
    }

    /// A failure, as the last thing a command says. A message that ends by naming the command to run
    /// (`… See them with: puz swords list`) shows that command on a line of its own at a terminal.
    pub fn error(&self, text: &str) -> String {
        match text.rsplit_once(": puz ") {
            Some((before, command)) if self.rich && !command.contains('\n') => format!("{}\n{}", self.fail(&wrap(&format!("{before}:"), WIDTH - 2, "  ")), self.cmd(&format!("puz {command}"))),
            _ if self.rich && !text.contains('\n') => self.fail(&wrap(text, WIDTH - 2, "  ")),
            _ => self.fail(text),
        }
    }

    /// A puzzle's title inside a sentence: bold at a terminal, in quotes when plain.
    pub fn title(&self, title: &str) -> String {
        if self.rich { self.bold(title) } else { format!("\"{title}\"") }
    }
    /// A link on a line of its own.
    pub fn link(&self, url: &str) -> String {
        format!("  {}", self.url(url))
    }
    /// A fact that comes second: dim, under the line it belongs to.
    pub fn note(&self, text: &str) -> String {
        if self.rich { format!("  {}", self.dim(text)) } else { text.to_string() }
    }
    /// A command to run: `  $ command`.
    pub fn cmd(&self, command: &str) -> String {
        if self.rich { format!("  {} {command}", self.dim("$")) } else { format!("  {command}") }
    }
    /// The command to run next. At a terminal the `$` says it all; plain text names it: `Push it: puz publish FILE`.
    pub fn next(&self, label: &str, command: &str) -> String {
        if self.rich { self.cmd(command) } else { format!("{label}: {command}") }
    }
    /// A label and a link on one line: `All your puzzles: LINK`.
    pub fn labelled_link(&self, label: &str, url: &str) -> String {
        if self.rich { format!("  {} {}", self.dim(&format!("{label}:")), self.url(url)) } else { format!("{label}: {url}") }
    }

    /// Labels and their values, the values in one column. An empty label carries on the value above.
    pub fn pairs(&self, rows: &[(&str, String)]) -> String {
        let wide = rows.iter().map(|(label, _)| width(label)).max().unwrap_or(0);
        let line = |(label, value): &(&str, String)| format!("{}{}  {value}", self.dim(label), " ".repeat(wide - width(label)));
        rows.iter().map(line).collect::<Vec<_>>().join("\n")
    }

    /// A small table. `heads` names each column and says if it is set to the right (numbers are).
    /// No line ends in spaces, and the last column is as long as it needs to be.
    pub fn table(&self, heads: &[(&str, bool)], rows: &[Vec<Cell>]) -> String {
        let wide: Vec<usize> = heads.iter().enumerate().map(|(i, (head, _))| rows.iter().filter_map(|r| r.get(i)).map(Cell::width).chain([width(head)]).max().unwrap_or(0)).collect();
        let line = |cells: Vec<Cell>| {
            let mut out = String::new();
            for (i, cell) in cells.iter().enumerate() {
                let space = " ".repeat(wide[i].saturating_sub(cell.width()));
                let text = match cell.look {
                    Look::Plain => cell.text.clone(),
                    Look::Bold => self.bold(&cell.text),
                    Look::Dim => self.dim(&cell.text),
                    Look::Link => self.url(&cell.text),
                } + &self.dim(&cell.tail);
                let right = heads[i].1;
                if i > 0 {
                    out += "  ";
                }
                out += &if right { format!("{space}{text}") } else { format!("{text}{space}") };
            }
            out.trim_end().to_string()
        };
        let head = line(heads.iter().map(|(head, _)| Cell::new(*head, Look::Dim)).collect());
        std::iter::once(head).chain(rows.iter().map(|r| line(r.clone()))).collect::<Vec<_>>().join("\n")
    }
}

/// The terminal on Windows. A Windows console shows colors only after the program turns them on,
/// and it hides typed letters through its own settings, not `stty`. On macOS and Linux these do nothing.
pub mod console {
    pub const STDIN: u32 = -10i32 as u32;
    pub const STDOUT: u32 = -11i32 as u32;
    pub const STDERR: u32 = -12i32 as u32;

    /// Can this stream show colors? On Windows this turns them on first; an old console that can't
    /// gets plain text.
    pub fn colors(stream: u32) -> bool {
        imp::change(stream, imp::VIRTUAL_TERMINAL, 0).unwrap_or(true)
    }

    /// Turn the echo of typed letters on or off. False when it can't be changed here.
    pub fn echo(on: bool) -> bool {
        #[cfg(windows)]
        {
            let (set, clear) = if on { (imp::ECHO, 0) } else { (0, imp::ECHO) };
            imp::change(STDIN, set, clear) == Some(true)
        }
        #[cfg(not(windows))]
        {
            let flag = if on { "echo" } else { "-echo" };
            let quiet = std::process::Stdio::null;
            std::process::Command::new("stty").arg(flag).stdout(quiet()).stderr(quiet()).status().is_ok_and(|s| s.success())
        }
    }

    #[cfg(windows)]
    mod imp {
        type Handle = *mut core::ffi::c_void;
        pub const VIRTUAL_TERMINAL: u32 = 0x0004; // ENABLE_VIRTUAL_TERMINAL_PROCESSING, for output
        pub const ECHO: u32 = 0x0004; // ENABLE_ECHO_INPUT, for input

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(which: u32) -> Handle;
            fn GetConsoleMode(console: Handle, mode: *mut u32) -> i32;
            fn SetConsoleMode(console: Handle, mode: u32) -> i32;
        }

        /// Set and clear bits of a console's mode. Some(false) when it is a console that refused the
        /// change; None when the stream is not a console.
        pub fn change(stream: u32, set: u32, clear: u32) -> Option<bool> {
            // SAFETY: plain Win32 calls on this process's own standard handles, with a valid out pointer.
            unsafe {
                let console = GetStdHandle(stream);
                let mut mode = 0;
                if console.is_null() || console as isize == -1 || GetConsoleMode(console, &mut mode) == 0 {
                    return None;
                }
                Some(SetConsoleMode(console, (mode | set) & !clear) != 0)
            }
        }
    }

    #[cfg(not(windows))]
    mod imp {
        pub const VIRTUAL_TERMINAL: u32 = 0;
        pub fn change(_stream: u32, _set: u32, _clear: u32) -> Option<bool> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_COLOR: Pen = Pen { rich: true, color: false };

    #[test]
    fn plain_text_has_no_marks_and_no_colors() {
        let p = Pen::PLAIN;
        assert_eq!(p.ok("Deleted."), "Deleted.");
        assert_eq!(p.caution("Take care."), "Take care.");
        assert_eq!(p.fail("No file x"), "No file x");
        assert_eq!(p.title("Hello, World"), "\"Hello, World\"");
        assert_eq!(p.link("http://s/p/abcd2345/"), "  http://s/p/abcd2345/");
        assert_eq!(p.note("A note."), "A note.");
        assert_eq!(p.next("Push it", "puz publish a.puz"), "Push it: puz publish a.puz");
        assert_eq!(p.labelled_link("All your puzzles", "http://s/puzzles"), "All your puzzles: http://s/puzzles");
    }

    #[test]
    fn rich_text_has_marks_and_colors() {
        let p = Pen::RICH;
        assert_eq!(p.ok("Deleted."), "\x1b[32m✓\x1b[0m Deleted.");
        assert_eq!(p.caution("Take care."), "\x1b[33m!\x1b[0m Take care.");
        assert_eq!(p.fail("No file x"), "\x1b[31m✗\x1b[0m No file x");
        assert_eq!(p.title("Hello, World"), "\x1b[1mHello, World\x1b[0m");
        // A link has its color and nothing else around it.
        assert_eq!(p.link("http://s/p/abcd2345/"), "  \x1b[36mhttp://s/p/abcd2345/\x1b[0m");
        assert_eq!(p.note("A note."), "  \x1b[2mA note.\x1b[0m");
        assert_eq!(p.next("Push it", "puz publish a.puz"), "  \x1b[2m$\x1b[0m puz publish a.puz");
        assert_eq!(p.labelled_link("All your puzzles", "http://s/puzzles"), "  \x1b[2mAll your puzzles:\x1b[0m \x1b[36mhttp://s/puzzles\x1b[0m");
    }

    #[test]
    fn no_color_keeps_the_marks_and_the_layout() {
        assert_eq!(NO_COLOR.ok("Deleted."), "✓ Deleted.");
        assert_eq!(NO_COLOR.fail("No file x"), "✗ No file x");
        assert_eq!(NO_COLOR.title("T"), "T");
        assert_eq!(NO_COLOR.next("Push it", "puz publish a.puz"), "  $ puz publish a.puz");
        assert_eq!(NO_COLOR.note("A note."), "  A note.");
    }

    #[test]
    fn an_error_shows_the_command_to_run_on_its_own_line() {
        let text = "No puzzle \"nope\" among the ones you published. See them with: puz swords list";
        assert_eq!(Pen::PLAIN.error(text), text);
        assert_eq!(NO_COLOR.error(text), "✗ No puzzle \"nope\" among the ones you published. See them with:\n  $ puz swords list");
        assert_eq!(NO_COLOR.error("No file a.puz"), "✗ No file a.puz");
        // A long message is broken at a space, and goes on under its first letter.
        let long = "This takes \"Hello, World\" down for good: the link stops working and all of its stats go. To do it: puz swords unpublish a.puz --yes";
        assert_eq!(NO_COLOR.error(long), "✗ This takes \"Hello, World\" down for good: the link stops working and all of its\n  stats go. To do it:\n  $ puz swords unpublish a.puz --yes");
        assert_eq!(wrap("a bb ccc", 4, "> "), "a bb\n> ccc");
        assert_eq!(wrap("http://a-long-link/that/is/never/cut", 10, ""), "http://a-long-link/that/is/never/cut");
        assert_eq!(Pen::RICH.error("No file a.puz"), "\x1b[31m✗\x1b[0m No file a.puz");
    }

    #[test]
    fn long_text_is_cut_with_a_mark() {
        assert_eq!(truncate("Hello", 5), "Hello");
        assert_eq!(truncate("Hello, World", 8), "Hello,…");
        assert_eq!(truncate("Zoë's puzzle", 6), "Zoë's…");
        assert_eq!(width(&truncate("A very long title indeed", 10)), 10);
    }

    #[test]
    fn spark_bars() {
        assert_eq!(spark(&[]), "");
        assert_eq!(spark(&[0, 0]), "▁▁");
        assert_eq!(spark(&[0, 1, 2, 3, 4]), "▁▂▃▅▇");
        assert_eq!(spark(&[0, 1, 100]), "▁▂▇");
    }

    #[test]
    fn pairs_line_up_the_values() {
        let rows = [("Name", "Ada".to_string()), ("Email", "me@example.com".to_string()), ("", "more".to_string())];
        assert_eq!(NO_COLOR.pairs(&rows), "Name   Ada\nEmail  me@example.com\n       more");
        assert_eq!(Pen::RICH.pairs(&rows[..1]), "\x1b[2mName\x1b[0m  Ada");
    }

    #[test]
    fn tables_line_up_and_end_no_line_in_spaces() {
        let heads = [("TITLE", false), ("OPENED", true), ("LINK", false)];
        let mut locked = Cell::new("Secret", Look::Bold);
        locked.tail = " [PIN]".into();
        let rows = vec![vec![Cell::new("Hello", Look::Bold), Cell::new("12", Look::Plain), Cell::new("http://s/p/aaaa1111/", Look::Link)], vec![locked, Cell::new("3", Look::Plain), Cell::new("", Look::Plain)]];
        assert_eq!(NO_COLOR.table(&heads, &rows), "TITLE         OPENED  LINK\nHello             12  http://s/p/aaaa1111/\nSecret [PIN]       3");
        let rich = Pen::RICH.table(&heads, &rows[..1]);
        assert_eq!(rich, "\x1b[2mTITLE\x1b[0m  \x1b[2mOPENED\x1b[0m  \x1b[2mLINK\x1b[0m\n\x1b[1mHello\x1b[0m      12  \x1b[36mhttp://s/p/aaaa1111/\x1b[0m");
    }
}
