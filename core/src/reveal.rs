//! "Show in Finder / Explorer": ask the computer that runs `shoebox serve` to
//! open its file manager on a photo.
//!
//! The command is picked when shoebox is compiled (`cfg!(target_os)`), run
//! without a shell, and given only a path that `serve` took from the index. It
//! shows the original; it never opens it for writing. (The Finder may create a
//! `.DS_Store` in the folder, which the scanner skips.)

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// The text of the button: "Show in Finder", "Show in Explorer", or
/// "Open folder" where the file cannot be selected.
pub fn label() -> &'static str {
    match Platform::current() {
        Platform::Mac => "Show in Finder",
        Platform::Windows => "Show in Explorer",
        Platform::Other => "Open folder",
    }
}

/// What the file manager is called, for "Shown in …".
pub fn app_name() -> &'static str {
    match Platform::current() {
        Platform::Mac => "Finder",
        Platform::Windows => "Explorer",
        Platform::Other => "file manager",
    }
}

/// Program and arguments that show `path` for the platform shoebox was built
/// for. macOS and Windows select the file; on Linux there is no common way to
/// do that, so the folder is opened.
pub fn command(path: &Path) -> (OsString, Vec<OsString>) {
    command_for(Platform::current(), path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
    Other,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Other
        }
    }
}

/// `command` for a given platform, so every variant is testable anywhere.
pub fn command_for(platform: Platform, path: &Path) -> (OsString, Vec<OsString>) {
    match platform {
        // `--` so that a name starting with a dash is never read as an option.
        Platform::Mac => ("open".into(), vec!["-R".into(), "--".into(), path.as_os_str().to_owned()]),
        Platform::Windows => {
            // One argument: explorer wants `/select,` and the path together.
            // Canonical paths on Windows start with `\\?\`, which it rejects.
            let text = path.to_string_lossy();
            let plain = text.strip_prefix(r"\\?\").unwrap_or(&text);
            ("explorer.exe".into(), vec![format!("/select,{plain}").into()])
        }
        Platform::Other => {
            let folder = path.parent().unwrap_or(path);
            ("xdg-open".into(), vec![folder.as_os_str().to_owned()])
        }
    }
}

/// Show `path` in the file manager. Returns once the command has started; it
/// is reaped in the background (explorer.exe exits with 1 even when it worked,
/// so its status says nothing).
pub fn reveal(path: &Path) -> Result<()> {
    let (program, args) = command(path);
    let mut child = Command::new(&program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not start {}", program.to_string_lossy()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_selects_the_file_and_guards_dashes() {
        let (program, args) = command_for(Platform::Mac, Path::new("/Volumes/Fotos/-2020 Urlaub/a b.jpg"));
        assert_eq!(program, "open");
        assert_eq!(args, ["-R", "--", "/Volumes/Fotos/-2020 Urlaub/a b.jpg"]);
    }

    #[test]
    fn windows_selects_the_file_with_one_argument() {
        let (program, args) = command_for(Platform::Windows, Path::new(r"\\?\E:\Fotos\a b.jpg"));
        assert_eq!(program, "explorer.exe");
        assert_eq!(args, [r"/select,E:\Fotos\a b.jpg"]);
    }

    #[test]
    fn elsewhere_the_folder_is_opened() {
        let (program, args) = command_for(Platform::Other, Path::new("/media/fotos/2020-07 Urlaub/a.jpg"));
        assert_eq!(program, "xdg-open");
        assert_eq!(args, ["/media/fotos/2020-07 Urlaub"]);
    }
}
