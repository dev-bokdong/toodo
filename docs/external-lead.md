# External Lead

`toodo-server` can run without a lead of its own. Started with `TOODO_LEAD=external`, it
opens no herdr workspace or pane for a lead, yet keeps the Lead chat as with a built-in
lead: people's messages (typed or dictated), the inbox of task events, receipts, and the
server's finish reports. A process outside the server, the outside lead, reads all of
that and answers in the same chat through the API below. Any program can be the outside
lead: an agent, a bot that bridges the chat to another messenger, or a script.

The API is the MCP endpoint (`/mcp`, JSON-RPC over HTTP) with the lead token, plus the
server-sent event stream `GET /api/events` as a wake-up signal. Everything below is in
[`server.md`](server.md) too; this page is the contract for an outside lead.

## Setup

1. Start the server with `TOODO_LEAD=external` (in the systemd unit,
   `Environment=TOODO_LEAD=external`).
2. Give the outside lead the URL of the server and the contents of `lead-token` in the
   data directory. The token gives it the lead's rights on `/mcp` and opens
   `GET /api/events`; it opens nothing else under `/api`. Keep it like `mcp-token`.
3. Point its MCP client at `http://127.0.0.1:47616/mcp` (or the configured address) with
   `Authorization: Bearer <lead-token>`. The server is stateless: each `tools/call` is
   one `POST /mcp` with `Content-Type: application/json` and
   `Accept: application/json, text/event-stream`; no `initialize` is needed.

`get_lead` (for the web app) then reports `state: "external"` with:

- `connected`: true while the outside lead keeps an event stream open, or made a request
  with its token less than 90 seconds ago;
- `lastContactAt`: when it last made a request (seconds since the Unix epoch);
- `typing`: true while it reports that it composes a reply (see `set_typing`).

The chat shows the outside lead as "Connected" or "Disconnected", under the name and
picture set with `TOODO_LEAD_NAME` and `TOODO_LEAD_AVATAR` (see [`server.md`](server.md)).
A person's message stays "Waiting" until the lead takes it, then shows "Seen".

The chat accepts messages whether the outside lead is connected or not; they wait in the
inbox until it reads them.

## Waking up

`GET /api/events` with `Authorization: Bearer <lead-token>` streams the server's events
(`text/event-stream`). `chat-changed` means the chat or the inbox changed, `tasks-changed`
that tasks did. The stream carries no event IDs, and a reader that falls behind gets its
stream ended, so use it only as a hint to read again, never as the record. Reconnect when
it ends, and read again on a timer as well (for example every minute), so that nothing
depends on the stream.

## Reading the inbox without losing anything

The inbox holds people's messages (`chat-<id>`) and the status changes of tasks the lead
follows (`event-<id>`: a task reached `in_review` or `done`). An item stays pending until
it is taken. To survive a crash at any point:

1. `peek_pending {}` returns the oldest pending items (up to 50, with `remaining`) as
   `take_pending` would, without marking anything, and `open`: messages taken before and
   not handled yet.
2. Store the items where the outside lead keeps its own state, keyed by their IDs.
3. `take_pending { "ids": [...] }` marks those items taken. Taking IDs again returns them
   again with `takenBefore`, so repeating this step is harmless.
4. `ack_chat { "ids": [...] }` records that the lead received people's messages and is on
   them. Acknowledging again changes nothing.

A crash before step 3 leaves the items pending, and the next peek returns them again.
Never call `take_pending` without `ids` from an outside lead: it marks the items taken in
the same call, before they are stored.

## Answering

- `reply_in_chat { "text": "...", "handles": ["chat-12"] }` posts a reply as the lead and
  records it as what handled the listed messages.
- `mark_chat_handled { "id": "chat-12", "reply_id": 40 }` or `{ "id": ..., "task_id": 7 }`
  links a reply posted earlier, or a task made for the message (its `request_id` must
  start with the message's ID, such as `chat-12/create-1`).
- `set_typing { "typing": true }` shows the typing dots for 30 seconds; call it again to
  keep them, or with `false` to end them. A reply ends them too.

The task tools (`create_task`, `start_task`, `complete_task`, and the rest) work with the
lead token as described in [Lead tools](server.md#lead-tools): each change also gets a
note in the task's history and a lead message in the chat, and destructive actions wait
for a person's `confirm <code>` in the chat. A program that only bridges the chat may use
`mcp-token` for task tools instead, which posts nothing to the chat.

## Reading the whole chat

`list_chat { "after": 0, "limit": 100 }` returns every message with an ID above `after`,
oldest first: people's (`author: "user"`), the lead's (`"lead"`), and the server's finish
reports (`"system"`, with the task, pull request, and commit in the text). `more` is true
while messages remain; read on with the last ID as `after`. IDs only grow, so the last ID
read is a cursor that never skips or repeats a message, however long the chat is (the
web app lists only the latest 500). It marks nothing taken or read. Dictated messages are
people's messages like typed ones. The fields are those of `list_chat_messages`, plus
`origin` and `originId` for imported messages.

## Mirroring another chat

A bridge that shows the Toodo chat in another messenger, and that messenger's messages in
Toodo, imports each message written there:

```json
{ "author": "user", "text": "Plan the release", "origin": "matrix", "origin_id": "$abc" }
```

`import_chat_message` stores it with its `origin` (a label of the other chat: lowercase
letters, digits, `.`, `_`, `-`) and `origin_id` (the message's ID there). `author` is
`user` for a person's message and `lead` for the lead's. Importing the same origin and
ID again stores nothing and returns the stored message with `created: false`, so a
bridge can retry freely.

An imported message is one the lead already has in the other chat: a person's is stored
as taken, so it never enters the inbox, and without a due time, so it is never overdue.
It still counts as a person's message in every other way, for example as the answer
`confirm <code>` to a confirmation request. To keep a bridge from echoing, skip messages
with an `origin` when copying the Toodo chat out, since they came from outside.
