//! Simulates a password prompt — puppetty must NOT answer it, and should
//! cancel after --prompt-timeout.

use std::io::{BufRead, Write};

fn main() {
    print!("Enter your password: ");
    std::io::stdout().flush().unwrap();
    let mut pw = String::new();
    if std::io::stdin().lock().read_line(&mut pw).unwrap_or(0) > 0 {
        let pw = pw.trim_end_matches(['\r', '\n']);
        println!(
            "SECURITY FAILURE: something typed a password ({} chars)!",
            pw.len()
        );
        std::process::exit(1);
    }
}
