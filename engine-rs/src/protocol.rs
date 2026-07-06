use std::path::PathBuf;
use std::sync::OnceLock;

/// Key-name → escape-sequence map, identical to the Node engine's KEYMAP.
pub fn key_seq(key: &str) -> Option<&'static str> {
    Some(match key {
        "enter" => "\r",
        "tab" => "\t",
        "esc" => "\x1b",
        "space" => " ",
        "backspace" => "\x7f",
        "up" => "\x1b[A",
        "down" => "\x1b[B",
        "right" => "\x1b[C",
        "left" => "\x1b[D",
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        "ctrl-c" => "\x03",
        "ctrl-d" => "\x04",
        "ctrl-z" => "\x1a",
        _ => return None,
    })
}

pub fn sessions_dir() -> PathBuf {
    let dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".puppetty")
        .join("sessions");
    std::fs::create_dir_all(&dir).ok();
    dir
}

pub fn meta_path(name: &str) -> PathBuf {
    sessions_dir().join(format!("{name}.json"))
}

#[cfg(windows)]
pub fn pipe_path(name: &str) -> String {
    format!(r"\\.\pipe\puppetty-{name}")
}

#[cfg(not(windows))]
pub fn pipe_path(name: &str) -> String {
    // ~/.puppetty/run rather than the temp dir: macOS TMPDIR is ~50 chars
    // against the 104-byte sun_path cap, and it differs between login
    // contexts (SSH vs GUI), which made sessions started in one invisible
    // from the other. Long names are shortened with a hash to stay under
    // the cap. Mirrored in gui/src-tauri/src/main.rs.
    let dir = dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(".puppetty")
        .join("run");
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let file = if name.len() <= 40 {
        format!("{name}.sock")
    } else {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut h);
        let prefix: String = name.chars().take(24).collect();
        format!("{prefix}-{:016x}.sock", h.finish())
    };
    dir.join(file).to_string_lossy().into_owned()
}

/// The control endpoint for a session: prefer the path the host recorded in
/// the registry (it survives engine-version and environment differences),
/// falling back to the computed path.
pub fn endpoint_for(name: &str) -> String {
    if let Ok(text) = std::fs::read_to_string(meta_path(name)) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(p) = v["pipe"].as_str() {
                if !p.is_empty() {
                    return p.to_string();
                }
            }
        }
    }
    pipe_path(name)
}

/// A promptish line with the cursor elsewhere is weaker evidence that the
/// program is really reading input (a build step pausing after "Compiling
/// deps:" leaves the cursor on the next line) — so the quiet period gets
/// multiplied by this before such a screen may classify as a prompt.
pub const MISALIGNED_CURSOR_QUIET_FACTOR: u64 = 3;

/// Is the cursor where a program reading line input would leave it — on the
/// last non-empty line, at or past the end of its text? Line prompts
/// (readline, inquirer, `[y/N]`, password) hold the cursor there; a program
/// that merely paused mid-output usually leaves it at the start of the next
/// line. Column is compared in chars, an approximation for wide-char lines.
pub fn cursor_at_prompt(lines: &[String], cursor_x: usize, cursor_y: usize) -> bool {
    let Some(last) = lines.iter().rposition(|l| !l.trim().is_empty()) else {
        return false;
    };
    cursor_y == last && cursor_x >= lines[last].trim_end().chars().count()
}

/// Heuristic from the Node engine's policy.js: does this line look like it is
/// waiting for input? Progress/status output (percentages, "(3/10)", download
/// rates) is excluded even though it often ends in ')' or a digit.
pub fn is_promptish(line: &str) -> bool {
    static PROGRESS: OnceLock<regex::Regex> = OnceLock::new();
    static ENDING: OnceLock<regex::Regex> = OnceLock::new();
    static KEYPRESS: OnceLock<regex::Regex> = OnceLock::new();
    let progress = PROGRESS
        .get_or_init(|| regex::Regex::new(r"\d\s*%|\(\d+/\d+\)|\bMiB\b|\bKiB\b|\bGiB\b").unwrap());
    if progress.is_match(line) {
        return false;
    }
    // "Press ENTER to open in the browser..." (npm's web login) ends in an
    // ellipsis, which the ending heuristic can't accept in general — every
    // "Compiling..." would become a prompt. Match the phrasing instead.
    let keypress = KEYPRESS
        .get_or_init(|| regex::Regex::new(r"(?i)\bpress\s+(enter|return|any key)\b").unwrap());
    let ending = ENDING.get_or_init(|| regex::Regex::new(r"[:?>\])]\s*$").unwrap());
    ending.is_match(line) || line.contains('?') || keypress.is_match(line)
}

#[cfg(test)]
mod cursor_tests {
    use super::cursor_at_prompt;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cursor_after_prompt_text_is_aligned() {
        // "Project name:" with the cursor right after the trailing space.
        let l = lines(&["setup", "Project name:"]);
        assert!(cursor_at_prompt(&l, 14, 1));
        assert!(cursor_at_prompt(&l, 13, 1)); // exactly at the end
    }

    #[test]
    fn cursor_elsewhere_is_not_aligned() {
        let l = lines(&["Compiling deps:"]);
        assert!(
            !cursor_at_prompt(&l, 0, 1),
            "start of next line: paused output"
        );
        assert!(!cursor_at_prompt(&l, 4, 0), "mid-line: TUI drawing");
        assert!(!cursor_at_prompt(&[String::new()], 0, 0), "empty screen");
    }

    #[test]
    fn trailing_blank_lines_do_not_shift_the_target() {
        let l = lines(&["Continue? [y/N]", "", ""]);
        assert!(cursor_at_prompt(&l, 16, 0));
        assert!(!cursor_at_prompt(&l, 0, 2));
    }
}

#[cfg(test)]
mod promptish_tests {
    use super::is_promptish;

    #[test]
    fn press_enter_with_trailing_ellipsis_is_promptish() {
        assert!(is_promptish("Press ENTER to open in the browser..."));
        assert!(is_promptish("press any key to continue . . ."));
        assert!(is_promptish("Press Return to accept the default"));
    }

    #[test]
    fn progress_ellipsis_is_not_promptish() {
        assert!(!is_promptish("Compiling..."));
        assert!(!is_promptish("Downloading 42 MiB..."));
        assert!(!is_promptish("installing dependencies..."));
    }
}
