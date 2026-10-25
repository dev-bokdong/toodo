# Linux Server

`toodo-server` runs Toodo's core headless on a Linux machine without a desktop, next to the
herdr server there, for one user. It is the same Rust core as the desktop app, built with
the `server` feature instead of the Tauri shell, and is used locally on that machine:

- the MCP server for coding agents on the machine (`http://127.0.0.1:47616/mcp`),
- the background work of the desktop app: herdr event subscriptions, pull request checks,
  automatic backups, and MCP reverse tunnels to SSH machines,
- SQLite storage in the server's data directory.

It also serves the commands of the desktop app as an HTTP API with a server-sent event
stream (see [API](#api)), and the web build of the frontend, an installable web app (PWA)
that uses them from a browser (see [Web App](#web-app)).

Everything is served on one port, `127.0.0.1:47616` by default. The default is not the
desktop app's MCP port, 47615, on purpose: the desktop app keeps
`ssh -R 127.0.0.1:47615:127.0.0.1:47615` open to every SSH machine with a workspace, and the
server forwards its own port to its SSH machines the same way. If the server's machine is
also an SSH machine of the desktop app, or both reach the same SSH machine, the two would
fight over the same port: the loser either fails to listen or keeps retrying its tunnel,
and agents would get whichever token won. Keep `TOODO_LISTEN` off 47615 for that reason.

## Machines and Paths

The "Local" machine is the server: local workspaces and the local worktree root use POSIX
paths on it, and the local herdr is the herdr server running there. Workspaces on saved
herdr SSH machines work as in the desktop app. `herdr machine add <host>` asks questions in
a terminal, so run it on the server yourself.

The server is not a backend for the desktop app: each keeps its own database.

## Versions and Releases

`toodo-server` and the Windows app share one version and one release: every GitHub
release `vX.Y.Z` carries the Windows installers and
`toodo-server-vX.Y.Z-linux-x86_64.tar.gz` (the `toodo-server` binary for x86_64 Linux and,
under `web/`, the web app) with its `.sha256`, built from the same commit. `toodo-server --version` prints the
version, so an installed server compares directly with a release and with the Windows
app's version, and the commit it was built from (`toodo-server 0.12.0 (commit
1c78b7a1a2b3)`), so a build of a later commit is told apart from the release it reports.
The server prints the same line when it starts. See "Releases and Updates" in the README for how a release is made.

## Install and Update

On a server, install and update from releases. As the user who runs herdr (git, gh
logged in, ssh, curl, and a running herdr installed; no Rust toolchain needed):

```sh
scripts/install-server.sh --release latest
```

The script needs no checkout in this mode; fetch it alone with
`gh api repos/dev-bokdong/toodo/contents/scripts/install-server.sh --jq .content | base64 -d`
if there is none. It downloads the server asset of the release with `gh`, checks its
sha256 and that the binary reports the release's version, installs it to
`~/.local/bin/toodo-server` and the web app to `~/.local/share/toodo-server-web`, writes the
systemd user service `toodo-server.service` (see [systemd](#systemd)), restarts it, and
checks that it answers (`/api` without a token is `401`, `/` is the web app). It prints
where the tokens are, never the tokens.

- `--release latest|vX.Y.Z`: the release to install; `vX.Y.Z` pins a version, for example
  the Windows app's.
- `--listen ADDR`: listen address of the service (default `127.0.0.1:47616`; 47615 is
  refused, see above).
- `--no-service`: install only; leave the service alone.

To update, run it again with the same options; the database and tokens in the data
directory are kept.

Releases before the server was published (0.9.3 and older) have no server asset; build
those from a checkout. Releases before the web app was published carry the binary only;
the script then serves the API and MCP only.

### From a Checkout

For development, run the script without `--release` from a checkout of this repository
(Rust needed as well). It builds `toodo-server` from the checkout and installs it the same
way:

```sh
git pull
scripts/install-server.sh
```

- `--with-web`: also build the web app (needs Node.js and npm), install it to
  `~/.local/share/toodo-server-web`, and serve it (see [Web App](#web-app)). A release
  carries its web app already, so this option is for checkout builds only.

## Build

To build by hand instead, on the server (Rust installed):

```sh
cd src-tauri
cargo build --release --no-default-features --features server --bin toodo-server
```

The binary is `src-tauri/target/release/toodo-server`. It needs `git`, `ssh`, `gh`, and
`herdr` on `PATH` at runtime, like the desktop app.

## Run

```sh
toodo-server
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `TOODO_LISTEN` | `127.0.0.1:47616` | Address to listen on; avoid port 47615 (see above) |
| `TOODO_DATA_DIR` | `$XDG_DATA_HOME/toodo-server` or `~/.local/share/toodo-server` | Database, backups, and access tokens |
| `TOODO_WEB_DIR` | none | Web build to serve; without it only the API and MCP are served |
| `TOODO_PUSH_SUBJECT` | `https://github.com/dev-bokdong/toodo` | Contact sent to push services with every notification, a `mailto:` or `https:` URL (see [Notifications](#notifications)) |
| `TOODO_LEAD` | `on` | `off` runs no lead; `external` runs none and keeps the chat for a lead outside the server (see [Lead](#lead) and [`external-lead.md`](external-lead.md)) |
| `TOODO_LEAD_COMMAND` | omo (see below) | Shell command that starts the lead agent |
| `TOODO_LEAD_MODEL` | none | Model for the lead, passed to the command as `$TOODO_LEAD_MODEL` |
| `TOODO_LEAD_DIR` | `$TOODO_DATA_DIR/lead` | The lead's workspace |

On first start the data directory gets three access tokens:

- `mcp-token`: agents send it to `/mcp` as `Authorization: Bearer <token>`.
- `lead-token`: the lead's token for `/mcp`, which tells the tools that the lead calls
  them (see [Lead tools](#lead-tools)). Only the lead's launcher carries it, or, with
  `TOODO_LEAD=external`, the lead outside the server. It also opens `GET /api/events`,
  and nothing else under `/api`.
- `web-token`: the shared web token. A browser signs in with it once and is registered
  as a device with a token of its own (see [Devices](#devices)); `/api` accepts the
  shared token too, so existing clients keep working.

The data directory is created readable by its owner only (`0700`) and the token files
`0600`; the server tightens an existing directory and token files on start. To replace a
token, delete its file and restart the server. Replacing `web-token` does not sign out
registered devices; revoke them one by one.

### Devices

Each device has its own access token, so a lost phone is revoked alone instead of
rotating the token of every client. The database keeps a device's name, when it was
registered and last seen (to within a minute), and when it was revoked, but only a
SHA-256 hash of its token; the token itself is shown to the device once, when it
registers. Revoking is final: the token never works again, and the device signs in anew
with the shared token. Only the shared token registers devices, so a device token cannot
mint more tokens. A device keeps its id after it is revoked, so data kept per device can
refer to it.

The web app lists the devices under **Devices** in Settings, marks the current one,
and revokes them. Devices live in the database, so restoring a backup also restores the
devices of that backup: devices registered after it sign in again.

**Treat `web-token` like a password to the server account.** It opens every command of
the app, including ones that write `~/.ssh/config` (which can run commands through
`ProxyCommand`), list the server's directories, and download or replace the whole
database. Whoever holds it can run code as that user. `mcp-token` gives the MCP
tools only, but still lets an agent start, complete, and conclude tasks.

The server speaks plain HTTP. Keep the default loopback address; never expose the port.

### Notifications

A device can get notifications while Toodo is closed, through the Web Push service of
its browser: Chrome and Edge on Android and desktop, Firefox, and Safari on iOS and
iPadOS 16.4 or later once Toodo is added to the Home Screen (Safari has no Web Push in
a plain tab). Web Push needs HTTPS, so phones reach the server through an
authenticating HTTPS front (see [Off the Machine](#off-the-machine-cloudflare-tunnel-and-access));
on the server itself, `http://127.0.0.1` counts as secure.

Under **Notifications** in Settings, a device turns notifications on, picks which
ones it gets, and sends itself a test notification:

| Setting | Notifications |
| --- | --- |
| Agent finished or needs input | An agent of a task stopped working, or started waiting for input |
| Pull request checks and reviews | CI of a task's pull request passed or failed; a reviewer approved it or requested changes |
| Task awaiting completion | A task moved to awaiting completion |

Notifications report what changed on its own: agents, pull requests, and the server's
background checks. A change made in the app itself sends none. Opening a notification
opens its task. While notifications are on, the app shows no
notifications of its own on that device, so nothing arrives twice.

Subscriptions belong to devices: a browser signed in with the shared `web-token` signs
in again to get a device token first. Revoking a device ends its subscription, and a
subscription the push service reports gone is dropped.

The server signs its requests to push services with a P-256 key (VAPID) that it creates
in `push-key` in the data directory on first start (`0600`, like the tokens). Browsers
bind their subscriptions to that key: replacing or losing the file ends every
subscription, and each device turns notifications on again. Messages are encrypted for
the device (RFC 8291); the push service sees only where to deliver them and when.
Push services keep a message for an offline device for a day.

### Agents

An agent on the server uses the MCP server with this configuration (the token is the
contents of `mcp-token`):

```json
{
  "mcpServers": {
    "toodo": {
      "type": "http",
      "url": "http://127.0.0.1:47616/mcp",
      "headers": { "Authorization": "Bearer <mcp-token>" }
    }
  }
}
```

Agents on the server's SSH machines reach the same URL through the reverse tunnels the
server keeps open to them.

### Lead

The server keeps one lead agent running in herdr. People talk to it in its pane (or
through the pane view), and it turns their requests into Toodo tasks, starts them, runs
coding agents in their worktrees, and follows them up, with Toodo as its only record of
the work. Its system prompt is `src-tauri/src/server/lead-prompt.md`.

- **Workspace.** On first start the server creates `TOODO_LEAD_DIR` as a new git
  repository without a remote and adds it as the workspace "Lead". The lead keeps
  briefs, research, and scratch files there and never pushes it. Deleting the workspace
  in Toodo only makes the server add it again on its next check; the files stay.
- **Pane.** The server opens the workspace in herdr and runs the agent in its pane. Every
  5 seconds it checks the pane: when the pane's shell is back in the foreground, the
  agent has exited and is started again in the same pane. An agent that exits within a
  minute of starting is retried after 10 s, 30 s, 1 min, 2 min, then every 5 min, and
  after three such exits in a row the lead is reported `failed`. A closed pane or herdr
  workspace is opened again. After a server restart an agent still running in the pane
  is kept, not started a second time. When the server stops (Ctrl+C or SIGTERM) it
  stops checking, waiting up to 10 s for a check in progress, then closes the lead's
  herdr workspace, ending the agent; the next start opens it again.
- **Command.** The server writes `lead-start` (owner-only, `0600`) and `lead-prompt.md`
  to the data directory and types `sh <data dir>/lead-start` into the pane. The launcher
  exports `TOODO_MCP_URL`, `TOODO_MCP_TOKEN` (the contents of `lead-token`),
  `TOODO_LEAD_MODEL`, `TOODO_LEAD_PROMPT`
  (the prompt file), and `TOODO_LEAD_DIR`, changes to the workspace, and runs
  `TOODO_LEAD_COMMAND` with `sh`. The default runs omo:

  ```sh
  omo --approve ${TOODO_LEAD_MODEL:+--model "$TOODO_LEAD_MODEL"} \
    --append-system-prompt "$TOODO_LEAD_PROMPT"
  ```

  Another agent works the same way, for example
  `claude ${TOODO_LEAD_MODEL:+--model "$TOODO_LEAD_MODEL"} --append-system-prompt "$(cat "$TOODO_LEAD_PROMPT")"`.
- **MCP.** The server writes this server's entry, named `toodo`, into the workspace's
  `.omo/mcp.json` (omo, `bearerTokenEnv`) and `.mcp.json` (Claude Code, an
  `Authorization` header with `${TOODO_MCP_TOKEN}`), keeping other servers in those
  files. A file that is not plain JSON (comments included) is left alone and the lead is
  reported `failed` until it is fixed or removed. Neither file holds the token, which only the launcher's environment carries, so
  the lead reaches its own server even when another Toodo server uses a different token.
  A `toodo` server in the agent's global configuration takes precedence over the project
  one in omo; remove it, or point it at this server, on the machine the lead runs on.

`get_lead` returns the lead's status for the chat panel: `{ state, workspaceId,
herdrWorkspaceId, paneId, agent, agentStatus, typing, restarts, startedAt, error,
connected, lastContactAt }`, where `state` is `disabled`, `starting`, `running`,
`restarting`, `failed`, or `external`. The `lead-changed` event carries the same object
whenever it changes. The desktop app has no lead, and always reports `disabled`.

With `TOODO_LEAD=external` the server runs no lead, opens no herdr workspace for one,
and reports `external`: the chat, the inbox, the receipts, and the finish reports work as
below, and a lead outside the server reads and answers them with the lead token.
`connected` and `lastContactAt` tell whether it is in touch. See
[`external-lead.md`](external-lead.md) for that API.

The web app talks to the lead in a floating chat: a round Dori button at the bottom right of
every view opens a chat panel over it, or a full-screen sheet on a phone. While the chat is
closed, the button shows the count of unread replies, and dots while the lead is typing.
The panel keeps an unsent message while closed or while moving between views. `send_chat_message { text, clientId }`
stores the message in the lead's inbox first, then, while the lead runs, types only its
ID into the lead's pane (`[Toodo inbox] chat-12 - take them with the Toodo MCP tool
take_pending`) and marks it delivered. It is accepted while the lead starts, restarts,
or fails too, and by a server with an outside lead (which reads it itself), and rejected
only with `TOODO_LEAD=off`; a `clientId` already stored returns that
message without storing or typing it again. The lead reads the message with the MCP tool
`take_pending` and answers with `reply_in_chat`, as its prompt tells it to. Both store the
message in the database and send the `chat-changed` event, so every open client reloads
`list_chat_messages` (the latest 500 messages, and any older message of a person the lead
has not handled yet; see Receipts below). Each client keeps the ID of the last
message it showed in the open chat in local storage and counts the lead's later replies,
and the server's finish reports, as unread.

#### Finish reports

The server reports every task finish in the chat itself, whether the lead runs or not:
completing or concluding in the app, through MCP, by the background check, or with
`merged_with`, including a task done as not adopted that turns out to have shipped with
another pull request (its resolution changes without a new status). The change that
finishes the task stores a `task_results` row (`kind`, `pull_request_number`,
`pull_request_url`, `commit_sha`) and a chat message with author `system` pointing at it,
in one transaction: a crash keeps both or neither, and `result_id` is unique, so each
finish is stored in the chat once. It then sends `chat-changed`. The kind is what was seen:

- `merged`: the task's pull request was seen merged (with its merge commit), its branch was
  seen merged into the base branch locally, or it shipped with another merged pull request.
- `pr_ready`: its criteria were met while its pull request was open with passing checks
  (with the head commit the checks ran on). Toodo completes such a task without a merge, so
  the report never calls it merged.
- `not_adopted`: it was concluded, with the first line of the conclusion.
- `completed`: its criteria were met with neither, e.g. manual criteria only.

A report is no message of the lead or a person: it marks no message as seen, does not end
the lead's typing dots, is not in the lead's inbox (the lead gets the task event), and never
confirms an action. The panel shows it as a notice of its own, and it is not read aloud.

The chat also takes voice: a microphone button dictates a message and sends it, and the
lead's new replies can be read aloud (off by default, a per-browser setting). Both run in
the browser, not on the server; see [`voice.md`](voice.md).

#### Lead inbox

Nothing meant for the lead is lost while it restarts. People's chat messages
(`chat_messages`) and the status changes of tasks the lead follows, a task reaching
`in_review` or `done` (`task_events` rows with `for_lead`), wait as pending until the
lead takes them; their IDs are `chat-<id>` and `event-<id>`.

- **Signal.** Every 5 seconds, while the lead runs, the server types the IDs of new items
  into its pane as one `[Toodo inbox] ...` line: each message not delivered yet (stored
  while the lead was not running, or when typing it failed, for example when the server
  stopped between storing and typing) and each new event. A new agent session gets the
  IDs of everything still pending once. The pane never gets the text of an item.
- **Take.** `take_pending { ids? }`, for the lead token only, returns the items (`kind`
  `chat` with `body`, or `task_event` with `taskId`, `title`, `status`) and marks them
  taken; a taken message counts as delivered and seen. Without `ids` it returns the oldest
  pending items, up to 50, with `remaining`; with `ids` it returns those items even if
  taken before (`takenBefore`), and lists unknown IDs in `missing`. The lead's prompt
  tells it to take everything at the start of every session.
- **Request IDs.** `create_task` and `start_task` take `request_id`, which the lead
  builds from the item it acts on (`chat-12/create-1`). The ID is kept on the task event
  the call made; calling again with it returns that task and creates, starts, or runs
  nothing, so an item handled again after a restart does not repeat its effects.

#### Receipts

Each person's message shows how far it got. `delivered` (its ID reached the pane) and
`seen` (the lead started a turn after that, or replied) are the server's observations;
`seen` is an estimate and never counts as handled. The lead itself writes the two
receipts that say it acted, and only with the lead token:

- **Acknowledged.** `ack_chat { ids }` records `ackedAt`: the lead received the message
  and is on it. It is never an approval, and confirms nothing.
- **Handled.** `reply_in_chat { text, handles }` posts a reply and records it as what
  handled the messages in `handles`; `mark_chat_handled { id, reply_id | task_id }` links
  a reply posted earlier, or a task. A reply must be the lead's and newer than the message;
  a task must carry a request ID of the message (`chat-12` or `chat-12/...`) on one of its
  events. Other links, other authors' messages, and event IDs are rejected, and nothing is
  recorded for a call with one bad ID. Handled means answered or turned into a task, not
  that the task is done.

Both are kept once: repeating a call changes nothing, and a message handled before keeps
its first time and link, so a receipt never goes back. Handling a message acknowledges it
too. A message gets `dueAt`, its time plus 10 minutes by the server's clock, when it is
stored; past it and unhandled it is `overdue`, decided by the server when it lists the
chat, and the supervisor sends `chat-changed` when a message falls due so that clients show
it. Messages stored before receipts existed have no `dueAt` and are never overdue. A take
without IDs also returns `open`: the messages taken before and never handled, however
far back in the history.

Each `done` event the lead follows links the system's finish report of it (`report_id`,
written in the transaction of the finish; earlier events got theirs from the result stored
with them), and `take_pending` returns it as `reportId`, so the lead does not report the
finish again.

#### Lead tools

The lead drives tasks through the same MCP tools as other agents, and the server knows
its calls by the lead token:

- **Skill.** `read_toodo_skill` returns the lead skill (`src-tauri/src/mcp/toodo-lead-skill.md`,
  named `toodo-lead`) for the lead token and the worker skill (`toodo-skill.md`) for
  `mcp-token`. The lead skill is the lead's manual for running the work: requests into
  tasks, links, starting agents, following them up, merge rules, chat reports, and
  memory. The prompt tells the lead to read it at the start of every session.
- **Create.** `create_task` takes `depends_on` and `related`, so a plan is wired in one
  call; an unknown task in either rejects the call and creates nothing.
- **Start.** `start_task` takes `agent_command`, which Toodo runs in the shell pane of
  the task's new herdr workspace once the task is started (for the coding agent; only
  for workspaces on the server's machine). The reply is the task with `panes`, `ranIn`
  (the pane the command ran in), and `paneError` when the panes could not be listed or
  the command did not run; the task is started either way.
- **Record.** Each task the lead creates, starts, completes, or concludes gets a
  progress note in its history ("Started by the Toodo lead and ran `...` in its pane
  ..."). Creating and starting also post a lead message in the chat, and the
  `chat-changed` event fires; a completion or conclusion is reported by the server's
  finish report instead, so it is not posted twice.
- **Confirm.** `conclude_task`, `complete_task` with `merged_with`, and `complete_task`
  of a task awaiting completion (which confirms its manual criteria for a person) do
  nothing when the lead first calls them: Toodo posts a request with a random
  six-character code to the chat, naming the action and its arguments, and rejects the
  call. Once a person answers `confirm <code>` in the chat, the lead calls the tool again
  with the same arguments and `confirmation` set to the code. The request must still be
  among the latest chat messages and match the action exactly, so a code confirms only
  the action it was asked for. The lead cannot answer for a person: it posts only through
  `reply_in_chat`, as the lead, and has no web or device token.
- **Merge.** Toodo has no merge tool. The lead's prompt forbids merging pull requests and
  pushing to base branches; agents that implement tasks follow the workspace's agent
  merge policy, which never allows merging past the base branch.

Calls with `mcp-token` behave as before: the worker skill, no notes or chat messages,
and no confirmations.

### systemd

Run it as the user who runs herdr, so it finds herdr's socket, `~/.ssh/config`, and the
`gh` login. `scripts/install-server.sh` writes
`~/.config/systemd/user/toodo-server.service` like this, with `PATH` taken from where it
found `herdr`, `gh`, `git`, and `ssh` (and `TOODO_WEB_DIR` added when it installs the web
app):

```ini
[Unit]
Description=Toodo server
After=network.target herdr-server.service
Wants=herdr-server.service

[Service]
ExecStart=/home/me/.local/bin/toodo-server
Environment=TOODO_LISTEN=127.0.0.1:47616
Environment=TOODO_DATA_DIR=/home/me/.local/share/toodo-server
Environment=PATH=/home/me/.local/bin:/usr/local/bin:/usr/bin:/bin
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

`herdr-server.service` is the unit of a headless herdr server, if there is one; without it
the `Wants=` is ignored. To set the service up by hand, run
`systemctl --user enable --now toodo-server`. Either way, run
`loginctl enable-linger $USER` to keep it running without a login session.

## Web App

The web build of the frontend uses the server from a browser. A release install serves it
(`scripts/install-server.sh --release ...`); from a checkout, use `--with-web`, or
`npm ci && npm run build:web` into `dist-web/` with `TOODO_WEB_DIR` pointing at it. The
browser asks for `web-token` and a device name once, registers itself as a device, and
keeps its device token in local storage.

It is an installable web app (PWA): browsers offer to install it from the server URL, and
it then opens in its own window. The app offers it as well: a banner installs it in
browsers that can (Chrome, Edge), and on iPhone and iPad it explains *Share > Add to Home
Screen*; dismissing the banner hides it on that browser for good. Browsers install web apps only from a secure origin:
`https://`, or `http://127.0.0.1` and `http://localhost`. The build carries a web app
manifest (`/manifest.webmanifest`, icons under `/pwa/`) and a service worker (`/sw.js`)
that caches the app shell, the built HTML, scripts, styles, and icons, so the app starts
without the network. It never caches or answers `/api` or `/mcp`: everything the app
shows comes from the server. Each build has its own cache, which replaces the previous
one once no window uses it any more.

When the server does not answer (it is down, the device is offline, or a proxy in front of
it answers with its own 502-504 or 520-530 error page), the app shows an offline screen
over itself instead of a broken page, and the service worker starts the cached app instead
of showing such an error page. The app checks again after 2 s, doubling up to 30 s, at
once when the device comes back online or the app is shown again, and on *Try again*;
once the server answers, the screen goes away and everything is loaded again.

On the server, "Local" is the server itself, so the web app browses the server's
directories (from the server user's home) when you pick a folder for a workspace. Reports
are downloaded in the browser, and database backups are downloaded and uploaded (see
[API](#api)). Reach the loopback port through an SSH tunnel
(`ssh -L 47616:127.0.0.1:47616 <server>`, then `http://127.0.0.1:47616`); anything that
puts it on a network needs its own authentication in front, since the web token opens the
whole server account. To open it from a phone, see
[Off the Machine](#off-the-machine-cloudflare-tunnel-and-access).

The event stream (`/api/events`) is a long-lived server-sent event response with a
keep-alive comment every 15 seconds; a proxy in front must not buffer it. When the stream
drops, the web app reconnects and refetches everything. The pane view's WebSocket
(`/api/panes/stream`) needs the proxy to pass WebSocket upgrades through, with the
browser's `Host` (or `X-Forwarded-Host`), since the server only accepts upgrades whose
`Origin` matches it.

### Off the Machine: Cloudflare Tunnel and Access

To use the web app from a phone or another computer without an SSH tunnel, publish it
through a Cloudflare Tunnel with Cloudflare Access in front. The server keeps listening on
loopback; `cloudflared` connects out to Cloudflare, so no port is opened on the machine.
Access signs you in before any request reaches the server, and the web app still asks for
`web-token`, so a stolen token alone is not enough from outside, and a signed-in browser
without the token gets nothing.

You need a domain on Cloudflare and a Zero Trust organization (the free plan does). Below,
`toodo.example.com` stands for the hostname you pick.

1. **Access first.** In Zero Trust, add an Access application of type *self-hosted* for
   `toodo.example.com` (the whole hostname, no path) with an *Allow* policy that includes
   only your own identity, for example your e-mail address with the one-time PIN login. Do
   it before the hostname resolves, so nothing is ever reachable without it. Add no
   *Bypass* policy: the API, the event stream, and `/mcp` all sit behind the same
   application. The only other policy it takes is a *Service Auth* one for the desktop app
   (see [Through Cloudflare Access](#through-cloudflare-access)).
2. **A tunnel of its own.** Create a new, remotely managed tunnel (*Networks > Tunnels*)
   for the server instead of adding a hostname to a tunnel the machine already runs for
   something else: it gets its own token, can be stopped or deleted alone, and a mistake in
   its routes cannot take the other one down. Give it one public hostname,
   `toodo.example.com` to `http://127.0.0.1:47616` (the server's `TOODO_LISTEN`), and let
   everything else fall through to `http_status:404`. Cloudflare adds the proxied DNS
   record for the hostname.
3. **Run the connector as the server's user.** Save the tunnel's token to a file only you
   can read, and run `cloudflared` with it from a systemd user service next to
   `toodo-server.service`:

   ```sh
   mkdir -p -m 700 ~/.config/toodo-tunnel
   (umask 077 && cat > ~/.config/toodo-tunnel/token)   # paste the token, then Ctrl-D
   ```

   ```ini
   # ~/.config/systemd/user/toodo-tunnel.service
   [Unit]
   Description=Cloudflare tunnel for toodo-server
   After=network-online.target toodo-server.service
   Wants=network-online.target

   [Service]
   ExecStart=/usr/local/bin/cloudflared --no-autoupdate tunnel run --token-file %h/.config/toodo-tunnel/token
   Restart=always
   RestartSec=5

   [Install]
   WantedBy=default.target
   ```

   Then `systemctl --user daemon-reload && systemctl --user enable --now toodo-tunnel`.
   Never put the token on the command line or in the unit file, where other users of the
   machine can read it.
4. **Check from outside** (any machine, no cookies): every path is a redirect to the Access
   login, never the app:

   ```sh
   curl -s -o /dev/null -w '%{http_code} %{redirect_url}\n' https://toodo.example.com/api/rpc/list_tasks
   # 302 https://<team>.cloudflareaccess.com/cdn-cgi/access/login/toodo.example.com?...
   ```

   Then open `https://toodo.example.com` on the phone, sign in to Access, and enter
   `web-token` and a name for the phone once; the phone then keeps a device token of its
   own. Install it from the browser: *Install app* (or *Add to Home screen*)
   in Chrome on Android, *Share > Add to Home Screen* in Safari on iOS.

Things to know:

- The web app links its manifest with `crossorigin="use-credentials"`. Browsers fetch a
  manifest without cookies otherwise, Access would answer it with its login redirect, and
  the browser would not offer to install the app.
- The service worker loads pages from the network first, so when the Access session has
  expired, reopening or reloading the app goes through the Access login again. Until then,
  requests to `/api` fail (Access redirects them to its login, which a script cannot
  follow); reload the app to sign in again. The Access session duration of the
  application sets how often that happens.
- `/api/events` works through the tunnel: Cloudflare passes `text/event-stream` responses
  through unbuffered, and the server's 15-second keep-alive stays below Cloudflare's idle
  timeout.
- Agents keep using `/mcp` on loopback (or through the server's reverse SSH tunnels);
  do not point remote agents at the public hostname, where Access stops them.
- If a phone is lost, revoke it under **Devices** in Settings from another device; the shared
  `web-token` and the other devices keep working.
- To take it offline again, stop and disable `toodo-tunnel`; to remove it, delete the
  tunnel, its DNS record, and the Access application.

### The Desktop App as a Client

The desktop app can show a server's data instead of its own, as one more client of the
server next to the browsers. Under **General** in Settings, add the server's address
(for example `http://192.168.0.5:47616`) with a name such as "Home", and choose **Switch**;
the app restarts, asks for `web-token` and a device name once, and then keeps a device
token of its own, like a browser. **Use this computer's data** switches back; the local
data is untouched meanwhile.

The app keeps a list of servers, for example a home server and a work server:

- The name of the server in use is at the top of the sidebar (in the header on a phone);
  its menu switches to another saved server. Switching from one server to another only
  reloads the window, so the other server shows within a second or two, with nothing of
  the first one left: the cached data, the event stream, and the pane views go with the
  page. Switching between a server and this computer's data restarts the app.
- **Rename** changes a server's name; **Remove** takes it off the list and forgets the
  app's token and Access service token for it (revoke that token under **Devices** on the
  server, too). The server in use cannot be removed.
- Each saved server has its own Cloudflare Access service token, if it needs one
  ([Through Cloudflare Access](#through-cloudflare-access)), so switching servers sends
  each one only its own.
- **Sign in again** forgets the app's token for a server, so the app asks for `web-token`
  again, at once for the server in use and otherwise when it switches to it. A token one
  server revokes leaves the other servers signed in.
- When the server in use does not answer, the offline banner names it and offers the
  other saved servers.

- Connected, the app starts no database, MCP server, SSH tunnels, or background checks
  of its own: the server does all of that. It keeps its window, tray, updates, and
  notifications. The saved servers and the one in use are `connection.json` in the app
  data directory.
- The app keeps its token and the lead chat's read mark per server address, so switching
  between servers signs in to each once and keeps them apart. The theme and the voice
  settings are the app's own and stay the same on every server.
- The app sends its requests to the server from its Rust side, not from its pages: the
  API calls, the event stream, backup downloads and uploads, and the pane view's
  WebSocket. So no CORS applies, and the app can add a Cloudflare Access service token
  (below). It only sends them to the server it uses (or one it keeps a service token
  for); the token, not a cookie, authorizes each request. The server still answers CORS
  requests from the app's origins (`http://tauri.localhost` and `https://tauri.localhost`
  on Windows and Android, `tauri://localhost` on macOS and Linux).
- The app must reach the server: on the same network, through an SSH tunnel
  (`ssh -L 47616:127.0.0.1:47616 server` and `http://127.0.0.1:47616`), a VPN, or a
  hostname behind Cloudflare Access with a service token.
- Downloads of backups go through the WebView's own download handling.
- The macOS app is only ever used this way; see [docs/macos.md](macos.md).

#### Through Cloudflare Access

Access lets a browser in after its login page, which the app cannot show. Give the app a
**service token** instead: a client ID and secret it sends as `CF-Access-Client-Id` and
`CF-Access-Client-Secret` with every request. This assumes the hostname, tunnel, and
Access application of [Off the Machine](#off-the-machine-cloudflare-tunnel-and-access).

1. **Create the token.** In the Cloudflare dashboard, open **Zero Trust > Access controls >
   Service credentials > Service Tokens** (older dashboards: **Access > Service Auth**) and
   choose **Create Service Token**. Name it after the computer, for example `toodo
   laptop`, and pick a **Service Token Duration**, for example 1 year. Copy the **Client
   ID** and **Client Secret** now: Cloudflare shows the secret only once.
2. **Let it in.** Open the Access application of the hostname (**Access controls >
   Applications**, then *Configure*), and under **Policies** add a policy with the action
   **Service Auth** that includes **Service Token** is the token you created. Keep the
   *Allow* policy for your own sign-in next to it, so browsers keep working. Add no
   *Bypass* policy and leave **Bypass OPTIONS requests to origin** off: the app sends no
   CORS preflight, so everything stays behind Access.
3. **Enter it in the app.** Under **General** in Settings, enter the server's address
   (`https://toodo.example.com`) and, under **Cloudflare Access service token**, the
   client ID and the client secret, and choose **Add server**; for a server saved
   already, choose **Add service token** on it instead. Then **Switch** to it: the app
   asks for `web-token` and a name once, and keeps a device token, as with any server.
4. **Check from outside**, with the token (any machine; the answer must come from the
   server, `401` without a Toodo token, not Access's `302` to its login):

   ```sh
   curl -s -o /dev/null -w '%{http_code}\n' \
     -H "CF-Access-Client-Id: <client id>" -H "CF-Access-Client-Secret: <client secret>" \
     https://toodo.example.com/api
   ```

Things to know:

- The app keeps the tokens per server address in `access-tokens.json` in its app data
  directory, readable by your user only. It never hands the secret to its pages, never
  shows it again, and never writes it to a log; Settings shows the client ID only, on
  the saved server. **Remove service token** forgets it; to replace it, choose **Change
  service token**, enter a new client ID and secret, and choose **Save service token**,
  which takes effect at once.
- When Access turns a request away (no token, a wrong or revoked one, an expired one, or
  no *Service Auth* policy for it), the app says so in its banner, with the client ID it
  sent, and keeps retrying; it keeps its Toodo token, so fixing the service token is
  enough. Access answers such requests with its login redirect or, with strict service
  token authentication, with `401` or `403`; the app tells both from the server's own
  answers by Access's `WWW-Authenticate: Cloudflare-Access` challenge.
- Service tokens expire at the end of their duration. Before that, choose **Refresh** on
  the token in the dashboard to extend it (the secret stays), or create a new one and
  enter it in the app.
- To revoke one computer's access, delete its service token in the dashboard; the app
  then shows Access's refusal, and other computers with tokens of their own keep
  working.

## Development

`npm run dev:web` starts the Vite dev server with the web build's modules and forwards
`/api` and `/mcp` to a running `toodo-server` (`TOODO_SERVER`, default
`http://127.0.0.1:47616`).

## API

- `POST /api/rpc/<command>`: runs a command by its Tauri name (e.g. `list_tasks`) with
  its arguments as a JSON object with camelCase keys. A result is `200` with the JSON
  result; a command error is `{ "kind", "message" }` with `422` (invalidInput), `404`
  (notFound), `409` (rejected), `502` (external), or `500` (internal). An unknown command
  is `404` and malformed arguments `400`, both with a text body.
- `GET /api/events`: server-sent events `tasks-changed`, `workspaces-changed`,
  `agent-statuses-changed`, and `pull-request-statuses-changed` with the payloads of the
  desktop app's events, and `lead-changed` with the result of `get_lead` (see
  [Lead](#lead)).
- `GET /api/backups/export`: a consistent snapshot of the database, as an
  `application/vnd.sqlite3` attachment named `toodo-backup-<date>.sqlite3`.
- `GET /api/backups/{name}`: the backup named `name` from the backup directory, by the
  file name `list_backups` reports; `404` for any other name.
- `POST /api/backups/restore`: replaces all data with the Toodo database sent as the
  request body (`application/octet-stream`, at most 256 MB, else `413`). The current data
  is saved as a pre-restore backup first, which is the JSON result, like `restore_backup`.
  A file that is not a Toodo database is `422` and changes nothing.
- `GET /api/devices`: every device as `{ id, label, createdAt, lastSeenAt, revokedAt,
  current }` (Unix seconds; `current` marks the device whose token the request bears),
  active ones first.
- `POST /api/devices` with `{ "label": string }`: registers a device and returns `201`
  with `{ device, token }`. Only the shared web token may register (else `403`); a blank
  name or one over 80 characters is `422`.
- `POST /api/devices/{id}/revoke`: revokes the device for good and returns it; `404` for
  an unknown id. Revoking it again keeps the first revocation time.
- `POST /api/stream-tickets` with `{ "taskId", "paneId" }`: a ticket for the pane view of
  a pane in the task's herdr workspace, `{ "ticket", "expiresAt" }` (Unix seconds). A pane
  outside that workspace is `404`. The ticket opens one stream within 30 seconds.
- `GET /api/panes/stream?ticket=<ticket>`: the WebSocket of the pane view, authorized by
  the ticket instead of the token; `401` for an unknown, used, or expired ticket and `403`
  for an `Origin` other than the server's own or the desktop app's. The server sends
  `{ "type": "frame", "seq", "cols", "rows", "ansi" }` whenever the pane's screen
  changes, and `{ "type": "closed", "reason" }` before it closes. The view is read-only
  until the client sends `{ "type": "control", "on": true }` (answered with the same);
  then `{ "type": "text", "text" }` and `{ "type": "keys", "keys": [...] }` reach the
  pane. Messages are at most 64 KB. The server logs one line per control session.

Commands that read or write a path on the desktop app's own machine are not served:
`save_report` (the web app downloads reports instead), `export_database`, and
`restore_database` (replaced by the endpoints above and the `restore_backup { name }`
command). `list_directories { machine, path }` lists directories on the server for
`{ "kind": "local" }` and on an SSH machine for `{ "kind": "ssh", "label": ... }`;
`add_herdr_machine` is refused, since `herdr machine add` asks questions in a terminal.

All of them except the pane stream require `Authorization: Bearer <token>` with the shared web
token or the token of a device that is not revoked; `/mcp` requires the MCP token. The web
build swaps the Tauri modules for `src/web/`, so the generated bindings in
`src/lib/bindings.ts` serve both apps; a test checks that the server routes every command
they invoke.
