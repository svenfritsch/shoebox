// libheif and libde265 are shared libraries that ship next to the program
// (scripts/build-deps.sh, LGPL: whoever has shoebox can swap in their own
// build). The program finds them in its own folder; libheif brings the C++
// runtime and libde265 its compiler runtime with it.
//
// Debug builds and tests run from core/target, so they also look in the
// folder the libraries were built into (scripts/build.sh hands it over in
// SYSTEM_DEPS_LIBHEIF_SEARCH_NATIVE). Release builds carry no build path.

fn main() {
    println!("cargo:rerun-if-env-changed=SYSTEM_DEPS_LIBHEIF_SEARCH_NATIVE");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let next_to_program = match os.as_str() {
        "macos" => "@executable_path",
        "windows" => return, // Windows looks next to the .exe by itself
        _ => "$ORIGIN",      // expanded by the dynamic loader, not a shell
    };
    println!("cargo:rustc-link-arg=-Wl,-rpath,{next_to_program}");
    if std::env::var("PROFILE").as_deref() != Ok("release")
        && let Ok(dir) = std::env::var("SYSTEM_DEPS_LIBHEIF_SEARCH_NATIVE")
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
