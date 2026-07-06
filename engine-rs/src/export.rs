use std::io::Write;
use std::path::Path;

use serde_json::Value;

use crate::screen::Screen;
use crate::{raster, svg};

// Offline .cast → animated GIF. Nothing extra is recorded at runtime: the
// session log already carries the timestamped output stream, so a recording
// of any past session can be rendered after the fact.

/// Pauses longer than this are compressed to it (nobody wants to watch a
/// GIF idle for a minute).
const IDLE_CAP_SECS: f64 = 5.0;
/// Rasterization cost per frame is what bounds export time; past this the
/// GIF is truncated with a warning rather than silently grinding on.
const MAX_FRAMES: usize = 1_500;
/// Hold on the final screen before the loop restarts.
const LAST_FRAME_SECS: f64 = 2.0;

pub struct ExportStats {
    pub frames: usize,
    pub truncated: bool,
    pub width: u32,
    pub height: u32,
}

pub fn cast_to_gif(cast: &Path, out: &Path, fps: f64) -> Result<ExportStats, String> {
    let text = std::fs::read_to_string(cast)
        .map_err(|e| format!("cannot read {}: {e}", cast.display()))?;
    let mut lines = text.lines();
    let header: Value = lines
        .next()
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let cols = header["width"].as_u64().unwrap_or(120) as u16;
    let rows = header["height"].as_u64().unwrap_or(30) as u16;

    let mut events: Vec<(f64, String)> = Vec::new();
    for line in lines {
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if ev[1].as_str() == Some("o") {
            if let (Some(t), Some(data)) = (ev[0].as_f64(), ev[2].as_str()) {
                events.push((t, data.to_string()));
            }
        }
    }

    // Coalesce events into frames at most `fps` apart; identical renders
    // merge into one frame whose delay simply grows.
    let min_dt = 1.0 / fps.clamp(0.1, 50.0);
    let mut screen = Screen::new(cols, rows);
    let mut frames: Vec<(String, f64)> = Vec::new(); // (svg, shown-at)
    let mut truncated = false;
    let mut i = 0;
    while i < events.len() {
        let bucket_t = events[i].0;
        while i < events.len() && events[i].0 - bucket_t < min_dt {
            screen.write(events[i].1.as_bytes());
            i += 1;
        }
        let doc = svg::render(&screen.styled_snapshot(), true);
        if frames.last().map(|(prev, _)| prev != &doc).unwrap_or(true) {
            if frames.len() >= MAX_FRAMES {
                truncated = true;
                break;
            }
            frames.push((doc, bucket_t));
        }
    }
    if frames.is_empty() {
        frames.push((svg::render(&screen.styled_snapshot(), true), 0.0));
    }

    // The first frame fixes the canvas size for the whole GIF.
    let mut first = Some(raster::svg_to_pixmap(&frames[0].0)?);
    let (width, height) = {
        let p = first.as_ref().unwrap();
        (p.width(), p.height())
    };
    let file =
        std::fs::File::create(out).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    let mut encoder = gif::Encoder::new(
        std::io::BufWriter::new(file),
        width as u16,
        height as u16,
        &[],
    )
    .map_err(|e| e.to_string())?;
    encoder
        .set_repeat(gif::Repeat::Infinite)
        .map_err(|e| e.to_string())?;
    for (idx, (doc, t)) in frames.iter().enumerate() {
        let pixmap = match first.take() {
            Some(p) => p,
            None => raster::svg_to_pixmap(doc)?,
        };
        let shown_for = frames
            .get(idx + 1)
            .map(|(_, next_t)| (next_t - t).clamp(min_dt, IDLE_CAP_SECS))
            .unwrap_or(LAST_FRAME_SECS);
        let mut rgba = pixmap.data().to_vec();
        let mut frame = gif::Frame::from_rgba_speed(width as u16, height as u16, &mut rgba, 10);
        frame.delay = (shown_for * 100.0).round() as u16; // centiseconds
        encoder.write_frame(&frame).map_err(|e| e.to_string())?;
    }
    // Drop writes the trailer; flush explicitly so write errors surface.
    encoder
        .into_inner()
        .map_err(|e| e.to_string())?
        .flush()
        .map_err(|e| e.to_string())?;

    Ok(ExportStats {
        frames: frames.len(),
        truncated,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_exports_a_looping_gif() {
        let dir = std::env::temp_dir().join(format!("puppetty-gif-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cast = dir.join("t.cast");
        std::fs::write(
            &cast,
            concat!(
                "{\"version\":2,\"width\":20,\"height\":4}\n",
                "[0.0,\"o\",\"hello\"]\n",
                "[0.5,\"o\",\" \\u001b[31mred\\u001b[0m\"]\n",
                "[9.0,\"o\",\"\\r\\ndone\"]\n", // long pause → idle cap
            ),
        )
        .unwrap();
        let out = dir.join("t.gif");
        let stats = cast_to_gif(&cast, &out, 10.0).unwrap();
        assert_eq!(stats.frames, 3);
        assert!(!stats.truncated);

        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[..6], b"GIF89a");
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = opts.read_info(std::io::Cursor::new(&bytes)).unwrap();
        let mut delays = Vec::new();
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            delays.push(frame.delay);
        }
        // Pause between frames 2 and 3 was 8.5s — compressed to the 5s cap.
        assert_eq!(delays.len(), 3);
        assert_eq!(delays[1], 500);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
