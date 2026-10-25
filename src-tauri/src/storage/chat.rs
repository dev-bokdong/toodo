//! The chat panel's conversation with the lead.

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{Result, Storage, invalid_data};
use crate::domain::chat::{self, ChatAuthor, ChatMessage, ChatMessageId, ChatOrigin, HandledBy};
use crate::domain::finish::Finish;
use crate::domain::model::TaskId;

/// The most messages the chat panel shows: the latest ones, and older ones still open.
const HISTORY: i64 = 500;

/// The most messages one read of the chat after a cursor returns.
pub const MAX_FEED: usize = 500;

const COLUMNS: &str = "id, author, body, at, delivered_at, seen_at, due_at, acked_at,
    handled_at, handled_reply_id, handled_task_id, origin, origin_id";

/// A message as read, with its author and origin still as stored.
type Stored = (ChatMessage, String, Option<(String, String)>);

fn message(row: &Row<'_>) -> rusqlite::Result<Stored> {
    let reply: Option<i64> = row.get(9)?;
    let task: Option<i64> = row.get(10)?;
    let origin: Option<String> = row.get(11)?;
    let origin_id: Option<String> = row.get(12)?;
    Ok((
        ChatMessage {
            id: ChatMessageId(row.get(0)?),
            author: ChatAuthor::User,
            body: row.get(2)?,
            at: row.get(3)?,
            delivered_at: row.get(4)?,
            seen_at: row.get(5)?,
            due_at: row.get(6)?,
            acked_at: row.get(7)?,
            handled_at: row.get(8)?,
            handled_by: reply
                .map(|id| HandledBy::Reply(ChatMessageId(id)))
                .or(task.map(|id| HandledBy::Task(TaskId(id)))),
            overdue: false,
            origin: None,
        },
        row.get(1)?,
        origin.zip(origin_id),
    ))
}

impl Storage {
    fn with_author(&self, (mut message, author, origin): Stored) -> Result<ChatMessage> {
        message.author = ChatAuthor::parse(&author)
            .ok_or_else(|| invalid_data(format!("unknown chat author {author}")))?;
        message.origin = origin
            .map(|(label, id)| ChatOrigin::parse(&label, &id).map_err(invalid_data))
            .transpose()?;
        message.overdue = message.is_overdue(self.now());
        Ok(message)
    }

    /// The latest messages, and older messages of people the lead has not handled yet,
    /// oldest first.
    pub fn list_chat_messages(&self) -> Result<Vec<ChatMessage>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM chat_messages
             WHERE id IN (SELECT id FROM chat_messages ORDER BY id DESC LIMIT ?1)
                OR (handled_at IS NULL AND due_at IS NOT NULL)
             ORDER BY id"
        ))?;
        let rows = statement
            .query_map([HISTORY], message)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|row| self.with_author(row)).collect()
    }

    /// Every message, of people, the lead, and the system, with an ID above `after`, oldest
    /// first: at most `limit` (capped at [`MAX_FEED`]), however far back `after` is.
    pub fn chat_after(&self, after: i64, limit: usize) -> Result<Vec<ChatMessage>> {
        let limit = i64::try_from(limit.min(MAX_FEED)).unwrap_or(0);
        let mut statement = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM chat_messages WHERE id > ?1 ORDER BY id LIMIT ?2"
        ))?;
        let rows = statement
            .query_map([after, limit], message)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|row| self.with_author(row)).collect()
    }

    /// The message imported from `origin`, if it was.
    pub fn chat_message_by_origin(&self, origin: &ChatOrigin) -> Result<Option<ChatMessage>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM chat_messages WHERE origin = ?1 AND origin_id = ?2"
                ),
                [origin.label(), origin.id()],
                message,
            )
            .optional()?
            .map(|row| self.with_author(row))
            .transpose()
    }

    /// Stores a message of a person or the lead written in another chat, `origin`, and
    /// already read by the lead there: a person's message is stored as taken, delivered,
    /// and seen, so it never enters the lead's inbox, and without a due time, so it is
    /// never overdue. `body` must already be valid. A message imported from `origin`
    /// before is not stored again: returns it with `false`.
    pub fn import_chat_message(
        &mut self,
        author: ChatAuthor,
        body: &str,
        origin: &ChatOrigin,
    ) -> Result<(ChatMessage, bool)> {
        if author == ChatAuthor::System {
            return Err(invalid_data("the system's reports are never imported"));
        }
        if let Some(stored) = self.chat_message_by_origin(origin)? {
            return Ok((stored, false));
        }
        let at = self.now();
        let read = (author == ChatAuthor::User).then_some(at);
        self.conn.execute(
            "INSERT INTO chat_messages (author, body, at, delivered_at, seen_at, taken_at,
                 origin, origin_id)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4, ?5, ?6)",
            params![author.as_str(), body, at, read, origin.label(), origin.id()],
        )?;
        Ok((
            ChatMessage {
                id: ChatMessageId(self.conn.last_insert_rowid()),
                author,
                body: body.to_owned(),
                at,
                delivered_at: read,
                seen_at: read,
                due_at: None,
                acked_at: None,
                handled_at: None,
                handled_by: None,
                overdue: false,
                origin: Some(origin.clone()),
            },
            true,
        ))
    }

    /// One message, wherever it is in the history.
    pub fn chat_message(&self, id: ChatMessageId) -> Result<Option<ChatMessage>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM chat_messages WHERE id = ?1"),
                [id.0],
                message,
            )
            .optional()?
            .map(|row| self.with_author(row))
            .transpose()
    }

    /// Records that the lead acknowledged a person's message, now; a message acknowledged
    /// before keeps its first time. Returns whether `id` names a person's message.
    pub fn ack_chat_message(&mut self, id: ChatMessageId) -> Result<bool> {
        let at = self.now();
        Ok(self.conn.execute(
            "UPDATE chat_messages SET acked_at = coalesce(acked_at, ?1)
             WHERE id = ?2 AND author = 'user'",
            [at, id.0],
        )? > 0)
    }

    /// Records that the lead handled a person's message with `by`, now, acknowledging it
    /// too. A message handled before keeps its first time and link, so the state never
    /// goes back. Whether `by` is a valid link for the message is up to the caller. Returns
    /// whether `id` names a person's message.
    pub fn handle_chat_message(&mut self, id: ChatMessageId, by: HandledBy) -> Result<bool> {
        let at = self.now();
        let (reply, task) = match by {
            HandledBy::Reply(reply) => (Some(reply.0), None),
            HandledBy::Task(task) => (None, Some(task.0)),
        };
        Ok(self.conn.execute(
            "UPDATE chat_messages SET acked_at = coalesce(acked_at, ?1),
                 handled_reply_id = CASE WHEN handled_at IS NULL THEN ?3 ELSE handled_reply_id END,
                 handled_task_id = CASE WHEN handled_at IS NULL THEN ?4 ELSE handled_task_id END,
                 handled_at = coalesce(handled_at, ?1)
             WHERE id = ?2 AND author = 'user'",
            params![at, id.0, reply, task],
        )? > 0)
    }

    /// Whether the lead created or started `task` for the inbox item `item` (such as
    /// `chat-12`): it named the change with a request ID of that item.
    pub fn task_made_for(&self, task: TaskId, item: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM task_events WHERE task_id = ?1
                 AND (request_id = ?2 OR substr(request_id, 1, length(?2) + 1) = ?2 || '/'))",
            params![task.0, item],
            |row| row.get(0),
        )?)
    }

    /// People's messages the lead took but has not handled yet, oldest first, however far
    /// back in the history; messages from before receipts were kept are not among them.
    pub fn open_chat(&self) -> Result<Vec<ChatMessageId>> {
        let mut statement = self.conn.prepare(
            "SELECT id FROM chat_messages
             WHERE handled_at IS NULL AND due_at IS NOT NULL AND taken_at IS NOT NULL
             ORDER BY id",
        )?;
        let ids = statement
            .query_map([], |row| row.get(0).map(ChatMessageId))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Whether an unhandled message of a person fell due after `after` and by `up_to`
    /// (server time), so that the chat should now show it as overdue.
    pub fn chat_fell_due(&self, after: i64, up_to: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM chat_messages
                 WHERE handled_at IS NULL AND due_at > ?1 AND due_at <= ?2)",
            [after, up_to],
            |row| row.get(0),
        )?)
    }

    /// The newest message of a person or the lead, if there is any; the system's reports
    /// leave the conversation where it was.
    pub fn latest_chat_message(&self) -> Result<Option<ChatMessage>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM chat_messages WHERE author <> 'system'
                     ORDER BY id DESC LIMIT 1"
                ),
                [],
                message,
            )
            .optional()?
            .map(|row| self.with_author(row))
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
            .map(|row| self.with_author(row))
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
        let due_at = (author == ChatAuthor::User).then(|| chat::due_at(at));
        self.conn.execute(
            "INSERT INTO chat_messages (author, body, client_id, at, due_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![author.as_str(), body, client_id, at, due_at],
        )?;
        Ok(ChatMessage {
            id: ChatMessageId(self.conn.last_insert_rowid()),
            author,
            body: body.to_owned(),
            at,
            delivered_at: None,
            seen_at: None,
            due_at,
            acked_at: None,
            handled_at: None,
            handled_by: None,
            overdue: false,
            origin: None,
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

/// Records how `task`, titled `title`, finished at `at`, and posts the system's report of
/// it to the chat, on `conn`: called in the transaction of the change that finished the
/// task, so the finish and its one report are stored together or not at all.
pub(super) fn record_finish(
    conn: &Connection,
    task: TaskId,
    title: &str,
    finish: &Finish,
    at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO task_results (task_id, kind, pull_request_number, pull_request_url,
             commit_sha, at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            task.0,
            finish.kind().as_str(),
            finish.pull_request_number(),
            finish.pull_request_url(),
            finish.commit_sha(),
            at
        ],
    )?;
    let result = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO chat_messages (author, body, at, result_id) VALUES (?1, ?2, ?3, ?4)",
        params![
            ChatAuthor::System.as_str(),
            finish.report(task, title),
            at,
            result
        ],
    )?;
    // The done event recorded with this finish, if it made one, is reported by it.
    conn.execute(
        "UPDATE task_events SET report_id = ?1
         WHERE id = (SELECT max(id) FROM task_events
                     WHERE task_id = ?2 AND status = 'done' AND at = ?3)
           AND report_id IS NULL",
        params![conn.last_insert_rowid(), task.0, at],
    )?;
    Ok(())
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
    fn the_feed_reads_every_message_after_a_cursor_past_the_history() {
        let mut storage = Storage::open_in_memory().unwrap();
        let total = HISTORY + 100;
        for n in 0..total {
            let author = [ChatAuthor::User, ChatAuthor::Lead, ChatAuthor::System]
                [usize::try_from(n % 3).unwrap()];
            storage
                .add_chat_message(author, &n.to_string(), None)
                .unwrap();
        }
        let mut read = Vec::new();
        let mut cursor = 0;
        loop {
            let page = storage.chat_after(cursor, 120).unwrap();
            let Some(last) = page.last() else { break };
            cursor = last.id.0;
            read.extend(page);
        }
        let bodies: Vec<_> = read.iter().map(|message| message.body.clone()).collect();
        let expected: Vec<_> = (0..total).map(|n| n.to_string()).collect();
        assert_eq!(bodies, expected, "all of them, once, in order");
        assert_eq!(read[2].author, ChatAuthor::System);
        assert_eq!(storage.chat_after(0, 10_000).unwrap().len(), MAX_FEED);
        assert!(storage.chat_after(cursor, 10).unwrap().is_empty());
    }

    #[test]
    fn an_imported_message_is_stored_once_and_never_waits_for_the_lead() {
        let mut storage = Storage::open_in_memory().unwrap();
        storage.set_clock(|| 7);
        let origin = ChatOrigin::parse("matrix", "m1").unwrap();
        let (first, created) = storage
            .import_chat_message(ChatAuthor::User, "Plan it", &origin)
            .unwrap();
        assert!(created);
        storage.set_clock(|| 9);
        let (again, created) = storage
            .import_chat_message(ChatAuthor::User, "Plan it, edited", &origin)
            .unwrap();
        assert!(!created);
        assert_eq!(again, first);
        assert_eq!(first.origin, Some(origin));
        // The same ID from another origin is another message.
        let other = ChatOrigin::parse("irc", "m1").unwrap();
        let (reply, _) = storage
            .import_chat_message(ChatAuthor::Lead, "On it", &other)
            .unwrap();
        assert!(
            storage
                .import_chat_message(
                    ChatAuthor::System,
                    "x",
                    &ChatOrigin::parse("irc", "m2").unwrap()
                )
                .is_err()
        );

        let listed = storage.list_chat_messages().unwrap();
        assert_eq!(listed, [first.clone(), reply]);
        assert_eq!((first.due_at, first.overdue), (None, false));
        assert!(storage.pending_items().unwrap().is_empty());
        assert!(storage.undelivered_chat().unwrap().is_empty());
        assert!(storage.open_chat().unwrap().is_empty());
        assert!(!storage.chat_fell_due(0, i64::MAX).unwrap());
        assert!(storage.take_pending(None).unwrap().items.is_empty());
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
