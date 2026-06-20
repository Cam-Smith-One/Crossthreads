import { useCallback, useEffect, useState } from "react";
import {
  buildContext,
  getConversation,
  getFacets,
  getSaved,
  getStatus,
  openSource,
  search,
  setFlags,
  type Filters,
  type Hit,
  type Mode,
  type Status,
  type StoredConversation,
} from "./api";

const MODES: Mode[] = ["hybrid", "semantic", "lexical"];

export function App() {
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<Mode>("hybrid");
  const [filters, setFilters] = useState<Filters>({});
  const [hits, setHits] = useState<Hit[]>([]);
  const [selected, setSelected] = useState(-1);
  const [status, setStatus] = useState<Status | null>(null);
  const [tools, setTools] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [searched, setSearched] = useState(false);
  const [context, setContext] = useState<string | null>(null);
  const [open, setOpen] = useState<StoredConversation | null>(null);
  const [saved, setSaved] = useState<Hit[]>([]);
  const [theme, setTheme] = useState<string>(
    () => document.documentElement.dataset.theme || "dark",
  );

  const refreshSaved = useCallback(() => {
    getSaved().then(setSaved).catch(() => {});
  }, []);

  function toggleTheme() {
    const next = theme === "dark" ? "light" : "dark";
    setTheme(next);
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem("ct-theme", next);
    } catch {
      /* ignore storage failures */
    }
  }

  useEffect(() => {
    getStatus().then(setStatus).catch((e) => setError(String(e)));
    getFacets().then(setTools).catch(() => {});
    refreshSaved();
  }, [refreshSaved]);

  const runSearch = useCallback(
    async (e?: React.FormEvent) => {
      e?.preventDefault();
      if (!query.trim()) return;
      setLoading(true);
      setError(null);
      setContext(null);
      try {
        const results = await search(query, mode, filters);
        setHits(results);
        setSelected(results.length ? 0 : -1);
        setSearched(true);
      } catch (err) {
        setError(String(err));
      } finally {
        setLoading(false);
      }
    },
    [query, mode, filters],
  );

  async function openConversation(id: string) {
    try {
      const convo = await getConversation(id);
      if (convo) setOpen(convo);
    } catch (err) {
      setError(String(err));
    }
  }

  async function showContext() {
    setLoading(true);
    setError(null);
    try {
      setContext((await buildContext(query, mode, filters)).markdown);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  // Toggle a bookmark/pin flag and reflect it everywhere it's shown.
  async function toggleFlag(id: string, patch: { bookmarked?: boolean; pinned?: boolean }) {
    try {
      await setFlags(id, patch);
      const apply = (h: Hit): Hit => (h.conversation_id === id ? { ...h, ...patch } : h);
      setHits((hs) => hs.map(apply));
      setOpen((o) => (o && o.id === id ? { ...o, ...patch } : o));
      refreshSaved();
    } catch (err) {
      setError(String(err));
    }
  }

  async function reveal(id: string) {
    try {
      if (!(await openSource(id))) setError("Couldn't open the source file.");
    } catch (err) {
      setError(String(err));
    }
  }

  // Keyboard navigation over results (j/k or arrows; Enter opens; Esc closes).
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (open) {
        if (e.key === "Escape") setOpen(null);
        return;
      }
      if (!hits.length) return;
      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        setSelected((s) => Math.min(s + 1, hits.length - 1));
      } else if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        setSelected((s) => Math.max(s - 1, 0));
      } else if (e.key === "Enter" && selected >= 0 && document.activeElement?.tagName !== "INPUT") {
        openConversation(hits[selected].conversation_id);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [hits, selected, open]);

  const setF = (patch: Partial<Filters>) => setFilters((f) => ({ ...f, ...patch }));

  // Pin / bookmark toggles, shared by result rows and the saved panel.
  const flagButtons = (h: Pick<Hit, "conversation_id" | "pinned" | "bookmarked">) => (
    <span className="hit-actions" onClick={(e) => e.stopPropagation()}>
      <button
        className={`icon ${h.pinned ? "on" : ""}`}
        title={h.pinned ? "Unpin" : "Pin to top"}
        onClick={() => toggleFlag(h.conversation_id, { pinned: !h.pinned })}
      >
        {h.pinned ? "📌" : "📍"}
      </button>
      <button
        className={`icon ${h.bookmarked ? "on" : ""}`}
        title={h.bookmarked ? "Remove bookmark" : "Bookmark"}
        onClick={() => toggleFlag(h.conversation_id, { bookmarked: !h.bookmarked })}
      >
        {h.bookmarked ? "🔖" : "🏷️"}
      </button>
    </span>
  );

  return (
    <div className="app">
      <header>
        <div className="brand-row">
          <img className="brand-mark" src="/mark.png" alt="Crossthreads" />
          <h1 className="brand-name">
            <span className="brand-cross">Cross</span>
            <span className="brand-threads">threads</span>
          </h1>
          <button
            className="theme-toggle"
            onClick={toggleTheme}
            title={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
            aria-label="Toggle theme"
          >
            {theme === "dark" ? "☀️" : "🌙"}
          </button>
        </div>
        <p className="tagline">Search every AI coding conversation, across tools.</p>
        {status && (
          <p className="status">
            {status.conversations.toLocaleString()} conversations ·{" "}
            {status.embeddings.toLocaleString()} embeddings · {status.embedder}
          </p>
        )}
      </header>

      <form className="searchbar" onSubmit={runSearch}>
        <input
          autoFocus
          placeholder="e.g. where did we set up the OAuth refresh retry?"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <select value={mode} onChange={(e) => setMode(e.target.value as Mode)}>
          {MODES.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
        <button type="submit" disabled={loading}>
          {loading ? "…" : "Search"}
        </button>
        {searched && hits.length > 0 && (
          <button type="button" className="secondary" onClick={showContext} disabled={loading}>
            Build context
          </button>
        )}
      </form>

      <div className="filters">
        <select value={filters.kind ?? ""} onChange={(e) => setF({ kind: e.target.value || undefined })}>
          <option value="">threads + skills</option>
          <option value="thread">threads</option>
          <option value="skill">skills</option>
        </select>
        <select value={filters.tool ?? ""} onChange={(e) => setF({ tool: e.target.value || undefined })}>
          <option value="">all tools</option>
          {tools.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </select>
        <input
          placeholder="project contains…"
          value={filters.project ?? ""}
          onChange={(e) => setF({ project: e.target.value || undefined })}
        />
        <label>
          since <input type="date" value={filters.since ?? ""} onChange={(e) => setF({ since: e.target.value || undefined })} />
        </label>
        <label>
          until <input type="date" value={filters.until ?? ""} onChange={(e) => setF({ until: e.target.value || undefined })} />
        </label>
      </div>

      {error && <div className="error">{error}</div>}

      {saved.length > 0 && !searched && (
        <section className="saved">
          <h2>Saved &amp; pinned</h2>
          <div className="saved-list">
            {saved.map((h) => (
              <div
                key={h.conversation_id}
                className="saved-item"
                onClick={() => openConversation(h.conversation_id)}
              >
                <span className={`tool tool-${h.tool}`}>{h.tool}</span>
                <span className="title">{h.title ?? "(untitled)"}</span>
                {h.started_at && <span className="date">{h.started_at.slice(0, 10)}</span>}
                {flagButtons(h)}
              </div>
            ))}
          </div>
        </section>
      )}

      {context !== null && (
        <section className="context">
          <div className="context-head">
            <h2>Context block</h2>
            <button className="secondary" onClick={() => navigator.clipboard?.writeText(context)}>
              Copy
            </button>
          </div>
          <pre>{context}</pre>
        </section>
      )}

      <section className="results">
        {searched && hits.length === 0 && !loading && <p className="empty">No matches.</p>}
        {hits.map((h, i) => (
          <article
            key={h.conversation_id}
            className={`hit ${i === selected ? "selected" : ""}`}
            onClick={() => {
              setSelected(i);
              openConversation(h.conversation_id);
            }}
          >
            <div className="hit-head">
              {h.kind === "skill" && <span className="badge-skill">skill</span>}
              <span className={`tool tool-${h.tool}`}>{h.tool}</span>
              <span className="title">{h.title ?? "(untitled)"}</span>
              {h.started_at && <span className="date">{h.started_at.slice(0, 10)}</span>}
              {flagButtons(h)}
            </div>
            {h.project && <div className="project">{h.project}</div>}
            <div className="snippet" dangerouslySetInnerHTML={{ __html: highlight(h.snippet) }} />
          </article>
        ))}
      </section>

      {open && (
        <div className="overlay" onClick={() => setOpen(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-head">
              <div>
                <span className={`tool tool-${open.tool}`}>{open.tool}</span>{" "}
                <strong>{open.title ?? "(untitled)"}</strong>
                {open.project && <div className="project">{open.project}</div>}
              </div>
              <div className="modal-head-actions">
                {flagButtons({
                  conversation_id: open.id,
                  pinned: open.pinned,
                  bookmarked: open.bookmarked,
                })}
                <button className="secondary" onClick={() => setOpen(null)}>
                  Close
                </button>
              </div>
            </div>
            <div className="transcript">
              {open.messages.map((m, i) => (
                <div key={i} className={`turn turn-${m.role}`}>
                  <span className="role">{m.role}</span>
                  <div className="content">{m.content}</div>
                </div>
              ))}
            </div>
            {open.source_path && (
              <div className="modal-foot">
                <div className="source">
                  <span className="source-label">source</span>
                  <code>{open.source_path}</code>
                </div>
                <div className="source-actions">
                  <button
                    className="secondary"
                    title="Reveal the original chat file in your file manager"
                    onClick={() => reveal(open.id)}
                  >
                    Reveal
                  </button>
                  {resumeHint(open.tool, open.source_path) && (
                    <button
                      className="secondary"
                      title="Copy a command to resume this session in its original tool"
                      onClick={() =>
                        navigator.clipboard?.writeText(resumeHint(open.tool, open.source_path!)!)
                      }
                    >
                      Copy resume cmd
                    </button>
                  )}
                  <button
                    className="secondary"
                    onClick={() => navigator.clipboard?.writeText(open.source_path!)}
                  >
                    Copy path
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// A copy-pasteable command to reopen a session in its original tool, when one
// exists. Claude Code and Codex both key resume off the session UUID, which is
// the source file's stem (Codex prefixes it with `rollout-<timestamp>-`).
function resumeHint(tool: string, sourcePath: string): string | null {
  const stem = sourcePath.split("/").pop()?.replace(/\.jsonl$/i, "") ?? "";
  const uuid =
    stem.match(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i)?.[0] ?? "";
  if (!uuid) return null;
  if (tool === "claude-code") return `claude --resume ${uuid}`;
  if (tool === "codex") return `codex resume ${uuid}`;
  return null;
}

// The daemon marks matched terms with [brackets]; render them as <mark>.
function highlight(snippet: string): string {
  const escaped = snippet
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
  return escaped.replace(/\[([^\]]+)\]/g, "<mark>$1</mark>");
}
