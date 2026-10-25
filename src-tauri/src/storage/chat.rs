//! The chat panel's conversation with the lead.

use rusqlite::{OptionalExtension, Row};

use super::{Result, Storage, invalid_data};
use crate::domain::chat::{ChatAuthor, ChatMessage, ChatMessageId};

/// The most messages the chat panel shows: the latest ones.
const HISTORY: i64 = 500;

const COLUMNS: &str = "id, author, body, at, delivered_at, seen_at";

fn message(row: &Row<'_>) -> rusqlite::Result<(ChatMessage, String)> {
    Ok((
        ChatMessage {
            id: ChatMessageId(row.get(0)?),
            author: ChatAuthor::User,
            body: row.get(2)?,
            at: row.get(3)?,
            delivered_at: row.get(4)?,
            seen_at: row.get(5)?,
        },
        row.get(1)?,
    ))
}

fn with_author((mut message, author): (ChatMessage, String)) -> Result<ChatMessage> {
    message.author = ChatAuthor::parse(&author)
        .ok_or_else(|| invalid_data(format!("unknown chat author {author}")))?;
    Ok(message)
}

impl Storage {
    /// The latest messages, oldest first.
    pub fn list_chat_messages(&self) -> Result<Vec<ChatMessage>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM
                 (SELECT {COLUMNS} FROM chat_messages ORDER BY id DESC LIMIT ?1)
             ORDER BY id"
        ))?;
        let rows = statement
            .query_map([HISTORY], message)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(with_author).collect()
    }

    /// The newest message, if there is any.
    pub fn latest_chat_message(&self) -> Result<Option<ChatMessage>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM chat_messages ORDER BY id DESC LIMIT 1"),
                [],
                message,
            )
            .optional()?
            .map(with_author)
            .transpose()
    }

    /// The message a client sent under `client_id`, if it was stored.
    pub fn chat_message_by_client(&self, client_id: &str) -> Result<Option<ChatMessage>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM chat_messages WHERE client_id = ?1"),
                [client_id],
                message,
            )
            .optional()?
            .map(with_author)
            .transpose()
    }

    /// Stores a message; `body` must already be valid (see [`crate::domain::chat::body`]).
    /// A person's message is stored before it reaches the lead's pane, so it waits in the
    /// lead's inbox, undelivered (see [`Storage::mark_chat_delivered`]). A user message
    /// with the `client_id` of a stored one is not stored again: the stored one is
    /// returned.
    pub fn add_chat_message(
        &mut self,
        author: ChatAuthor,
        body: &str,
        client_id: Option<&str>,
    ) -> Result<ChatMessage> {
        if let Some(client_id) = client_id
            && let Some(stored) = self.chat_message_by_client(client_id)?
        {
            return Ok(stored);
        }
        let at = self.now();
        self.conn.execute(
            "INSERT INTO chat_messages (author, body, client_id, at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![author.as_str(), body, client_id, at],
        )?;
        Ok(ChatMessage {
            id: ChatMessageId(self.conn.last_insert_rowid()),
            author,
            body: body.to_owned(),
            at,
            delivered_at: None,
            seen_at: None,
        })
    }

    /// Marks the messages delivered by `delivered_by` (seconds since the Unix epoch) that
    /// the lead has not picked up yet as seen now, returning how many there were.
    pub fn mark_chat_seen(&mut self, delivered_by: i64) -> Result<usize> {
        let at = self.now();
        Ok(self.conn.execute(
            "UPDATE chat_messages SET seen_at = max(?1, delivered_at)
             WHERE delivered_at <= ?2 AND seen_at IS NULL",
            [at, delivered_by],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_listed_oldest_first_and_a_resent_one_is_stored_once() {
        let mut storage = Storage::open_in_memory().unwrap();
        storage.set_clock(|| 7);
        let first = storage
            .add_chat_message(ChatAuthor::User, "Hello", Some("c1"))
            .unwrap();
        let reply = storage
            .add_chat_message(ChatAuthor::Lead, "Hi", None)
            .unwrap();
        let again = storage
            .add_chat_message(ChatAuthor::User, "Hello", Some("c1"))
            .unwrap();

        assert_eq!(again, first);
        assert_eq!(
            storage.list_chat_messages().unwrap(),
            vec![first.clone(), reply]
        );
        assert_eq!(first.at, 7);
        assert_eq!((first.delivered_at, first.seen_at), (None, None));
        assert_eq!(storage.chat_message_by_client("c1").unwrap(), Some(first));
        assert_eq!(storage.chat_message_by_client("c2").unwrap(), None);
        assert_eq!(storage.list_chat_messages().unwrap()[1].delivered_at, None);
    }

    #[test]
    fn the_lead_sees_every_delivered_message_once() {
        let mut storage = Storage::open_in_memory().unwrap();
        storage.set_clock(|| 7);
        let hello = storage
            .add_chat_message(ChatAuthor::User, "Hello", Some("c1"))
            .unwrap();
        storage
            .add_chat_message(ChatAuthor::Lead, "Hi", None)
            .unwrap();
        // Not delivered yet: the lead cannot have seen it.
        assert_eq!(storage.mark_chat_seen(9).unwrap(), 0);
        storage.mark_chat_delivered(&[hello.id]).unwrap();
        storage.set_clock(|| 9);

        assert_eq!(storage.mark_chat_seen(9).unwrap(), 1);
        let next = storage
            .add_chat_message(ChatAuthor::User, "Next", Some("c2"))
            .unwrap();
        storage.mark_chat_delivered(&[next.id]).unwrap();
        storage.set_clock(|| 12);
        // Delivered after the lead was seen working: not picked up by that turn.
        assert_eq!(storage.mark_chat_seen(8).unwrap(), 0);
        assert_eq!(storage.mark_chat_seen(12).unwrap(), 1);
        assert_eq!(storage.mark_chat_seen(12).unwrap(), 0);

        let seen: Vec<_> = storage
            .list_chat_messages()
            .unwrap()
            .iter()
            .map(|message| message.seen_at)
            .collect();
        assert_eq!(seen, [Some(9), None, Some(12)]);
    }

    #[test]
    fn only_the_latest_messages_are_listed() {
        let mut storage = Storage::open_in_memory().unwrap();
        for n in 0..HISTORY + 2 {
            storage
                .add_chat_message(ChatAuthor::Lead, &n.to_string(), None)
                .unwrap();
        }
        let listed = storage.list_chat_messages().unwrap();
        assert_eq!(listed.len(), usize::try_from(HISTORY).unwrap());
        assert_eq!(listed[0].body, "2");
        assert_eq!(listed.last().unwrap().body, (HISTORY + 1).to_string());
    }
}
