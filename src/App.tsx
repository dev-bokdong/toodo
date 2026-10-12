import { useQuery } from "@tanstack/react-query";
import { commands } from "@/lib/bindings";

export default function App() {
  const appInfo = useQuery({
    queryKey: ["app-info"],
    queryFn: () => commands.appInfo(),
  });

  return (
    <main className="flex min-h-screen flex-col items-center justify-center gap-2 bg-background text-foreground">
      <h1 className="text-3xl font-semibold tracking-tight">Toodo</h1>
      <p className="text-sm text-muted-foreground">
        {appInfo.data ? `v${appInfo.data.version}` : "Connecting to backend..."}
      </p>
    </main>
  );
}
