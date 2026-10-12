import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { TaskList } from "@/components/task-list";
import { WorkspaceList } from "@/components/workspace-list";
import { commands, type WorkspaceId } from "@/lib/bindings";
import { useWorkspaces } from "@/lib/queries";

export default function App() {
  const appInfo = useQuery({
    queryKey: ["app-info"],
    queryFn: () => commands.appInfo(),
  });
  const workspaces = useWorkspaces();
  const [selectedId, setSelectedId] = useState<WorkspaceId | null>(null);
  const selected =
    workspaces.data?.find((workspace) => workspace.id === selectedId) ?? workspaces.data?.[0];

  return (
    <div className="flex h-screen bg-background text-foreground">
      <aside className="flex w-64 shrink-0 flex-col gap-4 border-r bg-sidebar p-3 text-sidebar-foreground">
        <h1 className="px-1 text-lg font-semibold tracking-tight">Toodo</h1>
        <WorkspaceList selectedId={selected?.id ?? null} onSelect={setSelectedId} />
        <p className="px-1 text-xs text-muted-foreground">
          {appInfo.data ? `v${appInfo.data.version}` : "Connecting to backend..."}
        </p>
      </aside>
      <main className="min-w-0 flex-1 overflow-y-auto p-6">
        {selected ? (
          <TaskList key={selected.id} workspace={selected} />
        ) : (
          <p className="text-sm text-muted-foreground">
            {workspaces.isPending ? "" : "Create a workspace to start planning tasks."}
          </p>
        )}
      </main>
    </div>
  );
}
