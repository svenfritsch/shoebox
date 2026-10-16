//! The Control Panel's path helpers (`core/launcher-web/launcher.js`) run under
//! Node: `core/tests/js/launcher.test.js`. Skipped, with a note, where there is
//! no `node` (CI for the guide has it).

use std::process::Command;

#[test]
fn launcher_path_helpers() {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/js/launcher.test.js");
    match Command::new("node").arg(script).output() {
        Ok(out) => assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
        Err(_) => eprintln!("node not found: launcher.js helpers not tested"),
    }
}
