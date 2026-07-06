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

            if let Some(bg) = bg {
                rects.push_str(&format!(
                    "<rect x=\"{x:.1}\" y=\"{y_top:.1}\" width=\"{w:.1}\" height=\"{CELL_H:.1}\" fill=\"{bg}\"/>\n"
                ));
            }
            if run.flags.contains(Flags::HIDDEN) || run.text.trim().is_empty() {
                continue;
            }

            let mut attrs = format!(
                "x=\"{x:.1}\" y=\"{y_base:.1}\" fill=\"{fg}\" textLength=\"{w:.1}\" lengthAdjust=\"spacingAndGlyphs\""
            );
            if run.flags.contains(Flags::BOLD) {
                attrs.push_str(" font-weight=\"bold\"");
            }
            if run.flags.contains(Flags::ITALIC) {
                attrs.push_str(" font-style=\"italic\"");
            }
            if run.flags.contains(Flags::DIM) {
                attrs.push_str(" opacity=\"0.6\"");
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
                attrs.push_str(&format!(" text-decoration=\"{}\"", deco.join(" ")));
            }
            texts.push_str(&format!(
                "<text {attrs} xml:space=\"preserve\">{}</text>\n",
                escape_xml(&run.text)
            ));
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
    fn indexed_palette_formulas() {
        assert_eq!(indexed_hex(1), "#cd3131");
        assert_eq!(indexed_hex(16), "#000000");
        assert_eq!(indexed_hex(208), "#ff8700");
        assert_eq!(indexed_hex(231), "#ffffff");
        assert_eq!(indexed_hex(232), "#080808");
        assert_eq!(indexed_hex(255), "#eeeeee");
    }
}
