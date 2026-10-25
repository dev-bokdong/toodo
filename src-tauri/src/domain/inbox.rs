//! The lead's inbox: what the lead has to act on and has not taken yet. People's chat
//! messages and the task status changes it follows are kept until the lead takes them,
//! so none is lost while the lead restarts. The lead's pane only gets a line naming the
//! new items; the lead takes them, by ID, over MCP.

use std::fmt;

use super::chat::ChatMessageId;
use super::status::TaskStatus;

/// An item of the inbox, named `chat-<id>` or `event-<id>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemId {
    /// A person's chat message.
    Chat(ChatMessageId),
    /// A status change of a task, by its ID in the task's history.
    Event(i64),
}

impl ItemId {
    pub fn parse(text: &str) -> Option<Self> {
        let (kind, id) = text.trim().split_once('-')?;
        let id = id.parse::<i64>().ok().filter(|id| *id > 0)?;
        match kind {
            "chat" => Some(Self::Chat(ChatMessageId(id))),
            "event" => Some(Self::Event(id)),
            _ => None,
        }
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Chat(id) => write!(f, "chat-{id}"),
            Self::Event(id) => write!(f, "event-{id}"),
        }
    }
}

/// Whether the lead follows a task reaching `status`: it awaits completion, or it is
/// done, so its dependents may start.
pub fn follows(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::InReview | TaskStatus::Done)
}

/// The line typed into the lead's pane for new items: their IDs only, never their text.
pub fn signal_line(ids: &[ItemId]) -> String {
    let ids = ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    format!("[Toodo inbox] {ids} - take them with the Toodo MCP tool take_pending")
}

/// The longest request ID the lead may name a task creation or start with.
pub const MAX_REQUEST_ID_CHARS: usize = 100;

/// A request ID without surrounding whitespace, if it has 1 to
/// [`MAX_REQUEST_ID_CHARS`] characters.
pub fn request_id(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty() && text.chars().count() <= MAX_REQUEST_ID_CHARS).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_ids_round_trip_and_reject_anything_else() {
        for id in [ItemId::Chat(ChatMessageId(12)), ItemId::Event(40)] {
            assert_eq!(ItemId::parse(&id.to_string()), Some(id));
        }
        assert_eq!(
            ItemId::parse(" chat-3 "),
            Some(ItemId::Chat(ChatMessageId(3)))
        );
        for text in [
            "", "chat", "chat-", "chat-0", "chat--1", "task-1", "event-x", "12",
        ] {
            assert_eq!(ItemId::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_pane_gets_ids_only() {
        assert_eq!(
            signal_line(&[ItemId::Chat(ChatMessageId(2)), ItemId::Event(9)]),
            "[Toodo inbox] chat-2 event-9 - take them with the Toodo MCP tool take_pending"
        );
    }

    #[test]
    fn the_lead_follows_tasks_awaiting_completion_and_done() {
        let followed: Vec<_> = [
            TaskStatus::Planned,
            TaskStatus::InProgress,
            TaskStatus::InReview,
            TaskStatus::Done,
        ]
        .into_iter()
        .filter(|status| follows(*status))
        .collect();
        assert_eq!(followed, [TaskStatus::InReview, TaskStatus::Done]);
    }

    #[test]
    fn request_ids_are_trimmed_and_bounded() {
        assert_eq!(request_id(" chat-2/create "), Some("chat-2/create"));
        assert_eq!(request_id("  "), None);
        let long = "x".repeat(MAX_REQUEST_ID_CHARS + 1);
        assert_eq!(request_id(&long), None);
    }
}
