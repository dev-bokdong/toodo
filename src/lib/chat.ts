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

/** The lead's replies and the server's reports newer than `readUpTo`. */
export function unreadCount(messages: readonly ChatMessage[], readUpTo: number): number {
  return messages.filter((message) => message.author !== "user" && message.id > readUpTo).length;
}

/**
 * How far a person's message got: sent by this client and not confirmed yet, stored by the
 * server and waiting for a lead that is not running or not connected, typed into the lead's
 * pane or taken by it, picked up by the lead (an estimate), acknowledged by the lead, or
 * handled by a reply or a task of the lead.
 */
export type Receipt = "sent" | "waiting" | "delivered" | "seen" | "acked" | "handled";

/** `message.id` is 0 for a message this client is still sending. */
export function receipt(
  message: Pick<ChatMessage, "id" | "deliveredAt" | "seenAt" | "ackedAt" | "handledAt">,
): Receipt {
  if (message.handledAt !== null) return "handled";
  if (message.ackedAt !== null) return "acked";
  if (message.seenAt !== null) return "seen";
  if (message.deliveredAt !== null) return "delivered";
  return message.id > 0 ? "waiting" : "sent";
}

/**
 * People's messages the lead has not handled within the time the server gives it. The
 * server decides by its own clock and tells clients when a message falls due.
 */
export function overdueCount(messages: readonly ChatMessage[]): number {
  return messages.filter((message) => message.overdue).length;
}

/** What handled a message, in words: the lead's reply, or a task by number. */
export function handledByText(message: Pick<ChatMessage, "handledBy">): string {
  const by = message.handledBy;
  if (by === null) return "Handled";
  return by.kind === "reply" ? "Handled in a reply" : `Handled by #${by.taskId}`;
}

export function newClientId(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
