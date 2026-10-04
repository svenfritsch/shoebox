use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use shoebox::{faces, probe, recognize, scan, serve, verify};

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
        Command::Scan { root, db, quick, no_thumbs, forget_missing } => {
            scan::run(&scan::Options { root, db, full_hash: !quick, thumbs: !no_thumbs, forget_missing })
                .map(|_| true)
        }
        Command::Verify { root, db, quick, limit } => {
            verify::run(&verify::Options { root, db, quick, limit }).map(|r| r.is_clean())
        }
        Command::Recognize { root, db, recognizer, limit, retry_failed, rotated } => {
            recognize::run(&recognize::Options {
                root,
                db,
                recognizer,
                limit,
                retry_failed,
                rotated,
                timeouts: recognize::Timeouts::default(),
            })
            .map(|_| true)
        }
        Command::Faces { command: FacesCommand::Stats { root, db } } => faces::print_stats(&root, db.as_deref()).map(|_| true),
        Command::Serve { root, db, port, lan, pin } => {
            serve::run(&serve::Options { root, db, port, lan, pin, reveal: None }).map(|_| true)
        }
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
