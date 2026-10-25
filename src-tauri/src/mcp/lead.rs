//! What sets the lead apart from other MCP callers. `toodo-server` gives the lead a token
//! of its own, so the tools know when the lead acts: what it does to a task is kept as a
//! note in the task's history and posted to the chat panel (a finish is reported there by
//! the server itself, see [`crate::domain::finish`]), and its destructive actions
//! wait until a person confirms them in the chat. The lead cannot post a person's message
//! (it has no web or device token), so it cannot confirm for them.

use axum::http::request::Parts;

use crate::commands::lead::reply;
use crate::commands::types::{Task, TaskId};
use crate::commands::{CommandError, CommandResult};
use crate::domain::chat::{self, ChatAuthor, ChatMessage};
use crate::domain::model::{self, NoteKind};
use crate::storage::Storage;

/// Who called a tool, from the token of the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Caller {
    /// A coding agent, or anyone else with the MCP token.
    Agent,
    /// The lead of `toodo-server`, with the lead token.
    Lead,
}

impl Caller {
    pub(crate) fn of(parts: &Parts) -> Self {
        parts
            .extensions
            .get::<Self>()
            .copied()
            .unwrap_or(Self::Agent)
    }
}

/// A change the lead made to a task: the note kept in the task's history and the message
/// posted to the chat, if the server's own report of the change does not already say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Activity {
    pub(crate) note: String,
    pub(crate) chat: Option<String>,
}

fn named(task: &Task) -> String {
    format!("#{} {}", task.id.0, task.title)
}

fn ids(ids: &[TaskId]) -> String {
    ids.iter()
        .map(|id| format!("#{}", id.0))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn created(task: &Task, related: &[TaskId]) -> Activity {
    let mut links = Vec::new();
    if !task.depends_on.is_empty() {
        links.push(format!("depends on {}", ids(&task.depends_on)));
    }
    if !related.is_empty() {
        links.push(format!("related to {}", ids(related)));
    }
    let links = if links.is_empty() {
        String::new()
    } else {
        format!("; it {}", links.join(" and is "))
    };
    Activity {
        note: format!("Created by the Toodo lead{links}."),
        chat: Some(format!("Created {}{links}.", named(task))),
    }
}

/// `ran`: the pane and the command the lead ran in it.
pub(crate) fn started(task: &Task, ran: Option<(&str, &str)>) -> Activity {
    let ran = ran.map_or_else(String::new, |(pane, command)| {
        format!(" and ran `{command}` in its pane {pane}")
    });
    Activity {
        note: format!("Started by the Toodo lead{ran}."),
        chat: Some(format!("Started {}{ran}.", named(task))),
    }
}

/// `confirmation`: the code a person confirmed the completion with, if it needed one. The
/// completion is reported to the chat by the server, so only the note is kept.
pub(crate) fn completed(task: &Task, confirmation: Option<&str>) -> Activity {
    let shipped = task
        .merged_with
        .as_ref()
        .map_or_else(String::new, |merged| {
            format!(" as shipped with {}", merged.pull_request_url)
        });
    let confirmed = confirmed_by(confirmation);
    Activity {
        note: format!("Completed by the Toodo lead{shipped}{confirmed}."),
        chat: None,
    }
}

/// Like [`completed`], the server reports the conclusion to the chat.
pub(crate) fn concluded(confirmation: &str) -> Activity {
    let confirmed = confirmed_by(Some(confirmation));
    Activity {
        note: format!("Concluded without a merge by the Toodo lead{confirmed}."),
        chat: None,
    }
}

fn confirmed_by(confirmation: Option<&str>) -> String {
    confirmation.map_or_else(String::new, |code| {
        format!(", confirmed in the chat (`confirm {code}`)")
    })
}

/// Keeps `activity` in the history of `task` and posts its message, if it has one, to the
/// chat. Returns the task with the note.
pub(crate) fn record(
    storage: &mut Storage,
    task: model::TaskId,
    activity: &Activity,
) -> CommandResult<Task> {
    let noted = storage.add_task_note(task, NoteKind::Progress, &activity.note)?;
    if let Some(chat) = &activity.chat {
        reply(storage, chat, &[])?;
    }
    Ok((&noted).into())
}

const CODE_LENGTH: usize = 6;

/// What a destructive action of the lead waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Guard {
    /// A person confirmed it with this code: go ahead.
    Confirmed(String),
    /// Not confirmed; the error tells the lead what to do. `asked` is true when the
    /// request for a confirmation was just posted to the chat.
    Refused { error: CommandError, asked: bool },
}

fn request(action: &str, code: &str) -> String {
    format!(
        "**Confirmation needed:** the lead wants to {action}\n\n\
         Reply `confirm {code}` to let it go ahead; otherwise it does nothing."
    )
}

fn is_confirmation(body: &str, code: &str) -> bool {
    body.replace('`', "")
        .trim()
        .trim_end_matches(['.', '!'])
        .eq_ignore_ascii_case(&format!("confirm {code}"))
}

/// Lets a destructive action of the lead (`tool`, described by `action`, a Markdown
/// sentence ending in a full stop) go ahead once a person confirmed it in the chat.
/// Without a `confirmation` code it posts a request with a new code to the chat. With
/// one it looks for that request, for the same action, and a later message of a person
/// answering `confirm <code>`; the action's arguments are part of the request, so a code
/// confirms only the action it was asked for.
pub(crate) fn guard(
    storage: &mut Storage,
    tool: &str,
    action: &str,
    confirmation: Option<&str>,
) -> CommandResult<Guard> {
    let Some(code) = confirmation.map(str::trim) else {
        let code = new_code()?;
        reply(storage, &request(action, &code), &[])?;
        return Ok(Guard::Refused {
            error: CommandError::rejected(format!(
                "a person must confirm this first: Toodo posted a request for it to the chat \
                 with the code {code}. Tell them why in the chat, and once a person answers \
                 `confirm {code}` there, call {tool} again with the same arguments and \
                 confirmation \"{code}\"; do nothing if they decline"
            )),
            asked: true,
        });
    };
    if code.len() != CODE_LENGTH || !code.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CommandError::invalid_input(format!(
            "confirmation must be the {CODE_LENGTH}-character code of a confirmation \
             request in the chat, not {code:?}"
        )));
    }
    let code = code.to_ascii_lowercase();
    let messages = storage.list_chat_messages()?;
    let error = match answered(&messages, &request(action, &code), &code) {
        Some(true) => return Ok(Guard::Confirmed(code)),
        Some(false) => CommandError::rejected(format!(
            "no person has answered `confirm {code}` in the chat yet; wait for their answer \
             and do nothing if they decline"
        )),
        None => CommandError::rejected(format!(
            "the chat has no request {code} for this action with these arguments; call \
             {tool} without confirmation to ask for one"
        )),
    };
    Ok(Guard::Refused {
        error,
        asked: false,
    })
}

/// Whether a person answered the request `body` with `code`: `None` when the chat has no
/// such request of the lead.
fn answered(messages: &[ChatMessage], body: &str, code: &str) -> Option<bool> {
    let body = chat::body(body).ok()?;
    let asked = messages
        .iter()
        .rposition(|message| message.author == ChatAuthor::Lead && message.body == body)?;
    Some(
        messages[asked + 1..].iter().any(|message| {
            message.author == ChatAuthor::User && is_confirmation(&message.body, code)
        }),
    )
}

fn new_code() -> CommandResult<String> {
    let mut bytes = [0u8; CODE_LENGTH / 2];
    getrandom::fill(&mut bytes).map_err(|error| CommandError::internal(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_code(storage: &mut Storage, action: &str) -> String {
        let Guard::Refused { error, asked } =
            guard(storage, "conclude_task", action, None).unwrap()
        else {
            panic!("asked without a confirmation")
        };
        assert!(asked);
        let messages = storage.list_chat_messages().unwrap();
        let posted = &messages.last().unwrap().body;
        let code = posted
            .split("`confirm ")
            .nth(1)
            .unwrap()
            .split('`')
            .next()
            .unwrap()
            .to_owned();
        assert_eq!(code.len(), CODE_LENGTH);
        assert!(error.message.contains(&code), "{error:?}");
        code
    }

    fn says(storage: &mut Storage, author: ChatAuthor, text: &str) {
        storage.add_chat_message(author, text, None).unwrap();
    }

    fn outcome(storage: &mut Storage, action: &str, code: &str) -> Result<String, String> {
        match guard(storage, "conclude_task", action, Some(code)).unwrap() {
            Guard::Confirmed(code) => Ok(code),
            Guard::Refused { error, asked } => {
                assert!(!asked);
                Err(error.message)
            }
        }
    }

    #[test]
    fn only_a_persons_later_answer_with_the_code_confirms_the_same_action() {
        let mut storage = Storage::open_in_memory().unwrap();
        let action = "conclude #3 Spike without a merge.";
        says(&mut storage, ChatAuthor::User, "confirm aaaaaa");
        let code = request_code(&mut storage, action);

        let waiting = outcome(&mut storage, action, &code).unwrap_err();
        assert!(waiting.contains("yet"), "{waiting}");
        // The lead posting the answer itself confirms nothing.
        says(&mut storage, ChatAuthor::Lead, &format!("confirm {code}"));
        assert!(outcome(&mut storage, action, &code).is_err());
        // Nor does a report of the server.
        says(&mut storage, ChatAuthor::System, &format!("confirm {code}"));
        assert!(outcome(&mut storage, action, &code).is_err());
        says(&mut storage, ChatAuthor::User, "something else");
        assert!(outcome(&mut storage, action, &code).is_err());

        says(
            &mut storage,
            ChatAuthor::User,
            &format!("`Confirm {code}`."),
        );
        assert_eq!(outcome(&mut storage, action, &code), Ok(code.clone()));
        let upper = code.to_ascii_uppercase();
        assert_eq!(outcome(&mut storage, action, &upper), Ok(code.clone()));

        let other = outcome(&mut storage, "conclude #4 Other without a merge.", &code);
        assert!(other.unwrap_err().contains("no request"));
    }

    #[test]
    fn a_persons_answer_imported_from_another_chat_confirms() {
        let mut storage = Storage::open_in_memory().unwrap();
        let action = "conclude #3 Spike without a merge.";
        let code = request_code(&mut storage, action);
        let lead = chat::ChatOrigin::parse("matrix", "m1").unwrap();
        storage
            .import_chat_message(ChatAuthor::Lead, &format!("confirm {code}"), &lead)
            .unwrap();
        assert!(
            outcome(&mut storage, action, &code).is_err(),
            "not the lead's"
        );

        let person = chat::ChatOrigin::parse("matrix", "m2").unwrap();
        storage
            .import_chat_message(ChatAuthor::User, &format!("confirm {code}"), &person)
            .unwrap();
        assert_eq!(outcome(&mut storage, action, &code), Ok(code));
    }

    #[test]
    fn a_malformed_code_is_invalid_input_and_posts_nothing() {
        let mut storage = Storage::open_in_memory().unwrap();
        for code in ["", "abc", "zzzzzz", "abcdef0"] {
            let error = guard(&mut storage, "conclude_task", "x.", Some(code)).unwrap_err();
            assert_eq!(
                error.kind,
                crate::commands::CommandErrorKind::InvalidInput,
                "{code:?}"
            );
        }
        assert!(storage.list_chat_messages().unwrap().is_empty());
    }
}
