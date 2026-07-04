use crate::protocol::key_seq;

/// Expand `{key}` tokens (case-insensitive) inside `text` into their control
/// bytes — e.g. "y{enter}", "{down}{down}{enter}". Unknown tokens stay
/// literal. Optionally append Enter at the very end.
pub fn expand_input(text: &str, enter: bool) -> String {
    let re = regex::Regex::new(r"(?i)\{([a-z0-9-]+)\}").unwrap();
    let mut out = String::new();
    let mut last = 0;
    for m in re.captures_iter(text) {
        let whole = m.get(0).unwrap();
        out.push_str(&text[last..whole.start()]);
        let name = m[1].to_lowercase();
        let seq = key_seq(&name).or(match name.as_str() {
            "space" => Some(" "),
            "cr" | "return" => Some("\r"),
            "lf" => Some("\n"),
            "del" => Some("\x7f"),
            _ => None,
        });
        match seq {
            Some(s) => out.push_str(s),
            None => out.push_str(whole.as_str()), // keep unknown tokens as typed
        }
        last = whole.end();
    }
    out.push_str(&text[last..]);
    if enter {
        out.push('\r');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_tokens() {
        assert_eq!(expand_input("y{enter}", false), "y\r");
        assert_eq!(expand_input("{down}{down}{enter}", false), "\x1b[B\x1b[B\r");
        assert_eq!(expand_input("plain", true), "plain\r");
        assert_eq!(expand_input("{unknown}", false), "{unknown}");
    }
}
