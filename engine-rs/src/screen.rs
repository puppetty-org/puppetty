use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;

/// Headless terminal screen model: feed raw PTY output in, read the rendered
/// screen (what a human would see) back out. Rust counterpart of the Node
/// engine's Screen (headless xterm.js); alacritty_terminal is the VT engine.
pub struct Screen {
    term: Term<VoidListener>,
    parser: Processor,
}

pub struct Snapshot {
    pub lines: Vec<String>,
    pub cursor_x: usize,
    pub cursor_y: usize,
}

#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Clone)]
struct VoidListener;

impl EventListener for VoidListener {
    fn send_event(&self, _event: Event) {}
}

impl Screen {
    pub fn new(cols: u16, rows: u16) -> Self {
        let config = Config {
            scrolling_history: 5_000,
            ..Config::default()
        };
        let term = Term::new(
            config,
            &Size {
                cols: cols as usize,
                rows: rows as usize,
            },
            VoidListener,
        );
        Self {
            term,
            parser: Processor::new(),
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(Size {
            cols: cols as usize,
            rows: rows as usize,
        });
    }

    /// Rendered lines + cursor, mirroring the Node engine's snapshot():
    /// trailing whitespace trimmed per line, trailing blank lines dropped,
    /// cursor y relative to the first returned line.
    pub fn snapshot(&self, scrollback: bool) -> Snapshot {
        let grid = self.term.grid();
        let rows = grid.screen_lines() as i32;
        let history = grid.history_size() as i32;
        let start = if scrollback { -history } else { 0 };

        let mut lines = Vec::with_capacity((rows - start) as usize);
        for l in start..rows {
            let row = &grid[Line(l)];
            let mut s = String::new();
            for c in 0..grid.columns() {
                let cell = &row[Column(c)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                s.push(cell.c);
            }
            lines.push(s.trim_end().to_string());
        }
        while lines.len() > 1 && lines.last().is_some_and(|s| s.is_empty()) {
            lines.pop();
        }

        let point = grid.cursor.point;
        let y = if scrollback {
            history + point.line.0
        } else {
            point.line.0
        };
        Snapshot {
            lines,
            cursor_x: point.column.0,
            cursor_y: y.max(0) as usize,
        }
    }

    /// Escape-sequence string that repaints the current screen on another
    /// terminal — used for attach replay. Unlike the Node engine's serializer
    /// this is plain text for now (colors/attributes are not yet preserved).
    pub fn restore_sequence(&self) -> String {
        let snap = self.snapshot(false);
        let mut out = String::from("\x1b[2J\x1b[H");
        out.push_str(&snap.lines.join("\r\n"));
        out.push_str(&format!(
            "\x1b[{};{}H",
            snap.cursor_y + 1,
            snap.cursor_x + 1
        ));
        out
    }
}
