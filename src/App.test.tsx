import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { commands, type CommitRecord, type HerdrMachine, type Task, type Workspace } from "@/lib/bindings";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

vi.mock("@/lib/bindings", () => ({
  commands: {
    appInfo: vi.fn(),
    listWorkspaces: vi.fn(),
    createWorkspace: vi.fn(),
    listTasks: vi.fn(),
    createTask: vi.fn(),
    herdrStatus: vi.fn(),
    getMachineSettings: vi.fn(),
    saveMachineSettings: vi.fn(),
    startTask: vi.fn(),
    openTaskInHerdr: vi.fn(),
    listCommitRecords: vi.fn(),
    refreshCommitRecords: vi.fn(),
    completeTask: vi.fn(),
    listHerdrMachines: vi.fn(),
    addHerdrMachine: vi.fn(),
    listSshHosts: vi.fn(),
    addSshHost: vi.fn(),
    listRemoteDirectories: vi.fn(),
  },
}));

const mocked = vi.mocked(commands);
const mockedOpenDialog = vi.mocked(openDialog);

const toodo: Workspace = {
  id: 1,
  name: "toodo",
  location: { kind: "local", path: "C:\\dev\\toodo" },
  defaultBaseBranch: "main",
};

const herdr: Workspace = {
  id: 2,
  name: "herdr",
  location: { kind: "ssh", machine: "devbox", path: "/srv/herdr" },
  defaultBaseBranch: "develop",
};

function task(id: number, workspaceId: number, title: string, overrides: Partial<Task> = {}): Task {
  return {
    id,
    workspaceId,
    taskType: "feat",
    title,
    description: "",
    status: "ready",
    dependsOn: [],
    branch: null,
    worktreePath: null,
    herdrWorkspaceId: null,
    pullRequestUrl: null,
    ...overrides,
  };
}

function savedMachine(target: string, enabled = true): HerdrMachine {
  return { id: `m-${target}`, label: target, target, session: "default", enabled };
}

function commit(taskId: number, sha: string, message: string): CommitRecord {
  return { taskId, sha, message, author: "Ada <ada@example.com>", committedAt: 1700000000 };
}

function renderApp() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>,
  );
}

describe("App", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    mocked.appInfo.mockResolvedValue({ name: "toodo", version: "1.2.3" });
    mocked.herdrStatus.mockResolvedValue({ status: "ok", data: { running: true, version: "0.9.3" } });
    mocked.getMachineSettings.mockImplementation(async (machine) => ({
      status: "ok",
      data: { machine, worktreeRoot: null },
    }));
    mocked.listCommitRecords.mockResolvedValue({ status: "ok", data: [] });
    mocked.listSshHosts.mockResolvedValue({ status: "ok", data: [] });
    mocked.listHerdrMachines.mockResolvedValue({ status: "ok", data: [] });
  });

  it("shows the backend version once the app info command resolves", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [] });
    renderApp();

    expect(screen.getByRole("heading", { name: "Toodo" })).toBeInTheDocument();
    expect(await screen.findByText("v1.2.3")).toBeInTheDocument();
    expect(await screen.findByText("No workspaces yet.")).toBeInTheDocument();
  });

  it("lists workspaces and the tasks of the selected one", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo, herdr] });
    mocked.listTasks.mockImplementation(async (workspaceId) => ({
      status: "ok",
      data:
        workspaceId === 1
          ? [task(1, 1, "Add storage", { status: "done" }), task(2, 1, "Add UI", { status: "planned", dependsOn: [1] })]
          : [task(3, 2, "Fix ssh")],
    }));
    renderApp();

    const tasks = await screen.findByRole("list", { name: "Tasks" });
    expect(within(tasks).getByText("Add storage")).toBeInTheDocument();
    expect(within(tasks).getByText("Depends on #1")).toBeInTheDocument();
    expect(within(tasks).getByText("Planned")).toBeInTheDocument();
    expect(mocked.listTasks).toHaveBeenCalledWith(1);

    fireEvent.click(screen.getByRole("button", { name: /herdr/ }));
    expect(await screen.findByText("Fix ssh")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "herdr" })).toBeInTheDocument();
    expect(mocked.listTasks).toHaveBeenLastCalledWith(2);
  });

  it("shows command errors returned by the backend", async () => {
    mocked.listWorkspaces.mockResolvedValue({
      status: "error",
      error: { kind: "internal", message: "database is locked" },
    });
    renderApp();

    expect(await screen.findByRole("alert")).toHaveTextContent("database is locked");
  });

  it("groups workspaces under Local and the machines saved in herdr", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo, herdr] });
    mocked.listHerdrMachines.mockResolvedValue({
      status: "ok",
      data: [savedMachine("gpu"), savedMachine("lab", false)],
    });
    mocked.listSshHosts.mockResolvedValue({ status: "ok", data: ["unsaved"] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [] });
    renderApp();

    const local = await screen.findByRole("region", { name: "Machine Local" });
    expect(within(local).getByText("toodo")).toBeInTheDocument();
    const gpu = await screen.findByRole("region", { name: "Machine gpu" });
    expect(within(gpu).queryByRole("listitem")).toBeNull();
    expect(within(gpu).queryByText(/in herdr/)).toBeNull();
    expect(within(screen.getByRole("region", { name: "Machine lab" })).getByText("Disabled in herdr")).toBeInTheDocument();
    const devbox = screen.getByRole("region", { name: "Machine devbox" });
    expect(within(devbox).getByText("/srv/herdr")).toBeInTheDocument();
    expect(within(devbox).getByText("Not saved in herdr")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Machine unsaved" })).toBeNull();
  });

  it("opens a local repository picked in the folder dialog", async () => {
    mocked.listWorkspaces.mockResolvedValueOnce({ status: "ok", data: [] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [] });
    mocked.createWorkspace.mockResolvedValue({ status: "ok", data: toodo });
    mockedOpenDialog.mockResolvedValue("C:\\dev\\toodo");
    renderApp();

    await screen.findByText("No workspaces yet.");
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    fireEvent.click(screen.getByRole("button", { name: "Open on Local" }));
    fireEvent.click(screen.getByRole("button", { name: "Browse..." }));
    await waitFor(() => expect(screen.getByLabelText("Folder")).toHaveValue("C:\\dev\\toodo"));
    expect(screen.getByLabelText("Name")).toHaveValue("toodo");
    expect(mockedOpenDialog).toHaveBeenCalledWith(expect.objectContaining({ directory: true, multiple: false }));
    fireEvent.click(screen.getByRole("button", { name: "Open workspace" }));

    expect(await screen.findByRole("heading", { name: "toodo" })).toBeInTheDocument();
    expect(mocked.createWorkspace).toHaveBeenCalledWith({
      name: "toodo",
      location: { kind: "local", path: "C:\\dev\\toodo" },
      source: { kind: "open" },
      defaultBaseBranch: null,
    });
  });

  it("clones into a folder browsed on an SSH host and selects the workspace", async () => {
    mocked.listWorkspaces.mockResolvedValueOnce({ status: "ok", data: [] });
    mocked.listHerdrMachines.mockResolvedValue({ status: "ok", data: [savedMachine("devbox")] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [] });
    mocked.createWorkspace.mockResolvedValue({ status: "ok", data: herdr });
    mocked.listRemoteDirectories.mockImplementation(async (_machine, path) => ({
      status: "ok",
      data:
        path === null
          ? { path: "/home/me", directories: ["src"] }
          : path === "/home/me/src"
            ? { path: "/home/me/src", directories: [] }
            : { path: "/home", directories: ["me"] },
    }));
    renderApp();

    await screen.findByText("No workspaces yet.");
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [herdr] });
    fireEvent.click(await screen.findByRole("button", { name: "Clone on devbox" }));
    fireEvent.change(screen.getByLabelText("Repository URL"), {
      target: { value: "git@github.com:o/herdr.git" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Browse..." }));
    const browser = screen.getByRole("group", { name: "Browse folders on devbox" });
    fireEvent.click(await within(browser).findByRole("button", { name: "src/" }));
    await within(browser).findByText("No subfolders");
    fireEvent.click(within(browser).getByRole("button", { name: "Select folder" }));

    expect(screen.getByLabelText("Parent folder")).toHaveValue("/home/me/src");
    expect(screen.getByText("Clones into /home/me/src/herdr")).toBeInTheDocument();
    expect(mocked.listRemoteDirectories).toHaveBeenCalledWith("devbox", null);
    fireEvent.click(screen.getByRole("button", { name: "Clone workspace" }));

    expect(await screen.findByRole("heading", { name: "herdr" })).toBeInTheDocument();
    expect(mocked.createWorkspace).toHaveBeenCalledWith({
      name: "herdr",
      location: { kind: "ssh", machine: "devbox", path: "/home/me/src/herdr" },
      source: { kind: "clone", url: "git@github.com:o/herdr.git" },
      defaultBaseBranch: null,
    });
  });

  it("adds a new SSH host to herdr and lists it as a machine", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [] });
    mocked.listSshHosts.mockResolvedValue({ status: "ok", data: ["gpu"] });
    mocked.addSshHost.mockResolvedValue({ status: "ok", data: ["gpu", "devbox"] });
    mocked.addHerdrMachine.mockResolvedValue({ status: "ok", data: [savedMachine("devbox")] });
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: "Add machine" }));
    expect(screen.queryByRole("region", { name: "Machine gpu" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "New host" }));
    fireEvent.change(screen.getByLabelText("Host alias"), { target: { value: "devbox" } });
    fireEvent.change(screen.getByLabelText("Host name"), { target: { value: "10.0.0.5" } });
    fireEvent.change(screen.getByLabelText("Port"), { target: { value: "2222" } });
    fireEvent.click(screen.getByRole("button", { name: "Add host" }));

    await waitFor(() => expect(screen.getByLabelText("SSH host")).toHaveValue("devbox"));
    expect(screen.queryByLabelText("Host alias")).not.toBeInTheDocument();
    expect(mocked.addSshHost).toHaveBeenCalledWith({ alias: "devbox", hostName: "10.0.0.5", user: null, port: 2222 });
    fireEvent.click(screen.getByRole("button", { name: "Add to herdr" }));

    expect(await screen.findByRole("region", { name: "Machine devbox" })).toBeInTheDocument();
    expect(screen.queryByRole("form", { name: "Add machine" })).not.toBeInTheDocument();
    expect(mocked.addHerdrMachine).toHaveBeenCalledWith("devbox");
  });

  it("keeps the add machine form open when herdr saves nothing", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [] });
    mocked.listSshHosts.mockResolvedValue({ status: "ok", data: ["gpu"] });
    mocked.addHerdrMachine.mockResolvedValue({
      status: "error",
      error: { kind: "rejected", message: "herdr saved no machine for SSH host 'gpu'" },
    });
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: "Add machine" }));
    const host = screen.getByLabelText("SSH host");
    await within(host).findByRole("option", { name: "gpu" });
    fireEvent.change(host, { target: { value: "gpu" } });
    fireEvent.click(screen.getByRole("button", { name: "Add to herdr" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("herdr saved no machine for SSH host 'gpu'");
    expect(screen.getByRole("form", { name: "Add machine" })).toBeInTheDocument();
  });

  it("creates a task and reloads the task list", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValueOnce({ status: "ok", data: [] });
    mocked.createTask.mockResolvedValue({ status: "ok", data: task(5, 1, "Write docs") });
    renderApp();

    await screen.findByText("No tasks in this workspace yet.");
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [task(5, 1, "Write docs")] });
    fireEvent.change(screen.getByLabelText("Task type"), { target: { value: "docs" } });
    fireEvent.change(screen.getByLabelText("Task title"), { target: { value: "Write docs" } });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));

    expect(await screen.findByText("Write docs")).toBeInTheDocument();
    expect(mocked.createTask).toHaveBeenCalledWith({
      workspaceId: 1,
      taskType: "docs",
      title: "Write docs",
      description: "",
    });
  });

  it("starts a ready task in herdr and shows its worktree", async () => {
    const started = task(1, 1, "Add login", {
      status: "in_progress",
      branch: "feat/add-login-1",
      worktreePath: "D:\\wt\\toodo\\feat-add-login-1",
      herdrWorkspaceId: "w27",
    });
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValueOnce({ status: "ok", data: [task(1, 1, "Add login")] });
    mocked.startTask.mockResolvedValue({ status: "ok", data: started });
    mocked.openTaskInHerdr.mockResolvedValue({ status: "ok", data: null });
    renderApp();

    const start = await screen.findByRole("button", { name: "Start #1" });
    await waitFor(() => expect(start).toBeEnabled());
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [started] });
    fireEvent.click(start);

    const open = await screen.findByRole("button", { name: "Open #1 in herdr" });
    expect(screen.getByText("In progress")).toBeInTheDocument();
    expect(screen.getByText(/feat\/add-login-1/)).toHaveTextContent("D:\\wt\\toodo\\feat-add-login-1");
    expect(mocked.startTask).toHaveBeenCalledWith(1);
    fireEvent.click(open);
    await waitFor(() => expect(mocked.openTaskInHerdr).toHaveBeenCalledWith(1));
  });

  it("shows why a task could not be started", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [task(1, 1, "Add login")] });
    mocked.startTask.mockResolvedValue({
      status: "error",
      error: { kind: "invalidInput", message: "worktree root is not set for machine local" },
    });
    renderApp();

    const start = await screen.findByRole("button", { name: "Start #1" });
    await waitFor(() => expect(start).toBeEnabled());
    fireEvent.click(start);

    expect(await screen.findByRole("alert")).toHaveTextContent("worktree root is not set for machine local");
    expect(screen.getByText("Ready")).toBeInTheDocument();
  });

  it("warns while herdr is not running and blocks starting tasks", async () => {
    mocked.herdrStatus.mockResolvedValue({ status: "ok", data: { running: false, version: null } });
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [task(1, 1, "Add login")] });
    renderApp();

    expect(await screen.findByRole("alert")).toHaveTextContent("herdr is not available");
    expect(await screen.findByRole("button", { name: "Start #1" })).toBeDisabled();

    mocked.herdrStatus.mockResolvedValue({ status: "ok", data: { running: true, version: "0.9.3" } });
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Start #1" })).toBeEnabled();
  });

  it("saves the worktree root for the machine of the workspace", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [herdr] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [] });
    mocked.saveMachineSettings.mockImplementation(async (settings) => ({ status: "ok", data: settings }));
    renderApp();

    expect(await screen.findByText("Set a worktree root to start tasks on this machine.")).toBeInTheDocument();
    const root = screen.getByLabelText("Worktree root on devbox");
    await waitFor(() => expect(root).toBeEnabled());
    fireEvent.change(root, { target: { value: " /srv/wt " } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(screen.queryByText("Set a worktree root to start tasks on this machine.")).not.toBeInTheDocument(),
    );
    expect(root).toHaveValue("/srv/wt");
    expect(mocked.getMachineSettings).toHaveBeenCalledWith({ kind: "ssh", label: "devbox" });
    expect(mocked.saveMachineSettings).toHaveBeenCalledWith({
      machine: { kind: "ssh", label: "devbox" },
      worktreeRoot: "/srv/wt",
    });
  });

  it("records commits and completes an in-progress task with a pull request", async () => {
    const started = task(1, 1, "Add login", {
      status: "in_progress",
      branch: "feat/add-login-1",
      worktreePath: "D:\\wt\\toodo\\feat-add-login-1",
      herdrWorkspaceId: "w27",
    });
    const first = commit(1, "1111111aaaa", "Add login form\n\nWith validation");
    const second = commit(1, "2222222bbbb", "Fix login redirect");
    const pr = "https://github.com/acme/toodo/pull/12";
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [started] });
    mocked.refreshCommitRecords.mockResolvedValue({ status: "ok", data: [first] });
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: "Refresh commits of #1" }));
    const commits = await screen.findByRole("list", { name: "Commits of #1" });
    expect(within(commits).getByText("1111111")).toBeInTheDocument();
    expect(within(commits).getByText("Add login form")).toBeInTheDocument();
    expect(mocked.refreshCommitRecords).toHaveBeenCalledWith(1);

    const done = { ...started, status: "done" as const, pullRequestUrl: pr };
    mocked.completeTask.mockResolvedValue({ status: "ok", data: done });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [done] });
    mocked.listCommitRecords.mockResolvedValue({ status: "ok", data: [first, second] });
    fireEvent.click(screen.getByRole("button", { name: "Complete #1" }));

    expect(await screen.findByText(pr)).toBeInTheDocument();
    expect(screen.getByText("Done")).toBeInTheDocument();
    expect(await screen.findByText("Fix login redirect")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Complete #1" })).not.toBeInTheDocument();
    expect(mocked.completeTask).toHaveBeenCalledWith(1);
  });

  it("shows why a task could not be completed", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({
      status: "ok",
      data: [task(1, 1, "Add login", { status: "in_progress", branch: "feat/add-login-1", worktreePath: "D:\\wt\\x" })],
    });
    mocked.completeTask.mockResolvedValue({
      status: "error",
      error: { kind: "rejected", message: "branch feat/add-login-1 has no commits that are not on main" },
    });
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: "Complete #1" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("has no commits that are not on main");
    expect(screen.getByText("In progress")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Complete #1" })).toBeEnabled();
  });
});
