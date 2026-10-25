//! The desktop app's requests to the `toodo-server` it connects to. The window does not
//! call the server itself: it hands each request to the inlined `remote` plugin, which
//! sends it from Rust, so it can carry the server's Cloudflare Access service token (see
//! [`access`]) without the secret entering the window, and so no CORS preflight is needed.
//!
//! - `fetch` sends a request, with the body as the raw IPC payload and the URL, method, and
//!   headers in the `toodo-request` IPC header, and answers with the status and headers;
//!   `read` returns the next chunk of the body (empty at its end), and `cancel` drops it.
//! - `socket_open` opens a WebSocket (the pane view's stream); `socket_receive` waits for
//!   its next text message (`null` once closed), `socket_send` sends one, and
//!   `socket_close` closes it.
//!
//! Failures are `{ kind, message }`: `access` when Cloudflare Access blocked the request,
//! `network` when the server could not be reached, `invalid` for a malformed request.

pub mod access;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime, State};
use tokio::sync::Notify;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;

use access::{AccessTokens, CLIENT_ID_HEADER, CLIENT_SECRET_HEADER};

const REQUEST_HEADER: &str = "toodo-request";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A body nobody read for this long is dropped: the window forgot it. The event stream is
/// read at least every 15 seconds, when the server sends its keep-alive.
const IDLE_BODY: Duration = Duration::from_secs(600);

/// The plugin that sends the window's requests to the server.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("remote")
        .invoke_handler(tauri::generate_handler![
            fetch,
            read,
            cancel,
            socket_open,
            socket_receive,
            socket_send,
            socket_close
        ])
        .setup(|app, _| {
            let mut remote = Remote::new().map_err(|error| error.to_string())?;
            remote.data_dir = Some(app.path().app_data_dir()?);
            app.manage(remote);
            Ok(())
        })
        .build()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum FailureKind {
    Access,
    Network,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    kind: FailureKind,
    message: String,
}

impl Failure {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Invalid,
            message: message.into(),
        }
    }

    fn network(error: reqwest::Error) -> Self {
        // Without the URL: it is the window's own, and a query may carry a ticket.
        let error = error.without_url();
        let mut message = error.to_string();
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            message = format!("{message}: {cause}");
            source = cause.source();
        }
        Self {
            kind: FailureKind::Network,
            message,
        }
    }

    fn access(origin: &str, token: Option<&access::ServiceToken>) -> Self {
        Self {
            kind: FailureKind::Access,
            message: access::blocked_message(origin, token),
        }
    }
}

/// What the window asks to send.
#[derive(Deserialize)]
struct Outgoing {
    url: String,
    method: String,
    #[serde(default)]
    headers: Vec<(String, String)>,
}

/// The status and headers of an answer, and the id to read its body with.
#[derive(Serialize)]
pub struct Head {
    id: u64,
    status: u16,
    headers: Vec<(String, String)>,
}

struct Body {
    response: tokio::sync::Mutex<reqwest::Response>,
    cancelled: Notify,
    used: Mutex<Instant>,
}

type Socket = WebSocketStream<reqwest::Upgraded>;

struct OpenSocket {
    sink: tokio::sync::Mutex<SplitSink<Socket, Message>>,
    stream: tokio::sync::Mutex<SplitStream<Socket>>,
    closed: Notify,
}

pub struct Remote {
    client: reqwest::Client,
    /// The app data directory, where `connection.json` names the server the app uses.
    data_dir: Option<std::path::PathBuf>,
    next: AtomicU64,
    bodies: Mutex<HashMap<u64, Arc<Body>>>,
    sockets: Mutex<HashMap<u64, Arc<OpenSocket>>>,
}

impl Remote {
    fn new() -> reqwest::Result<Self> {
        // reqwest is built without a default crypto provider; take ring, as the server does.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let builder = reqwest::Client::builder();
        // The platform verifier needs the Android app's JVM context set up through JNI; the
        // Mozilla roots serve the public certificates of servers behind Cloudflare.
        #[cfg(target_os = "android")]
        let builder = builder.tls_backend_preconfigured(
            rustls::ClientConfig::builder()
                .with_root_certificates(rustls::RootCertStore::from_iter(
                    webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
                ))
                .with_no_client_auth(),
        );
        let client = builder
            // A redirect may be Access's login page, which must reach `fetch` as is.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            data_dir: None,
            next: AtomicU64::new(1),
            bodies: Mutex::default(),
            sockets: Mutex::default(),
        })
    }

    /// Whether the window may send `request`: only to the server the app uses or a server
    /// it keeps a service token for, so a page cannot read other hosts' answers through
    /// the app, which does not apply CORS.
    fn permitted(&self, tokens: &AccessTokens, request: &reqwest::Request) -> Result<(), Failure> {
        let origin = request.url().origin().ascii_serialization();
        let in_use = self
            .data_dir
            .as_deref()
            .and_then(|dir| crate::connection::load(dir).server)
            .is_some_and(|server| server == origin);
        if in_use || tokens.get(&origin).is_some() {
            Ok(())
        } else {
            Err(Failure::invalid(format!(
                "{origin} is not the server the app uses"
            )))
        }
    }

    fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    fn body(&self, id: u64) -> Option<Arc<Body>> {
        lock(&self.bodies).get(&id).cloned()
    }

    fn socket(&self, id: u64) -> Option<Arc<OpenSocket>> {
        lock(&self.sockets).get(&id).cloned()
    }

    /// Drops the bodies the window stopped reading, other than one being read right now.
    fn drop_idle_bodies(&self) {
        lock(&self.bodies).retain(|_, body| {
            body.response.try_lock().is_err() || lock(&body.used).elapsed() < IDLE_BODY
        });
    }

    /// Sends `request` to its server with the server's service token, if any. Fails with
    /// an Access failure when Cloudflare Access answered instead of the server.
    async fn send(
        &self,
        tokens: &AccessTokens,
        mut request: reqwest::Request,
    ) -> Result<reqwest::Response, Failure> {
        let origin = request.url().origin().ascii_serialization();
        let token = tokens.get(&origin);
        if let Some(token) = &token {
            let headers = request.headers_mut();
            for (name, value) in [
                (CLIENT_ID_HEADER, &token.client_id),
                (CLIENT_SECRET_HEADER, &token.client_secret),
            ] {
                let mut value = HeaderValue::from_str(value)
                    .map_err(|_| Failure::invalid("the service token is not a header value"))?;
                value.set_sensitive(true);
                headers.insert(name, value);
            }
        }
        let response = self
            .client
            .execute(request)
            .await
            .map_err(Failure::network)?;
        if access::blocked_by_access(response.status(), response.headers()) {
            return Err(Failure::access(&origin, token.as_ref()));
        }
        Ok(response)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Builds the request the window asks for. Only `http` and `https` URLs are sent.
fn outgoing(
    client: &reqwest::Client,
    outgoing: Outgoing,
    body: Vec<u8>,
) -> Result<reqwest::Request, Failure> {
    let url = reqwest::Url::parse(&outgoing.url)
        .map_err(|error| Failure::invalid(format!("not a URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Failure::invalid("only http and https URLs can be sent"));
    }
    let method = Method::from_bytes(outgoing.method.as_bytes())
        .map_err(|_| Failure::invalid(format!("not a method: {}", outgoing.method)))?;
    let mut builder = client.request(method, url);
    for (name, value) in outgoing.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| Failure::invalid(format!("not a header name: {name}")))?;
        let value = HeaderValue::from_str(&value)
            .map_err(|_| Failure::invalid(format!("not a value for header {name}")))?;
        builder = builder.header(name, value);
    }
    if !body.is_empty() {
        builder = builder.body(body);
    }
    builder
        .build()
        .map_err(|error| Failure::invalid(error.without_url().to_string()))
}

fn header_pairs(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_owned())))
        .collect()
}

#[tauri::command]
async fn fetch(
    request: tauri::ipc::Request<'_>,
    remote: State<'_, Remote>,
    tokens: State<'_, AccessTokens>,
) -> Result<Head, Failure> {
    let wanted = request
        .headers()
        .get(REQUEST_HEADER)
        .ok_or_else(|| Failure::invalid(format!("the {REQUEST_HEADER} header is missing")))?;
    let wanted: Outgoing = serde_json::from_slice(wanted.as_bytes())
        .map_err(|error| Failure::invalid(format!("the {REQUEST_HEADER} header: {error}")))?;
    let body = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        tauri::ipc::InvokeBody::Json(_) => Vec::new(),
    };
    let request = outgoing(&remote.client, wanted, body)?;
    remote.permitted(&tokens, &request)?;
    remote.drop_idle_bodies();
    let response = remote.send(&tokens, request).await?;
    let id = remote.id();
    let head = Head {
        id,
        status: response.status().as_u16(),
        headers: header_pairs(response.headers()),
    };
    lock(&remote.bodies).insert(
        id,
        Arc::new(Body {
            response: tokio::sync::Mutex::new(response),
            cancelled: Notify::new(),
            used: Mutex::new(Instant::now()),
        }),
    );
    Ok(head)
}

/// The next chunk of the body of answer `id`; empty at its end, or once it was cancelled.
#[tauri::command]
async fn read(remote: State<'_, Remote>, id: u64) -> Result<tauri::ipc::Response, Failure> {
    let Some(body) = remote.body(id) else {
        return Ok(tauri::ipc::Response::new(Vec::<u8>::new()));
    };
    let chunk = {
        let mut response = body.response.lock().await;
        loop {
            let next = tokio::select! {
                next = response.chunk() => next,
                () = body.cancelled.notified() => Ok(None),
            };
            match next {
                Ok(Some(chunk)) if chunk.is_empty() => continue,
                other => break other,
            }
        }
    };
    *lock(&body.used) = Instant::now();
    match chunk {
        Ok(Some(chunk)) => Ok(tauri::ipc::Response::new(chunk.to_vec())),
        Ok(None) => {
            lock(&remote.bodies).remove(&id);
            Ok(tauri::ipc::Response::new(Vec::<u8>::new()))
        }
        Err(error) => {
            lock(&remote.bodies).remove(&id);
            Err(Failure::network(error))
        }
    }
}

#[tauri::command]
fn cancel(remote: State<'_, Remote>, id: u64) {
    if let Some(body) = lock(&remote.bodies).remove(&id) {
        body.cancelled.notify_one();
    }
}

/// Opens a WebSocket to `url` (an `http` or `https` URL), with the server's service token.
#[tauri::command]
async fn socket_open(
    remote: State<'_, Remote>,
    tokens: State<'_, AccessTokens>,
    url: String,
) -> Result<u64, Failure> {
    let mut key = [0u8; 16];
    getrandom::fill(&mut key).map_err(|error| Failure::invalid(error.to_string()))?;
    let key = base64::engine::general_purpose::STANDARD.encode(key);
    let wanted = Outgoing {
        url,
        method: "GET".to_owned(),
        headers: vec![
            ("Connection".to_owned(), "Upgrade".to_owned()),
            ("Upgrade".to_owned(), "websocket".to_owned()),
            ("Sec-WebSocket-Version".to_owned(), "13".to_owned()),
            ("Sec-WebSocket-Key".to_owned(), key),
        ],
    };
    let request = outgoing(&remote.client, wanted, Vec::new())?;
    remote.permitted(&tokens, &request)?;
    let response = remote.send(&tokens, request).await?;
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(Failure {
            kind: FailureKind::Network,
            message: format!("the server did not open the stream: {status} {text}")
                .trim()
                .to_owned(),
        });
    }
    let upgraded = response.upgrade().await.map_err(Failure::network)?;
    let (sink, stream) = WebSocketStream::from_raw_socket(upgraded, Role::Client, None)
        .await
        .split();
    let id = remote.id();
    lock(&remote.sockets).insert(
        id,
        Arc::new(OpenSocket {
            sink: tokio::sync::Mutex::new(sink),
            stream: tokio::sync::Mutex::new(stream),
            closed: Notify::new(),
        }),
    );
    Ok(id)
}

/// The next text message of socket `id`, or `None` once it is closed.
#[tauri::command]
async fn socket_receive(remote: State<'_, Remote>, id: u64) -> Result<Option<String>, Failure> {
    let Some(socket) = remote.socket(id) else {
        return Ok(None);
    };
    let mut stream = socket.stream.lock().await;
    loop {
        let next = tokio::select! {
            next = stream.next() => next,
            () = socket.closed.notified() => None,
        };
        match next {
            Some(Ok(Message::Text(text))) => return Ok(Some(text.to_string())),
            Some(Ok(Message::Binary(bytes))) => {
                return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
            Some(Ok(Message::Close(_)) | Err(_)) | None => {
                lock(&remote.sockets).remove(&id);
                return Ok(None);
            }
        }
    }
}

#[tauri::command]
async fn socket_send(remote: State<'_, Remote>, id: u64, text: String) -> Result<(), Failure> {
    let socket = remote.socket(id).ok_or_else(|| Failure {
        kind: FailureKind::Network,
        message: "the stream is closed".to_owned(),
    })?;
    socket
        .sink
        .lock()
        .await
        .send(Message::text(text))
        .await
        .map_err(|error| Failure {
            kind: FailureKind::Network,
            message: error.to_string(),
        })
}

#[tauri::command]
async fn socket_close(remote: State<'_, Remote>, id: u64) -> Result<(), Failure> {
    let Some(socket) = lock(&remote.sockets).remove(&id) else {
        return Ok(());
    };
    socket.closed.notify_one();
    let _ = socket.sink.lock().await.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> reqwest::Client {
        Remote::new().unwrap().client
    }

    fn wanted(url: &str, method: &str, headers: &[(&str, &str)]) -> Outgoing {
        Outgoing {
            url: url.to_owned(),
            method: method.to_owned(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn the_window_s_request_is_sent_as_asked() {
        let request = outgoing(
            &client(),
            wanted(
                "https://toodo.example/api/rpc/list_tasks",
                "POST",
                &[
                    ("authorization", "Bearer t"),
                    ("content-type", "application/json"),
                ],
            ),
            b"{}".to_vec(),
        )
        .unwrap();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://toodo.example/api/rpc/list_tasks"
        );
        assert_eq!(request.headers()["authorization"], "Bearer t");
        assert_eq!(request.body().unwrap().as_bytes(), Some(&b"{}"[..]));
    }

    #[test]
    fn only_well_formed_http_requests_are_sent() {
        for (url, method, headers) in [
            ("file:///etc/passwd", "GET", vec![]),
            ("not a url", "GET", vec![]),
            ("https://toodo.example/", "G ET", vec![]),
            ("https://toodo.example/", "GET", vec![("bad name", "x")]),
            (
                "https://toodo.example/",
                "GET",
                vec![("x-a", "line\nbreak")],
            ),
        ] {
            let failure =
                outgoing(&client(), wanted(url, method, &headers), Vec::new()).unwrap_err();
            assert_eq!(
                failure.kind,
                FailureKind::Invalid,
                "{url} {method} {headers:?}"
            );
        }
    }

    /// Answers each connection on a local port with `answer`, recording the requests.
    fn serve(answer: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap();
                lock(&recorded).push(String::from_utf8_lossy(&buffer[..read]).into_owned());
                stream.write_all(answer.as_bytes()).unwrap();
            }
        });
        (address, seen)
    }

    #[tokio::test]
    async fn requests_carry_the_server_s_service_token() {
        let (address, seen) = serve("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok");
        let dir = tempfile::tempdir().unwrap();
        let tokens = AccessTokens::load(dir.path());
        let remote = Remote::new().unwrap();

        let request = outgoing(&remote.client, wanted(&address, "GET", &[]), Vec::new()).unwrap();
        remote.send(&tokens, request).await.unwrap();
        let input = access::AccessTokenInput {
            client_id: "abc.access".to_owned(),
            client_secret: "s3cret".to_owned(),
        };
        tokens
            .set(
                &address,
                Some(access::ServiceToken::from_input(input).unwrap()),
            )
            .unwrap();
        let request = outgoing(&remote.client, wanted(&address, "GET", &[]), Vec::new()).unwrap();
        remote.send(&tokens, request).await.unwrap();

        let seen = lock(&seen).clone();
        assert!(
            !seen[0].to_lowercase().contains("cf-access-client"),
            "{}",
            seen[0]
        );
        let second = seen[1].to_lowercase();
        assert!(
            second.contains("cf-access-client-id: abc.access"),
            "{second}"
        );
        assert!(
            second.contains("cf-access-client-secret: s3cret"),
            "{second}"
        );
    }

    #[tokio::test]
    async fn an_access_login_redirect_is_an_access_failure() {
        let (address, _) = serve(
            "HTTP/1.1 302 Found\r\nlocation: https://team.cloudflareaccess.com/cdn-cgi/access/login/x\r\ncontent-length: 0\r\n\r\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let tokens = AccessTokens::load(dir.path());
        let remote = Remote::new().unwrap();
        let request = outgoing(&remote.client, wanted(&address, "GET", &[]), Vec::new()).unwrap();
        let failure = remote.send(&tokens, request).await.unwrap_err();
        assert_eq!(failure.kind, FailureKind::Access);
        assert!(failure.message.contains(&address), "{}", failure.message);
    }

    #[test]
    fn only_the_server_in_use_or_one_with_a_token_is_reached() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("connection.json"),
            r#"{"server":"https://toodo.example"}"#,
        )
        .unwrap();
        let tokens = AccessTokens::load(dir.path());
        let mut remote = Remote::new().unwrap();
        remote.data_dir = Some(dir.path().to_owned());
        let request =
            |url: &str| outgoing(&remote.client, wanted(url, "GET", &[]), Vec::new()).unwrap();

        assert!(
            remote
                .permitted(&tokens, &request("https://toodo.example/api"))
                .is_ok()
        );
        assert!(
            remote
                .permitted(&tokens, &request("https://intranet.example/"))
                .is_err()
        );
        tokens
            .set(
                "https://other.example",
                Some(access::ServiceToken::parse("a.access", "s").unwrap()),
            )
            .unwrap();
        assert!(
            remote
                .permitted(&tokens, &request("https://other.example/api"))
                .is_ok()
        );
    }

    #[tokio::test]
    async fn an_unreachable_server_is_a_network_failure() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let dir = tempfile::tempdir().unwrap();
        let remote = Remote::new().unwrap();
        let request = outgoing(&remote.client, wanted(&address, "GET", &[]), Vec::new()).unwrap();
        let failure = remote
            .send(&AccessTokens::load(dir.path()), request)
            .await
            .unwrap_err();
        assert_eq!(failure.kind, FailureKind::Network);
    }
}
