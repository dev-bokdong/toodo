/**
 * Client-side state of the chat with the lead: which replies this client has read, kept in
 * local storage, and the IDs that make a resent message harmless.
 */
import { useSyncExternalStore } from "react";
import type { ChatMessage } from "@/lib/bindings";
import { activeServer } from "@/web/server";

/** Kept for each server, whose messages it counts. */
const READ_KEY = activeServer.key("toodo-chat-read");
const listeners = new Set<() => void>();

function readUpTo(): number {
  const stored = Number(localStorage.getItem(READ_KEY));
  return Number.isSafeInteger(stored) ? stored : 0;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  const onStorage = (event: StorageEvent) => {
    if (event.key === READ_KEY) listener();
  };
  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", onStorage);
  };
}

/** The ID of the last message this client has read. */
export function useReadUpTo(): number {
  return useSyncExternalStore(subscribe, readUpTo);
}

/** Marks the messages up to `id` as read; never moves the mark back. */
export function markRead(id: number) {
  if (id <= readUpTo()) return;
  localStorage.setItem(READ_KEY, String(id));
  listeners.forEach((listener) => listener());
}

export function unreadCount(messages: readonly ChatMessage[], readUpTo: number): number {
  return messages.filter((message) => message.author === "lead" && message.id > readUpTo).length;
}

/**
 * How far a person's message got: sent by this client and not confirmed yet, typed into
 * the lead's pane, or picked up by the lead.
 */
export type Receipt = "sent" | "delivered" | "seen";

export function receipt(message: Pick<ChatMessage, "deliveredAt" | "seenAt">): Receipt {
  if (message.seenAt !== null) return "seen";
  return message.deliveredAt !== null ? "delivered" : "sent";
}

export function newClientId(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
