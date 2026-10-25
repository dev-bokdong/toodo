import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FloatingChat, LeadChat } from "@/components/lead-chat";
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
  typing: false,
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

function renderFloating(lead: LeadStatus) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <FloatingChat lead={lead} />
    </QueryClientProvider>,
  );
  const rerender = (next: LeadStatus) =>
    view.rerender(
      <QueryClientProvider client={client}>
        <FloatingChat lead={next} />
      </QueryClientProvider>,
    );
  return { rerender };
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

  it("shows Dori's typing bubble only while the lead is typing, still when motion is reduced", async () => {
    const view = renderChat(running);
    const log = await screen.findByRole("log", { name: "Messages" });
    await within(log).findByText("Plan the release");
    expect(within(log).queryByRole("listitem", { name: "Reply is being written" })).toBeNull();

    view.rerender(
      <QueryClientProvider client={new QueryClient()}>
        <LeadChat lead={{ ...running, agentStatus: "working", typing: true }} />
      </QueryClientProvider>,
    );
    const bubble = await within(log).findByRole("listitem", { name: "Reply is being written" });
    expect(bubble.querySelector("img")).toHaveAttribute("src", expect.stringContaining("dori-avatar"));
    const dots = bubble.querySelectorAll("span");
    expect(dots).toHaveLength(3);
    for (const dot of dots) {
      expect(dot).toHaveClass("animate-bounce", "motion-reduce:animate-none");
    }
    // The bubble comes last, after the newest message.
    expect(log.lastElementChild).toBe(bubble);
  });

  it("does not send while the lead is not running", async () => {
    renderChat({ ...running, state: "restarting", error: "the agent exited right after it started" });

    expect(await screen.findByRole("status", { name: "Lead status" })).toHaveTextContent("Restarting");
    fireEvent.change(screen.getByRole("textbox", { name: "Message to the lead" }), { target: { value: "Hi" } });
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  });
});

describe("FloatingChat", () => {
  beforeAll(() => import("@/components/markdown"));

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  beforeEach(() => {
    localStorage.clear();
    mocked.listChatMessages.mockReset().mockResolvedValue({ status: "ok", data: history });
    mocked.sendChatMessage.mockReset();
  });

  it("opens over the view with the focus in the message box, and Escape or the button closes it", async () => {
    renderFloating(running);
    const button = screen.getByRole("button", { name: /^Lead chat/ });
    const panel = screen.getByRole("dialog", { hidden: true });
    expect(panel).not.toBeVisible();
    expect(button).toHaveAttribute("aria-expanded", "false");

    fireEvent.click(button);
    expect(screen.getByRole("dialog", { name: "Lead chat" })).toBeVisible();
    expect(button).toHaveAttribute("aria-expanded", "true");
    const input = screen.getByRole("textbox", { name: "Message to the lead" });
    expect(input).toHaveFocus();

    fireEvent.keyDown(input, { key: "Escape" });
    expect(panel).not.toBeVisible();
    expect(button).toHaveFocus();

    fireEvent.click(button);
    expect(input).toHaveFocus();
    fireEvent.click(button);
    expect(panel).not.toBeVisible();

    fireEvent.click(button);
    fireEvent.click(screen.getByRole("button", { name: "Close lead chat" }));
    expect(panel).not.toBeVisible();
    expect(button).toHaveFocus();
  });

  it("keeps an unsent message while closed", async () => {
    renderFloating(running);
    const button = screen.getByRole("button", { name: /^Lead chat/ });
    fireEvent.click(button);
    fireEvent.change(screen.getByRole("textbox", { name: "Message to the lead" }), { target: { value: "Half a thought" } });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(screen.getByRole("textbox", { name: "Message to the lead" })).toHaveValue("Half a thought");
  });

  it("counts unread replies on the button only while closed, and marks them read when opened", async () => {
    renderFloating(running);
    const button = await screen.findByRole("button", { name: "Lead chat, 1 unread" });
    expect(button.querySelector("[data-unread]")).toHaveTextContent("1");
    // Shown while closed is not read.
    expect(localStorage.getItem("toodo-chat-read")).toBeNull();

    fireEvent.click(button);
    await waitFor(() => expect(localStorage.getItem("toodo-chat-read")).toBe("2"));
    expect(button).toHaveAccessibleName("Lead chat");
    fireEvent.click(button);
    expect(button).toHaveAccessibleName("Lead chat");
    expect(button.querySelector("[data-unread]")).toBeNull();
  });

  it("closes the phone sheet on a swipe down from its grabber", () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => ({ matches: true, addEventListener() {}, removeEventListener() {} })),
    );
    renderFloating(running);
    const button = screen.getByRole("button", { name: /^Lead chat/ });
    fireEvent.click(button);
    const panel = screen.getByRole("dialog", { name: "Lead chat" });
    const grabber = panel.querySelector("[data-grabber]")!.parentElement!;
    fireEvent.pointerDown(grabber, { pointerId: 1, button: 0, clientY: 100 });
    fireEvent.pointerMove(grabber, { pointerId: 1, clientY: 250 });
    fireEvent.pointerUp(grabber, { pointerId: 1, clientY: 250 });
    expect(panel).not.toBeVisible();
    expect(button).toHaveFocus();
  });

  it("shows typing dots on the button while the lead composes a reply", async () => {
    const { rerender } = renderFloating(running);
    const button = screen.getByRole("button", { name: /^Lead chat/ });
    const mark = button.querySelector("[data-chat-mark]")!;
    expect(mark.querySelectorAll(".animate-bounce")).toHaveLength(0);

    rerender({ ...running, agentStatus: "working", typing: true });
    expect(button).toHaveAccessibleName(expect.stringContaining("Dori is typing"));
    expect(mark.querySelectorAll(".animate-bounce")).toHaveLength(3);
  });
});
