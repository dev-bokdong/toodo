import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { commands, type Task, type Workspace } from "@/lib/bindings";

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
  },
}));

const mocked = vi.mocked(commands);

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

  it("clones a repository into a new workspace and selects it", async () => {
    mocked.listWorkspaces.mockResolvedValueOnce({ status: "ok", data: [] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [] });
    mocked.createWorkspace.mockResolvedValue({ status: "ok", data: herdr });
    renderApp();

    await screen.findByText("No workspaces yet.");
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [herdr] });
    fireEvent.click(screen.getByRole("button", { name: "New" }));
    expect(screen.queryByLabelText("Repository URL")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: "Clone" }));
    fireEvent.change(screen.getByLabelText("Repository URL"), {
      target: { value: "git@github.com:o/herdr.git" },
    });
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "herdr" } });
    fireEvent.change(screen.getByLabelText("Path"), { target: { value: "/srv/herdr" } });
    fireEvent.change(screen.getByLabelText("SSH machine"), { target: { value: "devbox" } });
    fireEvent.click(screen.getByRole("button", { name: "Clone workspace" }));

    expect(await screen.findByRole("heading", { name: "herdr" })).toBeInTheDocument();
    expect(mocked.createWorkspace).toHaveBeenCalledWith({
      name: "herdr",
      location: { kind: "ssh", machine: "devbox", path: "/srv/herdr" },
      source: { kind: "clone", url: "git@github.com:o/herdr.git" },
      defaultBaseBranch: null,
    });
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
});
