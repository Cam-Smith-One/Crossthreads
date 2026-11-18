// Rooms: where your agents talk to each other, and where you step in. A list
// of rooms beside the conversation (stacked on a phone).

import * as React from "react";
import { ArrowLeft, Ear, Lock, MessagesSquare, Send, Smartphone } from "lucide-react";
import { getRoomMessages, getRooms, postToRoom, type RoomMessage, type RoomView } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { CommandLine, Dot, PageHeader } from "@/components/common";
import { CrewBoard } from "@/components/CrewBoard";
import { cn } from "@/lib/utils";

/** Refresh slowly while the page is hidden (for the sidebar dot), quickly while shown. */
const IDLE_MS = 15_000;
const OPEN_MS = 2_000;

function clock(ts: number): string {
  return new Date(ts * 1000).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

function since(ts: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000 - ts));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

function dayLabel(ts: number): string {
  const d = new Date(ts * 1000);
  const today = new Date();
  const yesterday = new Date(Date.now() - 86400_000);
  if (d.toDateString() === today.toDateString()) return "Today";
  if (d.toDateString() === yesterday.toDateString()) return "Yesterday";
  return d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
}

// A steady colour per agent name, so each voice is easy to follow.
const AUTHOR_TONES = [
  "bg-sky-500/15 text-sky-700 dark:text-sky-300",
  "bg-violet-500/15 text-violet-700 dark:text-violet-300",
  "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300",
  "bg-amber-500/15 text-amber-700 dark:text-amber-300",
  "bg-rose-500/15 text-rose-700 dark:text-rose-300",
  "bg-teal-500/15 text-teal-700 dark:text-teal-300",
];
function authorTone(name: string): string {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return AUTHOR_TONES[h % AUTHOR_TONES.length];
}

function initials(name: string): string {
  const parts = name.split(/[\s\-_.@]+/).filter(Boolean);
  return ((parts[0]?.[0] ?? "?") + (parts[1]?.[0] ?? "")).toUpperCase();
}

export function RoomsPage({
  active,
  onError,
  canPost,
  onListening,
  onAsks,
}: {
  active: boolean;
  onError: (m: string) => void;
  canPost: boolean;
  onListening: (b: boolean) => void;
  /** How many requests to start an agent are waiting on this computer. */
  onAsks?: (n: number) => void;
}) {
  const open = active;
  const [rooms, setRooms] = React.useState<RoomView[] | null>(null);
  const [selected, setSelected] = React.useState<string | null>(null);
  // On a phone, whether the conversation (rather than the list) is showing.
  const [showConvo, setShowConvo] = React.useState(false);
  const [messages, setMessages] = React.useState<RoomMessage[]>([]);
  const [loadingMsgs, setLoadingMsgs] = React.useState(false);
  const [text, setText] = React.useState("");
  const [to, setTo] = React.useState("");
  const [sending, setSending] = React.useState(false);
  const stream = React.useRef<HTMLDivElement>(null);
  const stick = React.useRef(true);
  // The newest message id held for the selected room, read by the poller.
  const last = React.useRef(0);

  const list = rooms ?? [];
  const current = list.find((r) => r.room === selected) ?? null;
  const anyoneListening = list.some((r) => r.agents.some((a) => a.listening));
  React.useEffect(() => {
    onListening(anyoneListening);
  }, [anyoneListening, onListening]);
  const asking = list.filter((r) => (r.asks_here ?? []).length > 0);
  const askCount = asking.reduce((n, r) => n + (r.asks_here ?? []).length, 0);
  React.useEffect(() => {
    onAsks?.(askCount);
  }, [askCount, onAsks]);
  // Opening the page with a request waiting shows that room.
  const firstAsking = asking[0]?.room;
  React.useEffect(() => {
    if (open && firstAsking) setSelected(firstAsking);
  }, [open, firstAsking]);

  const refresh = React.useCallback(async () => {
    const l = await getRooms();
    setRooms(l);
    return l;
  }, []);

  // Keep the room list fresh: slowly when hidden, quickly when shown.
  React.useEffect(() => {
    let cancelled = false;
    const tick = () => {
      if (document.visibilityState !== "visible") return;
      refresh()
        .then((l) => {
          if (!cancelled && open) setSelected((s) => s ?? l[0]?.room ?? null);
        })
        .catch(() => {});
    };
    tick();
    const timer = window.setInterval(tick, open ? OPEN_MS : IDLE_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [open, refresh]);

  // Load the selected room, then keep appending what arrives.
  React.useEffect(() => {
    if (!open || !selected) return;
    let cancelled = false;
    last.current = 0;
    stick.current = true;
    setMessages([]);
    setLoadingMsgs(true);
    const pull = () =>
      getRoomMessages(selected, last.current)
        .then((fresh) => {
          if (cancelled) return;
          setLoadingMsgs(false);
          if (fresh.length === 0) return;
          last.current = fresh[fresh.length - 1].id;
          setMessages((old) => [...old, ...fresh].slice(-500));
        })
        .catch(() => !cancelled && setLoadingMsgs(false));
    pull();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") pull();
    }, OPEN_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [open, selected]);

  // Follow the conversation as it grows, unless you've scrolled up to read.
  React.useEffect(() => {
    const el = stream.current;
    if (el && stick.current) el.scrollTo({ top: el.scrollHeight });
  }, [messages, showConvo]);

  async function send(e?: React.FormEvent) {
    e?.preventDefault();
    if (!selected || !text.trim() || sending) return;
    setSending(true);
    try {
      await postToRoom(selected, text, to || undefined);
      setText("");
      stick.current = true;
      const fresh = await getRoomMessages(selected, last.current);
      if (fresh.length) {
        last.current = fresh[fresh.length - 1].id;
        setMessages((old) => [...old, ...fresh]);
      }
      refresh().catch(() => {});
    } catch (err) {
      onError(String(err));
    } finally {
      setSending(false);
    }
  }

  if (!open) return null;

  const listening = current?.agents.filter((a) => a.listening) ?? [];

  return (
    <div className="mx-auto flex h-full w-full max-w-6xl flex-col gap-5 px-4 py-6 sm:px-6 sm:py-8">
      <PageHeader
        title="Rooms"
        description="Where your agents hand work to each other, one room per repository. Watch any conversation live; what you type here overrides them."
      />

      {rooms === null ? (
        <div className="grid gap-4 md:grid-cols-[16rem_1fr]">
          <Skeleton className="h-64" />
          <Skeleton className="h-96" />
        </div>
      ) : list.length === 0 ? (
        <div className="rounded-lg border border-dashed px-6 py-10">
          <div className="mx-auto max-w-lg space-y-4 text-center">
            <MessagesSquare className="mx-auto size-8 text-muted-foreground" />
            <p className="font-medium">No room has been used yet</p>
            <div className="space-y-3 text-left text-[13px] text-muted-foreground">
              <p>
                Open two agents in the same project. In the one that should do the work, say{" "}
                <code className="font-mono text-foreground">/crossthreads listen</code>. In the other, say{" "}
                <code className="font-mono text-foreground">/crossthreads lead &lt;the task&gt;</code>. They find each other
                through the repository and the conversation appears here.
              </p>
              <p>Run this once so an agent that stops mid-task is woken when the reply arrives:</p>
              <CommandLine cmd="crossthreads hooks install" />
            </div>
          </div>
        </div>
      ) : (
        <div className="grid min-h-[32rem] flex-1 gap-4 md:h-[calc(100vh-13rem)] md:grid-cols-[16rem_1fr]">
          {/* Room list */}
          <nav
            aria-label="Rooms"
            className={cn("flex min-h-0 flex-col overflow-hidden rounded-lg border bg-card", showConvo && "hidden md:flex")}
          >
            <p className="border-b px-3 py-2 text-xs font-medium text-muted-foreground">
              {list.length} room{list.length === 1 ? "" : "s"}
            </p>
            <ul className="min-h-0 flex-1 overflow-y-auto p-1.5 scrollbar-thin">
              {list.map((r) => {
                const on = r.room === selected;
                const live = r.agents.some((a) => a.listening);
                return (
                  <li key={r.room}>
                    <button
                      type="button"
                      aria-current={on ? "true" : undefined}
                      onClick={() => {
                        setSelected(r.room);
                        setShowConvo(true);
                      }}
                      title={r.room}
                      className={cn(
                        "flex w-full flex-col gap-0.5 rounded-md px-2.5 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                        on ? "bg-accent text-accent-foreground" : "hover:bg-accent/60",
                      )}
                    >
                      <span className="flex items-center gap-2">
                        <span className="min-w-0 flex-1 truncate text-sm font-medium">{r.label}</span>
                        {live && <Dot tone="success" pulse />}
                      </span>
                      <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
                        <span className="truncate">
                          {r.agents.length > 0 ? r.agents.map((a) => a.name).join(", ") : "No agents yet"}
                        </span>
                        {r.last_ts > 0 && <span className="ml-auto shrink-0 tabular-nums">{since(r.last_ts)}</span>}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          </nav>

          {/* Conversation */}
          <section
            aria-label={current ? `Room ${current.label}` : "Room"}
            className={cn("flex min-h-[28rem] flex-col overflow-hidden rounded-lg border bg-card", !showConvo && "hidden md:flex")}
          >
            {current ? (
              <>
                <header className="space-y-2 border-b px-4 py-3">
                  <div className="flex items-center gap-2">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="-ml-1.5 md:hidden"
                      onClick={() => setShowConvo(false)}
                      aria-label="Back to rooms"
                    >
                      <ArrowLeft />
                    </Button>
                    <h2 className="min-w-0 flex-1 truncate text-sm font-semibold" title={current.room}>
                      {current.label}
                    </h2>
                    <Badge variant={current.closed ? "success" : "secondary"} className="shrink-0 font-normal">
                      {current.closed
                        ? "Exchange closed"
                        : `${current.turns}${current.max_turns ? `/${current.max_turns}` : ""} turn${current.turns === 1 ? "" : "s"}`}
                    </Badge>
                  </div>
                  {(current.agents.length > 0 || current.claims.length > 0) && (
                    <div className="flex flex-wrap items-center gap-1.5">
                      {current.agents.map((a) => (
                        <span
                          key={a.name}
                          className={cn(
                            "inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs",
                            a.listening ? "border-success/40 bg-success/10" : "text-muted-foreground",
                          )}
                          title={a.listening ? "Waiting for a message" : `Last seen ${since(a.seen)}`}
                        >
                          <Dot tone={a.listening ? "success" : "muted"} pulse={a.listening} />
                          {a.name}
                          <span className="text-muted-foreground">{a.listening ? "listening" : since(a.seen)}</span>
                        </span>
                      ))}
                      {current.claims.map((c) => (
                        <span
                          key={c.path}
                          title={`${c.path} is claimed by ${c.holder} until ${clock(c.expires)}`}
                          className="inline-flex items-center gap-1 rounded-full border border-warning/40 bg-warning/10 px-2 py-0.5 text-xs"
                        >
                          <Lock className="size-3 text-warning" />
                          <span className="font-mono">{c.path.split(/[\\/]/).slice(-2).join("/")}</span>
                          <span className="text-muted-foreground">
                            · {c.holder} until {clock(c.expires)}
                          </span>
                        </span>
                      ))}
                    </div>
                  )}
                  <CrewBoard
                    room={current.room}
                    tasks={current.tasks ?? []}
                    asksHere={current.asks_here ?? []}
                    agents={current.agents.map((a) => a.name)}
                    canEdit={canPost}
                    onChanged={() => getRooms().then(setRooms).catch(() => {})}
                    onError={onError}
                  />
                </header>

                <div
                  ref={stream}
                  aria-live="polite"
                  onScroll={(e) => {
                    const el = e.currentTarget;
                    stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
                  }}
                  className="min-h-0 flex-1 space-y-3 overflow-y-auto px-4 py-4 scrollbar-thin"
                >
                  {loadingMsgs && messages.length === 0 && (
                    <div className="space-y-3">
                      <Skeleton className="h-14 w-3/4" />
                      <Skeleton className="ml-auto h-10 w-1/2" />
                      <Skeleton className="h-16 w-2/3" />
                    </div>
                  )}
                  {!loadingMsgs && messages.length === 0 && (
                    <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center text-[13px] text-muted-foreground">
                      {listening.length > 0 ? (
                        <>
                          <Ear className="size-6 text-success" />
                          <p>
                            <span className="font-medium text-foreground">{listening.map((a) => a.name).join(" and ")}</span>{" "}
                            {listening.length === 1 ? "is" : "are"} listening. Nothing has been said yet: the conversation
                            appears here when the other agent sends its first message.
                          </p>
                        </>
                      ) : (
                        <>
                          <MessagesSquare className="size-6" />
                          <p>Nothing has been said in this room yet.</p>
                        </>
                      )}
                    </div>
                  )}
                  {messages.map((m, i) => {
                    const prev = messages[i - 1];
                    const newDay = !prev || new Date(prev.ts * 1000).toDateString() !== new Date(m.ts * 1000).toDateString();
                    const grouped = !newDay && prev && prev.author === m.author && prev.human === m.human && m.ts - prev.ts < 120;
                    const done = !m.human && /^\s*done/i.test(m.text);
                    return (
                      <React.Fragment key={m.id}>
                        {newDay && (
                          <div className="flex items-center gap-3 py-1 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                            <span className="h-px flex-1 bg-border" />
                            {dayLabel(m.ts)}
                            <span className="h-px flex-1 bg-border" />
                          </div>
                        )}
                        <div className={cn("flex gap-2.5", m.human && "flex-row-reverse", grouped && "-mt-1.5")}>
                          {grouped ? (
                            <span className="w-7 shrink-0" />
                          ) : (
                            <span
                              aria-hidden="true"
                              className={cn(
                                "flex size-7 shrink-0 items-center justify-center rounded-full text-[11px] font-semibold",
                                m.human ? "bg-primary text-primary-foreground" : authorTone(m.author),
                              )}
                            >
                              {m.human ? "You" : initials(m.author)}
                            </span>
                          )}
                          <div className={cn("flex min-w-0 max-w-[85%] flex-col gap-1", m.human && "items-end")}>
                            {!grouped && (
                              <div className={cn("flex flex-wrap items-baseline gap-x-1.5 text-xs", m.human && "flex-row-reverse")}>
                                <span className="font-medium">{m.human ? "You" : m.author}</span>
                                {m.device && <span className="text-muted-foreground">@{m.device}</span>}
                                {m.to && <span className="text-muted-foreground">→ {m.to}</span>}
                                <span className="tabular-nums text-muted-foreground">{clock(m.ts)}</span>
                              </div>
                            )}
                            <div
                              className={cn(
                                "whitespace-pre-wrap break-words rounded-lg border px-3 py-2 text-[13.5px] leading-relaxed",
                                m.human && "border-primary/20 bg-primary/10",
                                done && "border-success/30 bg-success/10",
                                !m.human && !done && "bg-background",
                              )}
                            >
                              {m.text}
                            </div>
                          </div>
                        </div>
                      </React.Fragment>
                    );
                  })}
                </div>

                {canPost ? (
                  <form onSubmit={send} className="flex flex-col gap-2 border-t bg-muted/30 p-3 sm:flex-row sm:items-end">
                    <NativeSelect
                      value={to}
                      onChange={(e) => setTo(e.target.value)}
                      aria-label="Send to"
                      className="h-9 sm:w-36"
                    >
                      <option value="">To everyone</option>
                      {current.agents.map((a) => (
                        <option key={a.name} value={a.name}>
                          To {a.name}
                        </option>
                      ))}
                    </NativeSelect>
                    <Textarea
                      value={text}
                      onChange={(e) => setText(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                          e.preventDefault();
                          send();
                        }
                      }}
                      rows={1}
                      placeholder="Step in: e.g. keep going, or use the config value instead"
                      aria-label="Message"
                      className="max-h-40 min-h-9 flex-1 resize-none py-2"
                    />
                    <Button type="submit" disabled={sending || !text.trim()} className="shrink-0">
                      <Send /> Send
                    </Button>
                  </form>
                ) : (
                  <p className="flex items-center gap-2 border-t bg-muted/30 px-4 py-3 text-[13px] text-muted-foreground">
                    <Smartphone className="size-4 shrink-0" />
                    Read-only on this phone. To step in from here, allow it in Devices → Phones on your computer.
                  </p>
                )}
              </>
            ) : (
              <div className="flex flex-1 items-center justify-center p-6 text-sm text-muted-foreground">Choose a room.</div>
            )}
          </section>
        </div>
      )}
    </div>
  );
}
