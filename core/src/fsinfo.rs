//! Which filesystem a path lives on (we expect exFAT on the photo drive).

use std::path::Path;

#[cfg(target_os = "macos")]
pub fn filesystem_type(path: &Path) -> Option<String> {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let name = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) };
    Some(name.to_string_lossy().into_owned())
}

#[cfg(target_os = "linux")]
pub fn filesystem_type(path: &Path) -> Option<String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_path.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let name = match st.f_type as u64 {
        0x2011_BAB0 => "exfat",
        0x4d44 => "vfat",
        0x5346_544e => "ntfs",
        0xEF53 => "ext4",
        0x9123_683E => "btrfs",
        0x5846_5342 => "xfs",
        0x0102_1994 => "tmpfs",
        0x794c_7630 => "overlayfs",
        0x6573_5546 => "fuse",
        0x6a65_6a63 => "virtiofs",
        other => return Some(format!("unknown (0x{other:x})")),
    };
    Some(name.to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn filesystem_type(_path: &Path) -> Option<String> {
    None
}

/// Human-readable OS name and version, for the probe report.
pub fn os_description() -> String {
    let version = if cfg!(target_os = "macos") {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    } else if cfg!(target_os = "linux") {
        std::fs::read_to_string("/etc/os-release").ok().and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
    } else {
        None
    };
    format!(
        "{} {} ({})",
        std::env::consts::OS,
        version.unwrap_or_default(),
        std::env::consts::ARCH
    )
}
