// Rooms: where your agents talk to each other, and where you step in.
// A header button (with a dot while any agent is listening) opens the view.

import { useCallback, useEffect, useRef, useState } from "react";
import { getRoomMessages, getRooms, postToRoom, type RoomMessage, type RoomView } from "./api";

/** Refresh while the view is closed (just for the button's dot) and open. */
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

export function Rooms({
  onError,
  canPost = true,
  active,
  onListening,
}: {
  onError: (message: string) => void;
  /** False on a phone that hasn't been allowed to post. */
  canPost?: boolean;
  /** The Rooms page is showing. When it isn't, the list is still polled
   * slowly so the sidebar can show that an agent is waiting. */
  active: boolean;
  onListening?: (listening: boolean) => void;
}) {
  const open = active;
  const [rooms, setRooms] = useState<RoomView[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [messages, setMessages] = useState<RoomMessage[]>([]);
  const [text, setText] = useState("");
  const [to, setTo] = useState("");
  const [sending, setSending] = useState(false);
  const stream = useRef<HTMLDivElement>(null);
  // The newest message id we hold for the selected room, read by the poller.
  const last = useRef(0);

  const current = rooms.find((r) => r.room === selected) ?? null;
  const anyoneListening = rooms.some((r) => r.agents.some((a) => a.listening));
  useEffect(() => {
    onListening?.(anyoneListening);
  }, [anyoneListening, onListening]);

  const refresh = useCallback(async () => {
    const list = await getRooms();
    setRooms(list);
    return list;
  }, []);

  // Keep the room list fresh: slowly when closed, quickly when open.
  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      if (document.visibilityState !== "visible") return;
      refresh()
        .then((list) => {
          if (!cancelled && open) setSelected((s) => s ?? list[0]?.room ?? null);
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
  useEffect(() => {
    if (!open || !selected) return;
    let cancelled = false;
    last.current = 0;
    setMessages([]);
    const pull = () =>
      getRoomMessages(selected, last.current)
        .then((fresh) => {
          if (cancelled || fresh.length === 0) return;
          last.current = fresh[fresh.length - 1].id;
          setMessages((old) => [...old, ...fresh].slice(-500));
        })
        .catch(() => {});
    pull();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") pull();
    }, OPEN_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [open, selected]);

  // Follow the conversation as it grows.
  useEffect(() => {
    stream.current?.scrollTo({ top: stream.current.scrollHeight });
  }, [messages]);

  async function send(e: React.FormEvent) {
    e.preventDefault();
    if (!selected || !text.trim()) return;
    setSending(true);
    try {
      await postToRoom(selected, text, to || undefined);
      setText("");
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

  return (
    <section className="page rooms-page">
      <div className="page-head">
        <h2>Rooms</h2>
        <p className="page-lede">
          Where your agents hand work to each other, one room per repository. Watch any
          conversation live; what you type here overrides them.
        </p>
      </div>
      {rooms.length === 0 ? (
        <div className="rooms-empty">
          <p>No room has been used yet.</p>
          <p className="settings-hint">
            Open two agents in the same project. In the one that should do the work, say{" "}
            <code>/crossthreads listen</code>. In the other, say{" "}
            <code>/crossthreads lead &lt;the task&gt;</code>. They find each other through
            the repository and the conversation appears here.
          </p>
          <p className="settings-hint">
            Run <code>crossthreads hooks install</code> once so an agent that stops mid-task
            is woken when the reply arrives.
          </p>
        </div>
      ) : (
        <div className="rooms-body">
          <div className="rooms-tabs" role="tablist" aria-label="Rooms">
            {rooms.map((r) => (
              <button
                key={r.room}
                role="tab"
                aria-selected={r.room === selected}
                className={`board-tab ${r.room === selected ? "on" : ""}`}
                onClick={() => setSelected(r.room)}
                title={r.room}
              >
                {r.label}
                {r.agents.some((a) => a.listening) && (
                  <span className="rooms-dot" aria-label="an agent is listening" />
                )}
              </button>
            ))}
          </div>

          {current && (
            <div className="rooms-status">
              {current.agents.map((a) => (
                <span key={a.name} className={`rooms-agent ${a.listening ? "on" : ""}`}>
                  <span className="rooms-dot" aria-hidden="true" />
                  {a.name} {a.listening ? "listening" : since(a.seen)}
                </span>
              ))}
              <span className="rooms-turns">
                {current.closed
                  ? "exchange closed"
                  : `${current.turns}${current.max_turns ? `/${current.max_turns}` : ""} turn${
                      current.turns === 1 ? "" : "s"
                    } this exchange`}
              </span>
              {current.claims.map((c) => (
                <span key={c.path} className="rooms-claim" title={c.path}>
                  🔒 {c.path.split(/[\\/]/).slice(-2).join("/")} · {c.holder} until{" "}
                  {clock(c.expires)}
                </span>
              ))}
            </div>
          )}

          <div className="rooms-stream" ref={stream} aria-live="polite">
            {messages.map((m) => (
              <div
                key={m.id}
                className={`rooms-msg ${m.human ? "human" : ""} ${
                  /^\s*done/i.test(m.text) && !m.human ? "done" : ""
                }`}
              >
                <div className="rooms-msg-head">
                  <strong>
                    {m.author}
                    {m.device && <span className="rooms-device">@{m.device}</span>}
                  </strong>
                  {m.human && <span className="rooms-you">you</span>}
                  {m.to && <span className="rooms-to">→ {m.to}</span>}
                  <span className="rooms-time">{clock(m.ts)}</span>
                </div>
                <div className="rooms-text">{m.text}</div>
              </div>
            ))}
          </div>

          {!canPost && (
            <p className="rooms-readonly">
              Read-only on this phone. To step in from here, allow it in Settings → Phone on
              your computer.
            </p>
          )}
          {canPost && (
            <form className="rooms-compose" onSubmit={send}>
              <select value={to} onChange={(e) => setTo(e.target.value)} aria-label="Recipient">
                <option value="">everyone</option>
                {current?.agents.map((a) => (
                  <option key={a.name} value={a.name}>
                    {a.name}
                  </option>
                ))}
              </select>
              <input
                value={text}
                onChange={(e) => setText(e.target.value)}
                placeholder="Step in: e.g. keep going, or use the config value instead"
                aria-label="Message"
              />
              <button type="submit" disabled={sending || !text.trim()}>
                Send
              </button>
            </form>
          )}
        </div>
      )}
    </section>
  );
}
