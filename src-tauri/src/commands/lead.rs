//! The lead: an agent that `toodo-server` keeps running in a herdr pane of its own
//! workspace and that turns requests into tasks. The desktop app has no lead, so its
//! status there is always [`LeadState::Disabled`].
//!
//! People talk to the lead in the chat panel: their messages are stored in the lead's
//! inbox (see [`crate::domain::inbox`]), their IDs are typed into the lead's pane, the
//! lead takes them over MCP, and posts its replies back through MCP ([`reply`]).

use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use specta::Type;
use specta_typescript::Number;

use super::types::{AgentStatus, Timestamp, WorkspaceId};
use super::{CommandError, CommandResult, HerdrState, State, StorageState, lock};
use crate::adapters::herdr::Herdr;
use crate::domain::chat;
use crate::domain::inbox::{self, ItemId};
use crate::storage::Storage;

/// The longest client ID of a chat message.
const MAX_CLIENT_ID_CHARS: usize = 100;

/// What the server is doing with the lead.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LeadState {
    /// No lead: the desktop app, or a server started with `TOODO_LEAD=off`.
    #[default]
    Disabled,
    /// The agent command was typed into the pane and has not shown up yet.
    Starting,
    Running,
    /// The agent exited; it is started again once its backoff has passed.
    Restarting,
    /// The lead cannot run: herdr or git failed, or the agent keeps exiting right after
    /// it starts. The server keeps trying.
    Failed,
}

/// The lead's workspace, pane, and state, for the chat panel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LeadStatus {
    pub state: LeadState,
    /// The Toodo workspace the lead works in.
    pub workspace_id: Option<WorkspaceId>,
    pub herdr_workspace_id: Option<String>,
    pub pane_id: Option<String>,
    /// The agent herdr recognized in the pane, e.g. `omo`.
    pub agent: Option<String>,
    pub agent_status: Option<AgentStatus>,
    /// The lead is composing a reply: it has seen the person's newest message, is working,
    /// and has not answered it yet.
    pub typing: bool,
    /// How often the server started the agent again since the server started.
    pub restarts: u32,
    /// When the running agent was started, or first seen running.
    pub started_at: Option<Timestamp>,
    /// Why the lead is failing or restarting.
    pub error: Option<String>,
}

pub type LeadStatusState = Mutex<LeadStatus>;

#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn get_lead(lead: State<'_, LeadStatusState>) -> LeadStatus {
    lead.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ChatAuthor {
    User,
    Lead,
    /// The server, reporting how a task finished.
    System,
}

/// A message of the chat with the lead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    #[specta(type = Number)]
    pub id: i64,
    pub author: ChatAuthor,
    /// Markdown.
    pub body: String,
    pub at: Timestamp,
    /// When the ID of a person's message was typed into the lead's pane, or the lead took
    /// it; null for the lead's, and for a person's that waits for the lead to run.
    pub delivered_at: Option<Timestamp>,
    /// When the lead picked the message up: it started a turn after the message reached
    /// its pane, or it replied in the chat.
    pub seen_at: Option<Timestamp>,
}

impl From<&chat::ChatMessage> for ChatMessage {
    fn from(message: &chat::ChatMessage) -> Self {
        Self {
            id: message.id.0,
            author: match message.author {
                chat::ChatAuthor::User => ChatAuthor::User,
                chat::ChatAuthor::Lead => ChatAuthor::Lead,
                chat::ChatAuthor::System => ChatAuthor::System,
            },
            body: message.body.clone(),
            at: Timestamp(message.at),
            delivered_at: message.delivered_at.map(Timestamp),
            seen_at: message.seen_at.map(Timestamp),
        }
    }
}

/// The latest messages of the chat with the lead, oldest first.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn list_chat_messages(storage: State<'_, StorageState>) -> CommandResult<Vec<ChatMessage>> {
    Ok(super::types::map_all(&lock(&storage).list_chat_messages()?))
}

/// Stores `text` as the user's message in the lead's inbox, then types its ID into the
/// lead's pane if the lead runs. A lead that is starting, restarting, or failing gets it
/// once it runs (see `crate::server::lead`); only a server without a lead rejects it.
/// `client_id`, chosen by the client for each message, makes sending it again harmless: a
/// message already stored under it is returned without storing or typing it again.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn send_chat_message(
    storage: State<'_, StorageState>,
    herdr: State<'_, HerdrState>,
    lead: State<'_, LeadStatusState>,
    text: String,
    client_id: String,
) -> CommandResult<ChatMessage> {
    send(&storage, &herdr, &lead, &text, &client_id)
}

fn send(
    storage: &StorageState,
    herdr: &Herdr,
    lead: &LeadStatusState,
    text: &str,
    client_id: &str,
) -> CommandResult<ChatMessage> {
    let body = chat::body(text).map_err(|error| CommandError::invalid_input(error.to_string()))?;
    let client_id = client_id.trim();
    if client_id.is_empty() || client_id.chars().count() > MAX_CLIENT_ID_CHARS {
        return Err(CommandError::invalid_input(format!(
            "clientId must have 1 to {MAX_CLIENT_ID_CHARS} characters"
        )));
    }
    let pane = running_pane(lead)?;
    if let Some(stored) = lock(storage).chat_message_by_client(client_id)? {
        return Ok((&stored).into());
    }
    let stored = lock(storage).add_chat_message(chat::ChatAuthor::User, &body, Some(client_id))?;
    let Some(pane) = pane else {
        return Ok((&stored).into());
    };
    if let Err(error) = signal(storage, herdr, &pane, &[ItemId::Chat(stored.id)]) {
        eprintln!(
            "toodo: chat message {} waits for the lead: {error}",
            stored.id
        );
    }
    let stored = lock(storage)
        .chat_message_by_client(client_id)?
        .unwrap_or(stored);
    Ok((&stored).into())
}

/// The pane of the lead's agent while it runs, `None` while it does not, and an error
/// when there is no lead.
fn running_pane(lead: &LeadStatusState) -> CommandResult<Option<String>> {
    let status = lead.lock().unwrap_or_else(PoisonError::into_inner).clone();
    match (status.state, status.pane_id) {
        (LeadState::Disabled, _) => Err(CommandError::rejected(
            "there is no lead: it runs on toodo-server unless TOODO_LEAD is off",
        )),
        (LeadState::Running, pane) => Ok(pane),
        _ => Ok(None),
    }
}

/// Types the IDs of `items` into the lead's `pane` and marks the chat messages among them
/// as delivered. Items that do not get there stay in the inbox all the same.
pub(crate) fn signal(
    storage: &StorageState,
    herdr: &Herdr,
    pane: &str,
    items: &[ItemId],
) -> Result<(), String> {
    herdr
        .run_in_pane(pane, &inbox::signal_line(items))
        .map_err(|error| error.to_string())?;
    let chat: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ItemId::Chat(id) => Some(*id),
            ItemId::Event(_) => None,
        })
        .collect();
    lock(storage)
        .mark_chat_delivered(&chat)
        .map(drop)
        .map_err(|error| error.to_string())
}

/// Stores a reply of the lead. The lead has picked up every message delivered before it.
pub(crate) fn reply(storage: &mut Storage, text: &str) -> CommandResult<ChatMessage> {
    let body = chat::body(text).map_err(|error| CommandError::invalid_input(error.to_string()))?;
    let stored = storage.add_chat_message(chat::ChatAuthor::Lead, &body, None)?;
    storage.mark_chat_seen(stored.at)?;
    Ok((&stored).into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::adapters::git::fake::FakeRunner;
    use crate::commands::CommandErrorKind;

    fn running() -> LeadStatusState {
        Mutex::new(LeadStatus {
            state: LeadState::Running,
            pane_id: Some("wL:p1".to_owned()),
            ..LeadStatus::default()
        })
    }

    fn herdr(runner: &Arc<FakeRunner>) -> Herdr {
        Herdr::new(Box::new(runner.clone()))
    }

    #[test]
    fn a_message_is_stored_and_only_its_id_is_typed_into_the_lead_pane_once() {
        let storage = Mutex::new(Storage::open_in_memory().unwrap());
        let runner = Arc::new(FakeRunner::new(vec![Ok(String::new())]));
        let lead = running();

        let sent = send(&storage, &herdr(&runner), &lead, " Hi\nthere ", "c1").unwrap();
        let again = send(&storage, &herdr(&runner), &lead, " Hi\nthere ", "c1").unwrap();

        assert_eq!(sent.body, "Hi\nthere");
        assert_eq!(sent.author, ChatAuthor::User);
        assert!(sent.delivered_at.is_some(), "{sent:?}");
        assert_eq!(again, sent);
        let calls = runner.invocations.lock().unwrap().clone();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(
            calls[0].args,
            [
                "pane",
                "run",
                "wL:p1",
                &format!(
                    "[Toodo inbox] chat-{} - take them with the Toodo MCP tool take_pending",
                    sent.id
                )
            ]
        );
        let stored = lock(&storage).list_chat_messages().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(ChatMessage::from(&stored[0]), sent);
        assert_eq!(
            lock(&storage).pending_items().unwrap(),
            [ItemId::Chat(stored[0].id)]
        );
    }

    #[test]
    fn nothing_is_stored_for_a_blank_message_or_a_server_without_a_lead() {
        let storage = Mutex::new(Storage::open_in_memory().unwrap());
        let runner = Arc::new(FakeRunner::new(Vec::new()));
        let herdr = herdr(&runner);

        let blank = send(&storage, &herdr, &running(), " \n", "c1").unwrap_err();
        assert_eq!(blank.kind, CommandErrorKind::InvalidInput);
        let no_id = send(&storage, &herdr, &running(), "Hi", " ").unwrap_err();
        assert_eq!(no_id.kind, CommandErrorKind::InvalidInput);
        let disabled = send(&storage, &herdr, &LeadStatusState::default(), "Hi", "c3");
        assert_eq!(disabled.unwrap_err().kind, CommandErrorKind::Rejected);

        assert!(runner.invocations.lock().unwrap().is_empty());
        assert!(lock(&storage).list_chat_messages().unwrap().is_empty());
    }

    #[test]
    fn a_message_to_a_lead_that_does_not_run_waits_in_its_inbox() {
        let storage = Mutex::new(Storage::open_in_memory().unwrap());
        let runner = Arc::new(FakeRunner::new(Vec::new()));
        for (state, client) in [
            (LeadState::Starting, "c1"),
            (LeadState::Restarting, "c2"),
            (LeadState::Failed, "c3"),
        ] {
            let lead = Mutex::new(LeadStatus {
                state,
                pane_id: Some("wL:p1".to_owned()),
                ..LeadStatus::default()
            });
            let sent = send(&storage, &herdr(&runner), &lead, "Hi", client).unwrap();
            assert_eq!(sent.delivered_at, None, "{state:?}");
        }

        assert!(runner.invocations.lock().unwrap().is_empty());
        let storage = lock(&storage);
        assert_eq!(storage.pending_items().unwrap().len(), 3);
        assert_eq!(storage.undelivered_chat().unwrap().len(), 3);
    }

    #[test]
    fn a_message_herdr_could_not_signal_is_kept_undelivered() {
        let storage = Mutex::new(Storage::open_in_memory().unwrap());
        let runner = Arc::new(FakeRunner::new(vec![Err(
            crate::adapters::git::fake::failed(),
        )]));

        let sent = send(&storage, &herdr(&runner), &running(), "Hi", "c1").unwrap();

        assert_eq!(sent.delivered_at, None);
        assert_eq!(
            lock(&storage).undelivered_chat().unwrap(),
            [crate::domain::chat::ChatMessageId(sent.id)]
        );
    }

    #[test]
    fn the_lead_replies_with_text() {
        let mut storage = Storage::open_in_memory().unwrap();
        let blank = reply(&mut storage, "  ").unwrap_err();
        assert_eq!(blank.kind, CommandErrorKind::InvalidInput);
        let asked = storage
            .add_chat_message(chat::ChatAuthor::User, "Plan it", Some("c1"))
            .unwrap();
        storage.mark_chat_delivered(&[asked.id]).unwrap();
        let answer = reply(&mut storage, "Created #4.").unwrap();
        assert_eq!(answer.author, ChatAuthor::Lead);
        let stored = storage.list_chat_messages().unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].seen_at, Some(answer.at.0), "{stored:?}");
        assert_eq!((answer.delivered_at, answer.seen_at), (None, None));
    }
}
