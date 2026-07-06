use std::sync::{Arc, OnceLock};

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

// Rasterize the SVG produced by svg::render() — one renderer for PNG files,
// MCP image results, and GIF frames, so they all look identical.

/// System fonts plus an embedded DejaVu Sans Mono (regular + bold) so text
/// renders even in bare containers with no fonts installed. Built once:
/// loading the system font list is far too slow to repeat per GIF frame.
fn fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        db.load_font_data(include_bytes!("../assets/fonts/DejaVuSansMono.ttf").to_vec());
        db.load_font_data(include_bytes!("../assets/fonts/DejaVuSansMono-Bold.ttf").to_vec());
        // The generic `monospace` at the end of svg.rs's font stack lands
        // here when none of the named families exist.
        db.set_monospace_family("DejaVu Sans Mono");
        Arc::new(db)
    })
    .clone()
}

/// Longest edge of a rasterized image; larger screens are scaled down
/// (vision models and chat UIs cap out around this size anyway).
const MAX_EDGE: f32 = 1600.0;

pub fn svg_to_pixmap(svg: &str) -> Result<Pixmap, String> {
    let opt = usvg::Options {
        fontdb: fontdb(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())?;
    let size = tree.size();
    let zoom = (MAX_EDGE / size.width().max(size.height())).min(1.0);
    let (w, h) = (
        (size.width() * zoom).round().max(1.0) as u32,
        (size.height() * zoom).round().max(1.0) as u32,
    );
    let mut pixmap = Pixmap::new(w, h).ok_or("cannot allocate image")?;
    resvg::render(
        &tree,
        Transform::from_scale(zoom, zoom),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

pub fn svg_to_png(svg: &str) -> Result<Vec<u8>, String> {
    svg_to_pixmap(svg)?.encode_png().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Screen;
    use crate::svg;

    /// Glyphs must actually rasterize (embedded font, no system deps): the
    /// canvas is not uniformly background-colored after drawing text.
    #[test]
    fn text_rasterizes_with_embedded_font() {
        let mut s = Screen::new(20, 2);
        s.write(b"\x1b[31mhello\x1b[0m");
        let pixmap = svg_to_pixmap(&svg::render(&s.styled_snapshot(), false)).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (188, 56));

        let bg = pixmap.pixel(2, 2).unwrap();
        assert!(
            pixmap
                .pixels()
                .iter()
                .any(|p| p.red() > bg.red().saturating_add(60)),
            "no reddish text pixels — glyphs did not render"
        );
        // Full-bleed background over an opaque canvas: no transparency.
        assert!(pixmap.pixels().iter().all(|p| p.alpha() == 255));
    }

    #[test]
    fn huge_screens_are_scaled_down() {
        let s = Screen::new(400, 30); // 400 cols → 3380px unscaled
        let pixmap = svg_to_pixmap(&svg::render(&s.styled_snapshot(), false)).unwrap();
        assert!(pixmap.width() <= 1600 && pixmap.height() <= 1600);
    }

    #[test]
    fn png_encodes() {
        let s = Screen::new(10, 2);
        let png = svg_to_png(&svg::render(&s.styled_snapshot(), false)).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }
}
