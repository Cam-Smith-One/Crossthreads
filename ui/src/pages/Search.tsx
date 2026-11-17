// Search: find any past session (by words or by meaning), or ask a question
// and get an answer drawn from them, with its sources.

import * as React from "react";
import {
  ArrowRight,
  BarChart3,
  ClipboardList,
  GitBranch,
  Laptop,
  Loader2,
  MessageSquareText,
  Search as SearchIcon,
  SlidersHorizontal,
  Sparkles,
  Star,
  X,
} from "lucide-react";
import {
  ask,
  buildContext,
  getDevices,
  getFacets,
  getSaved,
  search,
  setFlags,
  type Answer,
  type Device,
  type Filters,
  type Hit,
  type Mode,
} from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Empty } from "@/components/ui/empty";
import { Input, Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import {
  CopyButton,
  Dot,
  highlightSnippet,
  Kbd,
  markClass,
  Notice,
  PageHeader,
  Segmented,
  shortPath,
  SourceList,
  ToolChip,
  useLatest,
} from "@/components/common";
import { Markdown } from "@/lib/markdown";
import { ago, cn } from "@/lib/utils";
import { toolLabel } from "@/tools";

const PAGE = 25;
const MODES: [Mode, string][] = [
  ["hybrid", "Best match"],
  ["semantic", "By meaning"],
  ["lexical", "Exact words"],
];

/** Text the Insights page hands to Ask ("Reflect with Ask"). */
export const ASK_PREFILL_KEY = "ct-ask-prefill";

// What the page held last time, so leaving and coming back keeps the results.
const memory: {
  query: string;
  mode: Mode;
  filters: Filters;
  savedOnly: boolean;
  hits: Hit[];
  unreachable: string[];
  searched: boolean;
  limit: number;
  askQuery: string;
  answer: Answer | null;
} = {
  query: "",
  mode: "hybrid",
  filters: {},
  savedOnly: false,
  hits: [],
  unreachable: [],
  searched: false,
  limit: PAGE,
  askQuery: "",
  answer: null,
};

const typing = () => ["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement?.tagName ?? "");
const dialogOpen = () => !!document.querySelector('[role="dialog"], [role="menu"]');

export function SearchPage({
  phone,
  mode: tab,
  onMode,
  onOpenSession,
  onShowThreadOf,
  onError,
}: {
  phone: boolean;
  mode: "find" | "ask";
  onMode: (m: "find" | "ask") => void;
  onOpenSession: (conversationId: string, device?: string | null) => void;
  onShowThreadOf: (conversationId: string) => void;
  onError: (msg: string) => void;
}) {
  const view = phone ? "find" : tab;

  // Devices, facets and saved sessions.
  const [devices, setDevices] = React.useState<Device[]>([]);
  const [federation, setFederation] = React.useState(false);
  const [deviceSel, setDeviceSel] = React.useState<Set<string>>(new Set());
  const [tools, setTools] = React.useState<string[]>([]);
  const [tagFacets, setTagFacets] = React.useState<string[]>([]);
  const [saved, setSaved] = React.useState<Hit[] | null>(null);

  const refreshSaved = React.useCallback(() => {
    getSaved().then(setSaved).catch(() => setSaved([]));
  }, []);

  React.useEffect(() => {
    getDevices()
      .then(({ devices, federation }) => {
        setDevices(devices);
        setFederation(federation);
        setDeviceSel(new Set(devices.map((d) => d.name)));
      })
      .catch(() => {});
    getFacets()
      .then((f) => {
        setTools(f.tools);
        setTagFacets(f.tags);
      })
      .catch(() => {});
    refreshSaved();
  }, [refreshSaved]);

  const localDevice = devices.find((d) => d.local)?.name;
  const multiDevice = federation && devices.length > 1;

  // Device routing: undefined (= all) unless narrowed to a strict, non-empty subset.
  const deviceArg = React.useCallback((): string[] | undefined => {
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
      // Never leave none selected (that would silently read as "all").
      if (next.size === 0) return new Set(devices.map((d) => d.name));
      return next;
    });

  return (
    <div className="mx-auto w-full max-w-4xl space-y-6 px-4 py-6 sm:px-6 sm:py-8">
      <PageHeader
        title="Search"
        description={
          <>
            Every session from every tool{multiDevice ? " and computer" : ""}, searchable by words or by meaning.
            {!phone && " Or ask a question and get an answer drawn from them, with the sessions it came from."}
          </>
        }
      />
      {!phone && (
        <Segmented<"find" | "ask" | "insights">
          label="Search mode"
          value={view}
          onChange={(v) => {
            if (v === "insights") window.location.hash = "#/insights";
            else onMode(v);
          }}
          options={[
            { value: "find", label: <><SearchIcon /> Find</> },
            { value: "ask", label: <><MessageSquareText /> Ask</> },
            { value: "insights", label: <><BarChart3 /> How you work</> },
          ]}
        />
      )}
      {view === "find" ? (
        <FindView
          phone={phone}
          devices={devices}
          multiDevice={multiDevice}
          deviceSel={deviceSel}
          toggleDevice={toggleDevice}
          deviceArg={deviceArg}
          localDevice={localDevice}
          tools={tools}
          tagFacets={tagFacets}
          saved={saved}
          refreshSaved={refreshSaved}
          onOpenSession={onOpenSession}
          onShowThreadOf={onShowThreadOf}
          onError={onError}
        />
      ) : (
        <AskView deviceArg={deviceArg} onOpenSession={onOpenSession} />
      )}
    </div>
  );
}

// --- Find ---------------------------------------------------------------------

function FindView({
  phone,
  devices,
  multiDevice,
  deviceSel,
  toggleDevice,
  deviceArg,
  localDevice,
  tools,
  tagFacets,
  saved,
  refreshSaved,
  onOpenSession,
  onShowThreadOf,
  onError,
}: {
  phone: boolean;
  devices: Device[];
  multiDevice: boolean;
  deviceSel: Set<string>;
  toggleDevice: (name: string) => void;
  deviceArg: () => string[] | undefined;
  localDevice?: string;
  tools: string[];
  tagFacets: string[];
  saved: Hit[] | null;
  refreshSaved: () => void;
  onOpenSession: (id: string, device?: string | null) => void;
  onShowThreadOf: (id: string) => void;
  onError: (msg: string) => void;
}) {
  const [query, setQuery] = React.useState(memory.query);
  const [mode, setMode] = React.useState<Mode>(memory.mode);
  const [filters, setFilters] = React.useState<Filters>(memory.filters);
  const [savedOnly, setSavedOnly] = React.useState(memory.savedOnly);
  const [hits, setHits] = React.useState<Hit[]>(memory.hits);
  const [unreachable, setUnreachable] = React.useState<string[]>(memory.unreachable);
  const [searched, setSearched] = React.useState(memory.searched);
  const [limit, setLimit] = React.useState(memory.limit);
  const [loading, setLoading] = React.useState(false);
  const [selected, setSelected] = React.useState(memory.hits.length ? 0 : -1);
  const [showFilters, setShowFilters] = React.useState(false);
  const [context, setContext] = React.useState<string | null>(null);
  const [contextBusy, setContextBusy] = React.useState(false);
  const input = React.useRef<HTMLInputElement>(null);
  const list = React.useRef<HTMLDivElement>(null);
  const gen = React.useRef(0);
  const onErrorRef = useLatest(onError);

  // Remember for the next visit.
  React.useEffect(() => {
    Object.assign(memory, { query, mode, filters, savedOnly, hits, unreachable, searched, limit });
  }, [query, mode, filters, savedOnly, hits, unreachable, searched, limit]);

  React.useEffect(() => {
    if (!phone) input.current?.focus();
  }, [phone]);

  const runSearch = React.useCallback(
    async (lim = PAGE, keepSelection = false) => {
      const q = query.trim();
      if (!q) return;
      const g = ++gen.current;
      setLoading(true);
      setContext(null);
      try {
        const res = await search(q, mode, filters, lim, deviceArg());
        if (g !== gen.current) return; // a newer search superseded this one
        setHits(res.hits);
        setUnreachable(res.unreachable);
        setLimit(lim);
        setSelected((s) => (keepSelection ? Math.min(s, res.hits.length - 1) : res.hits.length ? 0 : -1));
        setSearched(true);
      } catch (err) {
        if (g === gen.current) onErrorRef.current(String(err));
      } finally {
        if (g === gen.current) setLoading(false);
      }
    },
    [query, mode, filters, deviceArg, onErrorRef],
  );

  // Changing a filter re-runs the search you already made.
  const firstRun = React.useRef(true);
  const runRef = React.useRef(runSearch);
  runRef.current = runSearch;
  const devicesKey = [...deviceSel].sort().join(",");
  React.useEffect(() => {
    if (firstRun.current) {
      firstRun.current = false;
      return;
    }
    if (memory.searched && memory.query.trim()) runRef.current();
  }, [mode, filters, devicesKey]);

  const shown = savedOnly ? hits.filter((h) => h.bookmarked || h.pinned) : hits;

  // Keyboard: "/" focuses the box; j/k or arrows move; Enter opens.
  React.useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (dialogOpen() || e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "/" && !typing()) {
        e.preventDefault();
        input.current?.focus();
        input.current?.select();
        return;
      }
      if (!shown.length || typing()) return;
      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        setSelected((s) => Math.min(s + 1, shown.length - 1));
      } else if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        setSelected((s) => Math.max(s - 1, 0));
      } else if (e.key === "Enter" && selected >= 0 && shown[selected]) {
        onOpenSession(shown[selected].conversation_id, shown[selected].device);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [shown, selected, onOpenSession]);

  React.useEffect(() => {
    list.current?.querySelector<HTMLElement>(`[data-index="${selected}"]`)?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  async function showContext() {
    setContextBusy(true);
    try {
      setContext((await buildContext(query, mode, filters, 3, 6000, deviceArg())).markdown);
    } catch (err) {
      onError(String(err));
    } finally {
      setContextBusy(false);
    }
  }

  async function toggleSaved(h: Pick<Hit, "conversation_id" | "bookmarked" | "pinned">) {
    const on = !!(h.bookmarked || h.pinned);
    const patch = on ? { bookmarked: false, pinned: false } : { bookmarked: true };
    try {
      await setFlags(h.conversation_id, patch);
      setHits((hs) => hs.map((x) => (x.conversation_id === h.conversation_id ? { ...x, ...patch } : x)));
      refreshSaved();
    } catch (err) {
      onError(String(err));
    }
  }

  const setF = (patch: Partial<Filters>) => setFilters((f) => ({ ...f, ...patch }));
  const narrowedDevices = !!deviceArg();
  const activeFilters =
    [filters.kind, filters.tool, filters.tag, filters.project, filters.since, filters.until].filter(Boolean).length +
    (savedOnly ? 1 : 0) +
    (narrowedDevices ? 1 : 0);

  function clearFilters() {
    setFilters({});
    setSavedOnly(false);
    if (narrowedDevices) devices.forEach((d) => !deviceSel.has(d.name) && toggleDevice(d.name));
  }

  return (
    <div className="space-y-5">
      {/* The search box */}
      <form
        onSubmit={(e) => {
          e.preventDefault();
          runSearch();
        }}
        className="space-y-3"
      >
        <div className="flex flex-col gap-2 sm:flex-row">
          <div className="relative flex-1">
            <SearchIcon className="pointer-events-none absolute left-3.5 top-1/2 size-5 -translate-y-1/2 text-muted-foreground" />
            <Input
              ref={input}
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="e.g. where did we set up the OAuth refresh retry?"
              aria-label="Search your sessions"
              title="Search (press / anywhere)"
              className="h-12 rounded-lg pl-11 pr-32 text-base shadow-sm sm:text-[15px]"
            />
            <NativeSelect
              value={mode}
              onChange={(e) => setMode(e.target.value as Mode)}
              aria-label="How to match"
              title="Best match combines exact words and meaning"
              className="absolute right-1.5 top-1/2 h-9 -translate-y-1/2 border-0 bg-muted text-[13px] shadow-none"
            >
              {MODES.map(([m, label]) => (
                <option key={m} value={m}>
                  {label}
                </option>
              ))}
            </NativeSelect>
          </div>
          <div className="flex gap-2">
            <Button
              type="button"
              variant={showFilters || activeFilters ? "secondary" : "outline"}
              className="h-12 flex-1 sm:flex-none"
              onClick={() => setShowFilters((v) => !v)}
              aria-expanded={showFilters}
              aria-controls="search-filters"
            >
              <SlidersHorizontal />
              Filters
              {activeFilters > 0 && (
                <Badge className="h-5 min-w-5 justify-center px-1.5 tabular-nums">{activeFilters}</Badge>
              )}
            </Button>
            <Button type="submit" className="h-12 flex-1 px-5 sm:flex-none" disabled={loading || !query.trim()}>
              {loading ? <Loader2 className="animate-spin" /> : <SearchIcon />}
              Search
            </Button>
          </div>
        </div>

        {showFilters && (
          <Card id="search-filters" className="space-y-4 p-4 shadow-none">
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              <Field label="What">
                <NativeSelect value={filters.kind ?? ""} onChange={(e) => setF({ kind: e.target.value || undefined })}>
                  <option value="">Everything</option>
                  <option value="thread">Sessions</option>
                  <option value="exchange">Room exchanges</option>
                  <option value="skill">Skills and prompts</option>
                </NativeSelect>
              </Field>
              <Field label="Tool">
                <NativeSelect value={filters.tool ?? ""} onChange={(e) => setF({ tool: e.target.value || undefined })}>
                  <option value="">All tools</option>
                  {tools.map((t) => (
                    <option key={t} value={t}>
                      {toolLabel(t)}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
              <Field label="Project">
                <Input
                  value={filters.project ?? ""}
                  onChange={(e) => setF({ project: e.target.value || undefined })}
                  placeholder="Path contains…"
                />
              </Field>
              <Field label="From">
                <Input type="date" value={filters.since ?? ""} onChange={(e) => setF({ since: e.target.value || undefined })} />
              </Field>
              <Field label="To">
                <Input type="date" value={filters.until ?? ""} onChange={(e) => setF({ until: e.target.value || undefined })} />
              </Field>
              {tagFacets.length > 0 && (
                <Field label="Tag">
                  <NativeSelect value={filters.tag ?? ""} onChange={(e) => setF({ tag: e.target.value || undefined })}>
                    <option value="">All tags</option>
                    {tagFacets.map((t) => (
                      <option key={t} value={t}>
                        #{t}
                      </option>
                    ))}
                  </NativeSelect>
                </Field>
              )}
            </div>
            {multiDevice && (
              <div className="space-y-1.5">
                <span className="text-xs font-medium text-muted-foreground">Computers to search</span>
                <div className="flex flex-wrap gap-1.5">
                  {devices.map((d) => {
                    const on = deviceSel.has(d.name);
                    return (
                      <button
                        key={d.name}
                        type="button"
                        aria-pressed={on}
                        onClick={() => toggleDevice(d.name)}
                        title={d.online ? "Online" : "Offline"}
                        className={cn(
                          "inline-flex h-8 items-center gap-2 rounded-md border px-2.5 text-[13px] transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                          on ? "border-primary/40 bg-primary/10 text-foreground" : "text-muted-foreground hover:bg-accent",
                        )}
                      >
                        <Dot tone={d.online ? "success" : "muted"} />
                        {d.name}
                        {d.local && <span className="text-muted-foreground">(this)</span>}
                      </button>
                    );
                  })}
                </div>
              </div>
            )}
            <div className="flex flex-wrap items-center justify-between gap-3 border-t pt-3">
              {!phone ? (
                <label className="flex items-center gap-2 text-[13px]">
                  <Checkbox checked={savedOnly} onCheckedChange={(v) => setSavedOnly(v === true)} />
                  Only saved sessions
                </label>
              ) : (
                <span />
              )}
              <Button type="button" variant="ghost" size="sm" onClick={clearFilters} disabled={activeFilters === 0}>
                Clear filters
              </Button>
            </div>
          </Card>
        )}
      </form>

      {/* Context block */}
      {context !== null && (
        <Card className="overflow-hidden shadow-none">
          <div className="flex items-center justify-between gap-3 border-b px-4 py-2.5">
            <div>
              <p className="text-sm font-medium">Context block</p>
              <p className="text-xs text-muted-foreground">Paste it into any agent to bring it up to speed.</p>
            </div>
            <div className="flex items-center gap-1">
              <CopyButton text={context} />
              <Button variant="ghost" size="icon-sm" onClick={() => setContext(null)} aria-label="Close context block">
                <X />
              </Button>
            </div>
          </div>
          <pre className="max-h-80 overflow-auto whitespace-pre-wrap bg-muted/30 p-4 font-mono text-xs leading-relaxed scrollbar-thin">
            {context}
          </pre>
        </Card>
      )}

      {searched && unreachable.length > 0 && (
        <Notice tone="warning" title={`${unreachable.length} computer${unreachable.length > 1 ? "s" : ""} didn't answer`}>
          {unreachable.join(", ")} {unreachable.length > 1 ? "are" : "is"} unreachable, so results may be incomplete.
        </Notice>
      )}

      {/* Results */}
      {searched && (
        <div className="space-y-2">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-[13px] text-muted-foreground" aria-live="polite">
              {loading
                ? "Searching…"
                : `${shown.length}${hits.length >= limit ? "+" : ""} result${shown.length === 1 ? "" : "s"}`}
              {!phone && shown.length > 0 && (
                <span className="ml-2 hidden sm:inline">
                  <Kbd>j</Kbd> <Kbd>k</Kbd> to move, <Kbd>↵</Kbd> to open
                </span>
              )}
            </p>
            {hits.length > 0 && (
              <Button variant="outline" size="sm" onClick={showContext} disabled={contextBusy} title="A paste-ready summary of the best matches, for an agent">
                {contextBusy ? <Loader2 className="animate-spin" /> : <ClipboardList />}
                Build context
              </Button>
            )}
          </div>

          {loading && hits.length === 0 ? (
            <ResultSkeleton />
          ) : shown.length === 0 ? (
            <Empty icon={<SearchIcon />} title={savedOnly && hits.length ? "None of these are saved" : "No matches"}>
              Try other words, switch to <em>By meaning</em>, or loosen the filters.
            </Empty>
          ) : (
            <div ref={list} className="space-y-2" role="list" aria-label="Results">
              {shown.map((h, i) => (
                <ResultRow
                  key={`${h.device ?? ""}:${h.conversation_id}`}
                  hit={h}
                  index={i}
                  selected={i === selected}
                  phone={phone}
                  remote={!!h.device && h.device !== localDevice}
                  onOpen={() => {
                    setSelected(i);
                    onOpenSession(h.conversation_id, h.device);
                  }}
                  onToggleSaved={() => toggleSaved(h)}
                  onShowThread={() => onShowThreadOf(h.conversation_id)}
                />
              ))}
            </div>
          )}

          {hits.length >= limit && !savedOnly && (
            <div className="flex justify-center pt-2">
              <Button variant="outline" onClick={() => runSearch(limit + PAGE, true)} disabled={loading}>
                {loading && <Loader2 className="animate-spin" />}
                Load more
              </Button>
            </div>
          )}
        </div>
      )}

      {/* Before the first search */}
      {!searched && (
        <div className="space-y-6">
          <p className="text-[13px] text-muted-foreground">
            Try a feature, an error message, a file name or a question:{" "}
            {["oauth refresh retry", "why is the table slow", "rate limiter tests"].map((ex, i) => (
              <React.Fragment key={ex}>
                {i > 0 && ", "}
                <button
                  type="button"
                  className="rounded text-foreground underline decoration-dotted underline-offset-4 hover:text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  onClick={() => {
                    setQuery(ex);
                    input.current?.focus();
                  }}
                >
                  {ex}
                </button>
              </React.Fragment>
            ))}
            . {!phone && <>Press <Kbd>/</Kbd> anywhere to search.</>}
          </p>
          {!phone && (
            <section className="space-y-2">
              <h2 className="flex items-center gap-2 text-sm font-semibold">
                <Star className="size-4 text-warning" /> Saved sessions
              </h2>
              {saved === null ? (
                <ResultSkeleton rows={2} />
              ) : saved.length === 0 ? (
                <p className="text-[13px] text-muted-foreground">
                  Star a result or a transcript to keep it here.
                </p>
              ) : (
                <Card className="divide-y overflow-hidden shadow-none">
                  {saved.map((h) => (
                    <div key={h.conversation_id} className="group flex items-center gap-2 px-3 py-2 hover:bg-accent/50">
                      <button
                        type="button"
                        onClick={() => onOpenSession(h.conversation_id, h.device)}
                        className="flex min-w-0 flex-1 items-center gap-2.5 rounded text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      >
                        <ToolChip tool={h.tool} />
                        <span className="min-w-0 flex-1 truncate text-sm">{h.title?.trim() || "(untitled)"}</span>
                        {h.started_at && (
                          <span className="shrink-0 text-xs text-muted-foreground" title={h.started_at}>
                            {ago(h.started_at)}
                          </span>
                        )}
                      </button>
                      <StarButton on onToggle={() => toggleSaved(h)} />
                    </div>
                  ))}
                </Card>
              )}
            </section>
          )}
        </div>
      )}
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="space-y-1.5">
      <Label className="text-xs font-medium text-muted-foreground">{label}</Label>
      <div className="[&>*]:w-full">{children}</div>
    </div>
  );
}

function StarButton({ on, onToggle }: { on: boolean; onToggle: () => void }) {
  return (
    <Button
      variant="ghost"
      size="icon-sm"
      onClick={(e) => {
        e.stopPropagation();
        onToggle();
      }}
      aria-pressed={on}
      aria-label={on ? "Remove from saved" : "Save for later"}
      title={on ? "Remove from saved" : "Save for later"}
      className={cn(!on && "opacity-60 group-hover:opacity-100 focus-visible:opacity-100")}
    >
      <Star className={cn(on && "fill-warning text-warning")} />
    </Button>
  );
}

function ResultRow({
  hit: h,
  index,
  selected,
  phone,
  remote,
  onOpen,
  onToggleSaved,
  onShowThread,
}: {
  hit: Hit;
  index: number;
  selected: boolean;
  phone: boolean;
  remote: boolean;
  onOpen: () => void;
  onToggleSaved: () => void;
  onShowThread: () => void;
}) {
  const saved = !!(h.bookmarked || h.pinned);
  const isSession = !h.kind || h.kind === "thread";
  return (
    <article
      role="listitem"
      data-index={index}
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === "Enter" && e.target === e.currentTarget) onOpen();
      }}
      className={cn(
        "group cursor-pointer rounded-lg border bg-card p-3.5 transition-colors hover:border-foreground/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring sm:p-4",
        selected && "border-primary/50 ring-1 ring-primary/30",
      )}
    >
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1 space-y-1.5">
          <div className="flex flex-wrap items-center gap-1.5">
            {h.kind === "exchange" ? <Badge variant="info">room exchange</Badge> : <ToolChip tool={h.tool} />}
            {h.kind === "skill" && <Badge variant="info">skill</Badge>}
            {remote && h.device && (
              <Badge variant="outline" title={`From ${h.device}`}>
                <Laptop /> {h.device}
              </Badge>
            )}
            {h.started_at && (
              <span className="text-xs text-muted-foreground" title={new Date(h.started_at).toLocaleString()}>
                {ago(h.started_at)}
              </span>
            )}
          </div>
          <h3 className="line-clamp-2 text-[15px] font-medium leading-snug">{h.title?.trim() || "(untitled)"}</h3>
          {h.project && (
            <p className="truncate font-mono text-xs text-muted-foreground" title={h.project}>
              {shortPath(h.project)}
            </p>
          )}
        </div>
        <div className="-mr-1 -mt-1 flex shrink-0 items-center" onClick={(e) => e.stopPropagation()}>
          {!phone && isSession && !remote && (
            <Button
              variant="ghost"
              size="icon-sm"
              onClick={onShowThread}
              aria-label="Show its thread"
              title="Show the thread this session belongs to"
              className="opacity-0 group-hover:opacity-100 focus-visible:opacity-100 group-focus-within:opacity-100"
            >
              <GitBranch />
            </Button>
          )}
          {!phone && <StarButton on={saved} onToggle={onToggleSaved} />}
        </div>
      </div>
      {h.snippet && (
        <p
          className={cn("mt-2 line-clamp-3 whitespace-pre-line text-[13px] leading-relaxed text-muted-foreground", markClass)}
          dangerouslySetInnerHTML={{ __html: highlightSnippet(h.snippet) }}
        />
      )}
      {h.tags && h.tags.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1">
          {h.tags.map((t) => (
            <Badge key={t} variant="outline" className="font-normal">
              #{t}
            </Badge>
          ))}
        </div>
      )}
    </article>
  );
}

function ResultSkeleton({ rows = 4 }: { rows?: number }) {
  return (
    <div className="space-y-2" aria-hidden="true">
      {Array.from({ length: rows }).map((_, i) => (
        <div key={i} className="space-y-2.5 rounded-lg border p-4">
          <div className="flex gap-2">
            <Skeleton className="h-5 w-24" />
            <Skeleton className="h-5 w-14" />
          </div>
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-3 w-full" />
        </div>
      ))}
    </div>
  );
}

// --- Ask -----------------------------------------------------------------------

function AskView({
  deviceArg,
  onOpenSession,
}: {
  deviceArg: () => string[] | undefined;
  onOpenSession: (id: string, device?: string | null) => void;
}) {
  const [q, setQ] = React.useState(() => {
    try {
      const pre = sessionStorage.getItem(ASK_PREFILL_KEY);
      if (pre) {
        sessionStorage.removeItem(ASK_PREFILL_KEY);
        return pre;
      }
    } catch {
      /* ignore */
    }
    return memory.askQuery;
  });
  const [answer, setAnswer] = React.useState<Answer | null>(memory.answer);
  const [asked, setAsked] = React.useState<string>(memory.answer ? memory.askQuery : "");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const box = React.useRef<HTMLTextAreaElement>(null);

  React.useEffect(() => {
    box.current?.focus();
  }, []);
  React.useEffect(() => {
    memory.askQuery = q;
  }, [q]);

  async function run(e?: React.FormEvent) {
    e?.preventDefault();
    const question = q.trim();
    if (!question || busy) return;
    setBusy(true);
    setError(null);
    setAnswer(null);
    setAsked(question);
    try {
      const a = await ask(question, {}, 12, deviceArg());
      setAnswer(a);
      memory.answer = a;
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  const noModel = !!answer && answer.sources.length > 0 && !answer.llm_used;

  return (
    <div className="space-y-5">
      <form onSubmit={run} className="space-y-2">
        <div className="relative">
          <Textarea
            ref={box}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                run();
              }
            }}
            rows={2}
            placeholder="e.g. how did we end up handling token refresh?"
            aria-label="Your question"
            className="min-h-[88px] resize-none rounded-lg pb-12 pr-4 text-base shadow-sm sm:text-[15px]"
          />
          <Button type="submit" size="sm" className="absolute bottom-2.5 right-2.5" disabled={busy || !q.trim()}>
            {busy ? <Loader2 className="animate-spin" /> : <ArrowRight />}
            Ask
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">
          Answered from your own past sessions across every tool, with citations. Uses your model (Settings → Models);
          without one you get the most relevant excerpts. <Kbd>↵</Kbd> to ask, <Kbd>⇧</Kbd>
          <Kbd>↵</Kbd> for a new line.
        </p>
      </form>

      {error && <Notice tone="error" title="Couldn't answer that">{error}</Notice>}

      {busy && (
        <Card className="space-y-3 p-5 shadow-none" aria-busy="true">
          <p className="flex items-center gap-2 text-sm text-muted-foreground">
            <Sparkles className="size-4 animate-pulse text-primary" /> Reading your past sessions…
          </p>
          <Skeleton className="h-3 w-full" />
          <Skeleton className="h-3 w-11/12" />
          <Skeleton className="h-3 w-4/5" />
        </Card>
      )}

      {!busy && answer && (
        <div className="space-y-4">
          <Card className="p-5 shadow-none">
            <p className="mb-3 text-xs font-medium uppercase tracking-wide text-muted-foreground">{asked}</p>
            {noModel ? (
              <pre className="max-h-[28rem] overflow-auto whitespace-pre-wrap font-mono text-xs leading-relaxed scrollbar-thin">
                {answer.markdown}
              </pre>
            ) : (
              <Markdown source={answer.markdown} />
            )}
            <div className="mt-4 flex justify-end">
              <CopyButton text={answer.markdown} label="Copy answer" />
            </div>
          </Card>
          {noModel && (
            <Notice
              tone="info"
              title="No model connected"
              action={
                <Button size="sm" variant="outline" onClick={() => (window.location.hash = "#/settings/models")}>
                  Set up a model
                </Button>
              }
            >
              These are the most relevant excerpts. Connect a model to get a written answer.
            </Notice>
          )}
          {answer.sources.length > 0 && (
            <section className="space-y-2">
              <h2 className="text-sm font-semibold">
                Sources <span className="font-normal text-muted-foreground">· {answer.sources.length} past session{answer.sources.length === 1 ? "" : "s"}</span>
              </h2>
              <SourceList ids={answer.sources} onOpen={(id) => onOpenSession(id)} />
            </section>
          )}
        </div>
      )}

      {!busy && !answer && !error && (
        <Empty icon={<MessageSquareText />} title="Ask your history anything">
          What you decided, how you fixed something, where you left off. The answer cites the sessions it came from.
        </Empty>
      )}
    </div>
  );
}
