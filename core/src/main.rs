mod classify;
mod fingerprint;
mod fsinfo;
mod media;
mod probe;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "shoebox", version, about = "Local photo library on an external drive")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check that this machine can read every file type in a folder,
    /// without modifying anything in it.
    Probe {
        /// Folder with sample photos and videos (searched recursively).
        folder: PathBuf,
        /// Where to write preview images (must be outside the folder).
        #[arg(long, default_value_os_t = std::env::temp_dir().join("shoebox-probe-previews"))]
        previews: PathBuf,
        /// Where to write the JSON report.
        #[arg(long, default_value = "shoebox-probe-report.json")]
        report: PathBuf,
        /// Stop after this many media files.
        #[arg(long)]
        limit: Option<usize>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Probe { folder, previews, report, limit } => probe::run(probe::Options {
            root: folder,
            thumbs: previews,
            report,
            limit,
        }),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
