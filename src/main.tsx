import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "@/App";
import { ServerAccess } from "@/components/server-access";
import "@/index.css";
import { runsOnServer } from "@/lib/platform";
import { initTheme } from "@/lib/theme";
import { onReconnect } from "@/web/event";

initTheme();

const queryClient = new QueryClient();

if (runsOnServer) {
  // Events sent while the stream was down are lost; refetch what they would have changed.
  onReconnect(() => void queryClient.invalidateQueries());
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      {runsOnServer ? (
        <ServerAccess>
          <App />
        </ServerAccess>
      ) : (
        <App />
      )}
    </QueryClientProvider>
  </React.StrictMode>,
);
