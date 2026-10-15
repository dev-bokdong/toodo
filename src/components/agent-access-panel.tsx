import { useState } from "react";
import { Button } from "@/components/ui/button";
import type { McpServerInfo, McpTunnel, McpTunnelState } from "@/lib/bindings";
import { useMcpServerInfo, useMcpTunnels } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { toneFill } from "@/lib/status-styles";

export function mcpConfig(info: McpServerInfo): string {
  const server = {
    type: "http",
    url: info.url,
    headers: { Authorization: `Bearer ${info.token ?? ""}` },
  };
  return JSON.stringify({ mcpServers: { toodo: server } }, null, 2);
}

const tunnelStates: Record<McpTunnelState, { label: string; className: string }> = {
  open: { label: "Connected", className: toneFill.success },
  connecting: { label: "Connecting", className: toneFill.attention },
  retrying: { label: "Retrying", className: "bg-destructive" },
};

function TunnelList({ tunnels }: { tunnels: McpTunnel[] }) {
  if (tunnels.length === 0) return null;
  return (
    <ul aria-label="Remote machines" className="flex flex-col gap-1">
      {tunnels.map((tunnel) => {
        const state = tunnelStates[tunnel.state];
        return (
          <li key={tunnel.machine} aria-label={`Tunnel to ${tunnel.machine}`} className="flex flex-col gap-0.5">
            <span className="flex items-center gap-1.5">
              <span aria-hidden className={cn("size-1.5 shrink-0 rounded-full", state.className)} />
              <span className="truncate font-medium">{tunnel.machine}</span>
              <span className="text-muted-foreground">{state.label}</span>
            </span>
            {tunnel.error && <span className="break-words text-destructive">{tunnel.error}</span>}
          </li>
        );
      })}
    </ul>
  );
}

export function AgentAccessPanel() {
  const info = useMcpServerInfo();
  const tunnels = useMcpTunnels();
  const [notice, setNotice] = useState<string | null>(null);

  async function copy(text: string, what: string) {
    try {
      await navigator.clipboard.writeText(text);
      setNotice(`Copied ${what}.`);
    } catch (error) {
      setNotice(`Could not copy: ${String(error)}`);
    }
  }

  const data = info.data;

  return (
    <details className="group rounded-lg border p-2">
      <summary className="cursor-pointer text-xs font-medium uppercase text-muted-foreground">Agent access</summary>
      <section aria-label="Agent access" className="mt-2 flex flex-col gap-2 text-xs">
        <p className="text-muted-foreground">
          Agents can plan tasks and connect dependencies through Toodo&apos;s MCP server while the app is running.
        </p>
        {data && (
          <>
            <code className="break-all rounded bg-muted px-1.5 py-1" aria-label="MCP server URL">
              {data.url}
            </code>
            {data.error ? (
              <p role="alert" className="text-destructive">
                MCP server is not running: {data.error}
              </p>
            ) : (
              data.token && (
                <div className="flex flex-wrap gap-1.5">
                  <Button size="xs" onClick={() => void copy(mcpConfig(data), "the MCP config")}>
                    Copy MCP config
                  </Button>
                  <Button size="xs" variant="outline" onClick={() => void copy(data.token ?? "", "the access token")}>
                    Copy token
                  </Button>
                </div>
              )
            )}
            {!data.error && (
              <>
                <p className="text-muted-foreground">
                  Agents on SSH machines use the same config: Toodo forwards the port to each machine with a workspace.
                </p>
                <TunnelList tunnels={tunnels.data ?? []} />
              </>
            )}
          </>
        )}
        {notice && <p role="status">{notice}</p>}
      </section>
    </details>
  );
}
