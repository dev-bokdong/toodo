import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChartColumn, FolderPlus, Layers, ListTodo } from "lucide-react";
import { BackupPanel } from "@/components/backup-panel";
import { HerdrBanner } from "@/components/herdr-banner";
import { Review } from "@/components/review";
import { EmptyState, TaskList } from "@/components/task-list";
import { ThemeToggle } from "@/components/theme-toggle";
import { Button } from "@/components/ui/button";
import { UpdateBanner } from "@/components/update-banner";
import { WorkspaceList } from "@/components/workspace-list";
import { commands, type WorkspaceId } from "@/lib/bindings";
import { useHerdrEvents, useWorkspaces } from "@/lib/queries";

export default function App() {
  const appInfo = useQuery({
    queryKey: ["app-info"],
    queryFn: () => commands.appInfo(),
  });
  const workspaces = useWorkspaces();
  useHerdrEvents();
  const [selectedId, setSelectedId] = useState<WorkspaceId | "all" | null>(null);
  const [reviewing, setReviewing] = useState(false);
  const showingAll = selectedId === "all" && !reviewing;
  const selected =
    workspaces.data?.find((workspace) => workspace.id === selectedId) ?? workspaces.data?.[0];

  return (
    <div className="flex h-screen bg-background text-foreground">
      <aside className="flex w-64 shrink-0 flex-col gap-4 border-r bg-sidebar p-3 text-sidebar-foreground">
        <h1 className="flex items-center gap-2 px-1 text-lg font-semibold tracking-tight">
          <span
            aria-hidden
            className="flex size-6 items-center justify-center rounded-md bg-primary text-primary-foreground"
          >
            <ListTodo className="size-4" />
          </span>
          Toodo
        </h1>
        <nav className="flex flex-col gap-0.5">
          <Button
            variant={reviewing ? "secondary" : "ghost"}
            className="justify-start"
            aria-pressed={reviewing}
            onClick={() => setReviewing(true)}
          >
            <ChartColumn />
            Review
          </Button>
          <Button
            variant={showingAll ? "secondary" : "ghost"}
            className="justify-start"
            aria-pressed={showingAll}
            disabled={!workspaces.data?.length}
            onClick={() => {
              setSelectedId("all");
              setReviewing(false);
            }}
          >
            <Layers />
            All workspaces
          </Button>
        </nav>
        <WorkspaceList
          selectedId={reviewing || showingAll ? null : (selected?.id ?? null)}
          onSelect={(id) => {
            setSelectedId(id);
            setReviewing(false);
          }}
        />
        <div className="mt-auto">
          <BackupPanel />
        </div>
        <div className="flex items-center justify-between gap-2 px-1">
          <p className="text-xs text-muted-foreground">
            {appInfo.data ? `v${appInfo.data.version}` : "Connecting to backend..."}
          </p>
          <ThemeToggle />
        </div>
      </aside>
      <div className="flex min-w-0 flex-1 flex-col">
        <main className="relative min-w-0 flex-1 overflow-y-auto px-8 py-6">
          <UpdateBanner />
          <HerdrBanner />
          {reviewing ? (
            <Review />
          ) : showingAll && workspaces.data?.length ? (
            <TaskList key="all" workspace={null} />
          ) : selected ? (
            <TaskList key={selected.id} workspace={selected} />
          ) : (
            !workspaces.isPending && (
              <EmptyState
                icon={<FolderPlus />}
                title="Create a workspace to start planning tasks."
                hint="Clone a repository or open one already cloned, on this computer or a saved herdr machine."
              />
            )
          )}
        </main>
      </div>
    </div>
  );
}
