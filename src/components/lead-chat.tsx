/**
 * The chat with the server's lead: messages typed into the lead's herdr pane, and the
 * replies it posts back. Every open client shows the same history, live.
 */
import { lazy, Suspense, useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { MessagesSquare, SendHorizontal } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { ChatMessage, LeadStatus } from "@/lib/bindings";
import { markRead, newClientId } from "@/lib/chat";
import { useChatMessages, useSendChatMessage } from "@/lib/queries";
import { cn } from "@/lib/utils";

const Markdown = lazy(() => import("@/components/markdown"));

function leadState(lead: LeadStatus): { text: string; ready: boolean } {
  switch (lead.state) {
    case "running":
      return {
        text: lead.agentStatus === "working" ? "Working" : lead.agentStatus === "blocked" ? "Waiting for input" : "Ready",
        ready: true,
      };
    case "starting":
      return { text: "Starting", ready: false };
    case "restarting":
      return { text: "Restarting", ready: false };
    case "failed":
      return { text: "Not running", ready: false };
    case "disabled":
      return { text: "Off", ready: false };
  }
}

export function UnreadBadge({ count }: { count: number }) {
  if (count === 0) return null;
  return (
    <span
      aria-label={`${count} unread`}
      className="ml-auto min-w-5 rounded-full bg-primary px-1.5 text-center text-xs font-medium text-primary-foreground"
    >
      {count > 99 ? "99+" : count}
    </span>
  );
}

export function LeadChat({ lead }: { lead: LeadStatus }) {
  const messages = useChatMessages(true);
  const send = useSendChatMessage();
  const [text, setText] = useState("");
  const [clientId, setClientId] = useState(newClientId);
  const log = useRef<HTMLOListElement>(null);
  const list = messages.data ?? [];
  const lastId = list.at(-1)?.id;
  const state = leadState(lead);

  // Whatever this panel shows is read, and the newest message is in view.
  useEffect(() => {
    if (lastId !== undefined) markRead(lastId);
    if (log.current) log.current.scrollTop = log.current.scrollHeight;
  }, [lastId]);

  function submit(event?: FormEvent) {
    event?.preventDefault();
    if (!text.trim() || send.isPending) return;
    send.mutate(
      { text, clientId },
      {
        onSuccess: () => {
          setText("");
          setClientId(newClientId());
        },
      },
    );
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      submit();
    }
  }

  return (
    <section aria-label="Chat with the lead" className="flex min-h-0 flex-1 flex-col gap-3">
      <header className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h2 className="text-xl font-semibold tracking-tight">Lead</h2>
        <span role="status" aria-label="Lead status" className="text-sm text-muted-foreground">
          <span
            aria-hidden
            className={cn(
              "mr-1.5 inline-block size-2 rounded-full",
              state.ready ? "bg-emerald-500" : lead.state === "failed" ? "bg-destructive" : "bg-muted-foreground",
            )}
          />
          {state.text}
          {lead.error && ` - ${lead.error}`}
        </span>
      </header>
      <p className="text-sm text-muted-foreground">
        Ask the lead to plan, start, or follow up on tasks. It reads your messages in its herdr pane and answers here.
      </p>
      {messages.error && (
        <p role="alert" className="text-sm text-destructive">
          {messages.error.message}
        </p>
      )}
      <ol
        ref={log}
        role="log"
        aria-label="Messages"
        className="flex min-h-40 flex-1 flex-col gap-3 overflow-y-auto rounded-lg border bg-muted/30 p-3"
      >
        {messages.data && list.length === 0 && (
          <li className="m-auto flex flex-col items-center gap-2 text-sm text-muted-foreground">
            <MessagesSquare className="size-6" />
            No messages yet.
          </li>
        )}
        {list.map((message) => (
          <Message key={message.id} message={message} />
        ))}
      </ol>
      <form onSubmit={submit} aria-label="Message the lead" className="flex items-end gap-2">
        <textarea
          aria-label="Message to the lead"
          value={text}
          rows={2}
          placeholder={state.ready ? "Message the lead (Enter sends, Shift+Enter for a new line)" : "The lead is not running"}
          onChange={(event) => {
            setText(event.target.value);
            setClientId(newClientId());
          }}
          onKeyDown={onKeyDown}
          className="min-h-16 flex-1 resize-y rounded-md border bg-background px-3 py-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
        />
        <Button type="submit" disabled={!state.ready || !text.trim() || send.isPending}>
          <SendHorizontal />
          Send
        </Button>
      </form>
      {send.error && (
        <p role="alert" className="text-sm text-destructive">
          {send.error.message}
        </p>
      )}
    </section>
  );
}

function Message({ message }: { message: ChatMessage }) {
  const mine = message.author === "user";
  const sentAt = new Date(message.at * 1000);
  return (
    <li
      data-author={message.author}
      aria-label={mine ? "You" : "Lead"}
      className={cn("flex max-w-[85%] flex-col gap-1", mine ? "items-end self-end" : "items-start self-start")}
    >
      <div
        className={cn(
          "rounded-lg px-3 py-2 text-sm break-words",
          mine ? "bg-primary text-primary-foreground" : "border bg-background",
        )}
      >
        {mine ? (
          <p className="whitespace-pre-wrap">{message.body}</p>
        ) : (
          <Suspense fallback={<p className="whitespace-pre-wrap">{message.body}</p>}>
            <Markdown source={message.body} />
          </Suspense>
        )}
      </div>
      <time className="text-xs text-muted-foreground" dateTime={sentAt.toISOString()}>
        {mine ? "You" : "Lead"}, {sentAt.toLocaleString([], { dateStyle: "short", timeStyle: "short" })}
      </time>
    </li>
  );
}
