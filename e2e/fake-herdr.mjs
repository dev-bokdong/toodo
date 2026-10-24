// A stand-in for the herdr CLI, for scratch servers that run the lead: it answers the
// commands the lead's supervisor runs, with one workspace and one pane, and never touches
// the real herdr. State lives in FAKE_HERDR_DIR:
// - `workspace.json`: the workspace the server opened.
// - `agent-status`: the status of the agent in the pane (`idle` unless a test writes one).
// - `typed.log`: every line typed into the pane, one per line.
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const dir = process.env.FAKE_HERDR_DIR;
const args = process.argv.slice(2);
const file = (name) => path.join(dir, name);

function reply(result) {
  process.stdout.write(JSON.stringify({ id: "fake", result }));
}

function option(name) {
  const at = args.indexOf(name);
  return at === -1 ? undefined : args[at + 1];
}

function workspace() {
  return existsSync(file("workspace.json")) ? JSON.parse(readFileSync(file("workspace.json"), "utf8")) : null;
}

const command = args.slice(0, 2).join(" ");
switch (command) {
  case "workspace list":
    reply({ workspaces: [] });
    break;
  case "workspace create": {
    const opened = { workspace_id: "wL", label: option("--label"), cwd: option("--cwd") };
    writeFileSync(file("workspace.json"), JSON.stringify(opened));
    reply({ workspace: { workspace_id: opened.workspace_id, label: opened.label } });
    break;
  }
  case "workspace close":
    reply({});
    break;
  case "pane list": {
    const open = workspace();
    const status = existsSync(file("agent-status")) ? readFileSync(file("agent-status"), "utf8").trim() : "idle";
    const panes = open
      ? [{ pane_id: "wL:p1", workspace_id: open.workspace_id, cwd: open.cwd, agent: "fake", agent_status: status }]
      : [];
    reply({ panes });
    break;
  }
  case "pane process-info":
    // An agent runs in the foreground of the pane's shell.
    reply({ process_info: { shell_pid: 10, foreground_process_group_id: 20 } });
    break;
  case "pane run":
    appendFileSync(file("typed.log"), `${args[3]}\n`);
    reply({});
    break;
  default:
    process.stderr.write(`fake herdr: unsupported command: ${args.join(" ")}\n`);
    process.exit(1);
}
