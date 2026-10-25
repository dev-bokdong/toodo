//! Cloudflare Access service tokens of the servers the desktop app connects to. A server
//! published through a Cloudflare Tunnel with Access in front only lets requests through
//! that sign in; the app cannot follow Access's login page, so it sends a service token
//! (`CF-Access-Client-Id` and `CF-Access-Client-Secret`) with every request instead.
//!
//! The tokens are kept per server origin in `access-tokens.json` in the app data
//! directory, readable by the user only. Only Rust reads the secret: the frontend sets a
//! token and learns its client ID, never the secret, and nothing logs it.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, LOCATION, WWW_AUTHENTICATE};
use serde::{Deserialize, Serialize};

use crate::commands::{CommandError, CommandResult};
use crate::connection::server_origin;

const FILE: &str = "access-tokens.json";

pub const CLIENT_ID_HEADER: &str = "CF-Access-Client-Id";
pub const CLIENT_SECRET_HEADER: &str = "CF-Access-Client-Secret";

/// A Cloudflare Access service token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceToken {
    pub client_id: String,
    pub client_secret: String,
}

impl fmt::Debug for ServiceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceToken")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

impl ServiceToken {
    /// The token from user input: both parts present, each a valid header value without
    /// spaces, as Cloudflare issues them.
    pub(crate) fn parse(client_id: &str, client_secret: &str) -> Result<Self, String> {
        let client_id = client_id.trim();
        let client_secret = client_secret.trim();
        for (name, value) in [("client ID", client_id), ("client secret", client_secret)] {
            if value.is_empty() {
                return Err(format!("the service token needs its {name}"));
            }
            if !value.chars().all(|c| c.is_ascii_graphic()) {
                return Err(format!(
                    "the service token's {name} must not contain spaces or other special characters"
                ));
            }
        }
        Ok(Self {
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
        })
    }

    pub(crate) fn from_input(input: AccessTokenInput) -> Result<Self, String> {
        Self::parse(&input.client_id, &input.client_secret)
    }
}

/// The service tokens of the servers, by origin.
pub struct AccessTokens {
    path: PathBuf,
    tokens: Mutex<BTreeMap<String, ServiceToken>>,
}

impl AccessTokens {
    /// Reads the tokens kept in `dir`. A file that cannot be read keeps no tokens, with a
    /// warning that names the file only.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE);
        let tokens = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|_| {
                eprintln!(
                    "toodo: {} is invalid; no Access service tokens",
                    path.display()
                );
                BTreeMap::new()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => {
                eprintln!("toodo: cannot read {}: {error}", path.display());
                BTreeMap::new()
            }
        };
        Self {
            path,
            tokens: Mutex::new(tokens),
        }
    }

    /// The token of the server at `origin`, if one is kept.
    pub fn get(&self, origin: &str) -> Option<ServiceToken> {
        self.lock().get(origin).cloned()
    }

    /// Keeps `token` for the server at `origin`, or removes its token for `None`.
    pub(crate) fn set(&self, origin: &str, token: Option<ServiceToken>) -> std::io::Result<()> {
        let mut tokens = self.lock();
        let mut next = tokens.clone();
        match token {
            Some(token) => next.insert(origin.to_owned(), token),
            None => next.remove(origin),
        };
        save(&self.path, &next)?;
        *tokens = next;
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, ServiceToken>> {
        self.tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Writes the tokens to `path`, readable and writable by the user only.
fn save(path: &Path, tokens: &BTreeMap<String, ServiceToken>) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(tokens).map_err(std::io::Error::other)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        if path.exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    options.open(path)?.write_all(json.as_bytes())
}

/// Whether Cloudflare Access, not the server, answered with `status` and `headers`: it
/// sends a client without a valid service token to its login page, or, with strict
/// service token authentication, answers 401 or 403 itself. Both carry Access's
/// `WWW-Authenticate: Cloudflare-Access ...` challenge.
pub fn blocked_by_access(status: StatusCode, headers: &HeaderMap) -> bool {
    let challenged = headers
        .get_all(WWW_AUTHENTICATE)
        .iter()
        .any(|value| value.as_bytes().starts_with(b"Cloudflare-Access"));
    let to_login = status.is_redirection()
        && headers
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|location| reqwest::Url::parse(location).ok())
            .is_some_and(|url| {
                url.host_str()
                    .is_some_and(|host| host.ends_with(".cloudflareaccess.com"))
                    || url.path().starts_with("/cdn-cgi/access/")
            });
    challenged || to_login
}

/// What to tell the user when Access blocked a request to `origin`.
pub fn blocked_message(origin: &str, token: Option<&ServiceToken>) -> String {
    match token {
        None => format!(
            "Cloudflare Access protects {origin} and wants a sign-in, which the app cannot do. \
             Add a service token for this server under Settings > General."
        ),
        Some(token) => format!(
            "Cloudflare Access protecting {origin} did not accept the service token {}. It may be \
             wrong, expired, or revoked, or the Access application has no Service Auth policy \
             for it. Check it under Settings > General.",
            token.client_id
        ),
    }
}

/// The service token kept for a server, without its secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AccessTokenInfo {
    pub client_id: String,
}

/// A service token entered by the user.
#[derive(Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AccessTokenInput {
    pub client_id: String,
    pub client_secret: String,
}

impl fmt::Debug for AccessTokenInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessTokenInput")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

/// The Cloudflare Access service token kept for the server at `server`, without its secret.
#[tauri::command]
#[specta::specta]
pub fn get_access_token(
    tokens: tauri::State<'_, AccessTokens>,
    server: String,
) -> CommandResult<Option<AccessTokenInfo>> {
    let origin = server_origin(&server).map_err(CommandError::invalid_input)?;
    Ok(tokens.get(&origin).map(|token| AccessTokenInfo {
        client_id: token.client_id,
    }))
}

/// Keeps `token` as the Cloudflare Access service token of the server at `server`, sent
/// with every request to it from now on, or removes the server's token for `None`.
#[tauri::command]
#[specta::specta]
pub fn set_access_token(
    tokens: tauri::State<'_, AccessTokens>,
    server: String,
    token: Option<AccessTokenInput>,
) -> CommandResult<()> {
    let origin = server_origin(&server).map_err(CommandError::invalid_input)?;
    let token = token
        .map(ServiceToken::from_input)
        .transpose()
        .map_err(CommandError::invalid_input)?;
    tokens
        .set(&origin, token)
        .map_err(|error| CommandError::internal(format!("cannot save {FILE}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn token() -> ServiceToken {
        ServiceToken::parse("abc.access", "s3cret").unwrap()
    }

    #[test]
    fn a_token_needs_both_parts_without_spaces() {
        assert_eq!(
            ServiceToken::parse(" abc.access ", " s3cret\n").unwrap(),
            token()
        );
        for (id, secret) in [
            ("", "s"),
            ("a", ""),
            ("a b", "s"),
            ("a", "s\u{e9}"),
            ("a", "s\ns"),
        ] {
            assert!(
                ServiceToken::parse(id, secret).is_err(),
                "{id:?} {secret:?}"
            );
        }
    }

    #[test]
    fn debug_output_hides_the_secret() {
        let shown = format!("{:?}", token());
        assert!(shown.contains("abc.access"), "{shown}");
        assert!(!shown.contains("s3cret"), "{shown}");
        let input = AccessTokenInput {
            client_id: "abc.access".to_owned(),
            client_secret: "s3cret".to_owned(),
        };
        assert!(!format!("{input:?}").contains("s3cret"));
    }

    #[test]
    fn tokens_are_kept_per_server_across_loads() {
        let dir = tempfile::tempdir().unwrap();
        let tokens = AccessTokens::load(dir.path());
        assert_eq!(tokens.get("https://a.example"), None);
        tokens.set("https://a.example", Some(token())).unwrap();
        let other = ServiceToken::parse("other.access", "x").unwrap();
        tokens
            .set("https://b.example", Some(other.clone()))
            .unwrap();

        let loaded = AccessTokens::load(dir.path());
        assert_eq!(loaded.get("https://a.example"), Some(token()));
        assert_eq!(loaded.get("https://b.example"), Some(other));
        loaded.set("https://a.example", None).unwrap();
        assert_eq!(
            AccessTokens::load(dir.path()).get("https://a.example"),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_token_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        AccessTokens::load(dir.path())
            .set("https://a.example", Some(token()))
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_invalid_file_keeps_no_tokens() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), "not json").unwrap();
        assert_eq!(
            AccessTokens::load(dir.path()).get("https://a.example"),
            None
        );
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn access_blocks_are_told_from_the_server_s_own_answers() {
        // Access sends a client without a valid token to its login page.
        assert!(blocked_by_access(
            StatusCode::FOUND,
            &headers(&[
                (
                    "location",
                    "https://team.cloudflareaccess.com/cdn-cgi/access/login/toodo.example?kid=1"
                ),
                (
                    "www-authenticate",
                    "Cloudflare-Access resource_metadata=\"https://toodo.example/x\""
                ),
            ])
        ));
        assert!(blocked_by_access(
            StatusCode::FOUND,
            &headers(&[(
                "location",
                "https://toodo.example/cdn-cgi/access/login/toodo.example"
            )])
        ));
        // Strict service token authentication answers 401 or 403 itself.
        assert!(blocked_by_access(
            StatusCode::FORBIDDEN,
            &headers(&[("www-authenticate", "Cloudflare-Access")])
        ));
        // The server's own answers are not Access blocks.
        assert!(!blocked_by_access(StatusCode::UNAUTHORIZED, &headers(&[])));
        assert!(!blocked_by_access(
            StatusCode::UNAUTHORIZED,
            &headers(&[("www-authenticate", "Bearer")])
        ));
        assert!(!blocked_by_access(
            StatusCode::FOUND,
            &headers(&[("location", "https://toodo.example/")])
        ));
        assert!(!blocked_by_access(StatusCode::OK, &headers(&[])));
    }

    #[test]
    fn the_blocked_message_names_the_token_but_not_its_secret() {
        let message = blocked_message("https://toodo.example", Some(&token()));
        assert!(message.contains("abc.access"), "{message}");
        assert!(!message.contains("s3cret"), "{message}");
        assert!(blocked_message("https://toodo.example", None).contains("Add a service token"));
    }
}
