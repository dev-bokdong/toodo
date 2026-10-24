//! Project memory: durable knowledge an agent reads before it acts and writes as it
//! learns, kept for one workspace or shared by all of them.
//!
//! An entry is never deleted to change its meaning: replacing it with a newer entry
//! (superseding) keeps the old one as history, linked to its replacement.

use super::model::{TaskId, WorkspaceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemoryId(pub i64);

impl std::fmt::Display for MemoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What a memory entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemoryKind {
    /// A choice that was made, and why.
    Decision,
    /// A standing rule to follow.
    Rule,
    /// Anything else worth knowing.
    Note,
    /// An overview, e.g. of the project's state or architecture.
    Summary,
}

impl MemoryKind {
    pub const ALL: [Self; 4] = [Self::Decision, Self::Rule, Self::Note, Self::Summary];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Rule => "rule",
            Self::Note => "note",
            Self::Summary => "summary",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

/// Whose entries to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    /// The entries shared by every workspace.
    Shared,
    /// The entries of one workspace, and the shared ones too when `with_shared`.
    Workspace {
        workspace: WorkspaceId,
        with_shared: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEntry {
    pub id: MemoryId,
    /// `None` for an entry shared by every workspace.
    pub workspace: Option<WorkspaceId>,
    pub kind: MemoryKind,
    pub title: String,
    /// Markdown.
    pub body: String,
    /// The task the entry came from, if any.
    pub task: Option<TaskId>,
    pub created_at: i64,
    pub updated_at: i64,
    /// The newer entry that replaced this one; `None` while it is current.
    pub superseded_by: Option<MemoryId>,
}

/// The content of an entry, as written when it is added, edited, or replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryContent {
    pub kind: MemoryKind,
    pub title: String,
    pub body: String,
    pub task: Option<TaskId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryContentError {
    EmptyTitle,
}

impl std::fmt::Display for MemoryContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyTitle => write!(f, "a memory entry needs a title"),
        }
    }
}

impl MemoryContent {
    /// Trims the title and body; rejects a blank title.
    pub fn new(
        kind: MemoryKind,
        title: &str,
        body: &str,
        task: Option<TaskId>,
    ) -> Result<Self, MemoryContentError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(MemoryContentError::EmptyTitle);
        }
        Ok(Self {
            kind,
            title: title.to_owned(),
            body: body.trim().to_owned(),
            task,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_through_their_names() {
        for kind in MemoryKind::ALL {
            assert_eq!(MemoryKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(MemoryKind::parse("Decision"), None);
    }

    #[test]
    fn content_is_trimmed_and_needs_a_title() {
        let content = MemoryContent::new(MemoryKind::Rule, "  Use npm ", "\n body \n", None);
        assert_eq!(
            content.map(|c| (c.title, c.body)),
            Ok(("Use npm".to_owned(), "body".to_owned()))
        );
        assert_eq!(
            MemoryContent::new(MemoryKind::Note, " \n", "body", None),
            Err(MemoryContentError::EmptyTitle)
        );
    }
}
