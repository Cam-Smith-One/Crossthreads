// The board: every thread (a piece of work, one or more sessions linked across
// tools), grouped by repository. This is the home screen; search sits above it.

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  getThread,
  getThreads,
  linkSessions,
  rejectLink,
  setThread,
  unlinkSession,
  type ThreadDetail,
  type ThreadStatus,
  type ThreadSummary,
} from "./api";
import { toolIcon, toolLabel } from "./tools";

const STATUSES: ThreadStatus[] = ["active", "parked", "done"];
const NO_REPO = "No repository";
const PAGE = 60;

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
  /** Open one session's transcript. */
  onOpenSession: (conversationId: string) => void;
  /** Changes when the index does, so the board reloads. */
  refreshKey: number;
  onError: (message: string) => void;
}

export function Board({ onOpenSession, refreshKey, onError }: Props) {
  const [threads, setThreads] = useState<ThreadSummary[] | null>(null);
  const [status, setStatus] = useState<StatusFilter>("active");
  const [tool, setTool] = useState("");
  const [text, setText] = useState("");
  const [shown, setShown] = useState(PAGE);
  const [open, setOpen] = useState<ThreadDetail | null>(null);
  const [busy, setBusy] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);

  const load = useCallback(() => {
    getThreads()
      .then(setThreads)
      .catch((e) => onError(String(e)));
  }, [onError]);

  useEffect(load, [load, refreshKey]);

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

  // Group by repository, keeping the most recently active repository first
  // (the list arrives sorted by last activity).
  const groups = useMemo(() => {
    const out = new Map<string, ThreadSummary[]>();
    for (const t of visible.slice(0, shown)) {
      const key = t.repo_label ?? NO_REPO;
      const list = out.get(key);
      if (list) list.push(t);
      else out.set(key, [t]);
    }
    return [...out.entries()];
  }, [visible, shown]);

  async function openThread(id: string) {
    try {
      setRenaming(null);
      setOpen(await getThread(id));
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
    if (!open) return [];
    return (threads ?? [])
      .filter((t) => t.id !== open.id && (t.repo_key ?? null) === (open.repo_key ?? null))
      .slice(0, 50);
  }, [threads, open]);

  async function mergeWith(otherId: string) {
    if (!open || !otherId) return;
    const anchor = open.sessions[0].conversation_id;
    await change(anchor, async () => {
      const other = await getThread(otherId);
      if (other) await linkSessions(anchor, other.sessions[0].conversation_id);
    });
  }

  if (!threads || threads.length === 0) return null;

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

      {visible.length === 0 && <p className="empty">No {status === "all" ? "" : status} threads match.</p>}

      {groups.map(([repo, list]) => (
        <div key={repo} className="board-group">
          <h3 className="board-repo">
            {repo} <span className="board-repo-count">{list.length}</span>
          </h3>
          {list.map((t) => (
            <button key={t.id} className="board-row" onClick={() => openThread(t.id)}>
              <span className="board-row-main">
                <span className="board-title">{t.title}</span>
                <span className="board-meta">
                  <ToolChips tools={t.tools} />
                  {t.session_count > 1 && (
                    <span className="board-badge">{t.session_count} sessions</span>
                  )}
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
          ))}
        </div>
      ))}

      {visible.length > shown && (
        <button className="load-more" onClick={() => setShown(shown + PAGE)}>
          Show more ({visible.length - shown} left)
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
              </div>
              <div className="modal-head-actions">
                {renaming === null && (
                  <button onClick={() => setRenaming(open.title)} disabled={busy}>
                    Rename
                  </button>
                )}
                <button onClick={() => setOpen(null)}>Close</button>
              </div>
            </div>

            <div className="thread-body">
              <div className="thread-status" role="group" aria-label="Status">
                {STATUSES.map((s) => (
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
                      </div>
                      <div className="thread-session-meta">
                        <span>
                          {when(s.started_at)} · {s.message_count} messages
                          {s.branch ? ` · ⎇ ${s.branch}` : ""}
                        </span>
                        <span className="thread-session-actions">
                          <button onClick={() => onOpenSession(s.conversation_id)}>Open</button>
                          {stays && (
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

              {open.suggestions.length > 0 && (
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
