//! Links the Windows resource (app icon as resource 1, which GPUI loads for its windows
//! and the taskbar, plus version info) into the exe. The `.res` is prebuilt by
//! `scripts/make-windows-icon.sh`, so no resource compiler is needed here.

fn main() {
    println!("cargo:rerun-if-changed=assets/windows/app.res");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    // MSVC's link.exe and lld-link take a compiled .res as a plain input file.
    if os == "windows" && env == "msvc" {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        println!("cargo:rustc-link-arg-bins={dir}/assets/windows/app.res");
    }
}
