import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { LeadChat } from "@/components/lead-chat";
import { commands, type ChatMessage, type LeadStatus } from "@/lib/bindings";

vi.mock("@/lib/bindings", () => ({ commands: { listChatMessages: vi.fn(), sendChatMessage: vi.fn() } }));

const mocked = vi.mocked(commands);

const running: LeadStatus = {
  state: "running",
  workspaceId: 1,
  herdrWorkspaceId: "wL",
  paneId: "wL:p1",
  agent: "omo",
  agentStatus: "idle",
  restarts: 0,
  startedAt: 1,
  error: null,
};

const history: ChatMessage[] = [
  { id: 1, author: "user", body: "Plan the release", at: 1_790_000_000, deliveredAt: 1_790_000_000, seenAt: 1_790_000_005 },
  { id: 2, author: "lead", body: "Created **#4**.", at: 1_790_000_060, deliveredAt: null, seenAt: null },
];

function sent(id: number, body: string, seenAt: number | null = null): ChatMessage {
  return { id, author: "user", body, at: 1_790_000_120, deliveredAt: 1_790_000_120, seenAt };
}

function renderChat(lead: LeadStatus) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <LeadChat lead={lead} />
    </QueryClientProvider>,
  );
}

describe("LeadChat", () => {
  // Replies render through the lazily loaded Markdown component.
  beforeAll(() => import("@/components/markdown"));

  beforeEach(() => {
    localStorage.clear();
    mocked.listChatMessages.mockReset().mockResolvedValue({ status: "ok", data: history });
    mocked.sendChatMessage.mockReset();
  });

  it("shows the history, marks it read, and sends a message with Enter", async () => {
    mocked.sendChatMessage.mockResolvedValue({ status: "ok", data: sent(3, "Start #4") });
    renderChat(running);

    const log = await screen.findByRole("log", { name: "Messages" });
    expect(await within(log).findByText("Plan the release")).toBeInTheDocument();
    expect(await within(log).findByText("#4")).toBeInTheDocument();
    await waitFor(() => expect(localStorage.getItem("toodo-chat-read")).toBe("2"));

    const input = screen.getByRole("textbox", { name: "Message to the lead" });
    fireEvent.change(input, { target: { value: "Start #4" } });
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(input).toHaveValue(""));
    expect(mocked.sendChatMessage).toHaveBeenCalledTimes(1);
    const [text, clientId] = mocked.sendChatMessage.mock.calls[0];
    expect(text).toBe("Start #4");
    expect(clientId).not.toBe("");
    expect(mocked.listChatMessages).toHaveBeenCalledTimes(2);
  });

  it("keeps the text and resends it under the same client ID after a failure", async () => {
    mocked.sendChatMessage
      .mockResolvedValueOnce({ status: "error", error: { kind: "external", message: "herdr failed" } })
      .mockResolvedValueOnce({ status: "ok", data: sent(3, "Hi") });
    renderChat(running);

    const input = screen.getByRole("textbox", { name: "Message to the lead" });
    fireEvent.change(input, { target: { value: "Hi" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("herdr failed");
    expect(input).toHaveValue("Hi");

    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(input).toHaveValue(""));
    const [first, second] = mocked.sendChatMessage.mock.calls;
    expect(second[1]).toBe(first[1]);
  });

  it("shows replies as Dori's and how far each message got", async () => {
    let deliver: (reply: Awaited<ReturnType<typeof commands.sendChatMessage>>) => void = () => {};
    mocked.sendChatMessage.mockReturnValue(new Promise((resolve) => (deliver = resolve)));
    renderChat(running);

    const log = await screen.findByRole("log", { name: "Messages" });
    const reply = await within(log).findByRole("listitem", { name: "Dori" });
    expect(within(reply).getByText("Dori")).toBeInTheDocument();
    expect(reply.querySelector("img")).toHaveAttribute("src", expect.stringContaining("dori-avatar"));
    const first = within(log).getByRole("listitem", { name: "You" });
    expect(within(first).getByText("Seen")).toHaveAttribute("data-receipt", "seen");

    const input = screen.getByRole("textbox", { name: "Message to the lead" });
    fireEvent.change(input, { target: { value: "Start #4" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const pending = await within(log).findByText("Sent");
    expect(pending.closest("li")).toHaveTextContent("Start #4");

    // The refetch after sending does not answer: the sent message shows on its own.
    mocked.listChatMessages.mockReturnValue(new Promise(() => {}));
    deliver({ status: "ok", data: sent(3, "Start #4") });
    expect(await within(log).findByText("Delivered")).toHaveAttribute("data-receipt", "delivered");
    expect(within(log).queryByText("Sent")).toBeNull();
    expect(within(log).getAllByRole("listitem", { name: "You" })).toHaveLength(2);
  });

  it("does not send while the lead is not running", async () => {
    renderChat({ ...running, state: "restarting", error: "the agent exited right after it started" });

    expect(await screen.findByRole("status", { name: "Lead status" })).toHaveTextContent("Restarting");
    fireEvent.change(screen.getByRole("textbox", { name: "Message to the lead" }), { target: { value: "Hi" } });
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });
});
