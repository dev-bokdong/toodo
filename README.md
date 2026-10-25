# Toodo

Toodo is a Windows desktop app for planning and shipping code changes. It connects a task plan to
isolated git worktrees, AI-agent implementation sessions in
[herdr](https://herdr.dev), commit tracking, and pull requests. It keeps a timeline of
when each task was planned, started, and completed, so the year's work can be compiled
into a performance report.

## Features

- **Workspaces**: use a local directory or a remote directory on a saved herdr SSH
  machine as a workspace.
- **Planning**: create tasks per workspace; express ordering as dependencies (a DAG).
- **Worktree preparation**: before a task is implemented, create a dedicated
  `git worktree` in a separate directory via the herdr API.
- **Implementation in herdr**: the work itself happens in herdr panes and agents
  attached to the task's worktree.
- **Commit tracking**: record the commits (SHA, message, author, time) made for a task.
- **Completion and PR**: a task's branch is pushed and a pull request opened for it;
  once that pull request is merged on GitHub, Toodo completes the task automatically.
- **Agent planning (MCP)**: while Toodo runs, coding agents can list tasks, create
  tasks, and connect dependencies through its local MCP server; copy the client config
  from Agent access in Settings (the gear at the bottom of the sidebar). Agents on SSH machines use the same config,
  through a reverse SSH tunnel Toodo keeps open to each machine with a workspace.
- **Timeline**: every status change is timestamped; the dependency graph is drawn on a
  time axis showing when each task was planned, started, reviewed, and completed.

## Documentation

- [AGENTS.md](AGENTS.md): contributor and AI-agent guidelines, including the language policy.
- [docs/architecture.md](docs/architecture.md): domain model and system components.
- [docs/workflow.md](docs/workflow.md): task lifecycle from planning to pull request.
- [docs/server.md](docs/server.md): running Toodo headless on a Linux server, used from a
  browser.

## Requirements

- Windows 10/11
- [herdr](https://herdr.dev), installed and running (Toodo controls it; it is not bundled)
- Git for Windows, Windows OpenSSH, and the GitHub CLI (`gh`) on `PATH`

Toodo can also run as a server on a Linux machine with herdr, used from a browser; see
[docs/server.md](docs/server.md).

## End-to-End Tests

`e2e/` holds Playwright tests that drive the web app (the `--mode web` build) against a
scratch `toodo-server`. Every test starts its own server on a free loopback port with a
temporary data directory, so a run never touches the data of a real server and never binds
the default server ports. The servers run without a lead (`TOODO_LEAD=off`), and a run
fails when it leaves a new workspace open in the herdr of the machine (`e2e/herdr-guard.ts`).
CI runs the suite on pull requests into `dev`
(`.github/workflows/e2e.yml`).

To run it locally (Node, Rust, and a Linux machine; the Windows app is not involved):

```sh
npm ci
npx playwright install chromium                       # once
(cd src-tauri && cargo build --no-default-features --features server --bin toodo-server)
npx vite build --mode web --outDir dist-web
npm run test:e2e
```

Rebuild the server after Rust changes and the web app after frontend changes; the tests use
the last builds. Pass Playwright options after `--`, for example
`npm run test:e2e -- e2e/timeline.spec.ts --headed`. `TOODO_SERVER_BIN` points the tests at
another `toodo-server` binary and `TOODO_E2E_WEB_DIR` at another web build. After a failure,
`npx playwright show-report` opens the report.

## Tech Stack

Tauri 2, Rust (tokio), React + TypeScript, React Flow, SQLite.
See [AGENTS.md](AGENTS.md#platform-and-stack) for details.

## Releases and Updates

Installed copies update themselves: on startup (and every six hours) Toodo checks
`https://github.com/bok-dong/toodo/releases/latest/download/latest.json` and offers an
"Install and restart" banner when a newer version is published.

To publish a release:

1. On `dev`, run `npm run bump <major|minor|patch|X.Y.Z>`. It sets the same version in
   `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`,
   `src-tauri/Cargo.toml`, and `src-tauri/Cargo.lock`. Commit it as `Release X.Y.Z`.
   `npm run check-version` checks that the five files agree; the workflow runs it first.
2. Open a pull request from `dev` to `main` and merge it.
3. On every push to `main`, the `Release` workflow (`.github/workflows/release.yml`)
   releases the version in `src-tauri/tauri.conf.json` unless it is already published:
   it builds the MSI and NSIS installers on Windows, signs the updater artifacts, tags the
   `main` commit `vX.Y.Z`, and publishes a GitHub release with `latest.json` (pointing at
   the NSIS installer). Once that is done, it builds the Linux `toodo-server` on Ubuntu
   and adds `toodo-server-vX.Y.Z-linux-x86_64.tar.gz` and its `.sha256` to the same
   release, so the server and the Windows app always share a version (see
   `docs/server.md`). A push to `main` without a version bump does nothing.

Releases run from `main` rather than from tags so the Rust build cache of one release is
reused by the next. If a release fails, run the `Release` workflow by hand on `main`; it
completes a release whose `latest.json` or server assets were not uploaded yet, skipping
the parts that are already there.

Updater artifacts are signed with a minisign key; the public key is in
`src-tauri/tauri.conf.json` (`plugins.updater.pubkey`). The workflow reads the private key
from the repository secrets `TAURI_SIGNING_PRIVATE_KEY` (key file content) and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (empty if the key has no password). If the private
key is lost, installed copies can no longer be updated and must be reinstalled by hand.

Local `npm run tauri build` does not create updater artifacts and needs no key; to
reproduce a release build locally, set `TAURI_SIGNING_PRIVATE_KEY` and pass
`--config src-tauri/tauri.release.conf.json`.

## Status

Early bootstrap. Setup instructions will be added with the project scaffold.

## Language Policy

All repository content (docs, code, comments, commits, PRs) is written in English.
