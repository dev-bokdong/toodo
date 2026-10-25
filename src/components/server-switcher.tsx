import { useState } from "react";
import { Menu } from "@base-ui/react/menu";
import { ChevronsUpDown, Server, Settings } from "lucide-react";
import { buttonVariants } from "@/components/ui/button";
import { connectedServer } from "@/lib/platform";
import { serverName, switchServer, useConnection } from "@/lib/servers";
import { cn } from "@/lib/utils";

const itemClass =
  "flex cursor-default items-center gap-2 rounded-md px-2 py-1.5 outline-none select-none data-[disabled]:pointer-events-none data-[disabled]:opacity-50 data-[highlighted]:bg-muted max-md:min-h-11 [&_svg]:size-4 [&_svg]:shrink-0 [&_svg]:text-muted-foreground";

/**
 * The name of the server the desktop app shows, with a menu to switch to another saved
 * server. Shown only while the app is connected to a server.
 */
export function ServerSwitcher({ onManage, className }: { onManage: () => void; className?: string }) {
  const connection = useConnection();
  const [error, setError] = useState<string | null>(null);
  if (connectedServer === null) return null;
  const name = serverName(connection.data, connectedServer);
  const others = connection.data?.servers.filter((saved) => saved.url !== connectedServer) ?? [];

  async function use(url: string) {
    setError(null);
    try {
      await switchServer(url);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  return (
    <div className={cn("flex min-w-0 flex-col gap-1", className)}>
      <Menu.Root>
        <Menu.Trigger
          aria-label={`Server: ${name}. Switch server`}
          title={connectedServer}
          className={cn(buttonVariants({ variant: "outline", size: "sm" }), "min-w-0 justify-start")}
        >
          <Server />
          <span data-server-name className="min-w-0 truncate">
            {name}
          </span>
          <ChevronsUpDown className="ml-auto" />
        </Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner side="bottom" align="start" sideOffset={4} className="z-50">
            <Menu.Popup
              aria-label="Servers"
              className="flex min-w-48 flex-col rounded-lg border bg-popover p-1 text-sm text-popover-foreground shadow-lg outline-none"
            >
              {others.map((saved) => (
                <Menu.Item key={saved.url} title={saved.url} onClick={() => void use(saved.url)} className={itemClass}>
                  <Server />
                  Switch to {saved.name}
                </Menu.Item>
              ))}
              <Menu.Item onClick={onManage} className={itemClass}>
                <Settings />
                Manage servers
              </Menu.Item>
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
    </div>
  );
}
