//! Simulates a script with a free-text prompt no built-in rule can answer —
//! exercises the --decider path.

use std::io::{BufRead, Write};

fn main() {
    print!("Project name: ");
    std::io::stdout().flush().unwrap();
    let mut name = String::new();
    std::io::stdin().lock().read_line(&mut name).unwrap();
    let name = name.trim_end_matches(['\r', '\n']);
    println!("Creating project {name:?}...");
    println!("Done.");
}
