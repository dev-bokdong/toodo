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

Everything is served on one port, `127.0.0.1:47616` by default, so agents on the server
reach MCP at `http://127.0.0.1:47616/mcp` (the "Agent access" panel shows the URL and token).

The default is not the desktop app's MCP port, 47615, on purpose: the desktop app keeps
`ssh -R 127.0.0.1:47615:127.0.0.1:47615` open to every SSH machine with a workspace, and the
server forwards its own port to its SSH machines the same way. If the server's machine is
also an SSH machine of the desktop app, or both reach the same SSH machine, the two would
fight over the same port: the loser either fails to listen or keeps retrying its tunnel,
and agents would get whichever token won. Keep `TOODO_LISTEN` off 47615 for that reason.

## Machines and Paths

The "Local" machine is the server: local workspaces and the local worktree root use POSIX
paths on it, and the local herdr is the herdr server running there. Workspaces on saved
herdr SSH machines work as in the desktop app. `herdr machine add <host>` asks questions in
a terminal, so run it on the server yourself; Toodo lists the machine afterwards.

The server is not a backend for the desktop app: each keeps its own database.

## Install and Update

On the server, as the user who runs herdr, from a checkout of this repository (Node.js,
npm, Rust, git, gh, ssh, curl, and a running herdr installed):

```sh
scripts/install-server.sh
```

The script builds the web app and `toodo-server`, installs the binary to
`~/.local/bin/toodo-server` and the web build to `~/.local/share/toodo-server-web`, writes
the systemd user service `toodo-server.service` (see [systemd](#systemd)), restarts it, and
checks that it answers. It prints where the access tokens are, never the tokens.

- `--listen ADDR`: listen address of the service (default `127.0.0.1:47616`; 47615 is
  refused, see above).
- `--no-service`: build and install only; leave the service alone.

To update, pull and run it again:

```sh
git pull
scripts/install-server.sh
```

The database and tokens in the data directory are kept.

## Build

To build by hand instead, on the server (Rust, Node.js, and npm installed):

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
| `TOODO_LISTEN` | `127.0.0.1:47616` | Address to listen on; avoid port 47615 (see above) |
| `TOODO_DATA_DIR` | `$XDG_DATA_HOME/toodo-server` or `~/.local/share/toodo-server` | Database, backups, and access tokens |
| `TOODO_WEB_DIR` | none | Web build to serve; without it only the API and MCP are served |

On first start the data directory gets two access tokens:

- `web-token`: the browser asks for it once and keeps it in local storage. Every
  `/api` request bears it.
- `mcp-token`: agents send it to `/mcp`, as shown in the app's "Agent access" panel.

The data directory is created readable by its owner only (`0700`) and the token files
`0600`; the server tightens an existing directory and token files on start. To replace a
token, delete its file and restart the server; browsers then ask for the new one.

**Treat `web-token` like a password to the server account.** It opens every command of
the app, including ones that write `~/.ssh/config` (which can run commands through
`ProxyCommand`), save reports, and export or restore the database at any path the server's
user can reach. Whoever holds it can run code as that user. `mcp-token` gives the MCP
tools only, but still lets an agent start, complete, and conclude tasks.

### Access

The server speaks plain HTTP. Keep the default loopback address and put something in front
of it; never expose the port itself.

- **SSH tunnel**: `ssh -L 47616:127.0.0.1:47616 <server>`, then open
  `http://127.0.0.1:47616`.
- **Cloudflare Tunnel**: with `cloudflared` on the server, add a public hostname (e.g.
  `toodo.example.com`) whose service is `http://127.0.0.1:47616`. Put a Cloudflare Access
  application (e.g. email one-time PIN for your address only) in front of that hostname, so
  the web token is not the only thing between the internet and the server account.
- **VPN** (e.g. Tailscale or WireGuard) to the loopback port, or a **TLS reverse proxy** on
  the server.

The event stream (`/api/events`) is a long-lived server-sent event response with a
keep-alive comment every 15 seconds, which stays under Cloudflare's 100-second idle limit.
A proxy must not buffer it: for nginx, set `proxy_buffering off;` (and a
`proxy_read_timeout` above 15 seconds) on that location. When the stream drops anyway, the
web app reconnects and refetches everything.

### systemd

Run it as the user who runs herdr, so it finds herdr's socket, `~/.ssh/config`, and the
`gh` login. `scripts/install-server.sh` writes
`~/.config/systemd/user/toodo-server.service` like this, with `PATH` taken from where it
found `herdr`, `gh`, `git`, and `ssh`:

```ini
[Unit]
Description=Toodo server
After=network.target herdr-server.service
Wants=herdr-server.service

[Service]
ExecStart=/home/me/.local/bin/toodo-server
Environment=TOODO_LISTEN=127.0.0.1:47616
Environment=TOODO_WEB_DIR=/home/me/.local/share/toodo-server-web
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
- `GET /api/events`: server-sent events `tasks-changed`, `agent-statuses-changed`, and
  `pull-request-statuses-changed` with the payloads of the desktop app's events.

Both require `Authorization: Bearer <web-token>`; `/mcp` requires the MCP token. The web
build swaps the Tauri modules for `src/web/`, so the generated bindings in
`src/lib/bindings.ts` serve both apps; a test checks that the server routes every command
they invoke.
