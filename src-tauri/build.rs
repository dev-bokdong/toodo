fn main() {
    commit();
    // Only the desktop app is a Tauri app; the server build has no Tauri context.
    #[cfg(feature = "desktop")]
    desktop();
}

/// Sets `TOODO_COMMIT` to the commit being built (empty outside a git checkout), so that
/// `toodo-server --version` shows when a build is not the release it reports.
fn commit() {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let dir = std::path::Path::new(&dir);
        // HEAD changes on a checkout, its log on every commit.
        for file in [dir.join("HEAD"), dir.join("logs").join("HEAD")] {
            if file.exists() {
                println!("cargo:rerun-if-changed={}", file.display());
            }
        }
    }
    let commit = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=TOODO_COMMIT={commit}");
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
    // The app's own plugin that sends the window's requests to a server (src/remote/).
    let remote = tauri_build::InlinedPlugin::new()
        .commands(&[
            "fetch",
            "read",
            "cancel",
            "socket_open",
            "socket_receive",
            "socket_send",
            "socket_close",
        ])
        .default_permission(tauri_build::DefaultPermissionRule::AllowAllCommands);
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(windows)
            .plugin("remote", remote),
    )
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
