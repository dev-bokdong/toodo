//! Subscribes to events of the local herdr server through its socket API
//! (`events.subscribe`): newline-delimited JSON over a Unix domain socket, or a named pipe
//! on Windows.
//!
//! One connection carries one subscription: herdr acknowledges it with
//! `subscription_started` and then pushes one event per line. herdr closes the connection
//! after an error such as `events_lost` (the subscriber fell behind) or `pane_not_found`
//! (a subscribed pane is gone), so the caller reconnects and reconciles with an
//! authoritative read. Agent status events are scoped to one pane, so the panes to watch
//! are fixed per connection and a new pane needs a new subscription.

use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};

use serde_json::{Value, json};

/// Subscriptions that do not depend on a pane.
const LIFECYCLE_SUBSCRIPTIONS: [&str; 7] = [
    "worktree.created",
    "worktree.opened",
    "worktree.removed",
    "workspace.closed",
    "pane.created",
    "pane.closed",
    "pane.agent_detected",
];

const REQUEST_ID: &str = "toodo:events";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HerdrEvent {
    /// A worktree checkout was created, or opened again, as a herdr workspace.
    WorktreeOpened { workspace_id: String, path: String },
    /// A worktree checkout was removed; its herdr workspace is closed.
    WorktreeRemoved { workspace_id: String, path: String },
    /// Panes or their agents changed: a pane was created or closed, an agent was detected
    /// or changed status, or a workspace was closed. Agent state must be read again.
    AgentsChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// herdr accepted the subscription; events follow.
    Started,
    Event(HerdrEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventStreamError {
    /// The socket could not be opened, read, or written.
    Io { message: String },
    /// herdr closed the connection.
    Closed,
    /// herdr answered with an error and closed the connection, e.g. `events_lost` or
    /// `pane_not_found`.
    Rejected { code: String, message: String },
    /// herdr sent a line that is not the JSON the socket API documents.
    Malformed { line: String },
}

impl fmt::Display for EventStreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { message } => write!(f, "herdr event socket failed: {message}"),
            Self::Closed => write!(f, "herdr closed the event subscription"),
            Self::Rejected { code, message } => {
                write!(
                    f,
                    "herdr rejected the event subscription ({code}): {message}"
                )
            }
            Self::Malformed { line } => write!(f, "herdr sent a malformed event: {line:?}"),
        }
    }
}

impl std::error::Error for EventStreamError {}

impl From<io::Error> for EventStreamError {
    fn from(error: io::Error) -> Self {
        Self::Io {
            message: error.to_string(),
        }
    }
}

impl EventStreamError {
    /// Whether the subscription should be opened again right away, because herdr dropped
    /// it for a reason that a fresh pane list and subscription resolve.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Rejected { code, .. } if code == "events_lost" || code == "pane_not_found")
    }
}

/// A byte stream to the herdr server.
pub trait Connection: Read + Write + Send {}

impl<T: Read + Write + Send> Connection for T {}

/// Opens connections to the herdr server; faked in tests.
pub trait Connector {
    fn connect(&self) -> io::Result<Box<dyn Connection>>;
}

/// The local herdr server's socket, as reported by `herdr status --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSocket {
    endpoint: String,
}

impl LocalSocket {
    /// On Windows, herdr serves the socket path as the named pipe
    /// `\\.\pipe\<socket path>`; elsewhere the path is a Unix domain socket.
    pub fn new(socket_path: &str) -> Self {
        let endpoint = if cfg!(windows) && !socket_path.starts_with(PIPE_PREFIX) {
            format!("{PIPE_PREFIX}{socket_path}")
        } else {
            socket_path.to_owned()
        };
        Self { endpoint }
    }
}

const PIPE_PREFIX: &str = r"\\.\pipe\";

impl Connector for LocalSocket {
    #[cfg(unix)]
    fn connect(&self) -> io::Result<Box<dyn Connection>> {
        Ok(Box::new(std::os::unix::net::UnixStream::connect(
            &self.endpoint,
        )?))
    }

    #[cfg(windows)]
    fn connect(&self) -> io::Result<Box<dyn Connection>> {
        // A named pipe client is a file opened for reading and writing.
        Ok(Box::new(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.endpoint)?,
        ))
    }
}

/// What to do after a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// End this subscription so the caller can subscribe again, e.g. to watch new panes.
    Resubscribe,
}

/// The `events.subscribe` request line: lifecycle events plus the agent status of each
/// pane in `panes`.
pub fn subscribe_request(panes: &BTreeSet<String>) -> String {
    let subscriptions: Vec<Value> = LIFECYCLE_SUBSCRIPTIONS
        .iter()
        .map(|kind| json!({ "type": kind }))
        .chain(
            panes
                .iter()
                .map(|pane| json!({ "type": "pane.agent_status_changed", "pane_id": pane })),
        )
        .collect();
    let request = json!({
        "id": REQUEST_ID,
        "method": "events.subscribe",
        "params": { "subscriptions": subscriptions },
    });
    format!("{request}\n")
}

/// Parses one line from the subscription. Events Toodo does not use are `None`.
pub fn parse_message(line: &str) -> Result<Option<Message>, EventStreamError> {
    let malformed = || EventStreamError::Malformed {
        line: line.to_owned(),
    };
    let value: Value = serde_json::from_str(line).map_err(|_| malformed())?;
    if let Some(error) = value.get("error") {
        let text = |key: &str| error.get(key).and_then(Value::as_str).map(str::to_owned);
        return Err(EventStreamError::Rejected {
            code: text("code").ok_or_else(malformed)?,
            message: text("message").unwrap_or_default(),
        });
    }
    if let Some(result) = value.get("result") {
        return match result.get("type").and_then(Value::as_str) {
            Some("subscription_started") => Ok(Some(Message::Started)),
            _ => Err(malformed()),
        };
    }
    let kind = value
        .get("event")
        .and_then(Value::as_str)
        .ok_or_else(malformed)?;
    let data = value.get("data").ok_or_else(malformed)?;
    let text = |pointer: &str| {
        data.pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(malformed)
    };
    // Lifecycle events arrive as `worktree_created`, pane-scoped ones as
    // `pane.agent_status_changed`.
    let event = match kind.replace('.', "_").as_str() {
        "worktree_created" | "worktree_opened" => HerdrEvent::WorktreeOpened {
            workspace_id: text("/workspace/workspace_id")?,
            path: text("/worktree/path")?,
        },
        "worktree_removed" => HerdrEvent::WorktreeRemoved {
            workspace_id: text("/workspace_id")?,
            path: text("/worktree/path")?,
        },
        "workspace_closed"
        | "pane_created"
        | "pane_closed"
        | "pane_agent_detected"
        | "pane_agent_status_changed" => HerdrEvent::AgentsChanged,
        _ => return Ok(None),
    };
    Ok(Some(Message::Event(event)))
}

/// Subscribes to herdr events and passes every message to `on_message` until it asks to
/// resubscribe (`Ok`) or the subscription ends (`Err`).
pub fn subscribe(
    connector: &dyn Connector,
    panes: &BTreeSet<String>,
    mut on_message: impl FnMut(Message) -> Flow,
) -> Result<(), EventStreamError> {
    let mut connection = connector.connect()?;
    connection.write_all(subscribe_request(panes).as_bytes())?;
    connection.flush()?;
    let mut reader = BufReader::new(connection);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(EventStreamError::Closed);
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(message) = parse_message(line)? else {
            continue;
        };
        if on_message(message) == Flow::Resubscribe {
            return Ok(());
        }
    }
}

#[cfg(test)]
pub mod fake {
    use std::collections::VecDeque;
    use std::io::{self, Cursor, Read, Write};
    use std::sync::{Arc, Mutex};

    use super::{Connection, Connector};

    /// Serves one scripted server transcript per connection and records each request.
    #[derive(Default)]
    pub struct FakeConnector {
        pub transcripts: Mutex<VecDeque<io::Result<String>>>,
        pub requests: Arc<Mutex<Vec<String>>>,
    }

    impl FakeConnector {
        pub fn new(transcripts: impl IntoIterator<Item = io::Result<String>>) -> Self {
            Self {
                transcripts: Mutex::new(transcripts.into_iter().collect()),
                requests: Arc::default(),
            }
        }
    }

    struct FakeConnection {
        output: Cursor<Vec<u8>>,
        request: Vec<u8>,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl Read for FakeConnection {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.output.read(buf)
        }
    }

    impl Write for FakeConnection {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.request.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            let request = String::from_utf8(std::mem::take(&mut self.request)).unwrap();
            self.requests.lock().unwrap().push(request);
            Ok(())
        }
    }

    impl Connector for FakeConnector {
        fn connect(&self) -> io::Result<Box<dyn Connection>> {
            let transcript = self
                .transcripts
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected herdr connection")?;
            Ok(Box::new(FakeConnection {
                output: Cursor::new(transcript.into_bytes()),
                request: Vec::new(),
                requests: self.requests.clone(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeConnector;
    use super::*;

    const STARTED: &str = r#"{"id":"toodo:events","result":{"type":"subscription_started"}}"#;

    // Captured from herdr 0.9.3.
    const WORKTREE_CREATED: &str = r#"{"data":{"type":"worktree_created","workspace":{"active_tab_id":"w2R:t1","agent_status":"unknown","label":"wt","workspace_id":"w2R","worktree":{"checkout_path":"/tmp/q/wt","is_linked_worktree":true}},"worktree":{"branch":"feat/x","is_bare":false,"is_detached":false,"is_linked_worktree":true,"is_prunable":false,"label":"repo","open_workspace_id":"w2R","path":"/tmp/q/wt"}},"event":"worktree_created"}"#;
    const WORKTREE_REMOVED: &str = r#"{"data":{"forced":true,"type":"worktree_removed","workspace":null,"workspace_id":"w2R","worktree":{"branch":"feat/x","is_bare":false,"is_detached":false,"is_linked_worktree":true,"is_prunable":false,"label":"repo","path":"/tmp/q/wt"}},"event":"worktree_removed"}"#;
    const AGENT_STATUS: &str = r#"{"data":{"agent":"claude","agent_status":"working","pane_id":"w2S:p1","workspace_id":"w2S"},"event":"pane.agent_status_changed"}"#;
    const PANE_UPDATED: &str =
        r#"{"data":{"pane":{"pane_id":"w2Q:p1"},"type":"pane_updated"},"event":"pane_updated"}"#;

    fn panes(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn request_subscribes_to_lifecycle_events_and_each_pane() {
        let request = subscribe_request(&panes(&["w1:p2", "w1:p1"]));
        assert!(request.ends_with('\n'));
        let value: Value = serde_json::from_str(&request).unwrap();
        assert_eq!(value["method"], "events.subscribe");
        let subscriptions = value["params"]["subscriptions"].as_array().unwrap();
        let types: Vec<_> = subscriptions
            .iter()
            .map(|entry| entry["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            [
                "worktree.created",
                "worktree.opened",
                "worktree.removed",
                "workspace.closed",
                "pane.created",
                "pane.closed",
                "pane.agent_detected",
                "pane.agent_status_changed",
                "pane.agent_status_changed",
            ]
        );
        assert_eq!(subscriptions[7]["pane_id"], "w1:p1");
        assert_eq!(subscriptions[8]["pane_id"], "w1:p2");
    }

    #[test]
    fn messages_are_parsed() {
        assert_eq!(parse_message(STARTED), Ok(Some(Message::Started)));
        assert_eq!(
            parse_message(WORKTREE_CREATED),
            Ok(Some(Message::Event(HerdrEvent::WorktreeOpened {
                workspace_id: "w2R".to_owned(),
                path: "/tmp/q/wt".to_owned(),
            })))
        );
        assert_eq!(
            parse_message(WORKTREE_REMOVED),
            Ok(Some(Message::Event(HerdrEvent::WorktreeRemoved {
                workspace_id: "w2R".to_owned(),
                path: "/tmp/q/wt".to_owned(),
            })))
        );
        assert_eq!(
            parse_message(AGENT_STATUS),
            Ok(Some(Message::Event(HerdrEvent::AgentsChanged)))
        );
        assert_eq!(parse_message(PANE_UPDATED), Ok(None));
    }

    #[test]
    fn error_responses_and_malformed_lines_end_the_subscription() {
        let lost = parse_message(
            r#"{"id":"toodo:events","error":{"code":"events_lost","message":"subscriber fell behind"}}"#,
        )
        .unwrap_err();
        assert_eq!(
            lost,
            EventStreamError::Rejected {
                code: "events_lost".to_owned(),
                message: "subscriber fell behind".to_owned(),
            }
        );
        assert!(lost.is_transient());
        let invalid = parse_message(
            r#"{"id":"toodo:events","error":{"code":"invalid_request","message":"bad"}}"#,
        )
        .unwrap_err();
        assert!(!invalid.is_transient());
        for line in [
            "not json",
            r#"{"event":"worktree_created","data":{"workspace":{}}}"#,
            r#"{"id":"toodo:events","result":{"type":"pong"}}"#,
        ] {
            assert!(
                matches!(parse_message(line), Err(EventStreamError::Malformed { .. })),
                "{line}"
            );
        }
    }

    #[test]
    fn subscribe_delivers_messages_until_resubscribe() {
        let connector = FakeConnector::new([Ok(format!(
            "{STARTED}\n{PANE_UPDATED}\n\n{WORKTREE_CREATED}\n{AGENT_STATUS}\n"
        ))]);
        let mut seen = Vec::new();
        let result = subscribe(&connector, &panes(&["w2S:p1"]), |message| {
            seen.push(message.clone());
            if message == Message::Event(HerdrEvent::AgentsChanged) {
                Flow::Resubscribe
            } else {
                Flow::Continue
            }
        });
        assert_eq!(result, Ok(()));
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0], Message::Started);
        let requests = connector.requests.lock().unwrap();
        assert_eq!(*requests, [subscribe_request(&panes(&["w2S:p1"]))]);
    }

    #[test]
    fn subscribe_reports_how_the_stream_ended() {
        let connector = FakeConnector::new([
            Ok(format!("{STARTED}\n")),
            Ok(format!(
                "{STARTED}\n{}\n",
                r#"{"id":"toodo:events","error":{"code":"events_lost","message":"lost"}}"#
            )),
            Err(io::Error::new(io::ErrorKind::NotFound, "no socket")),
        ]);
        let ignore = |_| Flow::Continue;
        assert_eq!(
            subscribe(&connector, &BTreeSet::new(), ignore),
            Err(EventStreamError::Closed)
        );
        assert!(matches!(
            subscribe(&connector, &BTreeSet::new(), ignore),
            Err(EventStreamError::Rejected { code, .. }) if code == "events_lost"
        ));
        assert!(matches!(
            subscribe(&connector, &BTreeSet::new(), ignore),
            Err(EventStreamError::Io { .. })
        ));
    }

    #[test]
    fn unix_sockets_are_used_as_is() {
        if cfg!(windows) {
            assert_eq!(
                LocalSocket::new(r"C:\Users\u\.config\herdr\herdr.sock").endpoint,
                r"\\.\pipe\C:\Users\u\.config\herdr\herdr.sock"
            );
        } else {
            assert_eq!(
                LocalSocket::new("/home/u/.config/herdr/herdr.sock").endpoint,
                "/home/u/.config/herdr/herdr.sock"
            );
        }
    }
}
