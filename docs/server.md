# Linux Server

`toodo-server` runs Toodo headless on a Linux machine that runs herdr itself, for one
user who opens Toodo in a browser. It is the same Rust core as the desktop app, built with
the `server` feature instead of the Tauri shell:

- the commands of the desktop app as an HTTP API, for the web build of the frontend,
- the events of the desktop app as a server-sent event stream,
- the MCP server for coding agents (`/mcp`, as in the desktop app),
- the background work of the desktop app: herdr event subscriptions, pull request checks,
  automatic backups, and MCP reverse tunnels to SSH machines,
- SQLite storage in the server's data directory.

Everything is served on one port, `127.0.0.1:47615` by default, the MCP port of the desktop
app, so agents on the server reach MCP at `http://127.0.0.1:47615/mcp` as usual.

## Machines and Paths

The "Local" machine is the server: local workspaces and the local worktree root use POSIX
paths on it, and the local herdr is the herdr server running there. Workspaces on saved
herdr SSH machines work as in the desktop app. `herdr machine add <host>` asks questions in
a terminal, so run it on the server yourself; Toodo lists the machine afterwards.

The server is not a backend for the desktop app: each keeps its own database.

## Build

On the server (Rust, Node.js, and npm installed):

```sh
npm ci
npm run build:web                      # web build of the frontend into dist-web/
cd src-tauri
cargo build --release --no-default-features --features server --bin toodo-server
```

The binary is `src-tauri/target/release/toodo-server`. It needs `git`, `ssh`, `gh`, and
`herdr` on `PATH` at runtime, like the desktop app.

## Run

```sh
TOODO_WEB_DIR=/path/to/dist-web toodo-server
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `TOODO_LISTEN` | `127.0.0.1:47615` | Address to listen on |
| `TOODO_DATA_DIR` | `$XDG_DATA_HOME/toodo-server` or `~/.local/share/toodo-server` | Database, backups, and access tokens |
| `TOODO_WEB_DIR` | none | Web build to serve; without it only the API and MCP are served |

On first start the data directory gets two access tokens:

- `web-token`: the browser asks for it once and keeps it in local storage. Every
  `/api` request bears it.
- `mcp-token`: agents send it to `/mcp`, as shown in the app's "Agent access" panel.

The server speaks plain HTTP. Keep the default loopback address and reach it through an SSH
tunnel (`ssh -L 47615:127.0.0.1:47615 <server>`) or a VPN such as Tailscale, or put a
reverse proxy with TLS in front of it; do not expose it unencrypted.

### systemd

Run it as the user who runs herdr, so it finds herdr's socket, `~/.ssh/config`, and the
`gh` login. `~/.config/systemd/user/toodo-server.service`:

```ini
[Unit]
Description=Toodo server

[Service]
ExecStart=%h/.local/bin/toodo-server
Environment=TOODO_WEB_DIR=%h/.local/share/toodo-server-web
Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin
Restart=on-failure

[Install]
WantedBy=default.target
```

Then `systemctl --user enable --now toodo-server` (and `loginctl enable-linger $USER` to
keep it running without a login session).

## Development

`npm run dev:web` starts the Vite dev server with the web build's modules and forwards
`/api` and `/mcp` to a running `toodo-server` (`TOODO_SERVER`, default
`http://127.0.0.1:47615`).

## API

- `POST /api/rpc/<command>`: runs a command by its Tauri name (e.g. `list_tasks`) with
  its arguments as a JSON object with camelCase keys. A result is `200` with the JSON
  result; a command error is `{ "kind", "message" }` with `422` (invalidInput), `404`
  (notFound), `409` (rejected), `502` (external), or `500` (internal). An unknown command
  is `404` and malformed arguments `400`, both with a text body.
- `GET /api/events`: server-sent events `tasks-changed`, `agent-statuses-changed`, and
  `pull-request-statuses-changed` with the payloads of the desktop app's events.

Both require `Authorization: Bearer <web-token>`; `/mcp` requires the MCP token. The web
build swaps the Tauri modules for `src/web/`, so the generated bindings in
`src/lib/bindings.ts` serve both apps; a test checks that the server routes every command
they invoke.
