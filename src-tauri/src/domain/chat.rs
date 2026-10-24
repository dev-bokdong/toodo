//! The conversation with the lead in the chat panel: people's messages, typed into the
//! lead's pane, and the replies the lead posts back.

use std::fmt;

/// The longest message, in characters; longer text belongs in a task description.
pub const MAX_BODY_CHARS: usize = 20_000;

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
}

impl ChatAuthor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Lead => "lead",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::User, Self::Lead]
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
    fn a_message_reaches_the_pane_as_one_marked_line() {
        assert_eq!(
            pane_line("Plan the release\n\n  - notes first\r\n- then tag"),
            "[Toodo chat] Plan the release / - notes first / - then tag"
        );
    }
}
