import { describe, expect, it } from "vitest";
import { parseServerSentEvents } from "@/web/event";

describe("parseServerSentEvents", () => {
  it("returns complete events and keeps the incomplete rest", () => {
    const { events, rest } = parseServerSentEvents(
      'event: tasks-changed\ndata: {"workspaceId":1}\n\n:keep-alive\n\nevent: agent-statuses-changed\ndata: {"sta',
    );
    expect(events).toEqual([{ event: "tasks-changed", data: '{"workspaceId":1}' }]);
    expect(rest).toBe('event: agent-statuses-changed\ndata: {"sta');
  });

  it("joins multi-line data and accepts CRLF line ends", () => {
    const { events, rest } = parseServerSentEvents("event: x\r\ndata: a\r\ndata: b\r\n\r\n");
    expect(events).toEqual([{ event: "x", data: "a\nb" }]);
    expect(rest).toBe("");
  });
});
