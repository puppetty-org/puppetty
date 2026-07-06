use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::Color;

use crate::screen::StyledSnapshot;

// Self-contained SVG rendering of a styled screen snapshot — text stays
// text (selectable, crisp at any zoom), no font rasterization needed.
// `textLength` pins every run to the terminal cell grid even when the
// viewer substitutes a monospace font with different metrics.

const CELL_W: f64 = 8.4; // 0.6 × font-size, the usual monospace advance
const CELL_H: f64 = 18.0;
const BASELINE: f64 = 13.5;
const FONT_SIZE: f64 = 14.0;
const PAD: f64 = 10.0;
const FONT_FAMILY: &str =
    "ui-monospace,'Cascadia Mono',Consolas,'DejaVu Sans Mono',Menlo,monospace";

const DEFAULT_BG: &str = "#1e1e1e";
const DEFAULT_FG: &str = "#cccccc";
/// VS Code's default dark palette — legible on the default background and
/// familiar to most users.
const PALETTE: [&str; 16] = [
    "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
    "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#ffffff",
];

/// Resolve a terminal color to hex; `None` means "the default for its role"
/// so the renderer can skip background rects that match the canvas.
fn hex(color: &Color) -> Option<String> {
    match color {
        Color::Named(n) => match *n as usize {
            i @ 0..=15 => Some(PALETTE[i].to_string()),
            _ => None, // Foreground/Background/Dim*/Cursor roles: default
        },
        Color::Indexed(i) => Some(indexed_hex(*i)),
        Color::Spec(rgb) => Some(format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b)),
    }
}

/// xterm 256-color: 16 base + 6×6×6 cube + 24-step grayscale ramp.
fn indexed_hex(i: u8) -> String {
    match i {
        0..=15 => PALETTE[i as usize].to_string(),
        16..=231 => {
            let n = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v as u16 };
            let (r, g, b) = (level(n / 36), level((n / 6) % 6), level(n % 6));
            format!("#{r:02x}{g:02x}{b:02x}")
        }
        232..=255 => {
            let g = 8 + 10 * (i - 232) as u16;
            format!("#{g:02x}{g:02x}{g:02x}")
        }
    }
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---- box-drawing / block characters ----
//
// Fonts disagree about these glyphs (metrics, fallback, gaps between rows),
// and TUI borders are exactly what an image snapshot must get right — so
// they are drawn geometrically on the cell grid instead of as text.

const U: u8 = 1;
const D: u8 = 2;
const L: u8 = 4;
const R: u8 = 8;

enum LineWeight {
    Light,
    Heavy,
    Double,
}

/// Direction bits + weight for pure-weight box-drawing chars; mixed-weight
/// hybrids (╒, ┽, …) and diagonals return None and fall back to the font.
fn line_char(c: char) -> Option<(u8, LineWeight)> {
    use LineWeight::*;
    Some(match c {
        '─' | '┄' | '┈' | '╌' => (L | R, Light),
        '━' | '┅' | '┉' | '╍' => (L | R, Heavy),
        '│' | '┆' | '┊' | '╎' => (U | D, Light),
        '┃' | '┇' | '┋' | '╏' => (U | D, Heavy),
        '┌' | '╭' => (D | R, Light),
        '┐' | '╮' => (D | L, Light),
        '└' | '╰' => (U | R, Light),
        '┘' | '╯' => (U | L, Light),
        '├' => (U | D | R, Light),
        '┤' => (U | D | L, Light),
        '┬' => (D | L | R, Light),
        '┴' => (U | L | R, Light),
        '┼' => (U | D | L | R, Light),
        '┏' => (D | R, Heavy),
        '┓' => (D | L, Heavy),
        '┗' => (U | R, Heavy),
        '┛' => (U | L, Heavy),
        '┣' => (U | D | R, Heavy),
        '┫' => (U | D | L, Heavy),
        '┳' => (D | L | R, Heavy),
        '┻' => (U | L | R, Heavy),
        '╋' => (U | D | L | R, Heavy),
        '═' => (L | R, Double),
        '║' => (U | D, Double),
        '╔' => (D | R, Double),
        '╗' => (D | L, Double),
        '╚' => (U | R, Double),
        '╝' => (U | L, Double),
        '╠' => (U | D | R, Double),
        '╣' => (U | D | L, Double),
        '╦' => (D | L | R, Double),
        '╩' => (U | L | R, Double),
        '╬' => (U | D | L | R, Double),
        '╴' => (L, Light),
        '╵' => (U, Light),
        '╶' => (R, Light),
        '╷' => (D, Light),
        '╸' => (L, Heavy),
        '╹' => (U, Heavy),
        '╺' => (R, Heavy),
        '╻' => (D, Heavy),
        _ => return None,
    })
}

/// Fraction of the cell a block-element char fills, anchored to a side:
/// (left, top, width, height) in cell units, plus fill opacity for shades.
fn block_char(c: char) -> Option<(f64, f64, f64, f64, f64)> {
    Some(match c {
        '█' => (0.0, 0.0, 1.0, 1.0, 1.0),
        '▀' => (0.0, 0.0, 1.0, 0.5, 1.0),
        '▄' => (0.0, 0.5, 1.0, 0.5, 1.0),
        '▌' => (0.0, 0.0, 0.5, 1.0, 1.0),
        '▐' => (0.5, 0.0, 0.5, 1.0, 1.0),
        // Lower eighths ▁▂▃▄▅▆▇ and left eighths ▏▎▍▌▋▊▉.
        '▁' | '▂' | '▃' | '▅' | '▆' | '▇' => {
            let n = match c {
                '▁' => 1.0,
                '▂' => 2.0,
                '▃' => 3.0,
                '▅' => 5.0,
                '▆' => 6.0,
                _ => 7.0,
            } / 8.0;
            (0.0, 1.0 - n, 1.0, n, 1.0)
        }
        '▏' | '▎' | '▍' | '▋' | '▊' | '▉' => {
            let n = match c {
                '▏' => 1.0,
                '▎' => 2.0,
                '▍' => 3.0,
                '▋' => 5.0,
                '▊' => 6.0,
                _ => 7.0,
            } / 8.0;
            (0.0, 0.0, n, 1.0, 1.0)
        }
        '▔' => (0.0, 0.0, 1.0, 0.125, 1.0),
        '▕' => (0.875, 0.0, 0.125, 1.0, 1.0),
        '░' => (0.0, 0.0, 1.0, 1.0, 0.25),
        '▒' => (0.0, 0.0, 1.0, 1.0, 0.5),
        '▓' => (0.0, 0.0, 1.0, 1.0, 0.75),
        _ => return None,
    })
}

fn is_drawn_cell(c: char) -> bool {
    line_char(c).is_some() || block_char(c).is_some()
}

/// SVG path data for the direction bits: full lines for opposite pairs, one
/// mitred polyline for a corner, stubs from the center otherwise (a stub's
/// butt end is always covered by the full line crossing it).
fn line_path_d(bits: u8, x: f64, y: f64) -> String {
    let (cx, cy) = (x + CELL_W / 2.0, y + CELL_H / 2.0);
    let (x2, y2) = (x + CELL_W, y + CELL_H);
    let mut d = String::new();
    let mut rest = bits;
    if bits & (U | D) == (U | D) {
        d.push_str(&format!("M{cx:.1} {y:.1}L{cx:.1} {y2:.1}"));
        rest &= !(U | D);
    }
    if bits & (L | R) == (L | R) {
        d.push_str(&format!("M{x:.1} {cy:.1}L{x2:.1} {cy:.1}"));
        rest &= !(L | R);
    }
    let vertical = if rest & U != 0 {
        Some(y)
    } else if rest & D != 0 {
        Some(y2)
    } else {
        None
    };
    let horizontal = if rest & L != 0 {
        Some(x)
    } else if rest & R != 0 {
        Some(x2)
    } else {
        None
    };
    match (vertical, horizontal) {
        // Corner: one polyline through the center, mitred by the renderer.
        (Some(vy), Some(hx)) => {
            d.push_str(&format!("M{cx:.1} {vy:.1}L{cx:.1} {cy:.1}L{hx:.1} {cy:.1}"))
        }
        (Some(vy), None) => d.push_str(&format!("M{cx:.1} {cy:.1}L{cx:.1} {vy:.1}")),
        (None, Some(hx)) => d.push_str(&format!("M{cx:.1} {cy:.1}L{hx:.1} {cy:.1}")),
        (None, None) => {}
    }
    d
}

/// Geometry for one drawn cell (top-left x/y), or None for font rendering.
/// `bg` is the resolved cell background — a double line is a wide stroke
/// with its middle struck back out in bg.
fn cell_geometry(c: char, x: f64, y: f64, fg: &str, bg: &str, dim: bool) -> Option<String> {
    let opacity = |base: f64| {
        let o = if dim { base * 0.6 } else { base };
        if o < 1.0 {
            format!(" opacity=\"{o}\"")
        } else {
            String::new()
        }
    };
    if let Some((bits, weight)) = line_char(c) {
        let d = line_path_d(bits, x, y);
        let op = opacity(1.0);
        return Some(match weight {
            LineWeight::Light => {
                format!(
                    "<path d=\"{d}\" stroke=\"{fg}\" stroke-width=\"1.5\" fill=\"none\"{op}/>\n"
                )
            }
            LineWeight::Heavy => {
                format!("<path d=\"{d}\" stroke=\"{fg}\" stroke-width=\"3\" fill=\"none\"{op}/>\n")
            }
            LineWeight::Double => format!(
                "<path d=\"{d}\" stroke=\"{fg}\" stroke-width=\"5\" fill=\"none\"{op}/>\n\
                 <path d=\"{d}\" stroke=\"{bg}\" stroke-width=\"2\" fill=\"none\"/>\n"
            ),
        });
    }
    let (bx, by, bw, bh, alpha) = block_char(c)?;
    Some(format!(
        "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{fg}\"{}/>\n",
        x + bx * CELL_W,
        y + by * CELL_H,
        bw * CELL_W,
        bh * CELL_H,
        opacity(alpha),
    ))
}

/// Render the snapshot as a standalone SVG document. `cursor` draws the
/// block cursor (pass false for exited sessions).
pub fn render(snap: &StyledSnapshot, cursor: bool) -> String {
    let width = snap.cols as f64 * CELL_W + 2.0 * PAD;
    let height = snap.rows as f64 * CELL_H + 2.0 * PAD;

    let mut rects = String::new();
    let mut texts = String::new();
    for (row, runs) in snap.lines.iter().enumerate() {
        let y_top = PAD + row as f64 * CELL_H;
        let y_base = y_top + BASELINE;
        for run in runs {
            let x = PAD + run.x as f64 * CELL_W;
            let w = run.width as f64 * CELL_W;

            let inverse = run.flags.contains(Flags::INVERSE);
            let (fg_role, bg_role) = if inverse {
                (&run.bg, &run.fg)
            } else {
                (&run.fg, &run.bg)
            };
            let fg = hex(fg_role)
                .unwrap_or_else(|| (if inverse { DEFAULT_BG } else { DEFAULT_FG }).to_string());
            let bg = match hex(bg_role) {
                Some(c) => Some(c),
                None if inverse => Some(DEFAULT_FG.to_string()),
                None => None,
            };

            if let Some(bg) = &bg {
                rects.push_str(&format!(
                    "<rect x=\"{x:.1}\" y=\"{y_top:.1}\" width=\"{w:.1}\" height=\"{CELL_H:.1}\" fill=\"{bg}\"/>\n"
                ));
            }
            if run.flags.contains(Flags::HIDDEN) || run.text.trim().is_empty() {
                continue;
            }

            let mut style_attrs = String::new();
            if run.flags.contains(Flags::BOLD) {
                style_attrs.push_str(" font-weight=\"bold\"");
            }
            if run.flags.contains(Flags::ITALIC) {
                style_attrs.push_str(" font-style=\"italic\"");
            }
            if run.flags.contains(Flags::DIM) {
                style_attrs.push_str(" opacity=\"0.6\"");
            }
            let mut deco = Vec::new();
            if run.flags.intersects(
                Flags::UNDERLINE
                    | Flags::DOUBLE_UNDERLINE
                    | Flags::UNDERCURL
                    | Flags::DOTTED_UNDERLINE
                    | Flags::DASHED_UNDERLINE,
            ) {
                deco.push("underline");
            }
            if run.flags.contains(Flags::STRIKEOUT) {
                deco.push("line-through");
            }
            if !deco.is_empty() {
                style_attrs.push_str(&format!(" text-decoration=\"{}\"", deco.join(" ")));
            }

            let text_span = |texts: &mut String, span: &str, start_cell: usize, cells: usize| {
                if span.trim().is_empty() {
                    return;
                }
                let sx = x + start_cell as f64 * CELL_W;
                let sw = cells as f64 * CELL_W;
                texts.push_str(&format!(
                    "<text x=\"{sx:.1}\" y=\"{y_base:.1}\" fill=\"{fg}\" textLength=\"{sw:.1}\" \
                     lengthAdjust=\"spacingAndGlyphs\"{style_attrs} xml:space=\"preserve\">{}</text>\n",
                    escape_xml(span)
                ));
            };

            // Box-drawing/block chars are drawn on the cell grid rather than
            // as glyphs. Only safe when every char is one cell wide (drawn
            // chars never mix with CJK in practice, so the fallback is rare).
            let char_count = run.text.chars().count();
            if char_count == run.width && run.text.chars().any(is_drawn_cell) {
                let bg_or_canvas = bg.as_deref().unwrap_or(DEFAULT_BG);
                let dim = run.flags.contains(Flags::DIM);
                let (mut span, mut span_start) = (String::new(), 0usize);
                for (ci, ch) in run.text.chars().enumerate() {
                    let cell_x = x + ci as f64 * CELL_W;
                    match cell_geometry(ch, cell_x, y_top, &fg, bg_or_canvas, dim) {
                        Some(geo) => {
                            text_span(&mut texts, &span, span_start, ci - span_start);
                            span.clear();
                            span_start = ci + 1;
                            texts.push_str(&geo);
                        }
                        None => span.push(ch),
                    }
                }
                text_span(&mut texts, &span, span_start, char_count - span_start);
            } else {
                text_span(&mut texts, &run.text, 0, run.width);
            }
        }
    }

    let cursor_rect = if cursor {
        let x = PAD + snap.cursor_x as f64 * CELL_W;
        let y = PAD + snap.cursor_y as f64 * CELL_H;
        format!(
            "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{CELL_W:.1}\" height=\"{CELL_H:.1}\" fill=\"{DEFAULT_FG}\" opacity=\"0.5\"/>\n"
        )
    } else {
        String::new()
    };

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width:.0}\" height=\"{height:.0}\" \
         viewBox=\"0 0 {width:.0} {height:.0}\" font-family=\"{FONT_FAMILY}\" font-size=\"{FONT_SIZE}\">\n\
         <rect width=\"100%\" height=\"100%\" fill=\"{DEFAULT_BG}\"/>\n\
         {rects}{cursor_rect}{texts}</svg>\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Screen;

    #[test]
    fn renders_colors_attributes_and_escapes() {
        let mut s = Screen::new(20, 3);
        s.write(b"\x1b[31m<red>\x1b[0m \x1b[1;4mbold\x1b[0m\r\n");
        s.write(b"\x1b[7minv\x1b[0m \x1b[38;5;208m256\x1b[0m \x1b[38;2;1;2;3mrgb\x1b[0m");

        let svg = render(&s.styled_snapshot(), true);
        assert!(svg.contains("fill=\"#cd3131\""), "named red");
        assert!(svg.contains("&lt;red&gt;"), "XML escaping");
        assert!(svg.contains("font-weight=\"bold\""));
        assert!(svg.contains("text-decoration=\"underline\""));
        // Inverse: default-fg text on default-fg background rect.
        assert!(
            svg.contains(&format!("fill=\"{DEFAULT_FG}\"/>")),
            "inverse bg rect"
        );
        assert!(svg.contains("fill=\"#ff8700\""), "indexed 208");
        assert!(svg.contains("fill=\"#010203\""), "truecolor");
        assert!(svg.contains("opacity=\"0.5\""), "cursor block");
        let no_cursor = render(&s.styled_snapshot(), false);
        assert!(!no_cursor.contains("opacity=\"0.5\""));
    }

    #[test]
    fn box_drawing_renders_as_geometry_not_glyphs() {
        let mut s = Screen::new(20, 3);
        s.write("┌─┐\r\n│x│\r\n╚═▓".as_bytes());
        let svg = render(&s.styled_snapshot(), false);
        assert!(svg.contains("<path d="), "line chars become paths");
        assert!(
            !svg.contains('┌') && !svg.contains('═'),
            "no drawn char reaches a <text>"
        );
        assert!(
            svg.contains("stroke-width=\"5\""),
            "double line: wide stroke..."
        );
        assert!(
            svg.contains("stroke=\"#1e1e1e\""),
            "...struck back out in bg"
        );
        assert!(svg.contains("opacity=\"0.75\""), "▓ shade");
        // The plain 'x' between pipes still renders as text, one cell wide.
        assert!(svg.contains(">x</text>"));
    }

    #[test]
    fn indexed_palette_formulas() {
        assert_eq!(indexed_hex(1), "#cd3131");
        assert_eq!(indexed_hex(16), "#000000");
        assert_eq!(indexed_hex(208), "#ff8700");
        assert_eq!(indexed_hex(231), "#ffffff");
        assert_eq!(indexed_hex(232), "#080808");
        assert_eq!(indexed_hex(255), "#eeeeee");
    }
}
