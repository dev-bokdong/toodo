fn main() {
    // Only the desktop app is a Tauri app; the server build has no Tauri context.
    #[cfg(feature = "desktop")]
    desktop();
}

#[cfg(feature = "desktop")]
fn desktop() {
    // tauri-build embeds the Common Controls v6 manifest in the app binary only, so test
    // binaries fail to start on Windows (STATUS_ENTRYPOINT_NOT_FOUND). Link it into every
    // target instead.
    let mut windows = tauri_build::WindowsAttributes::new();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        embed_windows_manifest();
    }
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
}

#[cfg(feature = "desktop")]
fn embed_windows_manifest() {
    let manifest = std::env::current_dir()
        .expect("build script has a current directory")
        .join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    println!("cargo:rustc-link-arg=/WX");
}
