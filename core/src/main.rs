use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use shoebox::{backup, faces, launcher, libraries, probe, recognize, report, scan, serve, verify};

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
        #[arg(long, conflicts_with = "index_next_to_program")]
        db: Option<PathBuf>,
        /// Keep the index next to the program (in `libraries/<name>/`) and write
        /// nothing into the library root; the folder is read-only for shoebox.
        /// Registers the folder first (see `shoebox libraries`).
        #[arg(long)]
        index_next_to_program: bool,
        /// Name of the library with `--index-next-to-program` (default: the
        /// folder's name).
        #[arg(long, requires = "index_next_to_program")]
        name: Option<String>,
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
    /// Find the faces (and with --pets the cats and dogs, with --text the words) in every photo (with the optional recognizer, see
    /// docs/protocol.md). Only reads originals; resumes where it stopped.
    Recognize {
        /// Library root (scanned before with `shoebox scan`).
        root: PathBuf,
        /// Database to use instead of `<root>/.shoebox/library.db`.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Recognizer program or `recognizer.py` (default: $SHOEBOX_RECOGNIZER,
        /// else the one in `recognizer/` next to shoebox).
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
        /// Then look for cats and dogs too (their own pass, with the pet
        /// models; resumes like the others). They show up among the faces.
        #[arg(long)]
        pets: bool,
        /// Then read the words in the photos too (their own pass, with the text
        /// models; resumes like the others). Screenshots first, then photos
        /// without camera data, then the rest.
        #[arg(long)]
        text: bool,
        /// Only read the words in the photos: no faces, no cats and dogs.
        #[arg(long)]
        text_only: bool,
        /// Print the result as JSON on standard output (the usual text goes to
        /// standard error).
        #[arg(long)]
        json: bool,
    },
    /// Is the backup complete? Compares a backup drive with the drive it
    /// copies by content (both scanned), without reading photos. Exits with
    /// status 2 if files are missing or different.
    Backup {
        /// The drive that is copied.
        primary: PathBuf,
        /// The backup drive.
        backup: PathBuf,
        /// Also re-read the backup's files and check them against its index
        /// (finds bit rot; reads the whole backup).
        #[arg(long)]
        deep: bool,
        /// List at most this many files of each kind.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Print the result as JSON on standard output (the usual text goes to
        /// standard error).
        #[arg(long)]
        json: bool,
    },
    /// Libraries whose index is kept next to the program instead of in the
    /// folder (a server, a shared or read-only folder): list, add, rename,
    /// point to another folder, remove, delete the index. Without a command:
    /// the list.
    Libraries {
        #[command(subcommand)]
        command: Option<LibrariesCommand>,
        /// Print the list as JSON on standard output (the usual text goes to
        /// standard error).
        #[arg(long)]
        json: bool,
    },
    /// Faces found by `shoebox recognize`.
    Faces {
        #[command(subcommand)]
        command: FacesCommand,
    },
    /// How every kind of recognition went on a drive: faces, pets, text.
    /// Only reads.
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
    /// The words read in the photos by `shoebox recognize --text`.
    Text {
        #[command(subcommand)]
        command: TextCommand,
    },
    /// Browse the library in a web browser. Only reads originals; missing
    /// thumbnails are made as they are viewed.
    Serve {
        /// Library root (scanned before with `shoebox scan`).
        root: PathBuf,
        /// More libraries (other drives) to open alongside. One that is not
        /// plugged in is shown as offline.
        more: Vec<PathBuf>,
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
        /// (default: $SHOEBOX_RECOGNIZER, else the one in `recognizer/` next to shoebox).
        #[arg(long)]
        recognizer: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum LibrariesCommand {
    /// Add a folder; nothing is written into it. Scan it afterwards.
    Add {
        /// The folder with the photos.
        root: PathBuf,
        /// A name for the library (default: the folder's name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Give a library another name (its index folder is renamed too).
    Rename { name: String, new_name: String },
    /// Point a library at another folder (the share moved, the drive letter
    /// changed). The index is kept; a folder that holds none of the indexed
    /// photos is refused unless --force.
    Root {
        name: String,
        new_root: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Take a library off the list; its index folder stays.
    Remove { name: String },
    /// Delete the index of a library and take it off the list (the photos are
    /// not touched; tags, dates and names added in shoebox go with it).
    Delete {
        name: String,
        /// Really do it.
        #[arg(long)]
        yes: bool,
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
        /// Cats and dogs (`recognize --pets`) instead of people's faces.
        #[arg(long)]
        pets: bool,
        /// Print the result as JSON on standard output (the usual text goes to
        /// standard error).
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum TextCommand {
    /// How reading the text went: photos read, with text, lines stored and
    /// shown, lines you hid, the limits. Only reads.
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
            | Command::Backup { json: true, .. }
            | Command::Libraries { json: true, .. }
            | Command::Faces { command: FacesCommand::Stats { json: true, .. } }
            | Command::Text { command: TextCommand::Stats { json: true, .. } }
            | Command::Stats { json: true, .. }
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
        Command::Scan { root, db, index_next_to_program, name, quick, no_thumbs, forget_missing, json } => {
            let registered = if index_next_to_program { libraries::register_for_scan(&root, name.as_deref()) } else { Ok(()) };
            registered
                .and_then(|()| scan::run(&scan::Options { root, db, full_hash: !quick, thumbs: !no_thumbs, forget_missing }))
                .map(|stats| {
                    json_out(json, &stats);
                    true
                })
        }
        Command::Libraries { command, json } => match command {
            None => libraries::cli_list().map(|v| {
                json_out(json, &v);
                true
            }),
            Some(LibrariesCommand::Add { root, name }) => libraries::cli_add(&root, name.as_deref()).map(|()| true),
            Some(LibrariesCommand::Rename { name, new_name }) => libraries::cli_rename(&name, &new_name).map(|()| true),
            Some(LibrariesCommand::Root { name, new_root, force }) => libraries::cli_root(&name, &new_root, force).map(|()| true),
            Some(LibrariesCommand::Remove { name }) => libraries::cli_remove(&name).map(|()| true),
            Some(LibrariesCommand::Delete { name, yes }) => libraries::cli_delete(&name, yes).map(|()| true),
        },
        Command::Verify { root, db, quick, limit, json } => {
            verify::run(&verify::Options { root, db, quick, limit }).map(|r| {
                json_out(json, &r);
                r.is_clean()
            })
        }
        Command::Recognize { root, db, recognizer, limit, retry_failed, rotated, pets, text, text_only, json } => {
            recognize::run(&recognize::Options {
                root,
                db,
                recognizer,
                limit,
                retry_failed,
                rotated,
                pets,
                text: text || text_only,
                text_only,
                timeouts: recognize::Timeouts::default(),
            })
            .map(|stats| {
                json_out(json, &stats);
                true
            })
        }
        Command::Backup { primary, backup, deep, limit, json } => {
            backup::run(&backup::Options { primary, backup, deep, limit }).map(|c| {
                json_out(json, &c);
                c.ok
            })
        }
        Command::Faces { command: FacesCommand::Stats { root, db, pets, json } } => {
            let space = if pets { shoebox::pets::Space::Pets } else { shoebox::pets::Space::Faces };
            faces::print_stats_of(&root, db.as_deref(), space).map(|stats| {
                json_out(json, &stats);
                true
            })
        }
        Command::Stats { root, db, json } => shoebox::overview::print_overview(&root, db.as_deref()).map(|o| {
            json_out(json, &o);
            true
        }),
        Command::Text { command: TextCommand::Stats { root, db, json } } => shoebox::text::print_stats(&root, db.as_deref()).map(|stats| {
            json_out(json, &stats);
            true
        }),
        Command::Serve { root, more, db, port, lan, pin, recognizer } => {
            serve::run(&serve::Options { root, more_roots: more, db, port, lan, pin, reveal: None, recognizer, regroup: Default::default(), hold: None }).map(|_| true)
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
