//! `meet-cli`: developer entry point to `meet-core` without the UI.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "meet-cli", version, about = "meet-assist developer CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the meet-core version.
    Version,
    /// Transcribe and diarize an audio or video file (not implemented yet).
    Transcribe {
        /// Input file.
        file: PathBuf,
    },
}

/// Exit code for commands that exist but do nothing yet.
const EXIT_NOT_IMPLEMENTED: u8 = 2;

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Version => {
            println!("meet-cli {}", meet_core::VERSION);
            ExitCode::SUCCESS
        }
        Command::Transcribe { file } => {
            eprintln!("transcribe {}: not implemented", file.display());
            ExitCode::from(EXIT_NOT_IMPLEMENTED)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_transcribe() {
        let cli = Cli::try_parse_from(["meet-cli", "transcribe", "a.wav"]).unwrap();
        assert!(
            matches!(cli.command, Command::Transcribe { file } if file == PathBuf::from("a.wav"))
        );
        assert!(Cli::try_parse_from(["meet-cli", "transcribe"]).is_err());
    }
}
