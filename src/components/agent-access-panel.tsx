import { useState } from "react";
import { Button } from "@/components/ui/button";
import type { McpServerInfo } from "@/lib/bindings";
import { useMcpServerInfo } from "@/lib/queries";

export function mcpConfig(info: McpServerInfo): string {
  const server = {
    type: "http",
    url: info.url,
    headers: { Authorization: `Bearer ${info.token ?? ""}` },
  };
  return JSON.stringify({ mcpServers: { toodo: server } }, null, 2);
}

export function AgentAccessPanel() {
  const info = useMcpServerInfo();
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
          </>
        )}
        {notice && <p role="status">{notice}</p>}
      </section>
    </details>
  );
}
