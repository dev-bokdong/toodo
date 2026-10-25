import { useEffect, useRef, useState } from "react";
import { BookMarked, ChartColumn, FolderPlus, Layers, ListTodo, Menu, Settings, X } from "lucide-react";
import { AgentAccessStatus } from "@/components/agent-access-panel";
import { HerdrBanner } from "@/components/herdr-banner";
import { InstallBanner } from "@/components/install-banner";
import { MemoryPanel, type MemoryDraft } from "@/components/memory";
import { NeedsAttention } from "@/components/needs-attention";
import { Review } from "@/components/review";
import { EmptyState, TaskList, type TaskDraft, type ShownTask } from "@/components/task-list";
import { SettingsDialog, type SettingsSection } from "@/components/settings-dialog";
import { Button } from "@/components/ui/button";
import { WorkspaceList } from "@/components/workspace-list";
import type { TaskId, WorkspaceId } from "@/lib/bindings";
import { keepFocusIn } from "@/lib/focus-trap";
import { useHerdrEvents, useWorkspaces } from "@/lib/queries";
import type { TimelineScale } from "@/lib/task-graph";
import { cn } from "@/lib/utils";
import { listenForOpenTask } from "@/web/push";

export default function App() {
  const workspaces = useWorkspaces();
  useHerdrEvents();
  const [selectedId, setSelectedId] = useState<WorkspaceId | "all">("all");
  const [reviewing, setReviewing] = useState(false);
  const [sharedMemory, setSharedMemory] = useState(false);
  // Whether a workspace shows its tasks or its memory; kept across workspaces.
  const [workspaceView, setWorkspaceView] = useState<"tasks" | "memory">("tasks");
  const openPage = (page: "review" | "memory" | null) => {
    setReviewing(page === "review");
    setSharedMemory(page === "memory");
    setMenuOpen(false);
  };
  // On narrow screens the sidebar is a modal drawer over the content.
  const [menuOpen, setMenuOpen] = useState(false);
  const openMenuButton = useRef<HTMLButtonElement>(null);
  const closeMenuButton = useRef<HTMLButtonElement>(null);
  const menuWasOpen = useRef(false);
  const drawer = useRef<HTMLElement>(null);
  // Settings take over the screen and the focus, also when opened from the drawer.
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection | null>(null);
  const settingsButton = useRef<HTMLButtonElement>(null);
  // Opened from the drawer, which closes, they give the focus back to the menu button.
  const settingsFromDrawer = useRef(false);
  const showSettings = (section: SettingsSection | null) => {
    if (!settingsOpen) settingsFromDrawer.current = menuOpen;
    setSettingsSection(section);
    setSettingsOpen(true);
    setMenuOpen(false);
  };
  useEffect(() => {
    if (menuOpen) closeMenuButton.current?.focus();
    else if (menuWasOpen.current && !settingsOpen) openMenuButton.current?.focus();
    menuWasOpen.current = menuOpen;
    if (!menuOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      // An inline form in the drawer handles its own Escape first.
      if (event.key === "Escape" && !event.defaultPrevented) setMenuOpen(false);
      if (event.key === "Tab") keepFocusIn(drawer.current, event);
    };
    // Past the md breakpoint (48rem) the sidebar is no drawer, so nothing may stay inert.
    const onResize = () => {
      if (window.innerWidth >= 768) setMenuOpen(false);
    };
    document.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", onResize);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", onResize);
    };
  }, [menuOpen]);
  // Kept here so switching workspaces or views does not reset them.
  const [timelineScale, setTimelineScale] = useState<TimelineScale>("week");
  const [drafts, setDrafts] = useState<ReadonlyMap<WorkspaceId, TaskDraft>>(new Map());
  const setDraft = (id: WorkspaceId, draft: TaskDraft) => setDrafts((current) => new Map(current).set(id, draft));
  // Keyed by workspace, or null for shared memory.
  const [memoryDrafts, setMemoryDrafts] = useState<ReadonlyMap<WorkspaceId | null, MemoryDraft>>(new Map());
  const setMemoryDraft = (id: WorkspaceId | null, draft: MemoryDraft) =>
    setMemoryDrafts((current) => new Map(current).set(id, draft));
  // A task to select on the All workspaces timeline, until the timeline shows it.
  const [shownTask, setShownTask] = useState<ShownTask | null>(null);
  const showTask = (id: TaskId) => {
    setSelectedId("all");
    openPage(null);
    setShownTask({ id });
  };
  // A clicked notification opens its task.
  useEffect(() => listenForOpenTask(showTask), []);
  const onPage = reviewing || sharedMemory;
  const showingAll = selectedId === "all" && !onPage;
  const selected =
    workspaces.data?.find((workspace) => workspace.id === selectedId) ?? workspaces.data?.[0];

  return (
    <div className="flex h-screen bg-background text-foreground">
      {menuOpen && (
        <div aria-hidden className="fixed inset-0 z-30 bg-black/40 md:hidden" onClick={() => setMenuOpen(false)} />
      )}
      <aside
        ref={drawer}
        inert={settingsOpen}
        role={menuOpen ? "dialog" : undefined}
        aria-modal={menuOpen || undefined}
        aria-label={menuOpen ? "Menu" : undefined}
        className={cn(
          "flex min-h-0 w-64 shrink-0 flex-col gap-4 overflow-y-auto border-r bg-sidebar p-3 text-sidebar-foreground",
          "max-md:fixed max-md:inset-y-0 max-md:left-0 max-md:z-40 max-md:shadow-lg",
          !menuOpen && "max-md:hidden",
        )}
      >
        <div className="flex items-center gap-2">
          <h1 className="flex items-center gap-2 px-1 text-lg font-semibold tracking-tight">
            <span
              aria-hidden
              className="flex size-6 items-center justify-center rounded-md bg-primary text-primary-foreground"
            >
              <ListTodo className="size-4" />
            </span>
            Toodo
          </h1>
          <Button
            variant="ghost"
            size="icon-sm"
            className="ml-auto md:hidden"
            aria-label="Close menu"
            ref={closeMenuButton}
            onClick={() => setMenuOpen(false)}
          >
            <X />
          </Button>
        </div>
        <NeedsAttention onShow={showTask} />
        <nav className="flex flex-col gap-0.5">
          <Button
            variant={reviewing ? "secondary" : "ghost"}
            className="justify-start"
            aria-pressed={reviewing}
            onClick={() => openPage("review")}
          >
            <ChartColumn />
            Review
          </Button>
          <Button
            variant={sharedMemory ? "secondary" : "ghost"}
            className="justify-start"
            aria-pressed={sharedMemory}
            onClick={() => openPage("memory")}
          >
            <BookMarked />
            Shared memory
          </Button>
          <Button
            variant={showingAll ? "secondary" : "ghost"}
            className="justify-start"
            aria-pressed={showingAll}
            disabled={!workspaces.data?.length}
            onClick={() => {
              setSelectedId("all");
              openPage(null);
            }}
          >
            <Layers />
            All workspaces
          </Button>
        </nav>
        <WorkspaceList
          selectedId={onPage || showingAll ? null : (selected?.id ?? null)}
          onSelect={(id) => {
            setSelectedId(id);
            openPage(null);
          }}
        />
        <Button
          variant="ghost"
          className="mt-auto shrink-0 justify-start"
          aria-haspopup="dialog"
          aria-expanded={settingsOpen}
          ref={settingsButton}
          onClick={() => showSettings(null)}
        >
          <Settings />
          Settings
          <span className="ml-auto text-xs font-normal">
            <AgentAccessStatus />
          </span>
        </Button>
      </aside>
      <div className="flex min-w-0 flex-1 flex-col" inert={menuOpen || settingsOpen}>
        <header className="flex items-center gap-2 border-b px-2 py-1.5 md:hidden">
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Open menu"
            ref={openMenuButton}
            onClick={() => setMenuOpen(true)}
          >
            <Menu />
          </Button>
          <span className="font-semibold tracking-tight">Toodo</span>
        </header>
        <main className="relative flex min-w-0 flex-1 flex-col overflow-y-auto px-4 py-4 md:px-8 md:py-6">
          <InstallBanner />
          <HerdrBanner />
          {reviewing ? (
            <Review onShowTask={showTask} />
          ) : sharedMemory ? (
            <MemoryPanel
              workspace={null}
              draft={memoryDrafts.get(null)}
              onDraftChange={(draft) => setMemoryDraft(null, draft)}
            />
          ) : showingAll && workspaces.data?.length ? (
            <TaskList
              key="all"
              workspace={null}
              timelineScale={timelineScale}
              onTimelineScaleChange={setTimelineScale}
              shownTask={shownTask}
              onTaskShown={() => setShownTask(null)}
            />
          ) : selected ? (
            <div className="flex flex-1 flex-col gap-4">
              <div role="group" aria-label="Workspace view" className="flex gap-1 max-md:gap-2">
                {(["tasks", "memory"] as const).map((view) => (
                  <Button
                    key={view}
                    size="sm"
                    variant={workspaceView === view ? "secondary" : "ghost"}
                    aria-pressed={workspaceView === view}
                    onClick={() => setWorkspaceView(view)}
                  >
                    {view === "tasks" ? <ListTodo /> : <BookMarked />}
                    {view === "tasks" ? "Tasks" : "Memory"}
                  </Button>
                ))}
              </div>
              {workspaceView === "memory" ? (
                <MemoryPanel
                  key={selected.id}
                  workspace={selected}
                  draft={memoryDrafts.get(selected.id)}
                  onDraftChange={(draft) => setMemoryDraft(selected.id, draft)}
                />
              ) : (
                <TaskList
                  key={selected.id}
                  workspace={selected}
                  timelineScale={timelineScale}
                  onTimelineScaleChange={setTimelineScale}
                  draft={drafts.get(selected.id)}
                  onDraftChange={(draft) => setDraft(selected.id, draft)}
                />
              )}
            </div>
          ) : (
            // A failed load is explained, with a retry, in the sidebar's workspace list.
            !workspaces.isPending && !workspaces.isError && (
              <EmptyState
                icon={<FolderPlus />}
                title="Create a workspace to start planning tasks."
                hint={`Clone a repository, open one already cloned, or make a folder a new repository, on the server or a saved herdr machine.`}
              />
            )
          )}
        </main>
      </div>
      <SettingsDialog
        open={settingsOpen}
        section={settingsSection}
        onShow={showSettings}
        onClose={() => setSettingsOpen(false)}
        returnFocus={() => (settingsFromDrawer.current ? openMenuButton.current : settingsButton.current)}
      />
    </div>
  );
}
