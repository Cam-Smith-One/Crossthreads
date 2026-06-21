import { useCallback, useEffect, useState } from "react";
import {
  approvePeer,
  buildContext,
  discoverDevices,
  forget,
  getConversation,
  getDevices,
  getFacets,
  getSaved,
  getStatus,
  openSource,
  reindex,
  removePeer,
  search,
  setFlags,
  setNote,
  setTags,
  type Device,
  type DiscoveredDevice,
  type Filters,
  type Hit,
  type Mode,
  type Status,
  type StoredConversation,
} from "./api";

const MODES: Mode[] = ["hybrid", "semantic", "lexical"];
const PAGE = 25;

export function App() {
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<Mode>("hybrid");
  const [filters, setFilters] = useState<Filters>({});
  const [hits, setHits] = useState<Hit[]>([]);
  const [selected, setSelected] = useState(-1);
  const [status, setStatus] = useState<Status | null>(null);
  const [tools, setTools] = useState<string[]>([]);
  const [tagFacets, setTagFacets] = useState<string[]>([]);
  const [limit, setLimit] = useState(PAGE);
  const [find, setFind] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [searched, setSearched] = useState(false);
  const [context, setContext] = useState<string | null>(null);
  const [open, setOpen] = useState<StoredConversation | null>(null);
  const [saved, setSaved] = useState<Hit[]>([]);
  const [theme, setTheme] = useState<string>(
    () => document.documentElement.dataset.theme || "dark",
  );

  const [reindexing, setReindexing] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [settingsTab, setSettingsTab] = useState<"docs" | "devices">("docs");

  // Cross-device (ADR-010): known devices, which are selected to search, and
  // the on-demand discovery results.
  const [devices, setDevices] = useState<Device[]>([]);
  const [federation, setFederation] = useState(false);
  const [deviceSel, setDeviceSel] = useState<Set<string>>(new Set());
  const [discovered, setDiscovered] = useState<DiscoveredDevice[] | null>(null);
  const [discovering, setDiscovering] = useState(false);

  const refreshSaved = useCallback(() => {
    getSaved().then(setSaved).catch(() => {});
  }, []);
  const refreshStatus = useCallback(() => {
    getStatus().then(setStatus).catch(() => {});
  }, []);
  const refreshFacets = useCallback(() => {
    getFacets()
      .then((f) => {
        setTools(f.tools);
        setTagFacets(f.tags);
      })
      .catch(() => {});
  }, []);
  const refreshDevices = useCallback(() => {
    getDevices()
      .then(({ devices, federation }) => {
        setDevices(devices);
        setFederation(federation);
        // Default the selection to every known device (= search all).
        setDeviceSel(new Set(devices.map((d) => d.name)));
      })
      .catch(() => {});
  }, []);

  async function runReindex() {
    setReindexing(true);
    setError(null);
    try {
      await reindex();
      refreshStatus();
      refreshFacets();
      refreshSaved();
    } catch (err) {
      setError(String(err));
    } finally {
      setReindexing(false);
    }
  }

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
    refreshFacets();
    refreshSaved();
    refreshDevices();
  }, [refreshSaved, refreshFacets, refreshDevices]);

  // The local device's name, so remote-only chips can be shown on results.
  const localDevice = devices.find((d) => d.local)?.name;

  // Device routing arg for searches: undefined (= all) unless the user has
  // narrowed the selection to a strict, non-empty subset.
  const deviceArg = useCallback((): string[] | undefined => {
    if (!federation) return undefined;
    const names = devices.map((d) => d.name);
    if (deviceSel.size === 0 || deviceSel.size === names.length) return undefined;
    return names.filter((n) => deviceSel.has(n));
  }, [federation, devices, deviceSel]);

  const toggleDevice = (name: string) =>
    setDeviceSel((sel) => {
      const next = new Set(sel);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });

  async function runDiscover() {
    setDiscovering(true);
    setError(null);
    try {
      setDiscovered(await discoverDevices());
    } catch (err) {
      setError(String(err));
    } finally {
      setDiscovering(false);
    }
  }

  async function approve(d: DiscoveredDevice) {
    try {
      const updated = await approvePeer(d.name, d.addr);
      setDevices(updated);
      setDeviceSel((sel) => new Set(sel).add(d.name));
      setDiscovered((list) =>
        (list ?? []).map((x) => (x.name === d.name ? { ...x, already_approved: true } : x)),
      );
    } catch (err) {
      setError(String(err));
    }
  }

  async function removeDevice(name: string) {
    try {
      const updated = await removePeer(name);
      setDevices(updated);
      setDeviceSel((sel) => {
        const next = new Set(sel);
        next.delete(name);
        return next;
      });
    } catch (err) {
      setError(String(err));
    }
  }

  const runSearch = useCallback(
    async (e?: React.FormEvent, lim = PAGE) => {
      e?.preventDefault();
      if (!query.trim()) return;
      setLoading(true);
      setError(null);
      setContext(null);
      try {
        const results = await search(query, mode, filters, lim, deviceArg());
        setHits(results);
        setLimit(lim);
        setSelected(results.length ? 0 : -1);
        setSearched(true);
      } catch (err) {
        setError(String(err));
      } finally {
        setLoading(false);
      }
    },
    [query, mode, filters, deviceArg],
  );

  // Fetch the next page by raising the limit and re-querying (search returns
  // top-N, so we replace rather than append).
  async function loadMore() {
    await runSearch(undefined, limit + PAGE);
  }

  async function openConversation(id: string) {
    try {
      const convo = await getConversation(id);
      if (convo) {
        setFind("");
        setOpen(convo);
      }
    } catch (err) {
      setError(String(err));
    }
  }

  async function showContext() {
    setLoading(true);
    setError(null);
    try {
      setContext((await buildContext(query, mode, filters, 3, 6000, deviceArg())).markdown);
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

  // Build a portable markdown transcript of an open conversation.
  function conversationMarkdown(c: StoredConversation): string {
    const head = `# ${c.title ?? "(untitled)"}\n\n_${c.tool}${c.project ? ` · ${c.project}` : ""}${
      c.started_at ? ` · ${c.started_at.slice(0, 10)}` : ""
    }_\n\n`;
    return head + c.messages.map((m) => `**${m.role}:** ${m.content.trim()}`).join("\n\n");
  }

  function download(name: string, text: string, type: string) {
    const url = URL.createObjectURL(new Blob([text], { type }));
    const a = document.createElement("a");
    a.href = url;
    a.download = name;
    a.click();
    URL.revokeObjectURL(url);
  }

  function exportConversation(c: StoredConversation, fmt: "md" | "json") {
    const stem = (c.title ?? c.id).replace(/[^\w.-]+/g, "_").slice(0, 60) || "conversation";
    if (fmt === "md") download(`${stem}.md`, conversationMarkdown(c), "text/markdown");
    else download(`${stem}.json`, JSON.stringify(c, null, 2), "application/json");
  }

  async function saveTags(id: string, raw: string) {
    const tags = Array.from(
      new Set(raw.split(",").map((t) => t.trim().toLowerCase()).filter(Boolean)),
    );
    try {
      await setTags(id, tags);
      setOpen((o) => (o && o.id === id ? { ...o, tags } : o));
      setHits((hs) => hs.map((h) => (h.conversation_id === id ? { ...h, tags } : h)));
      refreshFacets();
      refreshSaved();
    } catch (err) {
      setError(String(err));
    }
  }

  async function saveNote(id: string, note: string) {
    try {
      await setNote(id, note);
      setOpen((o) => (o && o.id === id ? { ...o, note } : o));
    } catch (err) {
      setError(String(err));
    }
  }

  // Forget a conversation: remove it from the index for good (tombstoned so it
  // won't be re-added). Drops it from the current results and saved panel.
  async function forgetConversation(id: string) {
    if (!window.confirm("Forget this conversation? It's removed from the index and won't be re-added.")) {
      return;
    }
    try {
      await forget(id);
      setOpen(null);
      setHits((hs) => hs.filter((h) => h.conversation_id !== id));
      refreshSaved();
    } catch (err) {
      setError(String(err));
    }
  }

  // Keyboard navigation over results (j/k or arrows; Enter opens; Esc closes).
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (showSettings) {
        if (e.key === "Escape") setShowSettings(false);
        return;
      }
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
  }, [hits, selected, open, showSettings]);

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
          <button
            className="settings-toggle"
            onClick={() => setShowSettings(true)}
            title="Documentation &amp; settings"
            aria-label="Open settings"
          >
            ⚙️
          </button>
        </div>
        <p className="tagline">Search every AI coding conversation, across tools.</p>
        {status && (
          <p className="status">
            {status.conversations.toLocaleString()} conversations ·{" "}
            {status.embeddings.toLocaleString()} embeddings · {status.embedder}
            <button className="linklike" onClick={runReindex} disabled={reindexing}>
              {reindexing ? "re-indexing…" : "re-index"}
            </button>
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
        {tagFacets.length > 0 && (
          <select value={filters.tag ?? ""} onChange={(e) => setF({ tag: e.target.value || undefined })}>
            <option value="">all tags</option>
            {tagFacets.map((t) => (
              <option key={t} value={t}>
                #{t}
              </option>
            ))}
          </select>
        )}
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
        {federation && devices.length > 1 && (
          <div className="device-filter" title="Which devices to search">
            {devices.map((d) => (
              <button
                key={d.name}
                type="button"
                className={`device-toggle ${deviceSel.has(d.name) ? "on" : ""}`}
                onClick={() => toggleDevice(d.name)}
                title={d.online ? "online" : "offline"}
              >
                <span className={`device-dot ${d.online ? "online" : "offline"}`} />
                {d.name}
                {d.local ? " (this)" : ""}
              </button>
            ))}
          </div>
        )}
      </div>

      {error && <div className="error">{error}</div>}

      {status && status.conversations === 0 && !searched && (
        <section className="onboarding">
          <h2>No conversations indexed yet</h2>
          <p>
            Crossthreads looks for history from <strong>Claude Code, Codex, Cursor, Aider,
            Cline, Copilot Chat, Gemini CLI, Windsurf,</strong> and <strong>Antigravity</strong>.
            Use any of them, then re-index — new sessions are picked up automatically as you work.
          </p>
          <button onClick={runReindex} disabled={reindexing}>
            {reindexing ? "Indexing…" : "Index now"}
          </button>
        </section>
      )}

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
              {h.device && h.device !== localDevice && (
                <span className="device-chip" title={`from ${h.device}`}>💻 {h.device}</span>
              )}
              <span className="title">{h.title ?? "(untitled)"}</span>
              {h.started_at && <span className="date">{h.started_at.slice(0, 10)}</span>}
              {flagButtons(h)}
            </div>
            {h.project && <div className="project">{h.project}</div>}
            <div className="snippet" dangerouslySetInnerHTML={{ __html: highlight(h.snippet) }} />
            {h.tags && h.tags.length > 0 && (
              <div className="tags">
                {h.tags.map((t) => (
                  <span key={t} className="tag-chip">#{t}</span>
                ))}
              </div>
            )}
          </article>
        ))}
        {searched && hits.length >= limit && (
          <button className="load-more" onClick={loadMore} disabled={loading}>
            {loading ? "…" : "Load more"}
          </button>
        )}
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
                <button
                  className="secondary"
                  title="Download this conversation as Markdown"
                  onClick={() => exportConversation(open, "md")}
                >
                  Export .md
                </button>
                <button
                  className="secondary"
                  title="Download this conversation as JSON"
                  onClick={() => exportConversation(open, "json")}
                >
                  JSON
                </button>
                <button
                  className="danger"
                  title="Remove this conversation from the index for good"
                  onClick={() => forgetConversation(open.id)}
                >
                  Forget
                </button>
                <button className="secondary" onClick={() => setOpen(null)}>
                  Close
                </button>
              </div>
            </div>
            <div className="transcript-find">
              <input
                placeholder="find in conversation…"
                value={find}
                onChange={(e) => setFind(e.target.value)}
              />
              {find.trim() && (
                <span className="find-count">
                  {open.messages.filter((m) => matchesFind(m.content, find)).length} match(es)
                </span>
              )}
            </div>
            <div className="transcript">
              {open.messages
                .filter((m) => !find.trim() || matchesFind(m.content, find))
                .map((m, i) => (
                  <div key={i} className={`turn turn-${m.role}`}>
                    <span className="role">{m.role}</span>
                    <div
                      className="content"
                      dangerouslySetInnerHTML={{ __html: highlightFind(m.content, find) }}
                    />
                  </div>
                ))}
            </div>
            <div className="meta-edit">
              <label className="meta-row">
                <span className="meta-label">tags</span>
                <input
                  defaultValue={(open.tags ?? []).join(", ")}
                  placeholder="comma,separated"
                  onBlur={(e) => saveTags(open.id, e.target.value)}
                />
              </label>
              <label className="meta-row">
                <span className="meta-label">note</span>
                <textarea
                  defaultValue={open.note ?? ""}
                  placeholder="add a private note…"
                  rows={2}
                  onBlur={(e) => saveNote(open.id, e.target.value)}
                />
              </label>
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

      {showSettings && (
        <div className="overlay" onClick={() => setShowSettings(false)}>
          <div className="modal settings-modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-head">
              <div className="settings-tabs">
                <button
                  className={settingsTab === "docs" ? "on" : ""}
                  onClick={() => setSettingsTab("docs")}
                >
                  Documentation
                </button>
                <button
                  className={settingsTab === "devices" ? "on" : ""}
                  onClick={() => setSettingsTab("devices")}
                >
                  Devices
                </button>
              </div>
              <button className="secondary" onClick={() => setShowSettings(false)}>
                Close
              </button>
            </div>

            {settingsTab === "docs" && (
              <div className="settings-body">
                <section className="settings-section">
                  <h3>Documentation</h3>
                  <p className="settings-hint">
                    Guides, references, and troubleshooting. Links open the docs on
                    GitHub.
                  </p>
                  <ul className="doc-links">
                    {DOC_LINKS.map((d) => (
                      <li key={d.href}>
                        <a href={d.href} target="_blank" rel="noreferrer noopener">
                          {d.title}
                        </a>
                        <span className="doc-desc">{d.desc}</span>
                      </li>
                    ))}
                  </ul>
                </section>
                <section className="settings-section">
                  <h3>Set up another device</h3>
                  <p className="settings-hint">
                    Search the history on your other machines too. Install
                    Crossthreads on each device, join them to one private Tailscale
                    network, then use the <strong>Devices</strong> tab to discover
                    and approve them. Full walkthrough:
                  </p>
                  <ul className="doc-links">
                    <li>
                      <a href={`${DOCS_BASE}MULTI_DEVICE_SETUP.md`} target="_blank" rel="noreferrer noopener">
                        Multi-device setup guide
                      </a>
                      <span className="doc-desc">Step-by-step: connect your devices for cross-device search.</span>
                    </li>
                  </ul>
                </section>
              </div>
            )}

            {settingsTab === "devices" && (
              <div className="settings-body">
                <section className="settings-section">
                  <h3>Devices</h3>
                  {!federation ? (
                    <p className="settings-hint">
                      Cross-device search isn't set up on this machine yet. Give this
                      device a name and a shared token, and join your machines to one
                      Tailscale network — then come back here to discover them. See the{" "}
                      <a href={`${DOCS_BASE}MULTI_DEVICE_SETUP.md`} target="_blank" rel="noreferrer noopener">
                        multi-device setup guide
                      </a>
                      .
                    </p>
                  ) : (
                    <>
                      <p className="settings-hint">
                        Search spans these devices. Liveness is checked when this list
                        loads — Crossthreads never scans your network in the background.
                      </p>
                      <ul className="device-list">
                        {devices.map((d) => (
                          <li key={d.name} className="device-row">
                            <span className={`device-dot ${d.online ? "online" : "offline"}`} />
                            <span className="device-name">
                              {d.name}
                              {d.local && <span className="device-tag">this device</span>}
                            </span>
                            {d.addr && <code className="device-addr">{d.addr}</code>}
                            {!d.local && (
                              <button className="linklike" onClick={() => removeDevice(d.name)}>
                                remove
                              </button>
                            )}
                          </li>
                        ))}
                      </ul>
                      <div className="device-discover">
                        <button className="secondary" onClick={runDiscover} disabled={discovering}>
                          {discovering ? "Discovering…" : "Discover my devices"}
                        </button>
                        {discovered !== null && (
                          <div className="discovered">
                            {discovered.length === 0 ? (
                              <p className="settings-hint">
                                No other Crossthreads devices found on your tailnet. Make
                                sure each is running with a tailnet <code>--addr</code> and
                                that Tailscale is connected.
                              </p>
                            ) : (
                              <ul className="device-list">
                                {discovered.map((d) => (
                                  <li key={d.addr} className="device-row">
                                    <span className="device-name">{d.name}</span>
                                    <code className="device-addr">{d.addr}</code>
                                    {d.already_approved ? (
                                      <span className="device-tag">added</span>
                                    ) : (
                                      <button className="secondary" onClick={() => approve(d)}>
                                        Approve
                                      </button>
                                    )}
                                  </li>
                                ))}
                              </ul>
                            )}
                          </div>
                        )}
                      </div>
                    </>
                  )}
                </section>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// GitHub base for the repo's docs, so in-app links resolve from the
// locally-served UI. The daemon serves the UI from a file:// or 127.0.0.1
// origin where relative doc paths wouldn't exist, so we link out.
const DOCS_BASE = "https://github.com/Cam-Smith-One/Crossthreads/blob/main/docs/";
const REPO_BASE = "https://github.com/Cam-Smith-One/Crossthreads/blob/main/";
const DOC_LINKS: { title: string; href: string; desc: string }[] = [
  { title: "Getting started (README)", href: `${REPO_BASE}README.md`, desc: "Install, index, and run your first search." },
  { title: "Multi-device setup", href: `${DOCS_BASE}MULTI_DEVICE_SETUP.md`, desc: "Connect your other machines for cross-device search." },
  { title: "Troubleshooting", href: `${DOCS_BASE}TROUBLESHOOTING.md`, desc: "Common issues, fixes, and a bug-report checklist." },
  { title: "Agents & MCP (Agent API)", href: `${DOCS_BASE}AGENT_API.md`, desc: "Wire Crossthreads into Claude Code, Codex, and other agents." },
  { title: "Architecture", href: `${DOCS_BASE}ARCHITECTURE.md`, desc: "How the daemon, index, and connectors fit together." },
  { title: "Report an issue", href: "https://github.com/Cam-Smith-One/Crossthreads/issues", desc: "Found a bug or missing a tool? Let us know." },
];

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

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function matchesFind(content: string, find: string): boolean {
  return content.toLowerCase().includes(find.trim().toLowerCase());
}

// Escape the content, then wrap case-insensitive matches of `find` in <mark>.
function highlightFind(content: string, find: string): string {
  const escaped = escapeHtml(content);
  const term = find.trim();
  if (!term) return escaped;
  return escaped.replace(new RegExp(escapeRegex(escapeHtml(term)), "gi"), "<mark>$&</mark>");
}
