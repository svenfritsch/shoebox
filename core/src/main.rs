use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use shoebox::{faces, launcher, probe, recognize, report, scan, serve, verify};

#[derive(Parser)]
#[command(name = "shoebox", version, about = "Local photo library on an external drive")]
struct Cli {
    /// Without a command, shoebox opens the launcher page in the browser.
    #[command(subcommand)]
    command: Option<Command>,
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
    /// Index a library: new, changed, moved and missing files. Only
    /// `.shoebox/` in the library root is written to.
    Scan {
        /// Library root (the drive or the folder that holds the photos).
        root: PathBuf,
        /// Database to use instead of `<root>/.shoebox/library.db`.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Skip computing full hashes (the next scan catches up).
        #[arg(long)]
        quick: bool,
        /// Skip making thumbnails (`shoebox serve` makes them as they are viewed).
        #[arg(long)]
        no_thumbs: bool,
        /// Remove records of files that are no longer on the drive.
        #[arg(long)]
        forget_missing: bool,
        /// Print the result as JSON on standard output (the usual text goes to
        /// standard error).
        #[arg(long)]
        json: bool,
    },
    /// Re-read files and compare them with the index (missing, changed,
    /// damaged). Exits with status 2 if anything is wrong.
    Verify {
        /// Library root.
        root: PathBuf,
        /// Database to use instead of `<root>/.shoebox/library.db`.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Only compare size and modification date, without reading contents.
        #[arg(long)]
        quick: bool,
        /// Check at most this many files (least recently verified first).
        #[arg(long)]
        limit: Option<usize>,
        /// Print the result as JSON on standard output (the usual text goes to
        /// standard error).
        #[arg(long)]
        json: bool,
    },
    /// Find the faces in every photo (with the optional recognizer, see
    /// docs/protocol.md). Only reads originals; resumes where it stopped.
    Recognize {
        /// Library root (scanned before with `shoebox scan`).
        root: PathBuf,
        /// Database to use instead of `<root>/.shoebox/library.db`.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Recognizer program or `recognizer.py` (default: $SHOEBOX_RECOGNIZER,
        /// else the one in `<root>/.shoebox/recognizer/`).
        #[arg(long)]
        recognizer: Option<PathBuf>,
        /// Look at most at this many photos.
        #[arg(long)]
        limit: Option<usize>,
        /// Try photos again that could not be looked at before.
        #[arg(long)]
        retry_failed: bool,
        /// Then look at the photos turned 90° and 270° too, for faces of
        /// people lying down (about twice the time of the upright pass;
        /// resumes like it).
        #[arg(long)]
        rotated: bool,
        /// Print the result as JSON on standard output (the usual text goes to
        /// standard error).
        #[arg(long)]
        json: bool,
    },
    /// Faces found by `shoebox recognize`.
    Faces {
        #[command(subcommand)]
        command: FacesCommand,
    },
    /// Browse the library in a web browser. Only reads originals; missing
    /// thumbnails are made as they are viewed.
    Serve {
        /// Library root (scanned before with `shoebox scan`).
        root: PathBuf,
        /// Database to use instead of `<root>/.shoebox/library.db`.
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long, default_value_t = serve::DEFAULT_PORT)]
        port: u16,
        /// Let other devices on the network (an iPad) connect, with a PIN.
        #[arg(long)]
        lan: bool,
        /// PIN for other devices instead of a random one (at least 4 characters).
        #[arg(long)]
        pin: Option<String>,
        /// Recognizer for faces drawn by hand, started when one is drawn
        /// (default: $SHOEBOX_RECOGNIZER, else the one in `<root>/.shoebox/recognizer/`).
        #[arg(long)]
        recognizer: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum FacesCommand {
    /// How recognition went: photos looked at, errors, faces found, their
    /// sizes and scores, the last runs. Only reads.
    Stats {
        /// Library root.
        root: PathBuf,
        /// Database to use instead of `<root>/.shoebox/library.db`.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Print the result as JSON on standard output (the usual text goes to
        /// standard error).
        #[arg(long)]
        json: bool,
    },
}

fn json_out<T: serde::Serialize>(json: bool, value: &T) {
    if json {
        println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        return finish(launcher::run().map(|_| true));
    };
    // With --json the usual text moves to standard error.
    if matches!(
        &command,
        Command::Scan { json: true, .. }
            | Command::Verify { json: true, .. }
            | Command::Recognize { json: true, .. }
            | Command::Faces { command: FacesCommand::Stats { json: true, .. } }
    ) {
        report::text_to_stderr(true);
    }
    let result = match command {
        Command::Probe { folder, previews, report, limit } => probe::run(probe::Options {
            root: folder,
            thumbs: previews,
            report,
            limit,
        }),
        Command::Scan { root, db, quick, no_thumbs, forget_missing, json } => {
            scan::run(&scan::Options { root, db, full_hash: !quick, thumbs: !no_thumbs, forget_missing })
                .map(|stats| {
                    json_out(json, &stats);
                    true
                })
        }
        Command::Verify { root, db, quick, limit, json } => {
            verify::run(&verify::Options { root, db, quick, limit }).map(|r| {
                json_out(json, &r);
                r.is_clean()
            })
        }
        Command::Recognize { root, db, recognizer, limit, retry_failed, rotated, json } => {
            recognize::run(&recognize::Options {
                root,
                db,
                recognizer,
                limit,
                retry_failed,
                rotated,
                timeouts: recognize::Timeouts::default(),
            })
            .map(|stats| {
                json_out(json, &stats);
                true
            })
        }
        Command::Faces { command: FacesCommand::Stats { root, db, json } } => {
            faces::print_stats(&root, db.as_deref()).map(|stats| {
                json_out(json, &stats);
                true
            })
        }
        Command::Serve { root, db, port, lan, pin, recognizer } => {
            serve::run(&serve::Options { root, db, port, lan, pin, reveal: None, recognizer }).map(|_| true)
        }
    };
    finish(result)
}

fn finish(result: anyhow::Result<bool>) -> ExitCode {
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
