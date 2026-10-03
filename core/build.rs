// libheif and libde265 are linked statically (see scripts/build.sh) and are
// written in C++, so the platform's C++ runtime has to be linked too.
//
// On x86 macOS, libde265 picks its SSE/AVX2 kernels at runtime via
// `__builtin_cpu_supports`, which lives in clang's compiler runtime
// (libclang_rt.osx.a). Rust links with -nodefaultlibs, so we add it here.
use std::path::Path;
use std::process::Command;

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    match os.as_str() {
        "macos" => {
            println!("cargo:rustc-link-lib=c++");
            if arch == "x86_64" {
                link_clang_runtime();
            }
        }
        "windows" => {}
        // GNU ld resolves libraries in command-line order, so libstdc++ must
        // come after libheif; link args are appended at the very end.
        _ => println!("cargo:rustc-link-arg=-lstdc++"),
    }
}

fn link_clang_runtime() {
    let output = Command::new("clang")
        .arg("-print-file-name=lib/darwin/libclang_rt.osx.a")
        .output()
        .expect("clang not found (needed to locate libclang_rt.osx.a)");
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let lib = Path::new(&path);
    assert!(lib.is_file(), "libclang_rt.osx.a not found at {path}");
    println!("cargo:rustc-link-search=native={}", lib.parent().unwrap().display());
    println!("cargo:rustc-link-lib=static=clang_rt.osx");
}
