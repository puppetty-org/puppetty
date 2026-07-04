//! Simulates a script that blocks on interactive prompts.

use std::io::{BufRead, Write};

fn ask(q: &str) -> String {
    print!("{q}");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).unwrap();
    line.trim_end_matches(['\r', '\n']).to_string()
}

fn main() {
    println!("Preparing installation...");
    std::thread::sleep(std::time::Duration::from_millis(1_500));

    let a1 = ask("Do you want to continue? [y/N] ");
    println!("  -> got: {a1:?}");
    if !a1.to_lowercase().starts_with('y') {
        println!("Aborted.");
        std::process::exit(1);
    }

    println!("Installing (simulated)...");
    std::thread::sleep(std::time::Duration::from_millis(2_000));

    let a2 = ask("Enable color output? (yes/no): ");
    println!("  -> got: {a2:?}");

    ask("Press Enter to finish...");
    println!("Done! All prompts were answered.");
}
