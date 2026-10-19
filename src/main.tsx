import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React, { type ReactNode } from "react";
import ReactDOM from "react-dom/client";
import App from "@/App";
import "@/index.css";
import { initTheme } from "@/lib/theme";

initTheme();

const queryClient = new QueryClient();

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);

function render(app: ReactNode) {
  root.render(
    <React.StrictMode>
      <QueryClientProvider client={queryClient}>{app}</QueryClientProvider>
    </React.StrictMode>,
  );
}

// A literal condition, so the desktop build drops the web build's access form and event stream.
if (import.meta.env.MODE === "web") {
  void Promise.all([import("@/components/server-access"), import("@/web/event")]).then(
    ([{ ServerAccess }, { onReconnect }]) => {
      // Events sent while the stream was down are lost; refetch what they would have changed.
      onReconnect(() => void queryClient.invalidateQueries());
      render(
        <ServerAccess>
          <App />
        </ServerAccess>,
      );
    },
  );
} else {
  render(<App />);
}
