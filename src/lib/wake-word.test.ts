import { renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { commands, events } from "@/lib/bindings";
import { followWakeWord, useWakeWord } from "@/lib/wake-word";

type Handler = (event: { payload: unknown }) => void;
const handlers = new Map<string, Handler>();

vi.mock("@/lib/bindings", () => {
  const event = (name: string) => ({
    listen: vi.fn(async (handler: Handler) => {
      handlers.set(name, handler);
      return () => handlers.delete(name);
    }),
  });
  return {
    commands: {
      getWakeWord: vi.fn(async () => ({ status: "ok", data: { enabled: true, status: { state: "listening" } } })),
      sendChatMessage: vi.fn(),
    },
    events: { wakeWordChanged: event("wakeWordChanged"), wakeWordHeard: event("wakeWordHeard") },
  };
});

describe("wake word", () => {
  it("follows the app's state and sends each heard sentence once to the lead chat", async () => {
    vi.mocked(commands.sendChatMessage).mockResolvedValue({
      status: "ok",
      data: {
        id: 9,
        author: "user",
        body: "today's tasks",
        at: 1,
        deliveredAt: null,
        seenAt: null,
        dueAt: null,
        ackedAt: null,
        handledAt: null,
        handledBy: null,
        overdue: false,
      },
    });
    const stop = followWakeWord();
    const state = renderHook(() => useWakeWord());
    await waitFor(() => expect(state.result.current.status).toEqual({ state: "listening" }));
    expect(events.wakeWordHeard.listen).toHaveBeenCalledTimes(1);

    handlers.get("wakeWordChanged")!({ payload: { enabled: true, status: { state: "hearing", transcript: "today" } } });
    await waitFor(() => expect(state.result.current.status).toEqual({ state: "hearing", transcript: "today" }));

    handlers.get("wakeWordHeard")!({ payload: { id: 1, message: "today's tasks" } });
    await waitFor(() => expect(commands.sendChatMessage).toHaveBeenCalledTimes(1));
    expect(vi.mocked(commands.sendChatMessage).mock.calls[0][0]).toBe("today's tasks");

    stop();
    await waitFor(() => expect(handlers.size).toBe(0));
  });
});
