use std::env;
use std::path::PathBuf;

fn main() {
    let is_windows = env::var("CARGO_CFG_TARGET_OS").is_ok_and(|v| v == "windows");
    let is_msvc = env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|v| v == "msvc");

    if is_windows && is_msvc {
        // Temporary workaround for tauri-apps/tauri#13419 (tracked in #1):
        // tauri-build 2.7.6 links the Windows resource only into binaries via
        // `rustc-link-arg-bins`, so `cargo test` executables lack the Common Controls v6
        // manifest and fail on startup with STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139).
        let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        let attributes = tauri_build::Attributes::new().windows_attributes(windows);
        tauri_build::try_build(attributes).expect("failed to run tauri-build");
        embed_windows_manifest();
    } else {
        tauri_build::build();
    }
}

fn embed_windows_manifest() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    let manifest = PathBuf::from(manifest_dir).join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}
