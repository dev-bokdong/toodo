import { beforeEach, describe, expect, it } from "vitest";
import type { ChatMessage } from "@/lib/bindings";
import { markRead, receipt, unreadCount } from "@/lib/chat";

const message = (id: number, author: ChatMessage["author"]): ChatMessage => ({
  id,
  author,
  body: `m${id}`,
  at: id,
  deliveredAt: author === "user" ? id : null,
  seenAt: null,
});

describe("chat read mark", () => {
  beforeEach(() => localStorage.clear());

  it("counts the lead's replies and the server's reports after the read mark", () => {
    const messages = [message(1, "user"), message(2, "lead"), message(3, "user"), message(4, "system"), message(5, "lead")];
    expect(unreadCount(messages, 0)).toBe(3);
    expect(unreadCount(messages, 2)).toBe(2);
    expect(unreadCount(messages, 4)).toBe(1);
    expect(unreadCount(messages, 5)).toBe(0);
  });

  it("tells how far a message got from when it was delivered and seen", () => {
    expect(receipt({ deliveredAt: null, seenAt: null })).toBe("sent");
    expect(receipt({ deliveredAt: 1, seenAt: null })).toBe("delivered");
    expect(receipt({ deliveredAt: 1, seenAt: 2 })).toBe("seen");
  });

  it("never moves the read mark back", () => {
    markRead(5);
    markRead(3);
    expect(localStorage.getItem("toodo-chat-read")).toBe("5");
  });
});
