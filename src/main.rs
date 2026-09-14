use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(name = "sosie", version = "1.0", about = "TO DEFINE")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Transform,
    Init,
    Check,
    Presets,
}

fn main() -> ExitCode {
    let args = Cli::parse();

    match args.command {
        Command::Transform => {
            eprintln!("Not yet implemented");
        }
        Command::Init => {
            eprintln!("Not yet implemented");
        }
        Command::Check => {
            eprintln!("Not yet implemented");
        }
        Command::Presets => {
            eprintln!("Not yet implemented");
        }
    }

    ExitCode::FAILURE
}
