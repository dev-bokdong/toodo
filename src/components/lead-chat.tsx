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
import {
  Check,
  CheckCheck,
  CircleCheck,
  CircleCheckBig,
  Clock,
  MessageCircle,
  MessagesSquare,
  Mic,
  SendHorizontal,
  Square,
  Volume2,
  VolumeX,
  XIcon,
} from "lucide-react";
import doriAvatar from "@/assets/dori-avatar.webp";
import { Button } from "@/components/ui/button";
import { onChatRequest, takeChatRequest } from "@/lib/app-links";
import { events, type ChatMessage, type LeadStatus } from "@/lib/bindings";
import {
  handledByText,
  markRead,
  newClientId,
  overdueCount,
  receipt,
  unreadCount,
  useReadUpTo,
  type Receipt,
} from "@/lib/chat";
import { openPrivacySettings } from "@/lib/mac-dictation";
import { openAppSettings } from "@/lib/native-speech";
import { builtForWeb, mobileApp } from "@/lib/platform";
import { useChatMessages, useSendChatMessage } from "@/lib/queries";
import { useSwipeClose } from "@/lib/swipe-close";
import { formatFull } from "@/lib/timeline";
import {
  setReadAloud,
  speak,
  speechSupported,
  stopSpeaking,
  useDictation,
  useReadAloud,
  useSpeaking,
} from "@/lib/voice";
import { When } from "@/components/when";
import { cn } from "@/lib/utils";
import { WakeWordChat } from "@/components/wake-word";

const Markdown = lazy(() => import("@/components/markdown"));

/**
 * `ready`: the lead runs, or a lead outside the server is in touch. `accepts`: the server takes
 * messages, which wait for a lead that is starting, restarting, failing, or out of touch; only a
 * server without a lead refuses them.
 */
function leadState(lead: LeadStatus): { text: string; ready: boolean; accepts: boolean } {
  switch (lead.state) {
    case "running":
      return {
        text: lead.agentStatus === "working" ? "Working" : lead.agentStatus === "blocked" ? "Waiting for input" : "Ready",
        ready: true,
        accepts: true,
      };
    case "starting":
      return { text: "Starting", ready: false, accepts: true };
    case "restarting":
      return { text: "Restarting", ready: false, accepts: true };
    case "failed":
      return { text: "Not running", ready: false, accepts: true };
    case "external":
      // A lead outside the server; it reads the stored messages once it is back in touch.
      return { text: lead.connected ? "Connected" : "Disconnected", ready: lead.connected, accepts: true };
    case "disabled":
      return { text: "Off", ready: false, accepts: false };
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
  const focusInput = useRef(true);
  const panelId = useId();
  const messages = useChatMessages(true);
  const readUpTo = useReadUpTo();
  const unread = open ? 0 : unreadCount(messages.data ?? [], readUpTo);
  const close = () => setOpen(false);
  const drag = useSwipeClose(close);
  const speaking = useSpeaking();
  // Voice input asked for by a link into the app or by the app itself (its tray, or a wake
  // word), counted so that each starts dictation once.
  const [voiceRequests, setVoiceRequests] = useState(0);

  useEffect(() => {
    const take = () => {
      const request = takeChatRequest();
      if (request === null) return;
      focusInput.current = request.focusInput ?? true;
      setOpen(true);
      if (request.voice) setVoiceRequests((count) => count + 1);
    };
    take();
    return onChatRequest(take);
  }, []);

  useEffect(() => {
    if (builtForWeb) return;
    const subscription = events.voiceMessageRequested.listen(() => {
      setOpen(true);
      setVoiceRequests((count) => count + 1);
    });
    return () => void subscription.then((unlisten) => unlisten());
  }, []);

  useLayoutEffect(() => {
    if (open) {
      if (focusInput.current) input.current?.focus({ preventScroll: true });
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
        onClick={() => {
          focusInput.current = true;
          setOpen(!open);
        }}
        className="fixed right-4 bottom-[calc(1rem+env(safe-area-inset-bottom))] z-20 size-14 rounded-full bg-card shadow-lg ring-1 ring-border outline-none transition-transform hover:scale-105 focus-visible:ring-3 focus-visible:ring-ring/50 motion-reduce:transition-none motion-reduce:hover:scale-100 md:right-6 md:bottom-6"
      >
        <img src={doriAvatar} alt="" className="size-full rounded-full" />
        <span className="sr-only">
          {["Lead chat", lead.typing && "Dori is typing", unread > 0 && `${unread} unread`].filter(Boolean).join(", ")}
        </span>
        <span
          data-chat-mark
          aria-hidden
          className="absolute -right-0.5 -bottom-0.5 flex h-5 min-w-5 items-center justify-center rounded-full bg-primary px-1 text-primary-foreground ring-2 ring-background"
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
      {speaking && !open && (
        <Button
          size="sm"
          variant="outline"
          onClick={stopSpeaking}
          className="fixed right-4 bottom-[calc(5.5rem+env(safe-area-inset-bottom))] z-20 rounded-full shadow-md md:right-6 md:bottom-[6.5rem]"
        >
          <Square />
          Stop reading aloud
        </Button>
      )}
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
        <LeadChat lead={lead} open={open} onClose={close} inputRef={input} voiceRequests={voiceRequests} />
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
  voiceRequests = 0,
}: {
  lead: LeadStatus;
  open?: boolean;
  onClose?: () => void;
  inputRef?: Ref<HTMLTextAreaElement>;
  /** How many times voice input was asked for from outside; each new one starts dictation. */
  voiceRequests?: number;
}) {
  const messages = useChatMessages(true);
  const send = useSendChatMessage();
  const [text, setText] = useState("");
  const [clientId, setClientId] = useState(newClientId);
  const log = useRef<HTMLOListElement>(null);
  const list = messages.data ?? [];
  const lastId = list.at(-1)?.id;
  const overdue = overdueCount(list);
  const state = leadState(lead);
  const readAloud = useReadAloud();
  const speaking = useSpeaking();
  // The text typed before dictation started; what is heard goes after it.
  const typed = useRef("");
  const withTyped = (heard: string) => [typed.current.trimEnd(), heard.trim()].filter(Boolean).join(" ");
  const dictation = useDictation(
    (heard) => setText(withTyped(heard)),
    (heard) => {
      if (heard.trim() && state.accepts) sendText(withTyped(heard));
    },
  );
  // Replies newer than this are read aloud; the history there on opening is not.
  const spokenUpTo = useRef<number | null>(null);

  useEffect(() => {
    if (!messages.data) return;
    const newest = messages.data.at(-1)?.id ?? 0;
    const from = spokenUpTo.current;
    spokenUpTo.current = Math.max(from ?? 0, newest);
    if (from === null || !readAloud) return;
    for (const message of messages.data) {
      if (message.author === "lead" && message.id > from) speak(message.body);
    }
  }, [messages.data, readAloud]);

  useEffect(() => {
    if (!open) return;
    if (lastId !== undefined) markRead(lastId);
    if (log.current) log.current.scrollTop = log.current.scrollHeight;
  }, [open, lastId, lead.typing]);

  function startDictation() {
    typed.current = text;
    setClientId(newClientId());
    dictation.start();
  }

  // A request waits until the lead takes messages and nothing is being sent or heard.
  const voiceStarted = useRef(0);
  useEffect(() => {
    if (voiceRequests <= voiceStarted.current) return;
    if (!dictation.supported || !state.accepts || send.isPending || dictation.listening) return;
    voiceStarted.current = voiceRequests;
    startDictation();
  });

  function submit(event?: FormEvent) {
    event?.preventDefault();
    sendText(text);
  }

  function sendText(text: string) {
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
          {overdue > 0 && (
            <span
              data-overdue-count
              className="inline-flex items-center gap-1 text-xs font-medium text-amber-700 dark:text-amber-400"
            >
              <Clock aria-hidden className="size-3" />
              {overdue === 1 ? "1 message waits too long" : `${overdue} messages wait too long`}
            </span>
          )}
        </div>
        <div className="ml-auto flex shrink-0 items-center gap-1">
          {speaking && (
            <Button size="sm" variant="outline" onClick={stopSpeaking}>
              <Square />
              Stop reading aloud
            </Button>
          )}
          {speechSupported() && (
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label="Read replies aloud"
              aria-pressed={readAloud}
              title={readAloud ? "Replies are read aloud" : "Read replies aloud"}
              onClick={() => setReadAloud(!readAloud)}
            >
              {readAloud ? <Volume2 /> : <VolumeX />}
            </Button>
          )}
          {onClose && (
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label="Close lead chat"
              title="Close lead chat (Esc)"
              onClick={onClose}
            >
              <XIcon />
            </Button>
          )}
        </div>
      </header>
      {mobileApp && <WakeWordChat />}
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
              dueAt: null,
              ackedAt: null,
              handledAt: null,
              handledBy: null,
              overdue: false,
            }}
          />
        )}
        {lead.typing && <TypingBubble />}
      </ol>
      <form
        onSubmit={submit}
        aria-label="Message the lead"
        onKeyDown={(event) => {
          // Escape drops what is being dictated before it closes the chat.
          if (event.key === "Escape" && dictation.listening) {
            event.preventDefault();
            dictation.cancel();
            setText(typed.current);
          }
        }}
        className="flex shrink-0 items-end gap-2 border-t p-3"
      >
        <textarea
          ref={inputRef}
          aria-label="Message to the lead"
          value={text}
          rows={2}
          placeholder={
            state.ready
              ? "Message Dori (Enter sends, Shift+Enter for a new line)"
              : state.accepts
                ? "Dori is not running yet; your message waits for it"
                : "The lead is not running"
          }
          onChange={(event) => {
            setText(event.target.value);
            setClientId(newClientId());
          }}
          onKeyDown={onKeyDown}
          className="max-h-40 min-h-16 min-w-0 flex-1 resize-none rounded-md border bg-background px-3 py-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
        />
        {dictation.supported &&
          (dictation.listening ? (
            <Button
              type="button"
              size="icon"
              variant="destructive"
              aria-label="Stop voice input and send"
              title="Stop voice input and send (Esc cancels)"
              onClick={dictation.stop}
            >
              <Square className="motion-safe:animate-pulse" />
            </Button>
          ) : (
            <Button
              type="button"
              size="icon"
              variant="outline"
              aria-label="Voice input"
              title="Speak a message"
              disabled={!state.accepts || send.isPending}
              onClick={startDictation}
            >
              <Mic />
            </Button>
          ))}
        <Button type="submit" disabled={!state.accepts || !text.trim() || send.isPending}>
          <SendHorizontal />
          Send
        </Button>
      </form>
      {dictation.listening && (
        <p role="status" aria-label="Voice input" className="shrink-0 px-3 pb-3 text-xs text-muted-foreground">
          Listening. Speak, then pause; Esc cancels.
        </p>
      )}
      {dictation.error && (
        <div role="alert" className="flex shrink-0 flex-wrap items-center gap-2 px-3 pb-3 text-sm text-destructive">
          <p>{dictation.error}</p>
          {mobileApp && dictation.blocked && (
            <Button type="button" size="sm" variant="outline" onClick={() => void openAppSettings()}>
              Open app settings
            </Button>
          )}
          {dictation.privacyPane && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              changesData={false}
              onClick={() => void openPrivacySettings(dictation.privacyPane!).catch(console.error)}
            >
              Open System Settings
            </Button>
          )}
        </div>
      )}
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
  if (message.author === "system") {
    return (
      <li data-author="system" aria-label="Toodo" className="flex w-full flex-col items-center gap-1 self-center">
        <div className="flex max-w-full items-start gap-2 rounded-lg border border-dashed bg-background/60 px-3 py-2 text-sm break-words">
          <CircleCheck aria-hidden className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <div className="min-w-0">
            <Suspense fallback={<p className="whitespace-pre-wrap">{message.body}</p>}>
              <Markdown source={message.body} />
            </Suspense>
          </div>
        </div>
        <When className="text-xs text-muted-foreground" at={message.at} prefix="Toodo, " />
      </li>
    );
  }
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
      <div className="flex flex-wrap items-center justify-end gap-x-2 text-xs text-muted-foreground">
        <When at={message.at} prefix="You, " />
        <ReceiptMark message={message} />
        {message.overdue && (
          <span
            data-overdue
            title={message.dueAt === null ? undefined : `Not handled since ${formatFull(message.dueAt)}`}
            className="inline-flex items-center gap-0.5 font-medium text-amber-700 dark:text-amber-400"
          >
            <Clock aria-hidden className="size-3.5" />
            Overdue
          </span>
        )}
      </div>
    </li>
  );
}

const receipts: Record<Receipt, { text: string; seen: boolean }> = {
  sent: { text: "Sent", seen: false },
  delivered: { text: "Delivered", seen: false },
  seen: { text: "Seen", seen: true },
  acked: { text: "Received", seen: true },
  handled: { text: "Handled", seen: true },
};

/**
 * Sent, delivered to the lead's pane, seen by the lead, acknowledged by it, or handled by its
 * reply or a task, with the time and what handled it in the tooltip.
 */
function ReceiptMark({ message }: { message: ChatMessage }) {
  const state = receipt(message);
  const at = {
    sent: null,
    delivered: message.deliveredAt,
    seen: message.seenAt,
    acked: message.ackedAt,
    handled: message.handledAt,
  }[state];
  const text = state === "handled" ? handledByText(message) : receipts[state].text;
  const Icon = state === "sent" ? Check : state === "handled" ? CircleCheckBig : CheckCheck;
  return (
    <span
      data-receipt={state}
      title={at === null ? undefined : `${text} ${formatFull(at)}`}
      className={cn("inline-flex items-center gap-0.5", receipts[state].seen && "text-sky-700 dark:text-sky-400")}
    >
      <Icon aria-hidden className="size-3.5" />
      {receipts[state].text}
    </span>
  );
}
