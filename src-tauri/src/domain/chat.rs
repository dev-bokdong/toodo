//! The conversation with the lead in the chat panel: people's messages, typed into the
//! lead's pane, and the replies the lead posts back.

use std::fmt;

use super::model::TaskId;

/// The longest message, in characters; longer text belongs in a task description.
pub const MAX_BODY_CHARS: usize = 20_000;

/// How long the lead has to handle a person's message, in seconds from when the server
/// stored it, before the chat shows the message as overdue.
pub const HANDLE_WITHIN_SECS: i64 = 10 * 60;

/// When a person's message stored at `at` (server time) becomes overdue.
pub fn due_at(at: i64) -> i64 {
    at.saturating_add(HANDLE_WITHIN_SECS)
}

/// What the lead handled a person's message with: a reply it posted after the message,
/// or a task it created or started for it (named with the message's request ID).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandledBy {
    Reply(ChatMessageId),
    Task(TaskId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChatMessageId(pub i64);

impl fmt::Display for ChatMessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatAuthor {
    /// A person, from a client of the server.
    User,
    Lead,
    /// The server itself, reporting how a task finished (see [`crate::domain::finish`]).
    /// Its reports are not answers of the lead or a person: they mark nothing as seen and
    /// confirm nothing.
    System,
}

impl ChatAuthor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Lead => "lead",
            Self::System => "system",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::User, Self::Lead, Self::System]
            .into_iter()
            .find(|author| author.as_str() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: ChatMessageId,
    pub author: ChatAuthor,
    /// Markdown.
    pub body: String,
    pub at: i64,
    /// When a person's message was typed into the lead's pane; `None` for the lead's.
    pub delivered_at: Option<i64>,
    /// When the lead picked a delivered message up: it started a turn after the message
    /// reached its pane, or it replied. An estimate: it never counts as handled.
    pub seen_at: Option<i64>,
    /// When a person's message becomes overdue unless handled; `None` for the lead's and
    /// the system's, and for people's messages stored before receipts were kept.
    pub due_at: Option<i64>,
    /// When the lead acknowledged the message: it received it and is on it. Never an
    /// approval of anything.
    pub acked_at: Option<i64>,
    /// When the lead handled the message, with what (see [`HandledBy`]); `handled_by` is
    /// `None` for a handled message whose task was deleted since.
    pub handled_at: Option<i64>,
    pub handled_by: Option<HandledBy>,
    /// Whether the message was past its `due_at`, unhandled, when it was read.
    pub overdue: bool,
    /// Where an imported message came from; `None` for messages written in Toodo.
    pub origin: Option<ChatOrigin>,
}

/// The longest origin label, in characters.
pub const MAX_ORIGIN_CHARS: usize = 40;
/// The longest ID of a message in its origin, in characters.
pub const MAX_ORIGIN_ID_CHARS: usize = 200;

/// Where a message imported from another chat came from: the origin's label, such as
/// `matrix`, and the message's ID there, which is unique within the origin.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChatOrigin {
    label: String,
    id: String,
}

impl ChatOrigin {
    /// A label of 1 to [`MAX_ORIGIN_CHARS`] lowercase ASCII letters, digits, `.`, `_`, or
    /// `-`, and an ID of 1 to [`MAX_ORIGIN_ID_CHARS`] characters without control
    /// characters, both without surrounding whitespace.
    pub fn parse(label: &str, id: &str) -> Result<Self, String> {
        let label = label.trim();
        let id = id.trim();
        let label_ok = (1..=MAX_ORIGIN_CHARS).contains(&label.len())
            && label.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            });
        if !label_ok {
            return Err(format!(
                "origin must have 1 to {MAX_ORIGIN_CHARS} lowercase letters, digits, '.', '_', \
                 or '-', not {label:?}"
            ));
        }
        if !(1..=MAX_ORIGIN_ID_CHARS).contains(&id.chars().count())
            || id.chars().any(char::is_control)
        {
            return Err(format!(
                "origin_id must have 1 to {MAX_ORIGIN_ID_CHARS} characters without control \
                 characters"
            ));
        }
        Ok(Self {
            label: label.to_owned(),
            id: id.to_owned(),
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

impl ChatMessage {
    /// Whether the message is unhandled past its due time at `now`, server time.
    pub fn is_overdue(&self, now: i64) -> bool {
        self.handled_at.is_none() && self.due_at.is_some_and(|due| now >= due)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatBodyError {
    Blank,
    TooLong,
}

impl fmt::Display for ChatBodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank => write!(f, "the message is empty"),
            Self::TooLong => write!(f, "the message is longer than {MAX_BODY_CHARS} characters"),
        }
    }
}

/// The body of a new message: `text` without surrounding whitespace.
pub fn body(text: &str) -> Result<String, ChatBodyError> {
    let text = text.trim();
    if text.is_empty() {
        Err(ChatBodyError::Blank)
    } else if text.chars().count() > MAX_BODY_CHARS {
        Err(ChatBodyError::TooLong)
    } else {
        Ok(text.to_owned())
    }
}

/// The line typed into the lead's pane for a person's message. Agents' prompts take one
/// line (Enter submits), so line breaks become ` / `; the stored message keeps them.
pub fn pane_line(body: &str) -> String {
    let lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    format!("[Toodo chat] {}", lines.join(" / "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_are_trimmed_and_must_have_text_within_the_limit() {
        assert_eq!(body("  hi \n").unwrap(), "hi");
        assert_eq!(body(" \n\t").unwrap_err(), ChatBodyError::Blank);
        assert!(body(&"x".repeat(MAX_BODY_CHARS)).is_ok());
        assert_eq!(
            body(&"x".repeat(MAX_BODY_CHARS + 1)).unwrap_err(),
            ChatBodyError::TooLong
        );
    }

    #[test]
    fn an_unhandled_message_is_overdue_from_its_due_time_on() {
        let mut message = ChatMessage {
            id: ChatMessageId(1),
            author: ChatAuthor::User,
            body: "Plan it".to_owned(),
            at: 100,
            delivered_at: Some(100),
            seen_at: Some(101),
            due_at: Some(due_at(100)),
            acked_at: None,
            handled_at: None,
            handled_by: None,
            overdue: false,
            origin: None,
        };
        assert!(!message.is_overdue(100 + HANDLE_WITHIN_SECS - 1));
        assert!(message.is_overdue(100 + HANDLE_WITHIN_SECS));
        message.handled_at = Some(900);
        assert!(!message.is_overdue(i64::MAX));
        // Seen is an estimate: a message seen but not handled stays overdue.
        message.handled_at = None;
        message.due_at = None;
        assert!(
            !message.is_overdue(i64::MAX),
            "messages without a due time never are"
        );
    }

    #[test]
    fn an_origin_has_a_plain_label_and_an_id_without_control_characters() {
        let origin = ChatOrigin::parse(" matrix ", " !room:example.org/$event ").unwrap();
        assert_eq!(
            (origin.label(), origin.id()),
            ("matrix", "!room:example.org/$event")
        );
        assert!(ChatOrigin::parse("chat.v2_x-1", "1").is_ok());
        for (label, id) in [
            ("", "1"),
            ("Matrix", "1"),
            ("my chat", "1"),
            ("x", ""),
            ("x", "a\nb"),
        ] {
            assert!(ChatOrigin::parse(label, id).is_err(), "{label:?} {id:?}");
        }
        assert!(ChatOrigin::parse(&"x".repeat(MAX_ORIGIN_CHARS + 1), "1").is_err());
        assert!(ChatOrigin::parse("x", &"1".repeat(MAX_ORIGIN_ID_CHARS + 1)).is_err());
    }

    #[test]
    fn a_message_reaches_the_pane_as_one_marked_line() {
        assert_eq!(
            pane_line("Plan the release\n\n  - notes first\r\n- then tag"),
            "[Toodo chat] Plan the release / - notes first / - then tag"
        );
    }
}
