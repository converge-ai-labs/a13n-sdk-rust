#![forbid(unsafe_code)]

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "agent-foundation",
    version,
    about = "Command-line client for Agent Foundation Service"
)]
struct Cli {}

fn main() {
    Cli::parse();
}
