//! A stand-in for an LLM decider (like `claude -p`). Reads the terminal tail
//! from stdin and prints one directive. Real usage: puppetty --decider "claude -p" ...

use std::io::Read;

fn main() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let lower = input.to_lowercase();
    if lower.contains("project name") {
        println!("SEND:my-cool-app");
    } else if lower.contains("password") {
        println!("CANCEL");
    } else {
        println!("WAIT");
    }
}
