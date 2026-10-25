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
    /// its pane, or it replied in the chat. An estimate, never a sign it was handled.
    pub seen_at: Option<Timestamp>,
    /// When a person's message becomes overdue unless the lead handles it; null for the
    /// lead's and the system's, and for messages from before receipts were kept.
    pub due_at: Option<Timestamp>,
    /// When the lead acknowledged a person's message: it received it and is on it.
    pub acked_at: Option<Timestamp>,
    /// When the lead handled a person's message, and with what.
    pub handled_at: Option<Timestamp>,
    pub handled_by: Option<HandledBy>,
    /// The message was past `dueAt` without being handled when the server listed it, by
    /// the server's clock.
    pub overdue: bool,
}

/// What the lead handled a person's message with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HandledBy {
    /// Its reply in the chat.
    Reply {
        #[specta(type = Number)]
        message_id: i64,
    },
    /// A task it created or started for the message.
    Task { task_id: super::types::TaskId },
}

impl From<chat::HandledBy> for HandledBy {
    fn from(by: chat::HandledBy) -> Self {
        match by {
            chat::HandledBy::Reply(id) => Self::Reply { message_id: id.0 },
            chat::HandledBy::Task(id) => Self::Task { task_id: id.into() },
        }
    }
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
            due_at: message.due_at.map(Timestamp),
            acked_at: message.acked_at.map(Timestamp),
            handled_at: message.handled_at.map(Timestamp),
            handled_by: message.handled_by.map(Into::into),
            overdue: message.overdue,
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

/// Stores a reply of the lead. The lead has picked up every message delivered before it
/// (an estimate: it handles only the messages in `handles`, the person's messages it
/// answers, which are checked before anything is stored).
pub(crate) fn reply(
    storage: &mut Storage,
    text: &str,
    handles: &[String],
) -> CommandResult<ChatMessage> {
    let body = chat::body(text).map_err(|error| CommandError::invalid_input(error.to_string()))?;
    let handles = handles
        .iter()
        .map(|id| persons_message(storage, id))
        .collect::<CommandResult<Vec<_>>>()?;
    let stored = storage.add_chat_message(chat::ChatAuthor::Lead, &body, None)?;
    storage.mark_chat_seen(stored.at)?;
    for message in handles {
        storage.handle_chat_message(message, chat::HandledBy::Reply(stored.id))?;
    }
    Ok((&stored).into())
}

/// The person's message an inbox ID such as `chat-12` names.
fn persons_message(storage: &Storage, id: &str) -> CommandResult<chat::ChatMessageId> {
    let Some(ItemId::Chat(message)) = ItemId::parse(id) else {
        return Err(CommandError::invalid_input(format!(
            "{id:?} is not the ID of a chat message such as chat-12"
        )));
    };
    match storage.chat_message(message)? {
        Some(found) if found.author == chat::ChatAuthor::User => Ok(message),
        _ => Err(CommandError::not_found(format!(
            "{id} is not a person's message in the chat"
        ))),
    }
}

/// Records the lead's acknowledgement of the people's messages `ids` (inbox IDs such as
/// `chat-12`): it received them and is on them. Acknowledging again changes nothing. All
/// IDs are checked before any is recorded.
pub(crate) fn ack(storage: &mut Storage, ids: &[String]) -> CommandResult<Vec<ChatMessage>> {
    if ids.is_empty() {
        return Err(CommandError::invalid_input(
            "ids must name at least one message",
        ));
    }
    let messages = ids
        .iter()
        .map(|id| persons_message(storage, id))
        .collect::<CommandResult<Vec<_>>>()?;
    let mut acked = Vec::new();
    for message in messages {
        storage.ack_chat_message(message)?;
        if let Some(stored) = storage.chat_message(message)? {
            acked.push((&stored).into());
        }
    }
    Ok(acked)
}

/// Records that the lead handled the person's message `id` with `by`: a reply of its own
/// posted after the message, or a task it created or started for it (with a request ID
/// of the message, such as `chat-12/create-1`). Any other link is rejected. A message
/// handled before keeps its first link.
pub(crate) fn handle(
    storage: &mut Storage,
    id: &str,
    by: chat::HandledBy,
) -> CommandResult<ChatMessage> {
    let message = persons_message(storage, id)?;
    match by {
        chat::HandledBy::Reply(reply) => {
            let valid = storage
                .chat_message(reply)?
                .is_some_and(|reply| reply.author == chat::ChatAuthor::Lead && reply.id > message);
            if !valid {
                return Err(CommandError::rejected(format!(
                    "chat message {reply} is not a reply of yours posted after {id}"
                )));
            }
        }
        chat::HandledBy::Task(task) => {
            if !storage.task_made_for(task, id)? {
                return Err(CommandError::rejected(format!(
                    "task #{} was not created or started for {id}: name the change \
                     with a request_id such as {id}/create-1, or answer with a reply",
                    task.0
                )));
            }
        }
    }
    storage.handle_chat_message(message, by)?;
    let stored = storage
        .chat_message(message)?
        .ok_or_else(|| CommandError::not_found(format!("{id} is gone")))?;
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
        let blank = reply(&mut storage, "  ", &[]).unwrap_err();
        assert_eq!(blank.kind, CommandErrorKind::InvalidInput);
        let asked = storage
            .add_chat_message(chat::ChatAuthor::User, "Plan it", Some("c1"))
            .unwrap();
        storage.mark_chat_delivered(&[asked.id]).unwrap();
        let answer = reply(&mut storage, "Created #4.", &[]).unwrap();
        assert_eq!(answer.author, ChatAuthor::Lead);
        let stored = storage.list_chat_messages().unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].seen_at, Some(answer.at.0), "{stored:?}");
        // Seen is only an estimate: a reply without `handles` handles nothing.
        assert_eq!((stored[0].acked_at, stored[0].handled_at), (None, None));
        assert_eq!((answer.delivered_at, answer.seen_at), (None, None));
    }

    fn says(storage: &mut Storage, text: &str) -> String {
        let message = storage
            .add_chat_message(chat::ChatAuthor::User, text, None)
            .unwrap();
        format!("chat-{}", message.id)
    }

    fn message(storage: &Storage, id: &str) -> chat::ChatMessage {
        let Some(ItemId::Chat(id)) = ItemId::parse(id) else {
            panic!("{id}")
        };
        storage.chat_message(id).unwrap().unwrap()
    }

    fn task_for(storage: &mut Storage, request_id: Option<&str>) -> crate::domain::model::TaskId {
        use crate::domain::model::{TaskType, WorkspaceLocation};
        use crate::domain::path::WindowsPath;
        use crate::domain::status::TaskStatus;
        use crate::storage::{NewTask, NewWorkspace};
        let workspace = match storage.list_workspaces().unwrap().first() {
            Some(workspace) => workspace.id,
            None => {
                storage
                    .create_workspace(&NewWorkspace {
                        name: "toodo".to_owned(),
                        location: WorkspaceLocation::Local {
                            path: WindowsPath::parse(r"C:\dev\toodo").unwrap().into(),
                        },
                        default_base_branch: "main".to_owned(),
                    })
                    .unwrap()
                    .id
            }
        };
        let task = storage
            .create_task(&NewTask {
                workspace_id: workspace,
                task_type: TaskType::Docs,
                title: "Write the guide".to_owned(),
                description: String::new(),
                estimate: None,
                due_date: None,
                criteria: Vec::new(),
            })
            .unwrap()
            .id;
        if let Some(request_id) = request_id {
            storage
                .name_request(task, TaskStatus::Planned, request_id)
                .unwrap();
        }
        task
    }

    #[test]
    fn acknowledging_twice_keeps_the_first_time_and_only_peoples_messages_are_acked() {
        let mut storage = Storage::open_in_memory().unwrap();
        storage.set_clock(|| 10);
        let first = says(&mut storage, "Plan it");
        let lead = reply(&mut storage, "On it", &[]).unwrap();
        storage.set_clock(|| 20);

        let acked = ack(&mut storage, std::slice::from_ref(&first)).unwrap();
        assert_eq!(acked[0].acked_at, Some(Timestamp(20)));
        storage.set_clock(|| 30);
        let again = ack(&mut storage, std::slice::from_ref(&first)).unwrap();
        assert_eq!(again, acked);
        assert_eq!(acked[0].handled_at, None, "an ack is not handling");

        for (id, kind) in [
            (format!("chat-{}", lead.id), CommandErrorKind::NotFound),
            ("chat-999".to_owned(), CommandErrorKind::NotFound),
            ("event-1".to_owned(), CommandErrorKind::InvalidInput),
            ("12".to_owned(), CommandErrorKind::InvalidInput),
        ] {
            // A bad ID next to a good one records nothing.
            let ids = [says(&mut storage, "More"), id.clone()];
            assert_eq!(ack(&mut storage, &ids).unwrap_err().kind, kind, "{id}");
            assert_eq!(message(&storage, &ids[0]).acked_at, None, "{id}");
        }
        assert_eq!(
            ack(&mut storage, &[]).unwrap_err().kind,
            CommandErrorKind::InvalidInput
        );
    }

    #[test]
    fn only_the_answered_message_is_handled_and_its_first_link_stays() {
        let mut storage = Storage::open_in_memory().unwrap();
        storage.set_clock(|| 10);
        let a = says(&mut storage, "Plan the release");
        let b = says(&mut storage, "What is running?");
        storage.set_clock(|| 20);
        let answer = reply(&mut storage, "Two tasks run.", std::slice::from_ref(&b)).unwrap();

        let handled = message(&storage, &b);
        assert_eq!(handled.handled_at, Some(20));
        assert_eq!(handled.acked_at, Some(20), "handling acknowledges too");
        assert_eq!(
            handled.handled_by,
            Some(chat::HandledBy::Reply(chat::ChatMessageId(answer.id)))
        );
        assert_eq!(message(&storage, &a).handled_at, None, "A stays open");
        assert_eq!(
            storage.open_chat().unwrap(),
            Vec::new(),
            "neither was taken"
        );

        // Handling again, with another valid link, keeps the first one.
        storage.set_clock(|| 30);
        let task = task_for(&mut storage, Some(&format!("{b}/create-1")));
        let again = handle(&mut storage, &b, chat::HandledBy::Task(task)).unwrap();
        assert_eq!(again.handled_at, Some(Timestamp(20)));
        assert_eq!(
            again.handled_by,
            Some(HandledBy::Reply {
                message_id: answer.id
            })
        );
        // A task made for A handles A.
        let for_a = task_for(&mut storage, Some(&a));
        let a_handled = handle(&mut storage, &a, chat::HandledBy::Task(for_a)).unwrap();
        assert_eq!(
            a_handled.handled_by,
            Some(HandledBy::Task {
                task_id: for_a.into()
            })
        );
    }

    #[test]
    fn forged_or_unrelated_links_are_rejected_and_record_nothing() {
        let mut storage = Storage::open_in_memory().unwrap();
        let earlier = reply(&mut storage, "Hello", &[]).unwrap();
        let asked = says(&mut storage, "Plan it");
        let other = says(&mut storage, "Something else");
        let unrelated = task_for(&mut storage, None);
        // `chat-1N/...` is not a request of `chat-1`.
        let lookalike = task_for(&mut storage, Some(&format!("{asked}0/create-1")));
        let for_other = task_for(&mut storage, Some(&format!("{other}/create-1")));
        let Some(ItemId::Chat(other_id)) = ItemId::parse(&other) else {
            panic!()
        };

        for by in [
            chat::HandledBy::Reply(chat::ChatMessageId(earlier.id)),
            chat::HandledBy::Reply(other_id),
            chat::HandledBy::Reply(chat::ChatMessageId(999)),
            chat::HandledBy::Task(unrelated),
            chat::HandledBy::Task(lookalike),
            chat::HandledBy::Task(for_other),
            chat::HandledBy::Task(crate::domain::model::TaskId(999)),
        ] {
            let error = handle(&mut storage, &asked, by).unwrap_err();
            assert_eq!(error.kind, CommandErrorKind::Rejected, "{by:?}");
        }
        assert_eq!(
            handle(&mut storage, "event-1", chat::HandledBy::Task(for_other))
                .unwrap_err()
                .kind,
            CommandErrorKind::InvalidInput
        );
        let posted = storage.list_chat_messages().unwrap().len();
        let bad = reply(
            &mut storage,
            "Done",
            &[asked.clone(), "chat-999".to_owned()],
        );
        assert_eq!(bad.unwrap_err().kind, CommandErrorKind::NotFound);
        assert_eq!(
            storage.list_chat_messages().unwrap().len(),
            posted,
            "nothing posted"
        );
        let stored = message(&storage, &asked);
        assert_eq!((stored.acked_at, stored.handled_at), (None, None));
    }

    #[test]
    fn a_message_past_its_due_time_is_overdue_until_it_is_handled() {
        let mut storage = Storage::open_in_memory().unwrap();
        storage.set_clock(|| 1_000);
        let asked = says(&mut storage, "Plan it");
        let due = 1_000 + chat::HANDLE_WITHIN_SECS;
        assert_eq!(message(&storage, &asked).due_at, Some(due));
        storage.set_clock(move || due - 1);
        ack(&mut storage, std::slice::from_ref(&asked)).unwrap();
        assert!(!message(&storage, &asked).overdue);
        assert!(!storage.chat_fell_due(1_000, due - 1).unwrap());

        storage.set_clock(move || due);
        let listed = storage.list_chat_messages().unwrap();
        assert!(
            ChatMessage::from(&listed[0]).overdue,
            "acked is not handled"
        );
        assert!(storage.chat_fell_due(due - 1, due).unwrap());
        assert!(!storage.chat_fell_due(due, due + 5).unwrap(), "once");

        reply(&mut storage, "Created #4.", std::slice::from_ref(&asked)).unwrap();
        assert!(!message(&storage, &asked).overdue);
        let lead = &storage.list_chat_messages().unwrap()[1];
        assert_eq!((lead.due_at, lead.overdue), (None, false));
    }

    #[test]
    fn an_open_message_beyond_the_history_is_still_listed_and_can_be_handled() {
        let mut storage = Storage::open_in_memory().unwrap();
        let old = says(&mut storage, "An old request");
        let handled_old = says(&mut storage, "An old question");
        reply(
            &mut storage,
            "Answered.",
            std::slice::from_ref(&handled_old),
        )
        .unwrap();
        for n in 0..500 {
            reply(&mut storage, &n.to_string(), &[]).unwrap();
        }
        let listed = storage.list_chat_messages().unwrap();
        assert_eq!(listed.len(), 501, "the latest 500 and the open one");
        assert_eq!(format!("chat-{}", listed[0].id), old);

        let newest = listed.last().unwrap().id;
        let by = chat::HandledBy::Reply(newest);
        assert!(handle(&mut storage, &old, by).unwrap().handled_at.is_some());
        assert_eq!(storage.list_chat_messages().unwrap().len(), 500);
    }

    #[test]
    fn receipts_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("toodo.sqlite3");
        let (a, b) = {
            let mut storage = Storage::open(&path).unwrap();
            let a = says(&mut storage, "Plan it");
            let b = says(&mut storage, "And this");
            storage.take_pending(None).unwrap();
            ack(&mut storage, &[a.clone(), b.clone()]).unwrap();
            reply(&mut storage, "Planned.", std::slice::from_ref(&a)).unwrap();
            (a, b)
        };
        let storage = Storage::open(&path).unwrap();
        let (a, b) = (message(&storage, &a), message(&storage, &b));
        assert!(a.handled_at.is_some() && a.handled_by.is_some());
        assert!(b.acked_at.is_some() && b.handled_at.is_none());
        assert_eq!(storage.open_chat().unwrap(), [b.id], "B is still open");
    }
}
