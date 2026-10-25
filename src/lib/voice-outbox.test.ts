import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { commands, type ChatMessage } from "@/lib/bindings";

vi.mock("@/lib/bindings", () => ({ commands: { sendChatMessage: vi.fn() } }));

const send = vi.mocked(commands.sendChatMessage);

function sent(id: number, body: string): ChatMessage {
  return {
    id,
    author: "user",
    body,
    at: 1_000,
    deliveredAt: null,
    seenAt: null,
    dueAt: null,
    ackedAt: null,
    handledAt: null,
    handledBy: null,
    overdue: false,
  };
}

// The outbox is a module-level queue; each test starts with a fresh one.
async function outbox() {
  vi.resetModules();
  return import("@/lib/voice-outbox");
}

async function queueOf(module: Awaited<ReturnType<typeof outbox>>) {
  const { renderHook } = await import("@testing-library/react");
  return renderHook(() => module.useVoiceOutbox());
}

describe("voice outbox", () => {
  beforeEach(() => {
    send.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("sends a message once, and queues one said while the first is still sending", async () => {
    let deliver: (value: Awaited<ReturnType<typeof commands.sendChatMessage>>) => void = () => {};
    send.mockReturnValueOnce(new Promise((resolve) => (deliver = resolve)));
    send.mockResolvedValueOnce({ status: "ok", data: sent(2, "second") });
    const module = await outbox();

    module.sendVoiceMessage("  first  ", "id-1");
    module.sendVoiceMessage("second", "id-2");
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenLastCalledWith("first", "id-1");

    deliver({ status: "ok", data: sent(1, "first") });
    await vi.waitFor(() => expect(send).toHaveBeenCalledTimes(2));
    expect(send).toHaveBeenLastCalledWith("second", "id-2");
    const queue = await queueOf(module);
    await vi.waitFor(() => expect(queue.result.current).toEqual([]));
  });

  it("sends nothing for blank text or a client ID already queued", async () => {
    send.mockReturnValue(new Promise(() => {}));
    const module = await outbox();
    module.sendVoiceMessage("   ");
    module.sendVoiceMessage("hello", "id-1");
    module.sendVoiceMessage("hello again", "id-1");
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("keeps a failed message and retries it under the same client ID", async () => {
    vi.useFakeTimers();
    send
      .mockResolvedValueOnce({ status: "error", error: { kind: "external", message: "offline" } })
      .mockRejectedValueOnce(new Error("still offline"))
      .mockResolvedValueOnce({ status: "ok", data: sent(1, "plan the release") });
    const module = await outbox();
    const queue = await queueOf(module);

    module.sendVoiceMessage("plan the release", "id-1");
    await vi.waitFor(() =>
      expect(queue.result.current).toEqual([
        { clientId: "id-1", text: "plan the release", state: "failed", error: "offline" },
      ]),
    );

    await vi.advanceTimersByTimeAsync(module.RETRY_MS[0]);
    expect(send).toHaveBeenCalledTimes(2);
    await vi.waitFor(() => expect(queue.result.current[0]?.error).toBe("still offline"));

    module.retryVoiceMessage("id-1");
    await vi.waitFor(() => expect(queue.result.current).toEqual([]));
    expect(send.mock.calls.map(([, clientId]) => clientId)).toEqual(["id-1", "id-1", "id-1"]);
  });

  it("stops retrying by itself after the last wait, and drops a message on discard", async () => {
    vi.useFakeTimers();
    send.mockResolvedValue({ status: "error", error: { kind: "external", message: "offline" } });
    const module = await outbox();
    const queue = await queueOf(module);

    module.sendVoiceMessage("hello", "id-1");
    for (const wait of module.RETRY_MS) await vi.advanceTimersByTimeAsync(wait);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(send).toHaveBeenCalledTimes(module.RETRY_MS.length + 1);

    module.discardVoiceMessage("id-1");
    await vi.waitFor(() => expect(queue.result.current).toEqual([]));
  });
});
