import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { formatDate } from "@/lib/timeline";
import { periodBounds, periodContaining, weekStarting } from "@/lib/review";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
  commands,
  events,
  type CommitRecord,
  type HerdrMachine,
  type Task,
  type TaskAgentStatus,
  type Workspace,
} from "@/lib/bindings";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: () => Promise.resolve(null) }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));

vi.mock("@/lib/bindings", () => ({
  commands: {
    appInfo: vi.fn(),
    listWorkspaces: vi.fn(),
    createWorkspace: vi.fn(),
    updateWorkspace: vi.fn(),
    deleteWorkspace: vi.fn(),
    listTasks: vi.fn(),
    createTask: vi.fn(),
    updateTask: vi.fn(),
    pauseTask: vi.fn(),
    setCompletionNote: vi.fn(),
    deleteTask: vi.fn(),
    addDependency: vi.fn(),
    removeDependency: vi.fn(),
    herdrStatus: vi.fn(),
    getMachineSettings: vi.fn(),
    saveMachineSettings: vi.fn(),
    startTask: vi.fn(),
    openTaskInHerdr: vi.fn(),
    restoreTaskWorktree: vi.fn(),
    listCommitRecords: vi.fn(),
    refreshCommitRecords: vi.fn(),
    openPullRequest: vi.fn(),
    completeTask: vi.fn(),
    concludeTask: vi.fn(),
    listHerdrMachines: vi.fn(),
    addHerdrMachine: vi.fn(),
    listSshHosts: vi.fn(),
    addSshHost: vi.fn(),
    listRemoteDirectories: vi.fn(),
    listAgentStatuses: vi.fn(),
    listBackups: vi.fn(),
    getBackupSettings: vi.fn(),
    saveBackupSettings: vi.fn(),
    exportDatabase: vi.fn(),
    restoreDatabase: vi.fn(),
    reviewPeriod: vi.fn(),
    saveReport: vi.fn(),
  },
  events: {
    tasksChanged: { listen: vi.fn() },
    agentStatusesChanged: { listen: vi.fn() },
  },
}));

const mocked = vi.mocked(commands);
const mockedEvents = vi.mocked(events, { deep: true });
const mockedOpenDialog = vi.mocked(openDialog);
const mockedSaveDialog = vi.mocked(saveDialog);
const weekPeriod = (day: string) => periodBounds(weekStarting(day));

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
    status: "planned",
    dependsOn: [],
    branch: null,
    worktreePath: null,
    herdrWorkspaceId: null,
    pullRequestUrl: null,
    pullRequestMergedAt: null,
    pullRequest: null,
    resolution: null,
    completionNote: null,
    timeline: [],
    milestones: {
      plannedAt: null,
      startedAt: null,
      reviewAt: null,
      completedAt: null,
      pausedAt: null,
    },
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
    vi.useRealTimers();
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
    mocked.listAgentStatuses.mockResolvedValue([]);
    mocked.listBackups.mockResolvedValue({ status: "ok", data: [] });
    mocked.getBackupSettings.mockResolvedValue({ enabled: true, intervalHours: 24, keep: 7 });
    mockedEvents.tasksChanged.listen.mockResolvedValue(() => {});
    mockedEvents.agentStatusesChanged.listen.mockResolvedValue(() => {});
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
    mocked.listTasks.mockResolvedValue({
      status: "ok",
      data: [
        task(1, 1, "Add storage", {
          status: "done",
          milestones: { ...task(1, 1, "").milestones, plannedAt: 1760000000, completedAt: 1760500000 },
        }),
        task(2, 1, "Add UI", { status: "planned", dependsOn: [1] }),
        task(3, 2, "Fix ssh"),
      ],
    });
    renderApp();

    const tasks = await screen.findByRole("list", { name: "Tasks" });
    expect(within(tasks).getByText("Add storage")).toBeInTheDocument();
    const dependencies = within(tasks).getByRole("group", { name: "Dependencies of #2" });
    expect(within(dependencies).getByText("#1")).toBeInTheDocument();
    expect(within(tasks).getByText("Planned")).toBeInTheDocument();
    const timeline = within(tasks).getByLabelText("Timeline of #1");
    expect(timeline).toHaveTextContent(`Planned ${formatDate(1760000000)}`);
    expect(timeline).toHaveTextContent(`Completed ${formatDate(1760500000)}`);
    expect(timeline).not.toHaveTextContent("Paused");
    expect(within(tasks).queryByLabelText("Timeline of #2")).not.toBeInTheDocument();
    expect(within(tasks).queryByText("Fix ssh")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /^herdr/ }));
    expect(await screen.findByText("Fix ssh")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "herdr" })).toBeInTheDocument();
    expect(screen.queryByText("Add storage")).not.toBeInTheDocument();
  });

  it("plans dependencies across workspaces and lists every workspace together", async () => {
    const api = task(1, 1, "Add low level API");
    const ui = task(3, 2, "Build UI");
    const docs = task(4, 2, "Write docs");
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo, herdr] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [api, ui, docs] });
    mocked.addDependency.mockResolvedValue({ status: "ok", data: null });
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: /^herdr/ }));
    const add = await screen.findByRole("combobox", { name: "Add dependency to #3" });
    const groups = within(add).getAllByRole("group");
    expect(groups.map((group) => group.getAttribute("label"))).toEqual(["herdr", "toodo"]);
    expect(within(groups[1]).getByRole("option", { name: "#1 Add low level API" })).toBeInTheDocument();

    mocked.listTasks.mockResolvedValue({
      status: "ok",
      data: [api, { ...ui, status: "planned", dependsOn: [1] }, docs],
    });
    fireEvent.change(add, { target: { value: "1" } });
    const dependencies = await screen.findByRole("group", { name: "Dependencies of #3" });
    await waitFor(() => expect(within(dependencies).getByText("toodo #1")).toBeInTheDocument());
    expect(mocked.addDependency).toHaveBeenCalledWith(3, 1);

    fireEvent.click(screen.getByRole("button", { name: "All workspaces" }));
    expect(await screen.findByRole("heading", { name: "All workspaces" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "List" }));
    const tasks = screen.getByRole("list", { name: "Tasks" });
    expect(within(tasks).getAllByRole("listitem")).toHaveLength(3);
    expect(within(tasks).getByText("Add low level API")).toBeInTheDocument();
    expect(within(tasks).getByText("Write docs")).toBeInTheDocument();
    expect(within(screen.getByRole("group", { name: "Dependencies of #3" })).getByText("toodo #1")).toBeInTheDocument();
    expect(screen.queryByRole("form", { name: "New task" })).not.toBeInTheDocument();
  });

  it("reviews the chosen period across workspaces, copies it as Markdown, and saves it as CSV", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 9, 14, 10));
    const at = new Date(2026, 9, 13, 15).getTime() / 1000;
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo, herdr] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [] });
    const year = periodBounds(periodContaining("year", "2026-10-12"));
    mocked.saveReport.mockResolvedValue({ status: "ok", data: null });
    mockedSaveDialog.mockResolvedValue("C:\\reports\\review.csv");
    mocked.reviewPeriod.mockImplementation(async (start) => ({
      status: "ok",
      data:
        start === weekPeriod("2026-10-12").start || start === year.start
          ? [
              {
                task: task(1, 1, "Add storage", {
                  status: "done",
                  pullRequestUrl: "https://github.com/acme/toodo/pull/7",
                }),
                outcome: "completed",
                at,
                commits: [{ ...commit(1, "0123456789", "Add the storage schema\n\nBody"), committedAt: at }],
              },
              { task: task(3, 2, "Fix ssh"), outcome: "worked_on", at, commits: [] },
            ]
          : [],
    }));
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: "Review" }));

    expect(screen.getByRole("heading", { name: "Week review" })).toBeInTheDocument();
    expect(screen.getByLabelText("Week starting")).toHaveValue("2026-10-12");
    const completed = await screen.findByRole("region", { name: "Completed" });
    expect(within(completed).getByText("Add storage")).toBeInTheDocument();
    expect(within(completed).getByText(`toodo · ${formatDate(at)}`)).toBeInTheDocument();
    expect(within(completed).getByText("https://github.com/acme/toodo/pull/7")).toBeInTheDocument();
    expect(within(completed).getByRole("list", { name: "Commits of #1 this week" })).toHaveTextContent(
      "0123456Add the storage schema",
    );
    const workedOn = screen.getByRole("region", { name: "Worked on" });
    expect(within(workedOn).getByText(`herdr · ${formatDate(at)}`)).toBeInTheDocument();
    const { start, end } = weekPeriod("2026-10-12");
    expect(mocked.reviewPeriod).toHaveBeenCalledWith(start, end);
    const summary = screen.getByRole("region", { name: "Summary" });
    expect(within(summary).getByText("Completed by type").nextSibling).toHaveTextContent("feat 1");
    expect(within(summary).queryByRole("list", { name: "Completed per month" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Copy as Markdown" }));
    expect(await screen.findByText("Copied as Markdown.")).toBeInTheDocument();
    expect(writeText).toHaveBeenCalledWith(
      expect.stringContaining(
        "## Completed (1)\n\n- [feat] Add storage (toodo, 2026-10-13, https://github.com/acme/toodo/pull/7)",
      ),
    );

    fireEvent.click(screen.getByRole("button", { name: "Previous week" }));
    expect(await screen.findByText("No recorded activity in this week.")).toBeInTheDocument();
    expect(screen.getByLabelText("Week starting")).toHaveValue("2026-10-05");
    expect(mocked.reviewPeriod).toHaveBeenLastCalledWith(weekPeriod("2026-10-05").start, start);

    fireEvent.click(within(screen.getByRole("group", { name: "Review period" })).getByRole("button", { name: "Year" }));
    expect(screen.getByRole("heading", { name: "Year review" })).toBeInTheDocument();
    expect(await screen.findByRole("region", { name: "Completed" })).toBeInTheDocument();
    expect(mocked.reviewPeriod).toHaveBeenLastCalledWith(year.start, year.end);
    const months = within(screen.getByRole("region", { name: "Summary" })).getByRole("list", {
      name: "Completed per month",
    });
    expect(within(months).getAllByRole("listitem")).toHaveLength(12);
    expect(within(months).getAllByRole("listitem")[9]).toHaveTextContent("2026-101");

    fireEvent.click(screen.getByRole("button", { name: "Save as CSV" }));
    expect(await screen.findByText("Saved to C:\\reports\\review.csv")).toBeInTheDocument();
    expect(mockedSaveDialog).toHaveBeenCalledWith(expect.objectContaining({ defaultPath: "toodo-review-2026.csv" }));
    expect(mocked.saveReport).toHaveBeenCalledWith(
      "C:\\reports\\review.csv",
      expect.stringContaining(",toodo,feat,Add storage,Completed,"),
    );

    fireEvent.click(screen.getByRole("button", { name: /^toodo/ }));
    expect(await screen.findByRole("heading", { name: "toodo" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Year review" })).not.toBeInTheDocument();
  });

  it("shows the tasks of every workspace on a timeline, narrowed to one workspace on request", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 5, 15));
    const day = 24 * 60 * 60;
    const plannedAt = new Date(2026, 5, 1).getTime() / 1000;
    const planned = (at: number) => ({ ...task(0, 1, "").milestones, plannedAt: at });
    const lastYear = new Date(2025, 5, 1).getTime() / 1000;
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo, herdr] });
    mocked.listTasks.mockResolvedValue({
      status: "ok",
      data: [
        task(5, 2, "Add herdr API", { status: "planned", dependsOn: [1], milestones: planned(plannedAt + day) }),
        task(1, 1, "Add storage", {
          status: "done",
          milestones: { ...planned(plannedAt), completedAt: plannedAt + 3 * day },
        }),
        task(2, 1, "Add UI", { status: "planned", dependsOn: [1], milestones: planned(plannedAt + day) }),
        task(3, 1, "Write docs", { status: "in_progress" }),
        task(4, 1, "Fix login", {
          status: "done",
          milestones: { ...planned(lastYear), completedAt: lastYear + day },
        }),
      ],
    });
    renderApp();

    await screen.findByRole("list", { name: "Tasks" });
    expect(screen.queryByRole("button", { name: "Timeline" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "All workspaces" }));

    const timeline = await screen.findByRole("region", { name: "Task timeline" });
    expect(screen.getByRole("button", { name: "Timeline" })).toHaveAttribute("aria-pressed", "true");
    expect(await within(timeline).findByText("Add herdr API")).toBeInTheDocument();
    expect(await within(timeline).findByText("Add UI")).toBeInTheDocument();
    expect(within(timeline).getByText("Add storage")).toBeInTheDocument();
    const unplaced = within(timeline).getByRole("complementary", { name: "Tasks without a planned date" });
    expect(within(unplaced).getByText("Write docs")).toBeInTheDocument();
    expect(within(timeline).getByRole("button", { name: "Week" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("list", { name: "Tasks" })).not.toBeInTheDocument();

    fireEvent.click(within(timeline).getByRole("button", { name: "Month" }));
    expect(within(timeline).getByRole("button", { name: "Month" })).toHaveAttribute("aria-pressed", "true");
    const period = within(timeline).getByRole("combobox", { name: "Timeline period" });
    expect(within(period).getAllByRole("option").map((option) => option.textContent)).toEqual([
      "All time",
      "2026",
      "2025",
    ]);
    fireEvent.change(period, { target: { value: "2025" } });
    expect(await within(timeline).findByText("Fix login")).toBeInTheDocument();
    expect(within(timeline).queryByText("Add UI")).not.toBeInTheDocument();
    expect(within(unplaced).getByText("Write docs")).toBeInTheDocument();
    fireEvent.change(period, { target: { value: "all" } });

    const filter = screen.getByRole("combobox", { name: "Timeline workspace" });
    expect(within(filter).getAllByRole("option").map((option) => option.textContent)).toEqual([
      "All workspaces",
      "herdr",
      "toodo",
    ]);
    expect(within(timeline).getByText("Add storage").closest("[data-dimmed]")).toBeNull();
    fireEvent.change(filter, { target: { value: "2" } });
    expect(await within(timeline).findByText("Add herdr API")).toBeInTheDocument();
    expect(within(timeline).queryByText("Add UI")).not.toBeInTheDocument();
    expect(within(timeline).queryByText("Write docs")).not.toBeInTheDocument();
    // The prerequisite in another workspace stays on the timeline, dimmed and labeled.
    const prerequisite = within(timeline).getByText("Add storage").closest("[data-dimmed]");
    expect(prerequisite).not.toBeNull();
    expect(within(prerequisite as HTMLElement).getByText("toodo")).toBeInTheDocument();
    expect(within(timeline).getByText("Add herdr API").closest("[data-dimmed]")).toBeNull();
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

  it("removes a workspace from the list only after confirming", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo, herdr] });
    mocked.listTasks.mockResolvedValue({
      status: "ok",
      data: [task(1, 1, "Add storage"), task(3, 2, "Fix ssh")],
    });
    mocked.deleteWorkspace
      .mockResolvedValueOnce({ status: "error", error: { kind: "internal", message: "database is locked" } })
      .mockResolvedValueOnce({ status: "ok", data: null });
    renderApp();

    expect(await screen.findByText("Add storage")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Remove toodo" }));
    expect(mocked.deleteWorkspace).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.queryByRole("button", { name: "Confirm remove toodo" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Remove toodo" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm remove toodo" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("database is locked");
    expect(screen.getByRole("region", { name: "Machine Local" })).toHaveTextContent("toodo");

    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [herdr] });
    fireEvent.click(screen.getByRole("button", { name: "Confirm remove toodo" }));

    expect(await screen.findByText("Fix ssh")).toBeInTheDocument();
    expect(mocked.deleteWorkspace).toHaveBeenLastCalledWith(1);
    expect(within(screen.getByRole("region", { name: "Machine Local" })).queryByRole("listitem")).toBeNull();
    expect(screen.getByRole("heading", { name: "herdr" })).toBeInTheDocument();
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

  it("follows herdr events: agent statuses of active tasks and task changes", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({
      status: "ok",
      data: [
        task(1, 1, "Add login", { status: "in_progress", herdrWorkspaceId: "w27" }),
        task(2, 1, "Add logout", { status: "done" }),
      ],
    });
    mocked.listAgentStatuses.mockResolvedValue([{ taskId: 1, status: "working" }]);
    renderApp();

    const tasks = await screen.findByRole("list", { name: "Tasks" });
    expect(await within(tasks).findByLabelText("Agent of #1")).toHaveTextContent("Agent working");

    const [[onAgentStatuses]] = mockedEvents.agentStatusesChanged.listen.mock.calls;
    const blocked: TaskAgentStatus[] = [
      { taskId: 1, status: "blocked" },
      { taskId: 2, status: "idle" },
    ];
    act(() => {
      onAgentStatuses({ event: "agent-statuses-changed", id: 1, payload: { statuses: blocked } });
    });
    await waitFor(() =>
      expect(within(tasks).getByLabelText("Agent of #1")).toHaveTextContent("Agent needs input"),
    );
    expect(within(tasks).queryByLabelText("Agent of #2")).not.toBeInTheDocument();

    const loads = mocked.listTasks.mock.calls.length;
    const [[onTasksChanged]] = mockedEvents.tasksChanged.listen.mock.calls;
    act(() => {
      onTasksChanged({ event: "tasks-changed", id: 2, payload: { workspaceId: 1 } });
    });
    await waitFor(() => expect(mocked.listTasks.mock.calls.length).toBeGreaterThan(loads));
  });

  it("restores the worktree of an active task that lost it", async () => {
    const lost = task(1, 1, "Add login", { status: "in_progress", branch: "feat/add-login-1" });
    const restored = { ...lost, worktreePath: "D:\\wt\\toodo\\feat-add-login-1", herdrWorkspaceId: "w31" };
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValueOnce({ status: "ok", data: [lost] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [restored] });
    mocked.restoreTaskWorktree.mockResolvedValue({ status: "ok", data: restored });
    renderApp();

    const restore = await screen.findByRole("button", { name: "Restore worktree of #1" });
    expect(screen.queryByRole("button", { name: "Open pull request for #1" })).not.toBeInTheDocument();
    await waitFor(() => expect(restore).toBeEnabled());
    fireEvent.click(restore);

    expect(await screen.findByRole("button", { name: "Open pull request for #1" })).toBeInTheDocument();
    expect(mocked.restoreTaskWorktree).toHaveBeenCalledWith(1);
    expect(screen.queryByRole("button", { name: "Restore worktree of #1" })).not.toBeInTheDocument();
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
    expect(screen.getByText("Planned")).toBeInTheDocument();
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
    // Once set, the root collapses into a summary line with the trimmed path.
    expect(screen.getByText("/srv/wt")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Change worktree root" })).toBeInTheDocument();
    expect(mocked.getMachineSettings).toHaveBeenCalledWith({ kind: "ssh", label: "devbox" });
    expect(mocked.saveMachineSettings).toHaveBeenCalledWith({
      machine: { kind: "ssh", label: "devbox" },
      worktreeRoot: "/srv/wt",
    });
  });

  it("changes the base branch of the workspace", async () => {
    let saved = toodo;
    mocked.listWorkspaces.mockImplementation(async () => ({ status: "ok", data: [saved] }));
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [] });
    mocked.updateWorkspace
      .mockResolvedValueOnce({ status: "error", error: { kind: "invalidInput", message: "default base branch must not be empty" } })
      .mockImplementation(async (workspace) => {
        saved = { ...workspace, defaultBaseBranch: workspace.defaultBaseBranch.trim() };
        return { status: "ok", data: saved };
      });
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: "Change base branch" }));
    const branch = screen.getByLabelText("Base branch", { selector: "input" });
    expect(branch).toHaveValue("main");
    const form = screen.getByRole("form", { name: "Base branch" });
    // Unchanged and blank drafts cannot be saved.
    expect(within(form).getByRole("button", { name: "Save" })).toBeDisabled();
    fireEvent.change(branch, { target: { value: "  " } });
    expect(within(form).getByRole("button", { name: "Save" })).toBeDisabled();

    fireEvent.change(branch, { target: { value: "dev" } });
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));
    expect(await within(form).findByRole("alert")).toHaveTextContent("default base branch must not be empty");

    fireEvent.click(within(form).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("form", { name: "Base branch" })).not.toBeInTheDocument());
    expect(screen.getByText("dev")).toBeInTheDocument();
    expect(mocked.updateWorkspace).toHaveBeenLastCalledWith({ ...toodo, defaultBaseBranch: "dev" });

    // Discarding keeps the saved branch.
    fireEvent.click(screen.getByRole("button", { name: "Change base branch" }));
    fireEvent.change(screen.getByLabelText("Base branch", { selector: "input" }), { target: { value: "main" } });
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(screen.getByText("dev")).toBeInTheDocument();
    expect(mocked.updateWorkspace).toHaveBeenCalledTimes(2);
  });

  it("locks the base branch while a task of the workspace is in review", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo, herdr] });
    mocked.listTasks.mockResolvedValue({
      status: "ok",
      data: [task(3, 1, "Login", { status: "in_review" }), task(4, 2, "Other", { status: "in_review" })],
    });
    renderApp();

    expect(await screen.findByText("Locked while #3 is in review")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Change base branch" })).toBeDisabled();
  });

  it("records commits, opens a pull request, and completes the task once it is merged", async () => {
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

    expect(screen.queryByRole("button", { name: "Complete #1" })).not.toBeInTheDocument();
    const inReview = { ...started, status: "in_review" as const, pullRequestUrl: pr };
    mocked.openPullRequest.mockResolvedValue({ status: "ok", data: inReview });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [inReview] });
    mocked.listCommitRecords.mockResolvedValue({ status: "ok", data: [first, second] });
    fireEvent.click(screen.getByRole("button", { name: "Open pull request for #1" }));

    expect(await screen.findByText(pr)).toBeInTheDocument();
    expect(screen.getByText("In review")).toBeInTheDocument();
    expect(await screen.findByText("Fix login redirect")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Push #1 to its pull request" })).toBeInTheDocument();
    expect(mocked.openPullRequest).toHaveBeenCalledWith(1);

    const done = { ...inReview, status: "done" as const };
    mocked.completeTask.mockResolvedValue({ status: "ok", data: done });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [done] });
    fireEvent.click(screen.getByRole("button", { name: "Complete #1" }));

    expect(await screen.findByText("Done")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Complete #1" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Push #1 to its pull request" })).not.toBeInTheDocument();
    expect(mocked.completeTask).toHaveBeenCalledWith(1);
  });

  it("shows why a pull request could not be opened", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({
      status: "ok",
      data: [task(1, 1, "Add login", { status: "in_progress", branch: "feat/add-login-1", worktreePath: "D:\\wt\\x" })],
    });
    mocked.openPullRequest.mockResolvedValue({
      status: "error",
      error: { kind: "rejected", message: "branch feat/add-login-1 has no commits that are not on main" },
    });
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: "Open pull request for #1" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("has no commits that are not on main");
    expect(screen.getByText("In progress")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open pull request for #1" })).toBeEnabled();
  });

  it("shows why a task could not be completed", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({
      status: "ok",
      data: [
        task(1, 1, "Add login", {
          status: "in_review",
          branch: "feat/add-login-1",
          worktreePath: "D:\\wt\\x",
          pullRequestUrl: "https://github.com/acme/toodo/pull/12",
        }),
      ],
    });
    mocked.completeTask.mockResolvedValue({
      status: "error",
      error: { kind: "rejected", message: "pull request https://github.com/acme/toodo/pull/12 is not merged yet" },
    });
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: "Complete #1" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("is not merged yet");
    expect(screen.getByText("In review")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Complete #1" })).toBeEnabled();
  });

  it("asks for a completion note on a done task and saves it", async () => {
    const done = task(1, 1, "Add login", {
      status: "done",
      branch: "feat/add-login-1",
      pullRequestUrl: "https://github.com/acme/toodo/pull/12",
      pullRequest: {
        number: 12,
        title: "Add login",
        additions: 120,
        deletions: 30,
        changedFiles: 4,
        mergeCommitSha: "abc123",
        reviewCount: 2,
        reviewerCount: 1,
        firstReviewedAt: 1700000000,
      },
    });
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [done] });
    mocked.listCommitRecords.mockResolvedValue({ status: "ok", data: [] });
    renderApp();

    expect(await screen.findByLabelText("#1 needs a completion note")).toHaveTextContent("Needs note");
    expect(screen.getByLabelText("Pull request of #1")).toHaveTextContent("#12, +120/-30 in 4 files, 2 reviews");
    fireEvent.click(screen.getByRole("button", { name: "Add completion note to #1" }));
    const save = screen.getByRole("button", { name: "Save note" });
    expect(save).toBeDisabled();

    const noted = { ...done, completionNote: "Cut sign-in time in half" };
    mocked.setCompletionNote.mockResolvedValue({ status: "ok", data: noted });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [noted] });
    fireEvent.change(screen.getByRole("textbox", { name: "Completion note for #1" }), {
      target: { value: "Cut sign-in time in half" },
    });
    fireEvent.click(save);

    expect(await screen.findByLabelText("Completion note of #1")).toHaveTextContent("Cut sign-in time in half");
    expect(screen.queryByLabelText("#1 needs a completion note")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Completion note for #1" })).not.toBeInTheDocument();
    expect(mocked.setCompletionNote).toHaveBeenCalledWith(1, "Cut sign-in time in half");
  });

  it("concludes research without merging and keeps its dependents blocked", async () => {
    const research = task(1, 1, "Try a new cache", {
      taskType: "rnd",
      status: "in_review",
      branch: "rnd/try-a-new-cache-1",
      worktreePath: "D:\\wt\\toodo\\rnd-try-a-new-cache-1",
      herdrWorkspaceId: "w1",
      pullRequestUrl: "https://github.com/acme/toodo/pull/12",
    });
    const dependent = task(2, 1, "Use the new cache", { dependsOn: [1] });
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [research, dependent] });
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: "Conclude #1 without merging" }));
    const conclude = screen.getByRole("button", { name: "Conclude without merge" });
    expect(conclude).toBeDisabled();

    const concluded: Task = {
      ...research,
      status: "done",
      resolution: "not_adopted",
      herdrWorkspaceId: null,
      completionNote: "Twice as slow as the current cache",
    };
    mocked.concludeTask.mockResolvedValue({ status: "ok", data: concluded });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [concluded, dependent] });
    fireEvent.change(screen.getByRole("textbox", { name: "Conclusion of #1" }), {
      target: { value: "  Twice as slow as the current cache " },
    });
    fireEvent.click(conclude);

    expect(await screen.findByLabelText("Completion note of #1")).toHaveTextContent(
      "Twice as slow as the current cache",
    );
    expect(mocked.concludeTask).toHaveBeenCalledWith(1, "Twice as slow as the current cache");
    const tasks = screen.getByRole("list", { name: "Tasks" });
    expect(within(tasks).getByText("Not adopted")).toBeInTheDocument();
    expect(screen.queryByLabelText("#1 needs a completion note")).not.toBeInTheDocument();
    expect(screen.getByRole("note", { name: "Blocked prerequisites of #2" })).toHaveTextContent(
      "#1 was not adopted",
    );
    expect(screen.getByRole("button", { name: "Start #2" })).toBeDisabled();
  });

  it("adds and removes task dependencies", async () => {
    const storage = task(1, 1, "Add storage");
    const ui = task(2, 1, "Add UI");
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [storage, ui] });
    mocked.addDependency.mockResolvedValue({ status: "ok", data: null });
    renderApp();

    const add = await screen.findByRole("combobox", { name: "Add dependency to #2" });
    expect(within(add).getByRole("option", { name: "#1 Add storage" })).toBeInTheDocument();
    expect(within(add).queryByRole("option", { name: "#2 Add UI" })).not.toBeInTheDocument();
    mocked.listTasks.mockResolvedValue({
      status: "ok",
      data: [storage, { ...ui, status: "planned", dependsOn: [1] }],
    });
    fireEvent.change(add, { target: { value: "1" } });

    const remove = await screen.findByRole("button", { name: "Remove dependency #1 from #2" });
    expect(mocked.addDependency).toHaveBeenCalledWith(2, 1);
    expect(screen.getByRole("button", { name: "Start #2" })).toBeDisabled();
    mocked.removeDependency.mockResolvedValue({ status: "ok", data: null });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [storage, ui] });
    fireEvent.click(remove);

    await waitFor(() =>
      expect(screen.queryByRole("button", { name: "Remove dependency #1 from #2" })).not.toBeInTheDocument(),
    );
    expect(mocked.removeDependency).toHaveBeenCalledWith(2, 1);
  });

  it("shows why a dependency was rejected", async () => {
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({
      status: "ok",
      data: [task(1, 1, "Add storage"), task(2, 1, "Add UI", { status: "planned", dependsOn: [1] })],
    });
    mocked.addDependency.mockResolvedValue({
      status: "error",
      error: { kind: "rejected", message: "dependency would create a cycle: #1 -> #2 -> #1" },
    });
    renderApp();

    fireEvent.change(await screen.findByRole("combobox", { name: "Add dependency to #1" }), {
      target: { value: "2" },
    });

    expect(await screen.findByRole("alert")).toHaveTextContent("would create a cycle");
    expect(mocked.addDependency).toHaveBeenCalledWith(1, 2);
    expect(screen.queryByRole("combobox", { name: "Add dependency to #2" })).not.toBeInTheDocument();
  });

  it("edits the type, title, and description of a task", async () => {
    const original = task(1, 1, "Add login", { description: "old" });
    const edited = { ...original, taskType: "fix" as const, title: "Add sign-in", description: "new" };
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [original] });
    mocked.updateTask.mockResolvedValue({ status: "ok", data: edited });
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: "Edit #1" }));
    const form = screen.getByRole("form", { name: "Edit task #1" });
    fireEvent.change(within(form).getByLabelText("Type of #1"), { target: { value: "fix" } });
    fireEvent.change(within(form).getByLabelText("Title of #1"), { target: { value: "  Add sign-in " } });
    fireEvent.change(within(form).getByLabelText("Description of #1"), { target: { value: " new " } });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [edited] });
    fireEvent.click(within(form).getByRole("button", { name: "Save" }));

    expect(await screen.findByText("Add sign-in")).toBeInTheDocument();
    expect(screen.queryByRole("form", { name: "Edit task #1" })).not.toBeInTheDocument();
    expect(mocked.updateTask).toHaveBeenCalledWith({
      id: 1,
      taskType: "fix",
      title: "Add sign-in",
      description: "new",
    });
  });

  it("pauses a started task, then resumes or deletes it", async () => {
    const started = task(1, 1, "Add login", {
      status: "in_progress",
      branch: "feat/add-login-1",
      worktreePath: "D:\\wt\\toodo\\feat-add-login-1",
      herdrWorkspaceId: "w27",
      milestones: { ...task(1, 1, "").milestones, plannedAt: 1760000000, startedAt: 1760100000 },
    });
    const paused: Task = {
      ...started,
      status: "planned",
      milestones: { ...started.milestones, pausedAt: 1760200000 },
    };
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [started] });
    mocked.pauseTask.mockResolvedValue({ status: "ok", data: paused });
    mocked.deleteTask.mockResolvedValue({ status: "ok", data: null });
    renderApp();

    const pause = await screen.findByRole("button", { name: "Pause #1" });
    expect(screen.queryByRole("button", { name: "Delete #1" })).not.toBeInTheDocument();
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [paused] });
    fireEvent.click(pause);

    expect(await screen.findByRole("button", { name: "Resume #1" })).toBeEnabled();
    expect(mocked.pauseTask).toHaveBeenCalledWith(1);
    expect(screen.getByText("Paused")).toBeInTheDocument();
    expect(screen.getByLabelText("Timeline of #1")).toHaveTextContent(`Paused ${formatDate(1760200000)}`);
    expect(screen.queryByRole("button", { name: "Pause #1" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Delete #1" }));
    expect(mocked.deleteTask).not.toHaveBeenCalled();
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [] });
    fireEvent.click(screen.getByRole("button", { name: "Confirm delete #1" }));

    expect(await screen.findByText("No tasks in this workspace yet.")).toBeInTheDocument();
    expect(mocked.deleteTask).toHaveBeenCalledWith(1, false);
  });

  it("deletes a task with its worktree, forcing it only after a rejection", async () => {
    const finished = task(1, 1, "Add login", {
      status: "done",
      branch: "feat/add-login-1",
      worktreePath: "D:\\wt\\toodo\\feat-add-login-1",
      herdrWorkspaceId: "w27",
    });
    mocked.listWorkspaces.mockResolvedValue({ status: "ok", data: [toodo] });
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [finished] });
    mocked.deleteTask
      .mockResolvedValueOnce({
        status: "error",
        error: { kind: "rejected", message: "the worktree of task 1 has modified or untracked files" },
      })
      .mockResolvedValueOnce({ status: "ok", data: null });
    renderApp();

    fireEvent.click(await screen.findByRole("button", { name: "Delete #1" }));
    expect(mocked.deleteTask).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: /Remove worktree/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Force delete #1" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Confirm delete #1" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("modified or untracked files");
    expect(mocked.deleteTask).toHaveBeenCalledWith(1, false);
    mocked.listTasks.mockResolvedValue({ status: "ok", data: [] });
    fireEvent.click(screen.getByRole("button", { name: "Force delete #1" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm force delete #1" }));

    expect(await screen.findByText("No tasks in this workspace yet.")).toBeInTheDocument();
    expect(mocked.deleteTask).toHaveBeenLastCalledWith(1, true);
  });
});
