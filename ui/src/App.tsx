import { useEffect, useState } from "react";
import { buildContext, getStatus, search, type Hit, type Mode, type Status } from "./api";

const MODES: Mode[] = ["hybrid", "semantic", "lexical"];

export function App() {
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<Mode>("hybrid");
  const [hits, setHits] = useState<Hit[]>([]);
  const [status, setStatus] = useState<Status | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [searched, setSearched] = useState(false);
  const [context, setContext] = useState<string | null>(null);

  useEffect(() => {
    getStatus().then(setStatus).catch((e) => setError(String(e)));
  }, []);

  async function runSearch(e?: React.FormEvent) {
    e?.preventDefault();
    if (!query.trim()) return;
    setLoading(true);
    setError(null);
    setContext(null);
    try {
      setHits(await search(query, mode));
      setSearched(true);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  async function showContext() {
    setLoading(true);
    setError(null);
    try {
      const block = await buildContext(query, mode);
      setContext(block.markdown);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

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
        {searched && hits.length === 0 && !loading && (
          <p className="empty">No matches.</p>
        )}
        {hits.map((h) => (
          <article key={h.conversation_id} className="hit">
            <div className="hit-head">
              <span className={`tool tool-${h.tool}`}>{h.tool}</span>
              <span className="title">{h.title ?? "(untitled)"}</span>
              {h.started_at && <span className="date">{h.started_at.slice(0, 10)}</span>}
            </div>
            {h.project && <div className="project">{h.project}</div>}
            <div className="snippet" dangerouslySetInnerHTML={{ __html: highlight(h.snippet) }} />
          </article>
        ))}
      </section>
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
