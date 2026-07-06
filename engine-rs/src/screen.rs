use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};

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

/// A horizontal run of same-styled cells (the unit an image renderer draws).
pub struct StyledRun {
    /// Start column; `width` counts display cells, so a run with CJK text
    /// covers more columns than it has chars.
    pub x: usize,
    pub width: usize,
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
}

/// The visible viewport with styling, for rendering to an image.
pub struct StyledSnapshot {
    pub cols: u16,
    pub rows: u16,
    /// One entry per viewport row; trailing unstyled blanks are dropped.
    pub lines: Vec<Vec<StyledRun>>,
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

    /// The viewport as styled runs. Spacer cells of wide chars extend the
    /// run's width instead of contributing text; trailing runs that would
    /// draw nothing (blank text, default background, no visible flags) are
    /// dropped.
    pub fn styled_snapshot(&self) -> StyledSnapshot {
        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();

        let mut lines = Vec::with_capacity(rows);
        for l in 0..rows as i32 {
            let row = &grid[Line(l)];
            let mut runs: Vec<StyledRun> = Vec::new();
            for c in 0..cols {
                let cell = &row[Column(c)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    if let Some(run) = runs.last_mut() {
                        run.width += 1;
                    }
                    continue;
                }
                let (fg, bg, flags) = style_of(cell);
                match runs.last_mut() {
                    Some(run)
                        if (run.fg, run.bg, run.flags) == (fg, bg, flags)
                            && run.x + run.width == c =>
                    {
                        run.text.push(cell.c);
                        run.width += 1;
                    }
                    _ => runs.push(StyledRun {
                        x: c,
                        width: 1,
                        text: cell.c.to_string(),
                        fg,
                        bg,
                        flags,
                    }),
                }
            }
            while runs.last().is_some_and(|r| {
                r.text.trim().is_empty()
                    && r.bg == Color::Named(NamedColor::Background)
                    && r.flags.is_empty()
            }) {
                runs.pop();
            }
            // The pop above removes all-blank runs; the last run can still
            // carry the row's trailing padding inside its own text.
            if let Some(run) = runs.last_mut() {
                if run.bg == Color::Named(NamedColor::Background) && run.flags.is_empty() {
                    let kept = run.text.trim_end_matches(' ').len();
                    let removed = run.text.len() - kept; // spaces: 1 byte, 1 cell
                    run.text.truncate(kept);
                    run.width -= removed;
                }
            }
            lines.push(runs);
        }

        let point = grid.cursor.point;
        StyledSnapshot {
            cols: cols as u16,
            rows: rows as u16,
            lines,
            cursor_x: point.column.0,
            cursor_y: point.line.0.max(0) as usize,
        }
    }

    /// Escape-sequence string that repaints the current buffer — colors and
    /// attributes included — on another terminal of the same size. The Rust
    /// counterpart of xterm.js's serialize addon, used for attach replay:
    /// up to 1000 history lines scroll into the client's scrollback, then
    /// the visible rows land exactly in its viewport (history + rows ≥ rows,
    /// so no explicit clear is needed), then the cursor is positioned.
    pub fn restore_sequence(&self) -> String {
        const MAX_HISTORY: usize = 1_000;
        let grid = self.term.grid();
        let rows = grid.screen_lines() as i32;
        let history = grid.history_size().min(MAX_HISTORY) as i32;

        let mut out = String::new();
        let mut pen: Option<(Color, Color, Flags)> = None;
        for l in -history..rows {
            if l > -history {
                out.push_str("\r\n");
            }
            let row = &grid[Line(l)];
            // Print through the last cell that has visible content OR
            // non-default styling (a bg-colored space must be repainted).
            let mut last = None;
            for c in 0..grid.columns() {
                let cell = &row[Column(c)];
                if cell.c != ' ' || style_of(cell) != default_style() {
                    last = Some(c);
                }
            }
            let Some(last) = last else { continue };
            for c in 0..=last {
                let cell = &row[Column(c)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let style = style_of(cell);
                if pen != Some(style) {
                    out.push_str(&sgr(&style));
                    pen = Some(style);
                }
                out.push(cell.c);
            }
        }
        let point = grid.cursor.point;
        out.push_str(&format!(
            "\x1b[0m\x1b[{};{}H",
            point.line.0.max(0) + 1,
            point.column.0 + 1
        ));
        out
    }
}

// ---- SGR serialization (colors/attributes) ----

const STYLE_MASK: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::UNDERLINE)
    .union(Flags::DOUBLE_UNDERLINE)
    .union(Flags::UNDERCURL)
    .union(Flags::DOTTED_UNDERLINE)
    .union(Flags::DASHED_UNDERLINE)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT);

fn style_of(cell: &Cell) -> (Color, Color, Flags) {
    (cell.fg, cell.bg, cell.flags & STYLE_MASK)
}

fn default_style() -> (Color, Color, Flags) {
    (
        Color::Named(NamedColor::Foreground),
        Color::Named(NamedColor::Background),
        Flags::empty(),
    )
}

fn push_color(codes: &mut Vec<String>, color: &Color, background: bool) {
    let (base, bright_base, extended, default) = if background {
        (40, 100, 48, 49)
    } else {
        (30, 90, 38, 39)
    };
    match color {
        Color::Named(n) => match *n as usize {
            i @ 0..=7 => codes.push((base + i).to_string()),
            i @ 8..=15 => codes.push((bright_base + i - 8).to_string()),
            _ if *n == NamedColor::Foreground || *n == NamedColor::Background => {
                codes.push(default.to_string())
            }
            _ => codes.push(default.to_string()), // Dim*/Cursor variants: default
        },
        Color::Indexed(i) => codes.push(format!("{extended};5;{i}")),
        Color::Spec(rgb) => codes.push(format!("{extended};2;{};{};{}", rgb.r, rgb.g, rgb.b)),
    }
}

/// Full SGR reset-and-set for a cell style. Emitting from a reset keeps the
/// serializer stateless per run; runs of same-styled cells share one code.
fn sgr(style: &(Color, Color, Flags)) -> String {
    let (fg, bg, flags) = style;
    let mut codes = vec!["0".to_string()];
    for (flag, code) in [
        (Flags::BOLD, 1),
        (Flags::DIM, 2),
        (Flags::ITALIC, 3),
        (Flags::INVERSE, 7),
        (Flags::HIDDEN, 8),
        (Flags::STRIKEOUT, 9),
    ] {
        if flags.contains(flag) {
            codes.push(code.to_string());
        }
    }
    if flags.intersects(
        Flags::UNDERLINE
            | Flags::DOUBLE_UNDERLINE
            | Flags::UNDERCURL
            | Flags::DOTTED_UNDERLINE
            | Flags::DASHED_UNDERLINE,
    ) {
        codes.push(if flags.contains(Flags::DOUBLE_UNDERLINE) {
            "21".to_string()
        } else {
            "4".to_string()
        });
    }
    if *fg != Color::Named(NamedColor::Foreground) {
        push_color(&mut codes, fg, false);
    }
    if *bg != Color::Named(NamedColor::Background) {
        push_color(&mut codes, bg, true);
    }
    format!("\x1b[{}m", codes.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip: feed styled content into screen A, apply A's restore
    /// sequence to a fresh screen B, and require every visible cell's char,
    /// colors, and style flags to match. This is the property the GUI's
    /// attach replay depends on.
    #[test]
    fn restore_sequence_round_trips_colors() {
        let mut a = Screen::new(40, 10);
        a.write(b"\x1b[31mred\x1b[0m plain \x1b[1;44mboldblue\x1b[0m\r\n");
        a.write(b"\x1b[38;5;208morange256\x1b[0m \x1b[38;2;1;2;3mrgb\x1b[0m\r\n");
        a.write(b"\x1b[4;92munderbright\x1b[0m wide: \xe6\x97\xa5\xe6\x9c\xac\r\n");
        a.write(b"\x1b[7minverse\x1b[0m\x1b[3;9mitalic-strike\x1b[0m");

        let mut b = Screen::new(40, 10);
        b.write(a.restore_sequence().as_bytes());

        let (ga, gb) = (a.term.grid(), b.term.grid());
        for l in 0..10 {
            for c in 0..40 {
                let (ca, cb) = (&ga[Line(l)][Column(c)], &gb[Line(l)][Column(c)]);
                assert_eq!(ca.c, cb.c, "char at {l},{c}");
                assert_eq!(
                    style_of(ca),
                    style_of(cb),
                    "style at {l},{c} (char {:?})",
                    ca.c
                );
            }
        }
        assert_eq!(ga.cursor.point, gb.cursor.point, "cursor");
    }

    #[test]
    fn styled_snapshot_groups_runs_and_tracks_widths() {
        let mut s = Screen::new(20, 4);
        s.write(b"\x1b[31mred\x1b[0mplain\r\n\x1b[1;44mB\x1b[0m \xe6\x97\xa5x");

        let snap = s.styled_snapshot();
        assert_eq!((snap.cols, snap.rows), (20, 4));

        let row0 = &snap.lines[0];
        assert_eq!(row0.len(), 2, "two style runs on row 0");
        assert_eq!(
            (row0[0].x, row0[0].width, row0[0].text.as_str()),
            (0, 3, "red")
        );
        assert_eq!(row0[0].fg, Color::Named(NamedColor::Red));
        assert_eq!(
            (row0[1].x, row0[1].width, row0[1].text.as_str()),
            (3, 5, "plain")
        );
        assert_eq!(row0[1].fg, Color::Named(NamedColor::Foreground));

        // Bold-on-blue "B", then " 日x": the wide char widens its run.
        let row1 = &snap.lines[1];
        assert_eq!(row1.len(), 2);
        assert!(row1[0].flags.contains(Flags::BOLD));
        assert_eq!(row1[0].bg, Color::Named(NamedColor::Blue));
        assert_eq!(
            (row1[1].x, row1[1].width, row1[1].text.as_str()),
            (1, 4, " 日x")
        );

        // Blank rows render as no runs at all.
        assert!(snap.lines[2].is_empty() && snap.lines[3].is_empty());
    }

    /// With scrollback: history lines printed first must scroll into the
    /// client's own history so the visible rows land exactly in its viewport.
    #[test]
    fn restore_sequence_aligns_history() {
        let mut a = Screen::new(20, 5);
        for i in 1..=12 {
            a.write(format!("\x1b[3{}mline-{i}\x1b[0m\r\n", i % 8).as_bytes());
        }
        a.write(b"bottom");

        let mut b = Screen::new(20, 5);
        b.write(a.restore_sequence().as_bytes());

        let snap_a = a.snapshot(true);
        let snap_b = b.snapshot(true);
        assert_eq!(snap_a.lines, snap_b.lines, "full buffer incl. history");
        assert_eq!(
            a.term.grid().cursor.point,
            b.term.grid().cursor.point,
            "cursor"
        );
    }
}
