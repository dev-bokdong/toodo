import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

describe("ConnectionGate", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("covers the app while the server is unreachable and uncovers it once it answers", async () => {
    vi.resetModules();
    const { serverFetch } = await import("@/web/connection");
    const { ConnectionGate } = await import("@/components/offline-screen");
    let up = false;
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        if (!up) throw new TypeError("Failed to fetch");
        return new Response(null, { status: 401 });
      }),
    );
    render(
      <ConnectionGate>
        <input aria-label="Draft" defaultValue="typed before the outage" />
      </ConnectionGate>,
    );

    await act(async () => {
      await serverFetch("/api/rpc/list_tasks").catch(() => {});
    });

    const dialog = await screen.findByRole("alertdialog", { name: "Can't reach the Toodo server" });
    expect(screen.getByLabelText("Draft").closest("[inert]")).not.toBeNull();

    up = true;
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));

    await vi.waitFor(() => expect(dialog).not.toBeInTheDocument());
    expect(screen.getByLabelText("Draft")).toHaveValue("typed before the outage");
    expect(screen.getByLabelText("Draft").closest("[inert]")).toBeNull();
  });
});
