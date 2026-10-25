//! Which data the desktop app shows: its own (the local database and the background work
//! around it), or a `toodo-server` it connects to as one of the server's clients. The
//! choice and the servers saved in the app are kept in `connection.json` in the app data
//! directory, read at start. Switching between this computer's data and a server restarts
//! the app; switching from one server to another only reloads the window, since the app
//! runs nothing of its own while it uses a server. The window learns the server from
//! `window.__TOODO_SERVER__`, set before the page loads; the access token stays with the
//! window, kept per server.

use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Manager, Url};

use crate::commands::{CommandError, CommandResult};

const FILE: &str = "connection.json";

const NAME_MAX_CHARS: usize = 80;

/// The session storage key under which the window keeps the server it switched to since
/// the app started (see [`init_script`]).
const SWITCHED_SERVER_KEY: &str = "toodo-switched-server";

/// Serializes the changes of `connection.json`, each of which reads and rewrites the file.
static FILE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Connection {
    /// The origin of the server in use, e.g. `https://toodo.example`; `None` for local data.
    pub server: Option<String>,
    /// The servers saved in the app, in the order they were added; `server` is one of them.
    #[serde(default)]
    pub servers: Vec<SavedServer>,
}

/// A server saved in the app, to switch to by its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SavedServer {
    pub name: String,
    /// The origin of the server.
    pub url: String,
}

impl Connection {
    /// Uses `server` (an origin, or `None` for local data), saving a server not saved yet
    /// under the name of its host.
    fn connect(&mut self, server: Option<String>) {
        if let Some(url) = &server
            && !self.servers.iter().any(|saved| &saved.url == url)
        {
            self.servers.push(SavedServer {
                name: default_name(url),
                url: url.clone(),
            });
        }
        self.server = server;
    }

    /// Saves the server at `url` (an origin) as `name`, renaming it if it is saved already.
    fn save_server(&mut self, url: String, name: String) {
        match self.servers.iter_mut().find(|saved| saved.url == url) {
            Some(saved) => saved.name = name,
            None => self.servers.push(SavedServer { name, url }),
        }
    }

    /// Removes the saved server at `url` (an origin); the server in use cannot be removed.
    fn remove_server(&mut self, url: &str) -> CommandResult<()> {
        if self.server.as_deref() == Some(url) {
            return Err(CommandError::rejected(format!(
                "{url} is the server in use; switch to another one before removing it"
            )));
        }
        let index = self
            .servers
            .iter()
            .position(|saved| saved.url == url)
            .ok_or_else(|| CommandError::not_found(format!("no saved server {url}")))?;
        self.servers.remove(index);
        Ok(())
    }
}

/// The name a server gets when it is saved without one: its host, with the port if any.
fn default_name(origin: &str) -> String {
    Url::parse(origin)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_owned();
            Some(match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_else(|| origin.to_owned())
}

/// Reads the connection from `dir`. A missing file means local data; a file that cannot be
/// read or names no valid server falls back to local data too, with a warning. Saved
/// servers that are not valid are left out, with a warning.
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
    let stored = match serde_json::from_str::<Connection>(&text) {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!(
                "toodo: {} is invalid: {error}; using local data",
                path.display()
            );
            return Connection::default();
        }
    };
    let mut connection = Connection::default();
    for saved in stored.servers {
        match (server_origin(&saved.url), server_name(&saved.name)) {
            (Ok(url), Ok(name)) => connection.save_server(url, name),
            (Err(error), _) | (_, Err(error)) => {
                eprintln!(
                    "toodo: {}: leaving out a saved server: {error}",
                    path.display()
                );
            }
        }
    }
    match stored.server.as_deref().map(server_origin) {
        Some(Err(error)) => {
            eprintln!("toodo: {}: {error}; using local data", path.display());
        }
        Some(Ok(server)) => connection.connect(Some(server)),
        None => {}
    }
    connection
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

/// The name of a saved server: not empty, at most 80 characters, on one line.
fn server_name(input: &str) -> Result<String, String> {
    let name = input.trim();
    if name.is_empty() {
        Err("a server needs a name".to_owned())
    } else if name.chars().count() > NAME_MAX_CHARS {
        Err(format!(
            "a server name has at most {NAME_MAX_CHARS} characters"
        ))
    } else if name.chars().any(char::is_control) {
        Err("a server name must be on one line".to_owned())
    } else {
        Ok(name.to_owned())
    }
}

/// The script that tells the window which server to use before the page loads: the one
/// the app started with or, once the window switched to another server (which the app
/// started with a server allows without a restart), that one.
pub fn init_script(connection: &Connection) -> String {
    let server = serde_json::to_string(&connection.server).expect("a string serializes");
    let key = serde_json::to_string(SWITCHED_SERVER_KEY).expect("a string serializes");
    format!(
        "window.__TOODO_SERVER__ = (() => {{ const started = {server}; if (started === null) return null; \
         try {{ return sessionStorage.getItem({key}) ?? started; }} catch {{ return started; }} }})();"
    )
}

/// Reads `connection.json`, applies `change`, and saves it. Returns the connection as saved.
fn update(
    app: &tauri::AppHandle,
    change: impl FnOnce(&mut Connection) -> CommandResult<()>,
) -> CommandResult<Connection> {
    let _guard = FILE_LOCK
        .lock()
        .map_err(|_| CommandError::internal("the connection lock is poisoned"))?;
    let dir = data_dir(app)?;
    let mut connection = load(&dir);
    change(&mut connection)?;
    save(&dir, &connection)
        .map_err(|error| CommandError::internal(format!("cannot save {FILE}: {error}")))?;
    Ok(connection)
}

fn data_dir(app: &tauri::AppHandle) -> CommandResult<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|error| CommandError::internal(error.to_string()))
}

fn origin(input: &str) -> CommandResult<String> {
    server_origin(input).map_err(CommandError::invalid_input)
}

/// The server in use and the servers saved in the app.
#[tauri::command]
#[specta::specta]
pub fn get_connection(app: tauri::AppHandle) -> CommandResult<Connection> {
    Ok(load(&data_dir(&app)?))
}

/// Sets which server the app uses: `server` (an `http` or `https` address, saved if it is
/// not saved yet) or, for `None`, the local data. Returns the server as it will be used.
/// The app uses it from its next start, or, switching from one server to another, from the
/// window's next load.
#[tauri::command]
#[specta::specta]
pub fn set_connection(
    app: tauri::AppHandle,
    server: Option<String>,
) -> CommandResult<Option<String>> {
    let server = server.as_deref().map(origin).transpose()?;
    let connection = update(&app, |connection| {
        connection.connect(server);
        Ok(())
    })?;
    Ok(connection.server)
}

/// Saves the server at `url` as `name`, or renames it if it is saved already.
#[tauri::command]
#[specta::specta]
pub fn save_server(app: tauri::AppHandle, url: String, name: String) -> CommandResult<Connection> {
    let url = origin(&url)?;
    let name = server_name(&name).map_err(CommandError::invalid_input)?;
    update(&app, |connection| {
        connection.save_server(url, name);
        Ok(())
    })
}

/// Removes the saved server at `url`, and the Access service token the app keeps for it;
/// the server in use cannot be removed.
#[tauri::command]
#[specta::specta]
pub fn remove_server(
    app: tauri::AppHandle,
    tokens: tauri::State<'_, crate::remote::access::AccessTokens>,
    url: String,
) -> CommandResult<Connection> {
    let url = origin(&url)?;
    let connection = update(&app, |connection| connection.remove_server(&url))?;
    tokens.set(&url, None).map_err(|error| {
        CommandError::internal(format!("cannot forget the service token of {url}: {error}"))
    })?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(name: &str, url: &str) -> SavedServer {
        SavedServer {
            name: name.to_owned(),
            url: url.to_owned(),
        }
    }

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
    fn server_name_is_one_line_of_at_most_80_characters() {
        assert_eq!(server_name("  Home  ").unwrap(), "Home");
        assert_eq!(
            server_name(&"\u{c9d1}".repeat(80)).unwrap().chars().count(),
            80
        );
        for input in ["", "   ", "two\nlines", &"x".repeat(81)] {
            assert!(server_name(input).is_err(), "{input:?}");
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
    fn a_saved_connection_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let connection = Connection {
            server: Some("https://toodo.example".to_owned()),
            servers: vec![
                saved("Home", "http://192.168.0.5:47616"),
                saved("Work", "https://toodo.example"),
            ],
        };
        save(dir.path(), &connection).unwrap();
        assert_eq!(load(dir.path()), connection);
        save(dir.path(), &Connection::default()).unwrap();
        assert_eq!(load(dir.path()), Connection::default());
    }

    #[test]
    fn a_file_of_one_server_saves_it_under_its_host() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE),
            r#"{"server":"http://192.168.0.5:47616"}"#,
        )
        .unwrap();
        assert_eq!(
            load(dir.path()),
            Connection {
                server: Some("http://192.168.0.5:47616".to_owned()),
                servers: vec![saved("192.168.0.5:47616", "http://192.168.0.5:47616")],
            }
        );
    }

    #[test]
    fn invalid_saved_servers_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE),
            r#"{"server":null,"servers":[
                {"name":"Home","url":"https://home.example/"},
                {"name":"","url":"https://empty.example"},
                {"name":"Path","url":"https://toodo.example/toodo"},
                {"name":"Again","url":"https://home.example"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            load(dir.path()),
            Connection {
                server: None,
                servers: vec![saved("Again", "https://home.example")],
            }
        );
    }

    #[test]
    fn connecting_saves_a_new_server_and_keeps_the_others() {
        let mut connection = Connection::default();
        connection.connect(Some("https://toodo.example".to_owned()));
        connection.save_server("https://toodo.example".to_owned(), "Work".to_owned());
        connection.connect(Some("http://192.168.0.5:47616".to_owned()));
        connection.connect(Some("https://toodo.example".to_owned()));
        connection.connect(None);
        assert_eq!(
            connection,
            Connection {
                server: None,
                servers: vec![
                    saved("Work", "https://toodo.example"),
                    saved("192.168.0.5:47616", "http://192.168.0.5:47616"),
                ],
            }
        );
    }

    #[test]
    fn only_a_saved_server_not_in_use_is_removed() {
        let mut connection = Connection::default();
        connection.save_server("https://home.example".to_owned(), "Home".to_owned());
        connection.connect(Some("https://work.example".to_owned()));

        let in_use = connection
            .remove_server("https://work.example")
            .unwrap_err();
        assert_eq!(in_use.kind, crate::commands::CommandErrorKind::Rejected);
        let unknown = connection
            .remove_server("https://other.example")
            .unwrap_err();
        assert_eq!(unknown.kind, crate::commands::CommandErrorKind::NotFound);

        connection.remove_server("https://home.example").unwrap();
        assert_eq!(
            connection.servers,
            vec![saved("work.example", "https://work.example")]
        );
    }

    #[test]
    fn the_init_script_sets_the_server_or_null() {
        let local = init_script(&Connection::default());
        assert!(
            local.starts_with("window.__TOODO_SERVER__ = (() => { const started = null;"),
            "{local}"
        );
        let remote = init_script(&Connection {
            server: Some("https://toodo.example".to_owned()),
            servers: Vec::new(),
        });
        assert!(
            remote.contains(r#"const started = "https://toodo.example";"#),
            "{remote}"
        );
        assert!(
            remote.contains(r#"sessionStorage.getItem("toodo-switched-server")"#),
            "{remote}"
        );
    }
}
