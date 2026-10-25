mod adapters;
mod commands;
#[cfg(feature = "desktop")]
mod connection;
#[cfg(feature = "desktop")]
mod desktop;
pub mod domain;
mod mcp;
#[cfg(feature = "desktop")]
mod remote;
#[cfg(feature = "server")]
pub mod server;
pub mod storage;

// Commands take Tauri state in the desktop app and plain references in the server, so a
// build has exactly one of the two.
#[cfg(all(feature = "desktop", feature = "server"))]
compile_error!(
    "build either the desktop app (default) or the server (--no-default-features --features server)"
);

#[cfg(feature = "desktop")]
pub use desktop::run;
