//! Which data the desktop app shows: its own (the local database and the background work
//! around it), or a `toodo-server` it connects to as one of the server's clients. The
//! choice is kept in `connection.json` in the app data directory and read at start, so
//! switching restarts the app. The window learns the server from `window.__TOODO_SERVER__`,
//! set before the page loads; the access token stays with the window, kept per server.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{Manager, Url};

use crate::commands::{CommandError, CommandResult};

const FILE: &str = "connection.json";

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    /// The origin of the server, e.g. `https://toodo.example`; `None` for local data.
    pub server: Option<String>,
}

/// Reads the connection from `dir`. A missing file means local data; a file that cannot be
/// read or names no valid server falls back to local data too, with a warning.
pub fn load(dir: &Path) -> Connection {
    let path = dir.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Connection::default(),
        Err(error) => {
            eprintln!(
                "toodo: cannot read {}: {error}; using local data",
                path.display()
            );
            return Connection::default();
        }
    };
    let connection = match serde_json::from_str::<Connection>(&text) {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!(
                "toodo: {} is invalid: {error}; using local data",
                path.display()
            );
            return Connection::default();
        }
    };
    match connection.server.as_deref().map(server_origin) {
        Some(Err(error)) => {
            eprintln!("toodo: {}: {error}; using local data", path.display());
            Connection::default()
        }
        Some(Ok(server)) => Connection {
            server: Some(server),
        },
        None => Connection::default(),
    }
}

fn save(dir: &Path, connection: &Connection) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(connection).map_err(std::io::Error::other)?;
    std::fs::write(dir.join(FILE), json)
}

/// The origin of the server at `input`: an `http` or `https` URL with a host, without a
/// path, query, or credentials, since the API is at the root of the server.
pub fn server_origin(input: &str) -> Result<String, String> {
    let url =
        Url::parse(input.trim()).map_err(|error| format!("{input:?} is not a URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("{input:?} is not an http or https URL"));
    }
    if url.host().is_none() {
        return Err(format!("{input:?} has no host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{input:?} must not contain credentials"));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(format!(
            "{input:?} must be the address of the server only, without a path"
        ));
    }
    Ok(url.origin().ascii_serialization())
}

/// The script that tells the window which server to use before the page loads.
pub fn init_script(connection: &Connection) -> String {
    let server = serde_json::to_string(&connection.server).expect("a string serializes");
    format!("window.__TOODO_SERVER__ = {server};")
}

/// Sets which server the app uses from its next start: `server` (an `http` or `https`
/// address) or, for `None`, the local data. Returns the server as it will be used.
#[tauri::command]
#[specta::specta]
pub fn set_connection(
    app: tauri::AppHandle,
    server: Option<String>,
) -> CommandResult<Option<String>> {
    let server = server
        .as_deref()
        .map(server_origin)
        .transpose()
        .map_err(CommandError::invalid_input)?;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| CommandError::internal(error.to_string()))?;
    let connection = Connection { server };
    save(&dir, &connection)
        .map_err(|error| CommandError::internal(format!("cannot save {FILE}: {error}")))?;
    Ok(connection.server)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_origin_keeps_the_origin_of_a_server_address() {
        assert_eq!(
            server_origin("https://toodo.example").unwrap(),
            "https://toodo.example"
        );
        assert_eq!(
            server_origin(" https://Toodo.Example/ ").unwrap(),
            "https://toodo.example"
        );
        assert_eq!(
            server_origin("http://192.168.0.5:47616").unwrap(),
            "http://192.168.0.5:47616"
        );
        assert_eq!(
            server_origin("https://toodo.example:443").unwrap(),
            "https://toodo.example"
        );
    }

    #[test]
    fn server_origin_rejects_what_is_not_a_server_address() {
        for input in [
            "",
            "toodo.example",
            "ftp://toodo.example",
            "file:///srv/toodo",
            "https://user:secret@toodo.example",
            "https://toodo.example/toodo",
            "https://toodo.example/?token=x",
        ] {
            assert!(server_origin(input).is_err(), "{input:?}");
        }
    }

    #[test]
    fn load_falls_back_to_local_data() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), Connection::default());
        std::fs::write(dir.path().join(FILE), "not json").unwrap();
        assert_eq!(load(dir.path()), Connection::default());
        std::fs::write(dir.path().join(FILE), r#"{"server":"ftp://x"}"#).unwrap();
        assert_eq!(load(dir.path()), Connection::default());
    }

    #[test]
    fn a_saved_server_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let connection = Connection {
            server: Some("https://toodo.example".to_owned()),
        };
        save(dir.path(), &connection).unwrap();
        assert_eq!(load(dir.path()), connection);
        save(dir.path(), &Connection::default()).unwrap();
        assert_eq!(load(dir.path()), Connection::default());
    }

    #[test]
    fn the_init_script_sets_the_server_or_null() {
        assert_eq!(
            init_script(&Connection::default()),
            "window.__TOODO_SERVER__ = null;"
        );
        let remote = Connection {
            server: Some("https://toodo.example".to_owned()),
        };
        assert_eq!(
            init_script(&remote),
            r#"window.__TOODO_SERVER__ = "https://toodo.example";"#
        );
    }
}
