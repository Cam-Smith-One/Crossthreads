// The board: every thread (a piece of work, one or more sessions linked across
// tools), grouped by repository. This is the home screen; search sits above it.

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  getResume,
  getThread,
  getThreadHandoff,
  getThreads,
  linkSessions,
  rejectLink,
  setThread,
  unlinkSession,
  type Live,
  type ResumePlan,
  type ThreadDetail,
  type ThreadStatus,
  type ThreadSummary,
} from "./api";
import { toolIcon, toolLabel } from "./tools";

const STATUSES: ThreadStatus[] = ["active", "parked", "done"];
const NO_REPO = "No repository";
const PAGE = 60;
/** How often the board re-reads live state while it is on screen. */
const REFRESH_MS = 10_000;

type StatusFilter = ThreadStatus | "all";

/** "today", "yesterday", "3d ago", then the date. */
function ago(ts?: string | null): string {
  if (!ts) return "undated";
  const then = new Date(ts);
  if (Number.isNaN(then.getTime())) return ts.slice(0, 10);
  const days = Math.floor((Date.now() - then.getTime()) / 86_400_000);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 14) return `${days}d ago`;
  return ts.slice(0, 10);
}

function when(ts?: string | null): string {
  if (!ts) return "undated";
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** A pill for a state worth interrupting for; nothing for idle or ended. */
function LiveBadge({ live }: { live?: Live | null }) {
  if (live?.state === "listening") {
    return (
      <span className="board-live board-live-listening" title="Waiting in a room for another agent">
        <span className="board-live-dot" aria-hidden="true" />
        Listening
      </span>
    );
  }
  if (!live || (live.state !== "needs_you" && live.state !== "running")) return null;
  const needs = live.state === "needs_you";
  return (
    <span
      className={`board-live ${needs ? "board-live-needs" : "board-live-running"}`}
      title={
        live.inferred
          ? "Guessed from the transcript file. Install hooks for exact state: crossthreads hooks install"
          : needs
            ? "Waiting on a permission prompt"
            : "Working now"
      }
    >
      <span className="board-live-dot" aria-hidden="true" />
      {needs ? "Needs you" : "Running"}
    </span>
  );
}

function ToolChips({ tools }: { tools: string[] }) {
  return (
    <span className="board-tools">
      {tools.map((t) => (
        <span key={t} className="tool-chip" title={toolLabel(t)}>
          <span className="tool-chip-icon">{toolIcon(t)}</span>
          {toolLabel(t)}
        </span>
      ))}
    </span>
  );
}

interface Props {
  /** Open one session's transcript, from another device when `device` is set. */
  onOpenSession: (conversationId: string, device?: string | null) => void;
  /** This device's name when cross-device search is set up. */
  localDevice?: string;
  /** Changes when the index does, so the board reloads. */
  refreshKey: number;
  onError: (message: string) => void;
  /** A session whose thread should be opened (e.g. from the transcript view). */
  openThreadFor?: string | null;
  onThreadOpened?: () => void;
  /** No editing at all (a phone). */
  readOnly?: boolean;
}

export function Board({
  onOpenSession,
  localDevice,
  refreshKey,
  onError,
  openThreadFor,
  onThreadOpened,
  readOnly: everythingReadOnly = false,
}: Props) {
  const [threads, setThreads] = useState<ThreadSummary[] | null>(null);
  const [unreachable, setUnreachable] = useState<string[]>([]);
  const [status, setStatus] = useState<StatusFilter>("active");
  const [tool, setTool] = useState("");
  const [text, setText] = useState("");
  const [shown, setShown] = useState(PAGE);
  const [open, setOpen] = useState<ThreadDetail | null>(null);
  const [busy, setBusy] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  // How to continue the open thread: the latest session's resume command, and
  // whether a handoff brief was just copied.
  const [resume, setResume] = useState<ResumePlan | null>(null);
  const [copied, setCopied] = useState<"resume" | "handoff" | null>(null);

  // Opened from elsewhere (the transcript view's "Thread" button).
  useEffect(() => {
    if (!openThreadFor) return;
    getThread(openThreadFor)
      .then((t) => {
        setRenaming(null);
        setOpen(t);
      })
      .catch((e) => onError(String(e)))
      .finally(() => onThreadOpened?.());
  }, [openThreadFor, onError, onThreadOpened]);

  // The open thread's latest session, and how to resume it in its own tool.
  const latest = open?.sessions[open.sessions.length - 1];
  const latestId = latest?.conversation_id;
  const latestRemote = !!open && !!open.device && !!localDevice && open.device !== localDevice;
  useEffect(() => {
    setCopied(null);
    setResume(null);
    if (!latestId || latestRemote) return;
    let cancelled = false;
    getResume(latestId)
      .then((plan) => !cancelled && setResume(plan))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [latestId, latestRemote]);

  async function copy(what: "resume" | "handoff") {
    if (!open) return;
    try {
      const text =
        what === "resume"
          ? resume?.action.kind === "resume"
            ? resume.action.display
            : ""
          : await getThreadHandoff(open.id, latestRemote ? open.device : null);
      if (!text) return;
      await navigator.clipboard.writeText(text);
      setCopied(what);
    } catch (e) {
      onError(String(e));
    }
  }

  const load = useCallback(() => {
    getThreads()
      .then(({ threads, unreachable }) => {
        setThreads(threads);
        setUnreachable(unreachable);
      })
      .catch((e) => onError(String(e)));
  }, [onError]);

  useEffect(load, [load, refreshKey]);

  // Live state goes stale quickly, so re-read it while the page is visible.
  // A failed refresh is silent: the next one will say if the daemon is gone.
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.visibilityState !== "visible") return;
      getThreads()
        .then(({ threads, unreachable }) => {
          setThreads(threads);
          setUnreachable(unreachable);
        })
        .catch(() => {});
    }, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, []);

  /** True when a thread lives on another device (read-only from here). */
  const isRemote = useCallback(
    (t: { device?: string | null }) => !!t.device && !!localDevice && t.device !== localDevice,
    [localDevice],
  );

  const counts = useMemo(() => {
    const c: Record<StatusFilter, number> = { active: 0, parked: 0, done: 0, all: 0 };
    for (const t of threads ?? []) {
      c[t.status] += 1;
      c.all += 1;
    }
    return c;
  }, [threads]);

  const tools = useMemo(
    () => [...new Set((threads ?? []).flatMap((t) => t.tools))].sort(),
    [threads],
  );

  const visible = useMemo(() => {
    const want = text.trim().toLowerCase();
    return (threads ?? []).filter(
      (t) =>
        (status === "all" || t.status === status) &&
        (!tool || t.tools.includes(tool)) &&
        (!want ||
          t.title.toLowerCase().includes(want) ||
          (t.repo_label ?? "").toLowerCase().includes(want) ||
          t.branches.some((b) => b.toLowerCase().includes(want))),
    );
  }, [threads, status, tool, text]);

  // Anything blocked on you is pulled out to the top, whatever its repository.
  const waiting = useMemo(() => visible.filter((t) => t.live?.state === "needs_you"), [visible]);

  // Group by repository, keeping the most recently active repository first
  // (the list arrives sorted by last activity).
  const groups = useMemo(() => {
    const out = new Map<string, ThreadSummary[]>();
    for (const t of visible.filter((t) => t.live?.state !== "needs_you").slice(0, shown)) {
      const key = t.repo_label ?? NO_REPO;
      const list = out.get(key);
      if (list) list.push(t);
      else out.set(key, [t]);
    }
    return [...out.entries()];
  }, [visible, shown]);

  async function openThread(t: ThreadSummary) {
    try {
      setRenaming(null);
      setOpen(await getThread(t.id, isRemote(t) ? t.device : null));
    } catch (e) {
      onError(String(e));
    }
  }

  // Run a change, then reload the list and the open thread. A thread's id can
  // change when sessions join or leave, so re-open it through one of its
  // sessions.
  async function change(anchor: string | null, action: () => Promise<unknown>) {
    setBusy(true);
    try {
      await action();
      load();
      setOpen(anchor ? await getThread(anchor) : null);
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  // Other threads in the same repository, offered for a manual merge.
  const mergeable = useMemo(() => {
    if (!open || everythingReadOnly || isRemote(open)) return [];
    return (threads ?? [])
      .filter(
        (t) =>
          !isRemote(t) && t.id !== open.id && (t.repo_key ?? null) === (open.repo_key ?? null),
      )
      .slice(0, 50);
  }, [threads, open, isRemote, everythingReadOnly]);

  async function mergeWith(otherId: string) {
    if (!open || !otherId) return;
    const anchor = open.sessions[0].conversation_id;
    await change(anchor, async () => {
      const other = await getThread(otherId);
      if (other) await linkSessions(anchor, other.sessions[0].conversation_id);
    });
  }

  if (!threads || threads.length === 0) return null;

  const readOnly = everythingReadOnly || (!!open && isRemote(open));
  const row = (t: ThreadSummary) => (
    <button key={`${t.device ?? ""}:${t.id}`} className="board-row" onClick={() => openThread(t)}>
      <span className="board-row-main">
        <span className="board-title">{t.title}</span>
        <span className="board-meta">
          <LiveBadge live={t.live} />
          <ToolChips tools={t.tools} />
          {isRemote(t) && (
            <span className="device-chip" title={`on ${t.device}`}>
              💻 {t.device}
            </span>
          )}
          {t.session_count > 1 && <span className="board-badge">{t.session_count} sessions</span>}
          {t.branches.length > 0 && (
            <span className="board-branch" title={t.branches.join(", ")}>
              ⎇ {t.branches[t.branches.length - 1]}
            </span>
          )}
          {t.pending_suggestions > 0 && (
            <span className="board-badge board-badge-hint" title="Possibly related sessions to review">
              {t.pending_suggestions} suggested
            </span>
          )}
          {t.status !== "active" && (
            <span className={`board-badge board-status-${t.status}`}>{t.status}</span>
          )}
        </span>
      </span>
      <span className="board-when" title={t.last_activity ?? ""}>
        {ago(t.last_activity)}
      </span>
    </button>
  );

  return (
    <section className="board" aria-label="Threads">
      <div className="board-head">
        <h2>Threads</h2>
        <div className="board-tabs" role="tablist" aria-label="Thread status">
          {(["active", "parked", "done", "all"] as StatusFilter[]).map((s) => (
            <button
              key={s}
              role="tab"
              aria-selected={status === s}
              className={`board-tab ${status === s ? "on" : ""}`}
              onClick={() => {
                setStatus(s);
                setShown(PAGE);
              }}
            >
              {s} <span className="board-tab-count">{counts[s]}</span>
            </button>
          ))}
        </div>
      </div>
      <div className="board-filters">
        <input
          placeholder="filter by title, repository or branch…"
          value={text}
          onChange={(e) => setText(e.target.value)}
          aria-label="Filter threads"
        />
        {tools.length > 1 && (
          <select value={tool} onChange={(e) => setTool(e.target.value)} aria-label="Tool">
            <option value="">all tools</option>
            {tools.map((t) => (
              <option key={t} value={t}>
                {toolLabel(t)}
              </option>
            ))}
          </select>
        )}
      </div>

      {unreachable.length > 0 && (
        <div className="unreachable-banner">
          ⚠ {unreachable.length} device{unreachable.length > 1 ? "s" : ""} unreachable (
          {unreachable.join(", ")}) — their threads are missing from this list.
        </div>
      )}

      {waiting.length > 0 && (
        <div className="board-group board-waiting">
          <h3 className="board-repo">
            Needs you <span className="board-repo-count">{waiting.length}</span>
          </h3>
          {waiting.map(row)}
        </div>
      )}

      {visible.length === 0 && <p className="empty">No {status === "all" ? "" : status} threads match.</p>}

      {groups.map(([repo, list]) => (
        <div key={repo} className="board-group">
          <h3 className="board-repo">
            {repo} <span className="board-repo-count">{list.length}</span>
          </h3>
          {list.map(row)}
        </div>
      ))}

      {visible.length - waiting.length > shown && (
        <button className="load-more" onClick={() => setShown(shown + PAGE)}>
          Show more ({visible.length - waiting.length - shown} left)
        </button>
      )}

      {open && (
        <div className="overlay" onClick={() => setOpen(null)}>
          <div
            className="modal thread-modal"
            role="dialog"
            aria-modal="true"
            aria-label={`Thread: ${open.title}`}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="modal-head">
              <div className="thread-head-main">
                {renaming === null ? (
                  <strong className="thread-title">{open.title}</strong>
                ) : (
                  <form
                    className="thread-rename"
                    onSubmit={(e) => {
                      e.preventDefault();
                      const title = renaming;
                      setRenaming(null);
                      change(open.sessions[0].conversation_id, () => setThread(open.id, { title }));
                    }}
                  >
                    <input
                      autoFocus
                      value={renaming}
                      onChange={(e) => setRenaming(e.target.value)}
                      placeholder="leave empty to use the first session's title"
                      aria-label="Thread title"
                    />
                    <button type="submit">Save</button>
                  </form>
                )}
                <div className="project">
                  {open.repo_label ?? NO_REPO} · {open.session_count} session
                  {open.session_count === 1 ? "" : "s"} · {open.message_count} messages
                  {open.branches.length > 0 && <> · ⎇ {open.branches.join(", ")}</>}
                </div>
                {readOnly && (
                  <div className="project">
                    {everythingReadOnly
                      ? "📱 Read-only on your phone. Rename or regroup it on your computer."
                      : `💻 On ${open.device}. Open Crossthreads there to rename or regroup it.`}
                  </div>
                )}
              </div>
              <div className="modal-head-actions">
                {renaming === null && !readOnly && (
                  <button onClick={() => setRenaming(open.title)} disabled={busy}>
                    Rename
                  </button>
                )}
                <button onClick={() => setOpen(null)}>Close</button>
              </div>
            </div>

            <div className="thread-body">
              <div className="thread-status" role="group" aria-label="Status">
                {(readOnly ? [open.status] : STATUSES).map((s) => (
                  <button
                    key={s}
                    className={`board-tab ${open.status === s ? "on" : ""}`}
                    aria-pressed={open.status === s}
                    disabled={busy || open.status === s}
                    onClick={() =>
                      change(open.sessions[0].conversation_id, () => setThread(open.id, { status: s }))
                    }
                  >
                    {s}
                  </button>
                ))}
              </div>

              <div className="thread-continue">
                <div className="thread-continue-text">
                  <strong>Pick this up</strong>
                  <span>
                    {latest
                      ? `Latest: ${toolLabel(latest.tool)}, ${ago(latest.last_activity)}.`
                      : ""}{" "}
                    Resume it where it was, or copy a handoff to paste into another tool or agent.
                  </span>
                </div>
                <div className="thread-continue-actions">
                  {resume?.action.kind === "resume" && (
                    <button
                      onClick={() => copy("resume")}
                      title={resume.action.display}
                    >
                      {copied === "resume" ? "Copied ✓" : `Copy resume command`}
                    </button>
                  )}
                  <button
                    className="primary"
                    onClick={() => copy("handoff")}
                    title="The goal, every session, and where the latest one left off, as Markdown"
                  >
                    {copied === "handoff" ? "Copied ✓" : "Copy handoff"}
                  </button>
                </div>
              </div>

              <ol className="thread-timeline">
                {open.sessions.map((s) => {
                  const link = open.links.find((l) => l.to === s.conversation_id);
                  const from = link && open.sessions.find((x) => x.conversation_id === link.from);
                  // Removing a session re-opens the thread through one that stays.
                  const stays = open.sessions.find((x) => x.conversation_id !== s.conversation_id);
                  return (
                    <li key={s.session_key} className="thread-session">
                      {link && (
                        <div className="thread-link">
                          {link.kind === "manual"
                            ? "Linked by you"
                            : `${link.kind}${from ? ` from ${toolLabel(from.tool)}` : ""} · ${
                                link.origin === "detected"
                                  ? "detected automatically"
                                  : link.origin === "manual"
                                    ? "confirmed by you"
                                    : "recorded when it happened"
                              }`}
                        </div>
                      )}
                      <div className="thread-session-row">
                        <span className="tool-chip">
                          <span className="tool-chip-icon">{toolIcon(s.tool)}</span>
                          {toolLabel(s.tool)}
                        </span>
                        <span className="thread-session-title">{s.title ?? "(untitled)"}</span>
                        <LiveBadge live={s.live} />
                      </div>
                      <div className="thread-session-meta">
                        <span>
                          {when(s.started_at)} · {s.message_count} messages
                          {s.branch ? ` · ⎇ ${s.branch}` : ""}
                        </span>
                        <span className="thread-session-actions">
                          <button
                            onClick={() =>
                              onOpenSession(s.conversation_id, readOnly ? open.device : null)
                            }
                          >
                            Open
                          </button>
                          {stays && !readOnly && (
                            <button
                              disabled={busy}
                              title="Take this session out of the thread"
                              onClick={() =>
                                change(stays.conversation_id, () => unlinkSession(s.conversation_id))
                              }
                            >
                              Remove
                            </button>
                          )}
                        </span>
                      </div>
                    </li>
                  );
                })}
              </ol>

              {open.suggestions.length > 0 && !readOnly && (
                <div className="thread-suggestions">
                  <h3>Possibly the same work</h3>
                  <p className="settings-hint">
                    Started in another tool around the same time, in this repository. Not linked
                    until you say so.
                  </p>
                  {open.suggestions.map((sg) => (
                    <div key={sg.session.session_key} className="thread-suggestion">
                      <span className="tool-chip">
                        <span className="tool-chip-icon">{toolIcon(sg.session.tool)}</span>
                        {toolLabel(sg.session.tool)}
                      </span>
                      <button
                        className="thread-suggestion-title"
                        title="Open this session"
                        onClick={() => onOpenSession(sg.session.conversation_id)}
                      >
                        {sg.session.title ?? "(untitled)"}
                      </button>
                      <span className="thread-session-actions">
                        <button
                          disabled={busy}
                          onClick={() =>
                            change(sg.with, () =>
                              linkSessions(sg.with, sg.session.conversation_id, sg.kind),
                            )
                          }
                        >
                          Add to thread
                        </button>
                        <button
                          disabled={busy}
                          onClick={() =>
                            change(sg.with, () => rejectLink(sg.with, sg.session.conversation_id))
                          }
                        >
                          Not related
                        </button>
                      </span>
                    </div>
                  ))}
                </div>
              )}

              {mergeable.length > 0 && (
                <label className="thread-merge">
                  Merge another thread into this one
                  <select value="" disabled={busy} onChange={(e) => mergeWith(e.target.value)}>
                    <option value="">choose a thread in {open.repo_label ?? "no repository"}…</option>
                    {mergeable.map((t) => (
                      <option key={t.id} value={t.id}>
                        {ago(t.last_activity)} · {t.tools.map(toolLabel).join(" + ")} · {t.title}
                      </option>
                    ))}
                  </select>
                </label>
              )}
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
