import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import App from "@/App";

vi.mock("@/lib/bindings", () => ({
  commands: {
    appInfo: vi.fn().mockResolvedValue({ name: "toodo", version: "1.2.3" }),
  },
}));

describe("App", () => {
  it("shows the backend version once the app info command resolves", async () => {
    const queryClient = new QueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>,
    );

    expect(screen.getByRole("heading", { name: "Toodo" })).toBeInTheDocument();
    expect(await screen.findByText("v1.2.3")).toBeInTheDocument();
  });
});
