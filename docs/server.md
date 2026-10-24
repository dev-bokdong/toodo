# Linux Server

`toodo-server` runs Toodo's core headless on a Linux machine without a desktop, next to the
herdr server there, for one user. It is the same Rust core as the desktop app, built with
the `server` feature instead of the Tauri shell, and is used locally on that machine:

- the MCP server for coding agents on the machine (`http://127.0.0.1:47616/mcp`),
- the background work of the desktop app: herdr event subscriptions, pull request checks,
  automatic backups, and MCP reverse tunnels to SSH machines,
- SQLite storage in the server's data directory.

It also serves the commands of the desktop app as an HTTP API with a server-sent event
stream (see [API](#api)). An optional web build of the frontend can use them from a
browser (see [Web App](#web-app)); a normal install leaves it out.

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
`toodo-server-vX.Y.Z-linux-x86_64.tar.gz` (the `toodo-server` binary for x86_64 Linux)
with its `.sha256`, built from the same commit. `toodo-server --version` prints the
version, so an installed server compares directly with a release and with the Windows
app's version. See "Releases and Updates" in the README for how a release is made.

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
`~/.local/bin/toodo-server`, writes the systemd user service `toodo-server.service` (see
[systemd](#systemd)), restarts it, and checks that it answers (`/api` without a token is
`401`). It prints where the MCP token is, never the token.

- `--release latest|vX.Y.Z`: the release to install; `vX.Y.Z` pins a version, for example
  the Windows app's.
- `--listen ADDR`: listen address of the service (default `127.0.0.1:47616`; 47615 is
  refused, see above).
- `--no-service`: install only; leave the service alone.

To update, run it again with the same options; the database and tokens in the data
directory are kept.

Releases before the server was published (0.9.3 and older) have no server asset; build
those from a checkout.

### From a Checkout

For development, run the script without `--release` from a checkout of this repository
(Rust needed as well). It builds `toodo-server` from the checkout and installs it the same
way:

```sh
git pull
scripts/install-server.sh
```

- `--with-web`: also build the web app (needs Node.js and npm), install it to
  `~/.local/share/toodo-server-web`, and serve it (see [Web App](#web-app)). Releases carry
  no web app, so this works from a checkout only.

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

On first start the data directory gets two access tokens:

- `mcp-token`: agents send it to `/mcp` as `Authorization: Bearer <token>`.
- `web-token`: every `/api` request bears it.

The data directory is created readable by its owner only (`0700`) and the token files
`0600`; the server tightens an existing directory and token files on start. To replace a
token, delete its file and restart the server.

**Treat `web-token` like a password to the server account.** It opens every command of
the app, including ones that write `~/.ssh/config` (which can run commands through
`ProxyCommand`), save reports, and export or restore the database at any path the server's
user can reach. Whoever holds it can run code as that user. `mcp-token` gives the MCP
tools only, but still lets an agent start, complete, and conclude tasks.

The server speaks plain HTTP. Keep the default loopback address; never expose the port.

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

### systemd

Run it as the user who runs herdr, so it finds herdr's socket, `~/.ssh/config`, and the
`gh` login. `scripts/install-server.sh` writes
`~/.config/systemd/user/toodo-server.service` like this, with `PATH` taken from where it
found `herdr`, `gh`, `git`, and `ssh` (and `TOODO_WEB_DIR` added with `--with-web`):

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

A web build of the frontend exists for using the server from a browser. Install it with
`scripts/install-server.sh --with-web` (or `npm ci && npm run build:web` into `dist-web/`
and `TOODO_WEB_DIR` pointing at it). The browser asks for `web-token` once and keeps it in
local storage. Reach the loopback port through an SSH tunnel
(`ssh -L 47616:127.0.0.1:47616 <server>`, then `http://127.0.0.1:47616`); anything that
puts it on a network needs its own authentication in front, since the web token opens the
whole server account.

The event stream (`/api/events`) is a long-lived server-sent event response with a
keep-alive comment every 15 seconds; a proxy in front must not buffer it. When the stream
drops, the web app reconnects and refetches everything.

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
  desktop app's events.

Both require `Authorization: Bearer <web-token>`; `/mcp` requires the MCP token. The web
build swaps the Tauri modules for `src/web/`, so the generated bindings in
`src/lib/bindings.ts` serve both apps; a test checks that the server routes every command
they invoke.
