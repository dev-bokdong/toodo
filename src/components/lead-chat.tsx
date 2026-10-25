/**
 * The chat with the server's lead, Dori: messages typed into the lead's herdr pane, and the
 * replies it posts back. Every open client shows the same history, live, with how far each
 * of the person's messages got. It floats over every view, behind a round Dori button.
 */
import {
  lazy,
  Suspense,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type FormEvent,
  type KeyboardEvent,
  type Ref,
} from "react";
import { Check, CheckCheck, MessageCircle, MessagesSquare, SendHorizontal, XIcon } from "lucide-react";
import doriAvatar from "@/assets/dori-avatar.webp";
import { Button } from "@/components/ui/button";
import type { ChatMessage, LeadStatus } from "@/lib/bindings";
import { markRead, newClientId, receipt, unreadCount, useReadUpTo, type Receipt } from "@/lib/chat";
import { useChatMessages, useSendChatMessage } from "@/lib/queries";
import { useSwipeClose } from "@/lib/swipe-close";
import { formatFull } from "@/lib/timeline";
import { When } from "@/components/when";
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

/**
 * The round Dori button at the bottom right, and the chat it opens: a panel over the view, or
 * a full-screen sheet on a phone. The panel stays mounted while closed, so an unsent message
 * and a message still sending survive closing it and moving between views.
 */
export function FloatingChat({ lead }: { lead: LeadStatus }) {
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const wasOpen = useRef(false);
  const panelId = useId();
  const messages = useChatMessages(true);
  const readUpTo = useReadUpTo();
  const unread = open ? 0 : unreadCount(messages.data ?? [], readUpTo);
  const close = () => setOpen(false);
  const drag = useSwipeClose(close);

  useLayoutEffect(() => {
    if (open) {
      input.current?.focus({ preventScroll: true });
    } else if (wasOpen.current) {
      // Only take the focus back when closing the panel lost it, not when it moved on elsewhere.
      const active = document.activeElement;
      if (active === null || active === document.body || panel.current?.contains(active)) {
        button.current?.focus({ preventScroll: true });
      }
    }
    wasOpen.current = open;
  }, [open]);

  return (
    <>
      <button
        ref={button}
        type="button"
        aria-expanded={open}
        aria-controls={panelId}
        title={open ? "Close lead chat" : "Open lead chat"}
        onClick={() => setOpen(!open)}
        className="fixed right-4 bottom-[calc(1rem+env(safe-area-inset-bottom))] z-20 size-14 rounded-full bg-card shadow-lg ring-1 ring-border outline-none transition-transform hover:scale-105 focus-visible:ring-3 focus-visible:ring-ring/50 motion-reduce:transition-none motion-reduce:hover:scale-100 md:right-6 md:bottom-6"
      >
        <img src={doriAvatar} alt="" className="size-full rounded-full" />
        <span className="sr-only">
          {["Lead chat", lead.typing && "Dori is typing", unread > 0 && `${unread} unread`].filter(Boolean).join(", ")}
        </span>
        <span
          data-chat-mark
          aria-hidden
          className="absolute -bottom-0.5 -left-0.5 flex h-5 min-w-5 items-center justify-center rounded-full bg-primary px-1 text-primary-foreground ring-2 ring-background"
        >
          {lead.typing ? <TypingDots className="size-1 bg-primary-foreground" /> : <MessageCircle className="size-3" />}
        </span>
        {unread > 0 && (
          <span
            data-unread
            aria-hidden
            className="absolute -top-1 -right-1 flex h-5 min-w-5 items-center justify-center rounded-full bg-destructive px-1 text-xs font-medium text-white ring-2 ring-background"
          >
            {unread > 99 ? "99+" : unread}
          </span>
        )}
      </button>
      <div
        ref={panel}
        id={panelId}
        role="dialog"
        aria-label="Lead chat"
        hidden={!open}
        data-dragging={drag.dragging ? "" : undefined}
        onKeyDown={(event) => {
          // An inner control handles its own Escape first.
          if (event.key === "Escape" && !event.defaultPrevented) {
            event.preventDefault();
            close();
          }
        }}
        style={{ transform: drag.offset === 0 ? undefined : `translateY(${drag.offset}px)` } as CSSProperties}
        className="fixed z-50 flex flex-col overflow-hidden bg-card shadow-xl max-md:inset-0 max-md:pt-[env(safe-area-inset-top)] max-md:pb-[env(safe-area-inset-bottom)] max-md:transition-transform max-md:data-dragging:transition-none motion-reduce:transition-none md:right-6 md:bottom-24 md:h-[min(35rem,calc(100dvh-7.5rem))] md:w-[min(23.75rem,calc(100vw-3rem))] md:rounded-xl md:border"
      >
        <div className="flex shrink-0 touch-none justify-center pt-2 pb-1 md:hidden" {...drag.handlers}>
          <span data-grabber aria-hidden className="h-1 w-9 rounded-full bg-muted-foreground/40" />
        </div>
        <LeadChat lead={lead} open={open} onClose={close} inputRef={input} />
      </div>
    </>
  );
}

/** Three dots that bounce in turn; still when motion is reduced. */
function TypingDots({ className }: { className: string }) {
  return [0, 150, 300].map((delay) => (
    <span
      key={delay}
      aria-hidden
      style={{ animationDelay: `${delay}ms` }}
      className={cn("mx-px animate-bounce rounded-full motion-reduce:animate-none", className)}
    />
  ));
}

/**
 * The chat itself. Only while `open`, what it shows counts as read and the newest message is
 * scrolled into view.
 */
export function LeadChat({
  lead,
  open = true,
  onClose,
  inputRef,
}: {
  lead: LeadStatus;
  open?: boolean;
  onClose?: () => void;
  inputRef?: Ref<HTMLTextAreaElement>;
}) {
  const messages = useChatMessages(true);
  const send = useSendChatMessage();
  const [text, setText] = useState("");
  const [clientId, setClientId] = useState(newClientId);
  const log = useRef<HTMLOListElement>(null);
  const list = messages.data ?? [];
  const lastId = list.at(-1)?.id;
  const state = leadState(lead);

  useEffect(() => {
    if (!open) return;
    if (lastId !== undefined) markRead(lastId);
    if (log.current) log.current.scrollTop = log.current.scrollHeight;
  }, [open, lastId, lead.typing]);

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
    <section aria-label="Chat with the lead" className="flex min-h-0 flex-1 flex-col">
      <header className="flex shrink-0 items-center gap-3 border-b px-4 py-2.5">
        <img src={doriAvatar} alt="" className="size-9 shrink-0 rounded-full" />
        <div className="flex min-w-0 flex-col">
          <h2 className="text-base font-semibold tracking-tight">Dori</h2>
          <span role="status" aria-label="Lead status" className="truncate text-xs text-muted-foreground">
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
        </div>
        {onClose && (
          <Button
            size="icon-sm"
            variant="ghost"
            className="ml-auto"
            aria-label="Close lead chat"
            title="Close lead chat (Esc)"
            onClick={onClose}
          >
            <XIcon />
          </Button>
        )}
      </header>
      {messages.error && (
        <p role="alert" className="border-b px-4 py-2 text-sm text-destructive">
          {messages.error.message}
        </p>
      )}
      <ol
        ref={log}
        role="log"
        aria-label="Messages"
        className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto overscroll-contain bg-muted/30 p-3"
      >
        {messages.data && list.length === 0 && (
          <li className="m-auto flex max-w-64 flex-col items-center gap-2 text-center text-sm text-muted-foreground">
            <MessagesSquare className="size-6" />
            No messages yet.
            <span className="text-xs">
              Ask the lead to plan, start, or follow up on tasks. It reads your messages in its herdr pane and answers
              here.
            </span>
          </li>
        )}
        {list.map((message) => (
          <Message key={message.id} message={message} />
        ))}
        {send.isPending && send.variables && (
          <Message
            message={{
              id: 0,
              author: "user",
              body: send.variables.text.trim(),
              at: Math.floor(send.submittedAt / 1000),
              deliveredAt: null,
              seenAt: null,
            }}
          />
        )}
        {lead.typing && <TypingBubble />}
      </ol>
      <form onSubmit={submit} aria-label="Message the lead" className="flex shrink-0 items-end gap-2 border-t p-3">
        <textarea
          ref={inputRef}
          aria-label="Message to the lead"
          value={text}
          rows={2}
          placeholder={state.ready ? "Message Dori (Enter sends, Shift+Enter for a new line)" : "The lead is not running"}
          onChange={(event) => {
            setText(event.target.value);
            setClientId(newClientId());
          }}
          onKeyDown={onKeyDown}
          className="max-h-40 min-h-16 min-w-0 flex-1 resize-none rounded-md border bg-background px-3 py-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
        />
        <Button type="submit" disabled={!state.ready || !text.trim() || send.isPending}>
          <SendHorizontal />
          Send
        </Button>
      </form>
      {send.error && (
        <p role="alert" className="shrink-0 px-3 pb-3 text-sm text-destructive">
          {send.error.message}
        </p>
      )}
    </section>
  );
}

/** Three dots from Dori while the lead composes a reply; still when motion is reduced. */
function TypingBubble() {
  return (
    <li data-typing aria-label="Reply is being written" className="flex max-w-[85%] items-start gap-2 self-start">
      <img src={doriAvatar} alt="" className="size-8 shrink-0 rounded-full" />
      <div
        role="status"
        aria-label="Reply is being written"
        className="flex items-center gap-1 rounded-lg border bg-background px-3 py-3"
      >
        <TypingDots className="size-1.5 bg-muted-foreground" />
      </div>
    </li>
  );
}

function Message({ message }: { message: ChatMessage }) {
  if (message.author === "lead") {
    return (
      <li data-author="lead" aria-label="Dori" className="flex max-w-[85%] items-start gap-2 self-start">
        <img src={doriAvatar} alt="" className="size-8 shrink-0 rounded-full" />
        <div className="flex min-w-0 flex-col items-start gap-1">
          <span className="text-xs font-medium">Dori</span>
          <div className="max-w-full rounded-lg border bg-background px-3 py-2 text-sm break-words">
            <Suspense fallback={<p className="whitespace-pre-wrap">{message.body}</p>}>
              <Markdown source={message.body} />
            </Suspense>
          </div>
          <When className="text-xs text-muted-foreground" at={message.at} />
        </div>
      </li>
    );
  }
  return (
    <li data-author="user" aria-label="You" className="flex max-w-[85%] flex-col items-end gap-1 self-end">
      <div className="rounded-lg bg-primary px-3 py-2 text-sm break-words text-primary-foreground">
        <p className="whitespace-pre-wrap">{message.body}</p>
      </div>
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <When at={message.at} prefix="You, " />
        <ReceiptMark message={message} />
      </div>
    </li>
  );
}

const receipts: Record<Receipt, { text: string; seen: boolean }> = {
  sent: { text: "Sent", seen: false },
  delivered: { text: "Delivered", seen: false },
  seen: { text: "Seen", seen: true },
};

/** Sent, delivered to the lead's pane, or seen by the lead, with the time in the tooltip. */
function ReceiptMark({ message }: { message: ChatMessage }) {
  const state = receipt(message);
  const at = state === "seen" ? message.seenAt : state === "delivered" ? message.deliveredAt : null;
  const Icon = state === "sent" ? Check : CheckCheck;
  return (
    <span
      data-receipt={state}
      title={at === null ? undefined : `${receipts[state].text} ${formatFull(at)}`}
      className={cn("inline-flex items-center gap-0.5", receipts[state].seen && "text-sky-700 dark:text-sky-400")}
    >
      <Icon aria-hidden className="size-3.5" />
      {receipts[state].text}
    </span>
  );
}
