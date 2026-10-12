# Toodo

Toodo is a Windows desktop app for planning and shipping code changes. It connects a task plan to
isolated git worktrees, AI-agent implementation sessions in
[herdr](https://herdr.dev), commit tracking, and pull requests.

## Features

- **Workspaces**: use a local directory or a remote directory on a saved herdr SSH
  machine as a workspace.
- **Planning**: create tasks per workspace; express ordering as dependencies (a DAG).
- **Worktree preparation**: before a task is implemented, create a dedicated
  `git worktree` in a separate directory via the herdr API.
- **Implementation in herdr**: the work itself happens in herdr panes and agents
  attached to the task's worktree.
- **Commit tracking**: record the commits (SHA, message, author, time) made for a task.
- **Completion and PR**: completing a task opens a pull request for its branch.

## Documentation

- [AGENTS.md](AGENTS.md): contributor and AI-agent guidelines, including the language policy.
- [docs/architecture.md](docs/architecture.md): domain model and system components.
- [docs/workflow.md](docs/workflow.md): task lifecycle from planning to pull request.

## Requirements

- Windows 10/11
- [herdr](https://herdr.dev), installed and running (Toodo controls it; it is not bundled)
- Git for Windows, Windows OpenSSH, and the GitHub CLI (`gh`) on `PATH`

## Tech Stack

Tauri 2, Rust (tokio), React + TypeScript, React Flow, SQLite.
See [AGENTS.md](AGENTS.md#platform-and-stack) for details.

## Status

Early bootstrap. Setup instructions will be added with the project scaffold.

## Language Policy

All repository content (docs, code, comments, commits, PRs) is written in English.
