//! The lead's inbox (see [`crate::domain::inbox`]): people's chat messages and the task
//! status changes the lead follows, kept in `chat_messages` and `task_events` until the
//! lead takes them.

use rusqlite::{OptionalExtension, params};

use super::tasks::parse_task_status;
use super::{Result, Storage};
use crate::domain::chat::ChatMessageId;
use crate::domain::inbox::ItemId;
use crate::domain::model::TaskId;
use crate::domain::status::TaskStatus;

/// The most items one take without IDs returns: the oldest ones.
const TAKE_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxItem {
    pub id: ItemId,
    pub at: i64,
    /// When the lead took the item before this take: it gets an item again only when it
    /// asks for it by ID.
    pub taken_before: Option<i64>,
    pub content: InboxContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxContent {
    /// A person's message, in Markdown.
    Chat { body: String },
    /// A task reached `status`.
    Event {
        task: TaskId,
        title: String,
        status: TaskStatus,
        /// The system's report of the finish this event recorded, for a done event.
        report: Option<ChatMessageId>,
    },
}

/// What one take returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Taken {
    /// Oldest first.
    pub items: Vec<InboxItem>,
    /// Requested IDs that name no item of the inbox.
    pub missing: Vec<ItemId>,
    /// Items still pending after a take without IDs that hit its limit.
    pub remaining: usize,
}

const PENDING_CHAT: &str = "SELECT id FROM chat_messages
     WHERE author = 'user' AND taken_at IS NULL ORDER BY id";
const PENDING_EVENTS: &str = "SELECT id FROM task_events
     WHERE for_lead = 1 AND taken_at IS NULL ORDER BY id";

impl Storage {
    /// The IDs of the items the lead has not taken yet, chat messages first, each oldest
    /// first.
    pub fn pending_items(&self) -> Result<Vec<ItemId>> {
        let mut ids = Vec::new();
        for (sql, item) in [
            (
                PENDING_CHAT,
                (|id| ItemId::Chat(ChatMessageId(id))) as fn(i64) -> ItemId,
            ),
            (PENDING_EVENTS, ItemId::Event),
        ] {
            let mut statement = self.conn.prepare(sql)?;
            let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
            for id in rows {
                ids.push(item(id?));
            }
        }
        Ok(ids)
    }

    /// The pending chat messages whose ID was never typed into the lead's pane.
    pub fn undelivered_chat(&self) -> Result<Vec<ChatMessageId>> {
        let mut statement = self.conn.prepare(
            "SELECT id FROM chat_messages
             WHERE author = 'user' AND delivered_at IS NULL AND taken_at IS NULL
             ORDER BY id",
        )?;
        let ids = statement
            .query_map([], |row| row.get(0).map(ChatMessageId))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Marks people's messages as delivered now: their IDs reached the lead's pane.
    pub fn mark_chat_delivered(&mut self, ids: &[ChatMessageId]) -> Result<usize> {
        let at = self.now();
        let mut marked = 0;
        for id in ids {
            marked += self.conn.execute(
                "UPDATE chat_messages SET delivered_at = ?1
                 WHERE id = ?2 AND author = 'user' AND delivered_at IS NULL",
                [at, id.0],
            )?;
        }
        Ok(marked)
    }

    /// Hands items to the lead and marks them taken. Without `ids`, the oldest pending
    /// items, up to a limit; with them, those items, taken before or not. A chat message
    /// taken counts as delivered and seen.
    pub fn take_pending(&mut self, ids: Option<&[ItemId]>) -> Result<Taken> {
        let at = self.now();
        let tx = self.conn.transaction()?;
        let (wanted, remaining) = match ids {
            Some(ids) => (ids.to_vec(), 0),
            None => {
                let mut pending = Storage::pending_in(&tx)?;
                pending.sort_by_key(|(at, id)| (*at, *id));
                let remaining = pending.len().saturating_sub(TAKE_LIMIT);
                pending.truncate(TAKE_LIMIT);
                (pending.into_iter().map(|(_, id)| id).collect(), remaining)
            }
        };
        let mut taken = Taken {
            items: Vec::new(),
            missing: Vec::new(),
            remaining,
        };
        for id in wanted {
            let Some(item) = item(&tx, id)? else {
                taken.missing.push(id);
                continue;
            };
            match id {
                ItemId::Chat(chat) => tx.execute(
                    "UPDATE chat_messages SET taken_at = ?1,
                         delivered_at = coalesce(delivered_at, ?1),
                         seen_at = coalesce(seen_at, ?1)
                     WHERE id = ?2 AND taken_at IS NULL",
                    [at, chat.0],
                )?,
                ItemId::Event(event) => tx.execute(
                    "UPDATE task_events SET taken_at = ?1 WHERE id = ?2 AND taken_at IS NULL",
                    [at, event],
                )?,
            };
            taken.items.push(item);
        }
        tx.commit()?;
        taken.items.sort_by_key(|item| (item.at, item.id));
        Ok(taken)
    }

    fn pending_in(conn: &rusqlite::Connection) -> Result<Vec<(i64, ItemId)>> {
        let mut pending = Vec::new();
        let mut chat = conn.prepare(
            "SELECT at, id FROM chat_messages WHERE author = 'user' AND taken_at IS NULL",
        )?;
        for row in chat.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))? {
            let (at, id) = row?;
            pending.push((at, ItemId::Chat(ChatMessageId(id))));
        }
        let mut events =
            conn.prepare("SELECT at, id FROM task_events WHERE for_lead = 1 AND taken_at IS NULL")?;
        for row in events.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))? {
            let (at, id) = row?;
            pending.push((at, ItemId::Event(id)));
        }
        Ok(pending)
    }

    /// The task and status whose change the lead named with `request_id`, if any.
    pub fn task_of_request(&self, request_id: &str) -> Result<Option<(TaskId, TaskStatus)>> {
        let found = self
            .conn
            .query_row(
                "SELECT task_id, status FROM task_events WHERE request_id = ?1",
                [request_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        found
            .map(|(task, status)| Ok((TaskId(task), parse_task_status(&status)?)))
            .transpose()
    }

    /// Records a status change of `task` without the rules of its transitions.
    #[cfg(test)]
    pub(crate) fn record_event_for_tests(&mut self, task: TaskId, status: TaskStatus, at: i64) {
        super::tasks::record_event(&self.conn, task, status, at).unwrap();
    }

    /// Names the latest change of `task` to `status` with the lead's `request_id`.
    pub fn name_request(
        &mut self,
        task: TaskId,
        status: TaskStatus,
        request_id: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE task_events SET request_id = ?1
             WHERE id = (SELECT max(id) FROM task_events WHERE task_id = ?2 AND status = ?3)",
            params![request_id, task.0, status.as_str()],
        )?;
        Ok(())
    }
}

fn item(conn: &rusqlite::Connection, id: ItemId) -> Result<Option<InboxItem>> {
    match id {
        ItemId::Chat(chat) => Ok(conn
            .query_row(
                "SELECT at, taken_at, body FROM chat_messages WHERE id = ?1 AND author = 'user'",
                [chat.0],
                |row| {
                    Ok(InboxItem {
                        id,
                        at: row.get(0)?,
                        taken_before: row.get(1)?,
                        content: InboxContent::Chat { body: row.get(2)? },
                    })
                },
            )
            .optional()?),
        ItemId::Event(event) => {
            let row = conn
                .query_row(
                    "SELECT e.at, e.taken_at, e.task_id, t.title, e.status, e.report_id
                     FROM task_events e JOIN tasks t ON t.id = e.task_id
                     WHERE e.id = ?1 AND e.for_lead = 1",
                    [event],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<i64>>(5)?,
                        ))
                    },
                )
                .optional()?;
            row.map(|(at, taken_before, task, title, status, report)| {
                Ok(InboxItem {
                    id,
                    at,
                    taken_before,
                    content: InboxContent::Event {
                        task: TaskId(task),
                        title,
                        status: parse_task_status(&status)?,
                        report: report.map(ChatMessageId),
                    },
                })
            })
            .transpose()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::chat::ChatAuthor;
    use crate::domain::model::{TaskType, WorkspaceLocation};
    use crate::domain::path::WindowsPath;
    use crate::storage::{NewTask, NewWorkspace};

    fn task(storage: &mut Storage) -> TaskId {
        let workspace = storage
            .create_workspace(&NewWorkspace {
                name: "toodo".to_owned(),
                location: WorkspaceLocation::Local {
                    path: WindowsPath::parse(r"C:\dev\toodo").unwrap().into(),
                },
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        storage
            .create_task(&NewTask {
                workspace_id: workspace.id,
                task_type: TaskType::Docs,
                title: "Write the guide".to_owned(),
                description: String::new(),
                estimate: None,
                due_date: None,
                criteria: Vec::new(),
            })
            .unwrap()
            .id
    }

    #[test]
    fn messages_and_followed_events_wait_until_the_lead_takes_them() {
        let mut storage = Storage::open_in_memory().unwrap();
        storage.set_clock(|| 10);
        let task = task(&mut storage);
        let message = storage
            .add_chat_message(ChatAuthor::User, "Plan it", Some("c1"))
            .unwrap();
        storage
            .add_chat_message(ChatAuthor::Lead, "On it", None)
            .unwrap();
        storage.set_clock(|| 20);
        storage.record_event_for_tests(task, TaskStatus::InProgress, 20);
        storage.record_event_for_tests(task, TaskStatus::InReview, 21);
        let review: i64 = storage
            .conn
            .query_row("SELECT max(id) FROM task_events", [], |row| row.get(0))
            .unwrap();

        // Nothing was typed into the pane yet: the message is stored all the same.
        assert_eq!(message.delivered_at, None);
        assert_eq!(storage.undelivered_chat().unwrap(), [message.id]);
        let pending = vec![ItemId::Chat(message.id), ItemId::Event(review)];
        assert_eq!(storage.pending_items().unwrap(), pending);

        storage.set_clock(|| 30);
        let taken = storage.take_pending(None).unwrap();
        assert_eq!(
            taken.items,
            [
                InboxItem {
                    id: ItemId::Chat(message.id),
                    at: 10,
                    taken_before: None,
                    content: InboxContent::Chat {
                        body: "Plan it".to_owned()
                    },
                },
                InboxItem {
                    id: ItemId::Event(review),
                    at: 21,
                    taken_before: None,
                    content: InboxContent::Event {
                        task,
                        title: "Write the guide".to_owned(),
                        status: TaskStatus::InReview,
                        report: None,
                    },
                },
            ]
        );
        assert!(storage.pending_items().unwrap().is_empty());
        assert!(storage.undelivered_chat().unwrap().is_empty());
        let stored = storage.list_chat_messages().unwrap();
        assert_eq!(
            (stored[0].delivered_at, stored[0].seen_at),
            (Some(30), Some(30))
        );
        assert!(storage.take_pending(None).unwrap().items.is_empty());

        // Asked for by ID, a taken item comes again, marked as taken before.
        storage.set_clock(|| 40);
        let again = storage
            .take_pending(Some(&[
                ItemId::Chat(message.id),
                ItemId::Event(1),
                ItemId::Event(999),
            ]))
            .unwrap();
        assert_eq!(again.items.len(), 1);
        assert_eq!(again.items[0].taken_before, Some(30));
        // The planned event of the new task is not one the lead follows.
        assert_eq!(again.missing, [ItemId::Event(1), ItemId::Event(999)]);
    }

    #[test]
    fn a_done_event_names_the_systems_report_of_its_finish() {
        let mut storage = Storage::open_in_memory().unwrap();
        let task = task(&mut storage);
        for status in [
            TaskStatus::InProgress,
            TaskStatus::InReview,
            TaskStatus::Done,
        ] {
            storage.set_task_status(task, status).unwrap();
        }
        let report = storage.list_chat_messages().unwrap().pop().unwrap();
        assert_eq!(report.author, ChatAuthor::System);

        let taken = storage.take_pending(None).unwrap();
        let reports: Vec<_> = taken
            .items
            .iter()
            .map(|item| match &item.content {
                InboxContent::Event { status, report, .. } => (*status, *report),
                InboxContent::Chat { .. } => panic!("no chat"),
            })
            .collect();
        assert_eq!(
            reports,
            [
                (TaskStatus::InReview, None),
                (TaskStatus::Done, Some(report.id))
            ]
        );
    }

    #[test]
    fn a_take_without_ids_returns_the_oldest_items_up_to_the_limit() {
        let mut storage = Storage::open_in_memory().unwrap();
        for n in 0..TAKE_LIMIT + 3 {
            storage
                .add_chat_message(ChatAuthor::User, &n.to_string(), None)
                .unwrap();
        }
        let first = storage.take_pending(None).unwrap();
        assert_eq!(first.items.len(), TAKE_LIMIT);
        assert_eq!(first.remaining, 3);
        let rest = storage.take_pending(None).unwrap();
        assert_eq!((rest.items.len(), rest.remaining), (3, 0));
        assert_eq!(
            rest.items[0].content,
            InboxContent::Chat {
                body: TAKE_LIMIT.to_string()
            }
        );
    }

    #[test]
    fn delivering_marks_a_message_once() {
        let mut storage = Storage::open_in_memory().unwrap();
        storage.set_clock(|| 5);
        let message = storage
            .add_chat_message(ChatAuthor::User, "Hi", None)
            .unwrap();
        assert_eq!(storage.mark_chat_delivered(&[message.id]).unwrap(), 1);
        assert_eq!(storage.mark_chat_delivered(&[message.id]).unwrap(), 0);
        assert!(storage.undelivered_chat().unwrap().is_empty());
        assert_eq!(storage.pending_items().unwrap(), [ItemId::Chat(message.id)]);
    }

    #[test]
    fn a_request_id_names_the_change_it_made() {
        let mut storage = Storage::open_in_memory().unwrap();
        let task = task(&mut storage);
        assert_eq!(storage.task_of_request("chat-1/create").unwrap(), None);
        storage
            .name_request(task, TaskStatus::Planned, "chat-1/create")
            .unwrap();
        assert_eq!(
            storage.task_of_request("chat-1/create").unwrap(),
            Some((task, TaskStatus::Planned))
        );
    }
}
