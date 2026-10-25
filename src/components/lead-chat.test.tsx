import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { FloatingChat, LeadChat } from "@/components/lead-chat";
import { commands, type ChatMessage, type LeadStatus } from "@/lib/bindings";
import { QUIET_MS, stopSpeaking } from "@/lib/voice";

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
  connected: false,
  lastContactAt: null,
};

/** No receipts of the lead's work. */
const open = { dueAt: null, ackedAt: null, handledAt: null, handledBy: null, overdue: false } as const;

const history: ChatMessage[] = [
  {
    id: 1,
    author: "user",
    body: "Plan the release",
    at: 1_790_000_000,
    deliveredAt: 1_790_000_000,
    seenAt: 1_790_000_005,
    ...open,
  },
  { id: 2, author: "lead", body: "Created **#4**.", at: 1_790_000_060, deliveredAt: null, seenAt: null, ...open },
];

function sent(id: number, body: string, seenAt: number | null = null): ChatMessage {
  return { id, author: "user", body, at: 1_790_000_120, deliveredAt: 1_790_000_120, seenAt, ...open };
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

  it("shows which messages the lead acknowledged, handled, or let wait too long", async () => {
    const acked = { ...sent(3, "Start #4", 1_790_000_125), ackedAt: 1_790_000_126, dueAt: 1_790_000_720 };
    const handled: ChatMessage = {
      ...sent(4, "What runs?", 1_790_000_125),
      ackedAt: 1_790_000_126,
      handledAt: 1_790_000_130,
      handledBy: { kind: "task", taskId: 7 },
    };
    const late: ChatMessage = { ...sent(5, "Any news?", 1_790_000_125), dueAt: 1_790_000_720, overdue: true };
    mocked.listChatMessages.mockResolvedValue({ status: "ok", data: [...history, acked, handled, late] });
    renderChat(running);

    const log = await screen.findByRole("log", { name: "Messages" });
    const row = async (text: string) => (await within(log).findByText(text)).closest("li")!;
    expect(within(await row("Start #4")).getByText("Received")).toHaveAttribute("data-receipt", "acked");
    const done = within(await row("What runs?")).getByText("Handled");
    expect(done).toHaveAttribute("data-receipt", "handled");
    expect(done.getAttribute("title")).toMatch(/^Handled by #7 /);
    const waiting = await row("Any news?");
    expect(within(waiting).getByText("Seen")).toBeInTheDocument();
    expect(within(waiting).getByText("Overdue")).toHaveAttribute("data-overdue");
    expect(within(log).getAllByText("Overdue")).toHaveLength(1);
    expect(screen.getByText("1 message waits too long")).toBeInTheDocument();
  });

  it("shows the server's finish reports apart from Dori's replies and the person's messages", async () => {
    const report: ChatMessage = {
      id: 3,
      author: "system",
      body: "**Merged:** #4 Release. Its [pull request #9](https://github.com/acme/toodo/pull/9) was merged.",
      at: 1_790_000_120,
      deliveredAt: null,
      seenAt: null,
      ...open,
    };
    mocked.listChatMessages.mockResolvedValue({ status: "ok", data: [...history, report] });
    renderChat(running);

    const log = await screen.findByRole("log", { name: "Messages" });
    const shown = await within(log).findByRole("listitem", { name: "Toodo" });
    expect(shown).toHaveAttribute("data-author", "system");
    expect(within(shown).getByRole("link", { name: "pull request #9" })).toHaveAttribute(
      "href",
      "https://github.com/acme/toodo/pull/9",
    );
    expect(shown.querySelector("img")).toBeNull();
    expect(shown.querySelector("[data-receipt]")).toBeNull();
    expect(within(log).getAllByRole("listitem", { name: "Dori" })).toHaveLength(1);
    expect(within(log).getAllByRole("listitem", { name: "You" })).toHaveLength(1);
    await waitFor(() => expect(localStorage.getItem("toodo-chat-read")).toBe("3"));
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

  it("sends a message for the lead to take once it runs again while it restarts", async () => {
    mocked.sendChatMessage.mockResolvedValue({ status: "ok", data: { ...sent(3, "Hi"), deliveredAt: null } });
    renderChat({ ...running, state: "restarting", error: "the agent exited right after it started" });

    expect(await screen.findByRole("status", { name: "Lead status" })).toHaveTextContent("Restarting");
    const input = screen.getByRole("textbox", { name: "Message to the lead" });
    expect(input).toHaveAttribute("placeholder", "Dori is not running yet; your message waits for it");
    fireEvent.change(input, { target: { value: "Hi" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(mocked.sendChatMessage).toHaveBeenCalledTimes(1));
    expect(mocked.sendChatMessage.mock.calls[0][0]).toBe("Hi");
  });

  it("does not send without a lead", async () => {
    renderChat({ ...running, state: "disabled" });

    expect(await screen.findByRole("status", { name: "Lead status" })).toHaveTextContent("Off");
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

/** A stand-in for the browser's speech recognition that the test drives by hand. */
class FakeRecognition {
  static last: FakeRecognition | null = null;
  lang = "";
  interimResults = false;
  continuous = false;
  maxAlternatives = 0;
  onresult: ((event: { results: { isFinal: boolean; 0: { transcript: string } }[] }) => void) | null = null;
  onerror: ((event: { error: string }) => void) | null = null;
  onend: (() => void) | null = null;
  start = vi.fn();
  stop = vi.fn(() => this.onend?.());
  abort = vi.fn(() => {
    this.onerror?.({ error: "aborted" });
    this.onend?.();
  });
  constructor() {
    FakeRecognition.last = this;
  }
  hear(...pieces: string[]) {
    this.onresult?.({ results: pieces.map((transcript) => ({ isFinal: false, 0: { transcript } })) });
  }
}

class FakeUtterance {
  lang = "";
  voice: unknown = null;
  onend: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public text: string) {}
}

function fakeSynthesis() {
  const queue: FakeUtterance[] = [];
  const synthesis = {
    speak: vi.fn((utterance: FakeUtterance) => queue.push(utterance)),
    cancel: vi.fn(() => queue.splice(0).forEach((utterance) => utterance.onerror?.())),
    getVoices: vi.fn(() => [{ lang: "ko-KR", name: "Korean" }]),
  };
  vi.stubGlobal("speechSynthesis", synthesis);
  vi.stubGlobal("SpeechSynthesisUtterance", FakeUtterance);
  return { synthesis, queue };
}

describe("LeadChat voice", () => {
  beforeAll(() => import("@/components/markdown"));

  beforeEach(() => {
    localStorage.clear();
    FakeRecognition.last = null;
    mocked.listChatMessages.mockReset().mockResolvedValue({ status: "ok", data: history });
    mocked.sendChatMessage.mockReset().mockResolvedValue({ status: "ok", data: sent(3, "sent") });
  });

  afterEach(() => {
    stopSpeaking();
    vi.unstubAllGlobals();
  });

  it("sends what is dictated after the typed text, in the chosen language", async () => {
    vi.stubGlobal("webkitSpeechRecognition", FakeRecognition);
    localStorage.setItem("toodo-speech-language", "ko-KR");
    renderChat(running);
    const input = screen.getByRole("textbox", { name: "Message to the lead" });
    fireEvent.change(input, { target: { value: "Please" } });

    fireEvent.click(screen.getByRole("button", { name: "Voice input" }));
    const recognition = FakeRecognition.last!;
    expect(recognition.start).toHaveBeenCalledTimes(1);
    expect(recognition.lang).toBe("ko-KR");
    expect(recognition.interimResults).toBe(true);
    expect(recognition.continuous).toBe(true);
    expect(screen.getByRole("status", { name: "Voice input" })).toHaveTextContent("Listening");

    act(() => recognition.hear("fix the", " login bug"));
    expect(input).toHaveValue("Please fix the login bug");
    expect(mocked.sendChatMessage).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Stop voice input and send" }));
    await waitFor(() => expect(mocked.sendChatMessage).toHaveBeenCalledTimes(1));
    expect(mocked.sendChatMessage.mock.calls[0][0]).toBe("Please fix the login bug");
    await waitFor(() => expect(input).toHaveValue(""));
    expect(screen.getByRole("button", { name: "Voice input" })).toBeInTheDocument();
  });

  it("stops listening by itself after a quiet spell, and sends", async () => {
    vi.stubGlobal("webkitSpeechRecognition", FakeRecognition);
    renderChat(running);
    fireEvent.click(screen.getByRole("button", { name: "Voice input" }));
    const recognition = FakeRecognition.last!;
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      act(() => recognition.hear("first part,"));
      act(() => vi.advanceTimersByTime(QUIET_MS - 100));
      act(() => recognition.hear("first part,", " and the rest"));
      act(() => vi.advanceTimersByTime(QUIET_MS - 100));
      expect(recognition.stop).not.toHaveBeenCalled();
      act(() => vi.advanceTimersByTime(100));
      expect(recognition.stop).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
    await waitFor(() => expect(mocked.sendChatMessage).toHaveBeenCalledTimes(1));
    expect(mocked.sendChatMessage.mock.calls[0][0]).toBe("first part, and the rest");
  });

  it("drops the dictation on Escape and keeps the chat open", async () => {
    vi.stubGlobal("webkitSpeechRecognition", FakeRecognition);
    renderFloating(running);
    fireEvent.click(screen.getByRole("button", { name: /^Lead chat/ }));
    const input = screen.getByRole("textbox", { name: "Message to the lead" });
    fireEvent.change(input, { target: { value: "Draft" } });
    fireEvent.click(screen.getByRole("button", { name: "Voice input" }));
    act(() => FakeRecognition.last!.hear("never mind"));
    expect(input).toHaveValue("Draft never mind");

    fireEvent.keyDown(input, { key: "Escape" });
    expect(FakeRecognition.last!.abort).toHaveBeenCalledTimes(1);
    expect(input).toHaveValue("Draft");
    expect(screen.getByRole("dialog", { name: "Lead chat" })).toBeVisible();
    expect(mocked.sendChatMessage).not.toHaveBeenCalled();
  });

  it("explains a refused microphone, and offers no voice input without speech recognition", async () => {
    vi.stubGlobal("webkitSpeechRecognition", FakeRecognition);
    const view = renderChat(running);
    fireEvent.click(screen.getByRole("button", { name: "Voice input" }));
    act(() => {
      FakeRecognition.last!.onerror?.({ error: "not-allowed" });
      FakeRecognition.last!.onend?.();
    });
    expect(screen.getByRole("alert")).toHaveTextContent("Microphone access is blocked");
    expect(mocked.sendChatMessage).not.toHaveBeenCalled();
    view.unmount();

    vi.unstubAllGlobals();
    renderChat(running);
    await screen.findByRole("log", { name: "Messages" });
    expect(screen.queryByRole("button", { name: "Voice input" })).toBeNull();
  });

  it("reads new replies aloud only when turned on, and stops on request", async () => {
    const { synthesis, queue } = fakeSynthesis();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <LeadChat lead={running} />
      </QueryClientProvider>,
    );
    const log = await screen.findByRole("log", { name: "Messages" });
    await within(log).findByText("Plan the release");
    const toggle = screen.getByRole("button", { name: "Read replies aloud" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");

    const reply = (id: number, body: string): ChatMessage => ({
      id,
      author: "lead",
      body,
      at: 1_790_000_200,
      deliveredAt: null,
      seenAt: null,
      ...open,
    });
    mocked.listChatMessages.mockResolvedValue({ status: "ok", data: [...history, reply(3, "Quiet one.")] });
    await client.invalidateQueries();
    await within(log).findByText("Quiet one.");
    expect(synthesis.speak).not.toHaveBeenCalled();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(localStorage.getItem("toodo-read-aloud")).toBe("on");
    // The history there when it was turned on stays unread.
    expect(synthesis.speak).not.toHaveBeenCalled();

    mocked.listChatMessages.mockResolvedValue({
      status: "ok",
      data: [...history, reply(3, "Quiet one."), reply(4, "Opened **#7**: see [the PR](https://example.com/pr/7).")],
    });
    await client.invalidateQueries();
    await waitFor(() => expect(synthesis.speak).toHaveBeenCalledTimes(1));
    expect(queue[0].text).toBe("Opened #7: see the PR.");
    expect(queue[0].lang).not.toBe("");

    fireEvent.click(screen.getByRole("button", { name: "Stop reading aloud" }));
    expect(synthesis.cancel).toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Stop reading aloud" })).toBeNull();
  });

  it("offers a stop button over the closed chat while a reply is read aloud", async () => {
    const { synthesis } = fakeSynthesis();
    localStorage.setItem("toodo-read-aloud", "on");
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <FloatingChat lead={running} />
      </QueryClientProvider>,
    );
    await screen.findByRole("button", { name: "Lead chat, 1 unread" });
    expect(synthesis.speak).not.toHaveBeenCalled();

    mocked.listChatMessages.mockResolvedValue({
      status: "ok",
      data: [...history, { id: 3, author: "lead", body: "Done.", at: 1_790_000_200, deliveredAt: null, seenAt: null, ...open }],
    });
    await client.invalidateQueries();
    const stop = await screen.findByRole("button", { name: "Stop reading aloud" });
    expect(synthesis.speak).toHaveBeenCalledTimes(1);

    fireEvent.click(stop);
    expect(synthesis.cancel).toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Stop reading aloud" })).toBeNull();
  });
});
