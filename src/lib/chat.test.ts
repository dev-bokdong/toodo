import { beforeEach, describe, expect, it } from "vitest";
import type { ChatMessage } from "@/lib/bindings";
import { handledByText, markRead, overdueCount, receipt, unreadCount } from "@/lib/chat";

const message = (id: number, author: ChatMessage["author"]): ChatMessage => ({
  id,
  author,
  body: `m${id}`,
  at: id,
  deliveredAt: author === "user" ? id : null,
  seenAt: null,
  dueAt: null,
  ackedAt: null,
  handledAt: null,
  handledBy: null,
  overdue: false,
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

  it("tells how far a message got from its receipts", () => {
    const none = { id: 7, deliveredAt: null, seenAt: null, ackedAt: null, handledAt: null };
    // Still sending from this client, or stored and waiting for the lead to take it.
    expect(receipt({ ...none, id: 0 })).toBe("sent");
    expect(receipt(none)).toBe("waiting");
    expect(receipt({ ...none, deliveredAt: 1 })).toBe("delivered");
    expect(receipt({ ...none, deliveredAt: 1, seenAt: 2 })).toBe("seen");
    expect(receipt({ ...none, deliveredAt: 1, seenAt: 2, ackedAt: 3 })).toBe("acked");
    // Taken without a turn being seen: handled outranks the estimate.
    expect(receipt({ ...none, deliveredAt: 1, ackedAt: 3, handledAt: 4 })).toBe("handled");
  });

  it("counts what the server marked overdue and says what handled a message", () => {
    const late = { ...message(1, "user"), overdue: true };
    expect(overdueCount([late, message(2, "user"), message(3, "lead")])).toBe(1);
    expect(handledByText({ handledBy: { kind: "reply", messageId: 9 } })).toBe("Handled in a reply");
    expect(handledByText({ handledBy: { kind: "task", taskId: 7 } })).toBe("Handled by #7");
    expect(handledByText({ handledBy: null })).toBe("Handled");
  });

  it("never moves the read mark back", () => {
    markRead(5);
    markRead(3);
    expect(localStorage.getItem("toodo-chat-read")).toBe("5");
  });
});
