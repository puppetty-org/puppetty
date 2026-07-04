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
    std::env::temp_dir()
        .join(format!("puppetty-{name}.sock"))
        .to_string_lossy()
        .into_owned()
}

/// Heuristic from the Node engine's policy.js: does this line look like it is
/// waiting for input? Progress/status output (percentages, "(3/10)", download
/// rates) is excluded even though it often ends in ')' or a digit.
pub fn is_promptish(line: &str) -> bool {
    static PROGRESS: OnceLock<regex::Regex> = OnceLock::new();
    static ENDING: OnceLock<regex::Regex> = OnceLock::new();
    let progress = PROGRESS
        .get_or_init(|| regex::Regex::new(r"\d\s*%|\(\d+/\d+\)|\bMiB\b|\bKiB\b|\bGiB\b").unwrap());
    if progress.is_match(line) {
        return false;
    }
    let ending = ENDING.get_or_init(|| regex::Regex::new(r"[:?>\])]\s*$").unwrap());
    ending.is_match(line) || line.contains('?')
}
