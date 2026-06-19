import { useCallback, useEffect, useState } from "react";
import {
  buildContext,
  getConversation,
  getFacets,
  getStatus,
  search,
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

  useEffect(() => {
    getStatus().then(setStatus).catch((e) => setError(String(e)));
    getFacets().then(setTools).catch(() => {});
  }, []);

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

  return (
    <div className="app">
      <header>
        <h1>Crossthreads</h1>
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
              <button className="secondary" onClick={() => setOpen(null)}>
                Close
              </button>
            </div>
            <div className="transcript">
              {open.messages.map((m, i) => (
                <div key={i} className={`turn turn-${m.role}`}>
                  <span className="role">{m.role}</span>
                  <div className="content">{m.content}</div>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// The daemon marks matched terms with [brackets]; render them as <mark>.
function highlight(snippet: string): string {
  const escaped = snippet
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
  return escaped.replace(/\[([^\]]+)\]/g, "<mark>$1</mark>");
}
