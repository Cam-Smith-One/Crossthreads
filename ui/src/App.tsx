import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  approvePeer,
  buildContext,
  getSession,
  getSetupStatus,
  SignInRequiredError,
  UnpairedError,
  type Session,
  type SetupStatus,
  discoverDevices,
  forget,
  getConversation,
  getDevices,
  getFacets,
  getFederationConfig,
  getSaved,
  getStatus,
  getResume,
  openSource,
  pairWithCode,
  reindex,
  removePeer,
  setPeerRooms,
  search,
  getLlmConfig,
  getThemes,
  getInsight,
  ask,
  getActivity,
  getMetrics,
  getWeeklyReview,
  getOptimize,
  getFluency,
  setFederationConfig,
  setFlags,
  setLlmKey,
  setLlmProvider,
  setNote,
  setTags,
  type Device,
  type DiscoveredDevice,
  type FederationConfig,
  type Filters,
  type LlmConfig,
  type ResumePlan,
  type Theme,
  type InsightKind,
  type Answer,
  type ActivityBucket,
  type WorkMetrics,
  type WeeklyReview,
  type OptimizeReport,
  type FluencyReport,
  type Hit,
  type Mode,
  type Status,
  type StoredConversation,
} from "./api";
import { Board } from "./Board";
import { Rooms } from "./Rooms";
import { PairPhone, PhoneSettings } from "./Phone";
import { SetupChecklist, SetupNudge, staleClientRecently } from "./Setup";
import { SignIn } from "./SignIn";
import { toolIcon, toolLabel } from "./tools";

const MODES: Mode[] = ["hybrid", "semantic", "lexical"];
const MODE_LABELS: Record<Mode, string> = {
  hybrid: "best match",
  semantic: "by meaning",
  lexical: "exact words",
};

type InsightsTab = "overview" | "weekly" | "themes" | "analyses";
/** The app's places. Each is a page with its own address (#/search, …). */
type Page = "threads" | "rooms" | "search" | "insights" | "settings";
type SettingsTab = "start" | "devices" | "phone" | "models" | "index" | "help";
const PAGES: Page[] = ["threads", "rooms", "search", "insights", "settings"];

const ANALYSIS_GROUPS: [string, [InsightKind, string, string][]][] = [
  [
    "About how you work",
    [
      ["work_dna", "Work DNA", "A profile of how you work, backed by your numbers"],
      ["gaps", "Gaps", "What you start but don't finish, plus a draft CLAUDE.md"],
      ["prompt_coach", "Prompt coach", "Before-and-after rewrites of your own prompts"],
      ["process_miner", "Repeated processes", "Things you keep re-deriving, as draft skills"],
    ],
  ],
  [
    "From your recent work",
    [
      ["open_loops", "Open loops", "Unfinished work and fixes never confirmed"],
      ["decision_log", "Decisions", "What you chose over what, and why"],
      ["knowledge_cards", "Knowledge cards", "Answers worth keeping"],
      ["recurrence", "Recurring problems", "What you keep running into, and how to stop"],
    ],
  ],
];
const PAGE = 25;

/** The last two segments of a path: enough to recognise a project. */
function shortPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length <= 2 ? path : `…/${parts.slice(-2).join("/")}`;
}

function ToolChip({ tool }: { tool: string }) {
  return (
    <span className="tool-chip" title={toolLabel(tool)}>
      <span className="tool-chip-icon">{toolIcon(tool)}</span>
      {toolLabel(tool)}
    </span>
  );
}

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
  // Bumped whenever the index changes, so the board reloads its threads.
  const [boardKey, setBoardKey] = useState(0);
  const reportError = useCallback((message: string) => setError(message), []);
  // A session whose thread the board should open (from the transcript viewer).
  const [threadRequest, setThreadRequest] = useState<string | null>(null);

  /** Leave search, go back to the board, and open the thread a session is in. */
  function showThreadOf(conversationId: string) {
    setOpen(null);
    searchGen.current += 1;
    setSearched(false);
    setHits([]);
    setContext(null);
    setThreadRequest(conversationId);
    go("threads");
  }
  const [context, setContext] = useState<string | null>(null);
  const [open, setOpen] = useState<StoredConversation | null>(null);
  const [saved, setSaved] = useState<Hit[]>([]);
  const [theme, setTheme] = useState<string>(
    () => document.documentElement.dataset.theme || "dark",
  );

  const [reindexing, setReindexing] = useState(false);
  const [showFilters, setShowFilters] = useState(false);
  const [page, setPage] = useState<Page>("threads");
  const showSettings = page === "settings";
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("start");
  const [searchMode, setSearchMode] = useState<"find" | "ask">("find");
  const [roomsListening, setRoomsListening] = useState(false);
  const [setup, setSetup] = useState<SetupStatus | null>(null);
  const [nudgeDismissed, setNudgeDismissed] = useState(() => {
    try {
      return localStorage.getItem("ct-setup-nudge") === "dismissed";
    } catch {
      return false;
    }
  });
  const refreshSetup = useCallback(() => {
    getSetupStatus().then(setSetup).catch(() => {});
  }, []);

  /** Go to a place. The address changes, so Back works and links can be shared. */
  function go(p: Page, sub?: string) {
    const hash = `#/${p}${sub ? `/${sub}` : ""}`;
    if (window.location.hash !== hash) window.location.hash = hash;
    else routeRef.current();
  }
  // A paired phone sees a read-only board (SPEC FR-BRD-04); an unpaired one
  // sees the pairing screen.
  const [session, setSession] = useState<Session | null>(null);
  const [unpaired, setUnpaired] = useState(false);
  const [signedOut, setSignedOut] = useState(false);
  const phone = !!session?.phone;
  useEffect(() => {
    if (window.location.pathname === "/login") return;
    getSession()
      .then(setSession)
      .catch((e) => {
        if (e instanceof UnpairedError) setUnpaired(true);
        if (e instanceof SignInRequiredError) setSignedOut(true);
      });
  }, []);
  const [themes, setThemes] = useState<Theme[] | null>(null);
  // One Insights view with tabs replaces what used to be six separate panels.
  const [insightsTab, setInsightsTab] = useState<InsightsTab | null>(null);
  const [namingThemes, setNamingThemes] = useState(false);

  async function openThemes() {
    setInsightsTab("themes");
    setThemes(null);
    try {
      setThemes(await getThemes(8));
    } catch {
      setThemes([]);
    }
  }

  // Re-cluster, asking the model for a short name per theme (feature: LLM-named
  // themes). Best-effort — the daemon keeps keyword labels where it can't name.
  async function nameThemes() {
    setNamingThemes(true);
    try {
      setThemes(await getThemes(8, true));
    } catch {
      /* keep the existing labels on failure */
    } finally {
      setNamingThemes(false);
    }
  }

  // Insights (LLM synthesis over recent history).
  const [insightKind, setInsightKind] = useState<InsightKind>("open_loops");
  const [insightMd, setInsightMd] = useState<string | null>(null);
  const [insightSources, setInsightSources] = useState<number>(0);
  const [insightBusyKind, setInsightBusyKind] = useState<InsightKind | null>(null);
  const [insightError, setInsightError] = useState<string | null>(null);
  const [metrics, setMetrics] = useState<WorkMetrics | null>(null);

  // Proactive weekly review: fetch the cached one on load; show a banner when
  // it's one the user hasn't seen yet.
  const [weekly, setWeekly] = useState<WeeklyReview | null>(null);
  const [weeklyUnseen, setWeeklyUnseen] = useState(false);
  const [weeklyBusy, setWeeklyBusy] = useState(false);

  useEffect(() => {
    getWeeklyReview(false)
      .then((r) => {
        setWeekly(r);
        let seen = "";
        try {
          seen = localStorage.getItem("ct-weekly-seen") ?? "";
        } catch {
          /* ignore */
        }
        // Only nudge when there's real content and it's newer than last seen.
        if (r.generated_at && r.generated_at !== seen && !r.markdown.startsWith("No sessions")) {
          setWeeklyUnseen(true);
        }
      })
      .catch(() => {});
  }, []);

  function openWeekly() {
    setInsightsTab("weekly");
    setWeeklyUnseen(false);
    if (weekly?.generated_at) {
      try {
        localStorage.setItem("ct-weekly-seen", weekly.generated_at);
      } catch {
        /* ignore */
      }
    }
  }

  async function refreshWeekly() {
    setWeeklyBusy(true);
    try {
      const r = await getWeeklyReview(true);
      setWeekly(r);
      try {
        localStorage.setItem("ct-weekly-seen", r.generated_at);
      } catch {
        /* ignore */
      }
    } catch {
      /* ignore */
    } finally {
      setWeeklyBusy(false);
    }
  }

  // Optimize loop ("how you work" → biggest lever + measured verdict + trends).
  const [optimize, setOptimize] = useState<OptimizeReport | null>(null);
  const [optimizeBusy, setOptimizeBusy] = useState(false);
  const [optimizeError, setOptimizeError] = useState<string | null>(null);

  async function openOptimize(force = false) {
    setInsightsTab("overview");
    setOptimizeBusy(true);
    setOptimizeError(null);
    try {
      setOptimize(await getOptimize(force));
    } catch (err) {
      setOptimizeError(String(err));
    } finally {
      setOptimizeBusy(false);
    }
  }

  // AI-fluency (4D: delegation / description / discernment / diligence + reflection).
  const [fluency, setFluency] = useState<FluencyReport | null>(null);
  const [fluencyBusy, setFluencyBusy] = useState(false);
  const [fluencyError, setFluencyError] = useState<string | null>(null);

  async function openFluency() {
    setInsightsTab("overview");
    setFluencyBusy(true);
    setFluencyError(null);
    try {
      setFluency(await getFluency());
    } catch (err) {
      setFluencyError(String(err));
    } finally {
      setFluencyBusy(false);
    }
  }

  async function runInsight(kind: InsightKind) {
    setInsightsTab("analyses");
    setInsightKind(kind);
    setInsightMd(null);
    setInsightError(null);
    setInsightBusyKind(kind);
    // The deterministic metrics strip needs no model — load it once on open.
    if (!metrics) getMetrics().then(setMetrics).catch(() => {});
    try {
      const res = await getInsight(kind);
      setInsightMd(res.markdown);
      setInsightSources(res.sources.length);
    } catch (err) {
      setInsightError(String(err));
    } finally {
      setInsightBusyKind(null);
    }
  }

  // Ask (RAG over history), the second mode of the Search page.
  const [askQuery, setAskQuery] = useState("");
  const [askAnswer, setAskAnswer] = useState<Answer | null>(null);
  const [askBusy, setAskBusy] = useState(false);
  const [askError, setAskError] = useState<string | null>(null);

  async function runAsk(e?: React.FormEvent) {
    e?.preventDefault();
    const q = askQuery.trim();
    if (!q) return;
    setAskBusy(true);
    setAskError(null);
    setAskAnswer(null);
    try {
      setAskAnswer(await ask(q, {}, 12, deviceArg()));
    } catch (err) {
      setAskError(String(err));
    } finally {
      setAskBusy(false);
    }
  }

  // Temporal view (activity over time).
  const [activityBucket, setActivityBucket] = useState<"day" | "week">("week");
  const [activity, setActivity] = useState<ActivityBucket[] | null>(null);

  async function openActivity(bucket: "day" | "week" = activityBucket) {
    setInsightsTab("overview");
    setActivityBucket(bucket);
    setActivity(null);
    try {
      setActivity((await getActivity(bucket)).periods);
    } catch {
      setActivity([]);
    }
  }

  // Cross-device (ADR-010): known devices, which are selected to search, and
  // the on-demand discovery results.
  const [devices, setDevices] = useState<Device[]>([]);
  const [federation, setFederation] = useState(false);
  const [deviceSel, setDeviceSel] = useState<Set<string>>(new Set());
  const [discovered, setDiscovered] = useState<DiscoveredDevice[] | null>(null);
  const [discovering, setDiscovering] = useState(false);
  const [unreachable, setUnreachable] = useState<string[]>([]);
  const [openDevice, setOpenDevice] = useState<string | null>(null);
  const [resumePlan, setResumePlan] = useState<ResumePlan | null>(null);
  const [fedConfig, setFedConfig] = useState<FederationConfig | null>(null);
  const [pairCode, setPairCode] = useState("");
  const [tokenInput, setTokenInput] = useState("");
  const [showPairing, setShowPairing] = useState(false);
  const [llmConfig, setLlmConfig] = useState<LlmConfig | null>(null);
  const [keyInputs, setKeyInputs] = useState<Record<string, string>>({});
  // Monotonic search id so a slow earlier response can't overwrite a newer one.
  const searchGen = useRef(0);

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
  const refreshFedConfig = useCallback(() => {
    getFederationConfig().then(setFedConfig).catch(() => {});
  }, []);
  const refreshLlmConfig = useCallback(() => {
    getLlmConfig().then(setLlmConfig).catch(() => {});
  }, []);

  async function runReindex() {
    setReindexing(true);
    setError(null);
    try {
      await reindex();
      setBoardKey((k) => k + 1);
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

  // Load the federation identity/scope lazily when the Devices tab is shown.
  useEffect(() => {
    if (showSettings && settingsTab === "devices") refreshFedConfig();
    if (showSettings && settingsTab === "models") refreshLlmConfig();
  }, [showSettings, settingsTab, refreshFedConfig, refreshLlmConfig]);

  // Follow the address: #/threads, #/rooms, #/search[/ask], #/insights/<tab>,
  // #/settings/<tab>. A ref so the listener always sees the latest state.
  const routeRef = useRef<() => void>(() => {});
  routeRef.current = () => {
    const [p, sub] = window.location.hash.replace(/^#\/?/, "").split("/");
    let next: Page = PAGES.includes(p as Page) ? (p as Page) : "threads";
    if (phone && (next === "insights" || next === "settings")) next = "threads";
    setPage(next);
    if (next === "insights") openInsights((sub as InsightsTab) || "overview");
    if (next === "settings") setSettingsTab((sub as SettingsTab) || "start");
    if (next === "search") setSearchMode(sub === "ask" ? "ask" : "find");
    window.scrollTo({ top: 0 });
  };
  useEffect(() => {
    const onHash = () => routeRef.current();
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  // Apply the address once we know whether this is a phone.
  useEffect(() => {
    routeRef.current();
  }, [phone]);
  useEffect(() => {
    if (!phone) refreshSetup();
  }, [phone, refreshSetup]);
  useEffect(() => {
    if (page === "settings" && settingsTab === "start") refreshSetup();
  }, [page, settingsTab, refreshSetup]);

  // The local device's name, so remote-only chips can be shown on results.
  const localDevice = devices.find((d) => d.local)?.name;

  /** Open the Insights view on a tab, loading what that tab shows. Nothing
   * here calls a model on its own; the model-backed analyses run on request. */
  function openInsights(tab: InsightsTab) {
    setInsightsTab(tab);
    if (tab === "overview") {
      openFluency();
      openOptimize(false);
      openActivity(activityBucket);
    } else if (tab === "weekly") {
      openWeekly();
    } else if (tab === "themes") {
      if (!themes || themes.length === 0) openThemes();
    } else if (!metrics) {
      getMetrics().then(setMetrics).catch(() => {});
    }
  }

  // Device routing arg for searches: undefined (= all) unless the user has
  // narrowed the selection to a strict, non-empty subset.
  const deviceArg = useCallback((): string[] | undefined => {
    if (!federation) return undefined;
    const names = devices.map((d) => d.name);
    if (deviceSel.size === 0 || deviceSel.size === names.length) return undefined;
    return names.filter((n) => deviceSel.has(n));
  }, [federation, devices, deviceSel]);

  // Per-tool counts of sessions (not skills or room exchanges), busiest first.
  const toolChips = (status?.tools ?? [])
    .filter(([tool, sessions]) => tool !== "room" && sessions > 0)
    .sort((a, b) => b[1] - a[1]);
  const sessionCount = toolChips.reduce((n, [, sessions]) => n + sessions, 0);
  // Essential setup steps still to do (the sidebar and Settings show the count).
  const setupLeft = setup
    ? [
        sessionCount > 0,
        setup.hooks_claude || setup.hooks_codex,
        setup.mcp_claude || setup.mcp_codex,
        setup.skill_claude || setup.skill_codex,
      ].filter((done) => !done).length
    : 0;
  const activeFilters = [
    filters.kind,
    filters.tool,
    filters.tag,
    filters.project,
    filters.since,
    filters.until,
    mode !== "hybrid" ? mode : undefined,
    deviceArg() ? "devices" : undefined,
  ].filter(Boolean).length;

  const toggleDevice = (name: string) =>
    setDeviceSel((sel) => {
      const next = new Set(sel);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      // Never leave zero selected (that would silently read as "search all");
      // clearing the last device resets the picker to all devices.
      if (next.size === 0) return new Set(devices.map((d) => d.name));
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

  async function toggleRooms(name: string, rooms: boolean) {
    try {
      setDevices(await setPeerRooms(name, rooms));
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
    async (e?: React.FormEvent, lim = PAGE, keepSelection = false) => {
      e?.preventDefault();
      if (!query.trim()) return;
      const gen = ++searchGen.current;
      setLoading(true);
      setError(null);
      setContext(null);
      try {
        const { hits: results, unreachable } = await search(query, mode, filters, lim, deviceArg());
        if (gen !== searchGen.current) return; // a newer search superseded this one
        setHits(results);
        setUnreachable(unreachable);
        setLimit(lim);
        setSelected((s) =>
          keepSelection ? Math.min(s, results.length - 1) : results.length ? 0 : -1,
        );
        setSearched(true);
      } catch (err) {
        if (gen === searchGen.current) setError(String(err));
      } finally {
        if (gen === searchGen.current) setLoading(false);
      }
    },
    [query, mode, filters, deviceArg],
  );

  // Fetch the next page by raising the limit and re-querying (search returns
  // top-N, so we replace rather than append) — keep the user's selection.
  async function loadMore() {
    await runSearch(undefined, limit + PAGE, true);
  }

  async function openConversation(id: string, device?: string | null) {
    try {
      const convo = await getConversation(id, device);
      if (convo) {
        setFind("");
        setOpenDevice(device && device !== localDevice ? device : null);
        setOpen(convo);
      }
    } catch (err) {
      setError(String(err));
    }
  }

  // Resolve how to resume the open conversation (canonical, from the daemon).
  // Remote sources (on another device) can't be resumed locally.
  const openId = open?.id;
  useEffect(() => {
    if (!openId || openDevice) {
      setResumePlan(null);
      return;
    }
    let cancelled = false;
    getResume(openId)
      .then((r) => !cancelled && setResumePlan(r))
      .catch(() => !cancelled && setResumePlan(null));
    return () => {
      cancelled = true;
    };
  }, [openId, openDevice]);

  async function saveFedConfig(patch: Parameters<typeof setFederationConfig>[0]) {
    try {
      setFedConfig(await setFederationConfig(patch));
      refreshDevices();
    } catch (err) {
      setError(String(err));
    }
  }

  async function submitPairCode() {
    const code = pairCode.trim();
    if (!code) return;
    try {
      setDevices(await pairWithCode(code));
      setPairCode("");
      refreshFedConfig();
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
    // Defer revoke so the browser can start the download first (revoking
    // synchronously after click() races it in Firefox/Safari).
    setTimeout(() => URL.revokeObjectURL(url), 0);
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
      setBoardKey((k) => k + 1);
      refreshSaved();
    } catch (err) {
      setError(String(err));
    }
  }

  // "/" or Ctrl/Cmd+K jumps to search from anywhere that isn't a text field.
  const searchInput = useRef<HTMLInputElement>(null);
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const typing = ["INPUT", "TEXTAREA", "SELECT"].includes(
        document.activeElement?.tagName ?? "",
      );
      const palette = e.key.toLowerCase() === "k" && (e.metaKey || e.ctrlKey);
      if (palette || (e.key === "/" && !typing)) {
        e.preventDefault();
        go("search", "find");
        window.setTimeout(() => {
          searchInput.current?.focus();
          searchInput.current?.select();
        }, 0);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Keyboard navigation over results (j/k or arrows; Enter opens; Esc closes).
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (open) {
        if (e.key === "Escape") setOpen(null);
        return;
      }
      if (page !== "search" || !hits.length) return;
      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        setSelected((s) => Math.min(s + 1, hits.length - 1));
      } else if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        setSelected((s) => Math.max(s - 1, 0));
      } else if (
        e.key === "Enter" &&
        selected >= 0 &&
        !["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement?.tagName ?? "")
      ) {
        openConversation(hits[selected].conversation_id, hits[selected].device);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    hits,
    selected,
    open,
    page,
  ]);

  const setF = (patch: Partial<Filters>) => setFilters((f) => ({ ...f, ...patch }));

  // One "save" star, shared by result rows, the saved list and the viewer.
  // (Older indexes may hold pins; a pinned session counts as saved too.)
  const flagButtons = (h: Pick<Hit, "conversation_id" | "pinned" | "bookmarked">) => {
    if (phone) return null;
    const saved = !!(h.bookmarked || h.pinned);
    return (
      <span className="hit-actions" onClick={(e) => e.stopPropagation()}>
        <button
          className={`icon star ${saved ? "on" : ""}`}
          title={saved ? "Remove from saved" : "Save for later"}
          aria-label={saved ? "Remove from saved" : "Save for later"}
          aria-pressed={saved}
          onClick={() =>
            toggleFlag(
              h.conversation_id,
              saved ? { bookmarked: false, pinned: false } : { bookmarked: true },
            )
          }
        >
          {saved ? "★" : "☆"}
        </button>
      </span>
    );
  };

  // Filter + highlight the open transcript once per [open, find] — not twice per
  // render (count + list) and not on every unrelated re-render.
  const visibleTurns = useMemo(() => {
    if (!open) return [];
    const term = find.trim();
    return open.messages
      .map((m, i) => ({ m, i }))
      .filter(({ m }) => !term || matchesFind(m.content, term))
      .map(({ m, i }) => ({ key: i, role: m.role, html: highlightFind(m.content, term) }));
  }, [open, find]);

  if (signedOut || window.location.pathname === "/login") return <SignIn />;
  if (unpaired || window.location.pathname === "/pair") return <PairPhone />;

  return (
    <div className={`shell${phone ? " on-phone" : ""}`}>
      <aside className="sidebar">
        <a className="side-brand" href="#/threads" aria-label="Crossthreads home">
          <img className="brand-mark" src="/mark.png" alt="" />
          <span className="brand-name">
            <span className="brand-cross">Cross</span>
            <span className="brand-threads">threads</span>
          </span>
        </a>
        <nav className="side-nav" aria-label="Main">
            {(
              [
                ["threads", "🧵", "Threads", "Your work, one thread per piece of work"],
                ["rooms", "🤝", "Rooms", "Where your agents talk to each other"],
                ["search", "🔎", "Search", "Find any past session, or ask a question"],
                ...(phone
                  ? []
                  : [
                      ["insights", "💡", "Insights", "How you work, your weekly review, themes"],
                      ["settings", "⚙️", "Settings", "Set up, devices, phone, models"],
                    ]),
              ] as [Page, string, string, string][]
            ).map(([p, icon, label, hint]) => (
              <a
                key={p}
                href={`#/${p}`}
                className={`side-link ${page === p ? "on" : ""}`}
                aria-current={page === p ? "page" : undefined}
                title={hint}
              >
                <span className="side-icon" aria-hidden="true">
                  {icon}
                </span>
                <span className="side-label">{label}</span>
                {p === "rooms" && roomsListening && (
                  <span className="nav-dot live" aria-label="an agent is listening" />
                )}
                {p === "insights" && weeklyUnseen && (
                  <span className="nav-dot" aria-label="new weekly review" />
                )}
                {p === "settings" && setupLeft > 0 && (
                  <span className="side-count" aria-label={`${setupLeft} setup steps left`}>
                    {setupLeft}
                  </span>
                )}
              </a>
            ))}
        </nav>
        <div className="side-foot">
          {status && (
            <a
              className="side-status"
              href={phone ? undefined : "#/settings/index"}
              title={`${status.conversations.toLocaleString()} records indexed · ${status.embeddings.toLocaleString()} embeddings · embedder ${status.embedder}`}
            >
              {sessionCount.toLocaleString()} session{sessionCount === 1 ? "" : "s"} ·{" "}
              {toolChips.length} tool{toolChips.length === 1 ? "" : "s"}
            </a>
          )}
          {phone && <span className="phone-badge">Read-only on {session?.name ?? "phone"}</span>}
          <button
            className="theme-toggle"
            onClick={toggleTheme}
            title={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
            aria-label="Toggle theme"
          >
            {theme === "dark" ? "☀️" : "🌙"}
          </button>
        </div>
      </aside>

      <main className="main">
      {error && (
        <div className="error">
          {error}
          <button className="linklike" onClick={() => setError(null)}>
            dismiss
          </button>
        </div>
      )}

      {page === "threads" && (
        <section className="page">
          <div className="page-head">
            <h2>Threads</h2>
            <p className="page-lede">
              Each thread is one piece of work: every session that touched it, across your tools
              {federation && devices.length > 1 ? " and machines" : ""}. Open one to see its
              history and pick it up where it left off.
            </p>
          </div>
          {!phone && setup && (!nudgeDismissed || staleClientRecently(setup)) && (
            <SetupNudge
              setup={setup}
              sessions={sessionCount}
              onOpen={() => go("settings", "start")}
              onDismiss={() => {
                setNudgeDismissed(true);
                try {
                  localStorage.setItem("ct-setup-nudge", "dismissed");
                } catch {
                  /* ignore */
                }
              }}
            />
          )}
          {weeklyUnseen && !phone && (
            <div className="nudge">
              <div>
                <strong>📋 Your weekly review is ready</strong>
                <span className="nudge-next">How you worked, and one thing to try.</span>
              </div>
              <span className="nudge-actions">
                <button onClick={() => go("insights", "weekly")}>Read it</button>
                <button className="linklike" onClick={() => setWeeklyUnseen(false)}>
                  Dismiss
                </button>
              </span>
            </div>
          )}
          {status && status.conversations === 0 && (
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


          <Board
            onOpenSession={(id, device) => openConversation(id, device)}
            openThreadFor={threadRequest}
            onThreadOpened={() => setThreadRequest(null)}
            localDevice={localDevice}
            readOnly={phone}
            refreshKey={boardKey}
            onError={reportError}
          />
        </section>
      )}

      <Rooms
        active={page === "rooms"}
        onError={reportError}
        canPost={session?.can_post ?? true}
        onListening={setRoomsListening}
      />

      {page === "search" && (
        <section className="page">
          <div className="page-head">
            <h2>Search</h2>
            <p className="page-lede">
              Every session from every tool{federation && devices.length > 1 ? " and machine" : ""},
              searchable by words or by meaning. Or ask a question and get an answer drawn from
              them, with the sessions it came from.
            </p>
          </div>
          {!phone && (
            <div className="seg" role="tablist" aria-label="Search mode">
              <button
                role="tab"
                aria-selected={searchMode === "find"}
                className={searchMode === "find" ? "on" : ""}
                onClick={() => go("search", "find")}
              >
                Find sessions
              </button>
              <button
                role="tab"
                aria-selected={searchMode === "ask"}
                className={searchMode === "ask" ? "on" : ""}
                onClick={() => go("search", "ask")}
              >
                Ask a question
              </button>
            </div>
          )}
          {(searchMode === "find" || phone) && (
            <>
            <form className="searchbar" onSubmit={runSearch}>
              <input
                ref={searchInput}
                aria-label="Search your sessions"
                title="Search (press / or Ctrl+K)"
                placeholder="e.g. where did we set up the OAuth refresh retry?"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
              <button
                type="button"
                className={`secondary filters-toggle ${showFilters ? "on" : ""}`}
                onClick={() => setShowFilters((v) => !v)}
                aria-expanded={showFilters}
                title="Narrow the search by tool, kind, project, date or device"
              >
                Filters{activeFilters > 0 ? ` · ${activeFilters}` : ""}
              </button>
              <button type="submit" disabled={loading}>
                {loading ? "…" : "Search"}
              </button>
              {searched && hits.length > 0 && (
                <button type="button" className="secondary" onClick={showContext} disabled={loading}>
                  Build context
                </button>
              )}
            </form>

            {showFilters && (
            <div className="filters">
              <select
                value={mode}
                onChange={(e) => setMode(e.target.value as Mode)}
                title="Hybrid combines keyword and meaning; keyword matches exact words; meaning finds related wording"
                aria-label="Search mode"
              >
                {MODES.map((m) => (
                  <option key={m} value={m}>
                    {MODE_LABELS[m]}
                  </option>
                ))}
              </select>
              <select
                value={filters.kind ?? ""}
                onChange={(e) => setF({ kind: e.target.value || undefined })}
                aria-label="What to search"
              >
                <option value="">everything</option>
                <option value="thread">sessions</option>
                <option value="exchange">room exchanges</option>
                <option value="skill">skills &amp; prompts</option>
              </select>
              <select value={filters.tool ?? ""} onChange={(e) => setF({ tool: e.target.value || undefined })}>
                <option value="">all tools</option>
                {tools.map((t) => (
                  <option key={t} value={t}>
                    {toolLabel(t)}
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

            {searched && unreachable.length > 0 && (
              <div className="unreachable-banner">
                ⚠ {unreachable.length} device{unreachable.length > 1 ? "s" : ""} unreachable (
                {unreachable.join(", ")}) — results may be incomplete.
              </div>
            )}

            <section className="results">
              {searched && hits.length === 0 && !loading && <p className="empty">No matches.</p>}
              {hits.map((h, i) => (
                <article
                  key={h.conversation_id}
                  className={`hit ${i === selected ? "selected" : ""}`}
                  onClick={() => {
                    setSelected(i);
                    openConversation(h.conversation_id, h.device);
                  }}
                >
                  <div className="hit-head">
                    {h.kind === "skill" && <span className="badge-skill">skill</span>}
                    {h.kind === "exchange" ? (
                      <span className="badge-skill">room exchange</span>
                    ) : (
                      <ToolChip tool={h.tool} />
                    )}
                    {h.device && h.device !== localDevice && (
                      <span className="device-chip" title={`from ${h.device}`}>💻 {h.device}</span>
                    )}
                    <span className="title">{h.title ?? "(untitled)"}</span>
                    {h.started_at && <span className="date">{h.started_at.slice(0, 10)}</span>}
                    {flagButtons(h)}
                  </div>
                  {h.project && (
                    <div className="project" title={h.project}>
                      {shortPath(h.project)}
                    </div>
                  )}
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

              {!searched && (
                <div className="search-empty">
                  <p className="settings-hint">
                    Try a feature, an error message, a file name or a question: <em>oauth refresh
                    retry</em>, <em>why is the table slow</em>, <em>rate limiter tests</em>. Press{" "}
                    <kbd>/</kbd> anywhere to search.
                  </p>
              {saved.length > 0 && !searched && (
                <section className="saved">
                  <h3>Saved sessions</h3>
                  <div className="saved-list">
                    {saved.map((h) => (
                      <div
                        key={h.conversation_id}
                        className="saved-item"
                        onClick={() => openConversation(h.conversation_id)}
                      >
                        <ToolChip tool={h.tool} />
                        <span className="title">{h.title ?? "(untitled)"}</span>
                        {h.started_at && <span className="date">{h.started_at.slice(0, 10)}</span>}
                        {flagButtons(h)}
                      </div>
                    ))}
                  </div>
                </section>
              )}


                </div>
              )}
            </>
          )}
          {searchMode === "ask" && !phone && (
            <div className="ask-pane">
            <form className="ask-form" onSubmit={runAsk}>
              <input
                autoFocus
                type="text"
                placeholder="e.g. how did we end up handling token refresh?"
                value={askQuery}
                onChange={(e) => setAskQuery(e.target.value)}
              />
              <button type="submit" disabled={askBusy || !askQuery.trim()}>
                {askBusy ? "Asking…" : "Ask"}
              </button>
            </form>
            <p className="doc-desc">
              Answered from your own past sessions across every tool, with citations.
              Uses your configured model (Settings → Models); without one it returns the
              retrieved context.
            </p>
            {askError && <div className="error">{askError}</div>}
            {!askBusy && askAnswer && (
              <>
                <pre className="insight-body">{askAnswer.markdown}</pre>
                <p className="doc-desc">
                  {askAnswer.sources.length > 0
                    ? `Answered from ${askAnswer.sources.length} past session(s)`
                    : "No matching sessions found"}
                  {askAnswer.sources.length > 0 && !askAnswer.llm_used
                    ? " — retrieved context (no model configured)."
                    : "."}
                </p>
              </>
            )}

            </div>
          )}
        </section>
      )}

      {open && (
        <div className="overlay" onClick={() => setOpen(null)}>
          <div className="modal" role="dialog" aria-modal="true" onClick={(e) => e.stopPropagation()}>
            <div className="modal-head">
              <div className="transcript-head-main">
                <ToolChip tool={open.tool} />{" "}
                <strong>{open.title ?? "(untitled)"}</strong>
                {open.project && (
                  <div className="project" title={open.project}>
                    {shortPath(open.project)}
                  </div>
                )}
              </div>
              <div className="modal-head-actions">
                {flagButtons({
                  conversation_id: open.id,
                  pinned: open.pinned,
                  bookmarked: open.bookmarked,
                })}
                {!openDevice && open.kind !== "skill" && open.kind !== "exchange" && (
                  <button
                    className="secondary"
                    title="See every session that is part of the same piece of work"
                    onClick={() => showThreadOf(open.id)}
                  >
                    Open its thread
                  </button>
                )}
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
                <span className="find-count">{visibleTurns.length} match(es)</span>
              )}
            </div>
            <div className="transcript">
              {visibleTurns.map((t) => (
                <div key={t.key} className={`turn turn-${t.role}`}>
                  <span className="role">{t.role}</span>
                  <div className="content" dangerouslySetInnerHTML={{ __html: t.html }} />
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
                  <span className="source-label">
                    {openDevice ? `source on ${openDevice}` : "source"}
                  </span>
                  <code>{open.source_path}</code>
                </div>
                <div className="source-actions">
                  {!openDevice && (
                    <button
                      className="secondary"
                      title="Reveal the original chat file in your file manager"
                      onClick={() => reveal(open.id)}
                    >
                      Reveal
                    </button>
                  )}
                  {resumePlan?.action.kind === "resume" && (
                    <button
                      className="secondary"
                      title={`Copy: ${resumePlan.action.display}`}
                      onClick={() =>
                        navigator.clipboard?.writeText(
                          resumePlan.action.kind === "resume" ? resumePlan.action.display : "",
                        )
                      }
                    >
                      Copy resume command
                    </button>
                  )}
                  <button
                    className="secondary"
                    onClick={() => navigator.clipboard?.writeText(open.source_path!)}
                  >
                    Copy path
                  </button>
                  <button
                    className="secondary"
                    title="Download this conversation as Markdown"
                    onClick={() => exportConversation(open, "md")}
                  >
                    Export
                  </button>
                  <button
                    className="danger"
                    title="Remove this conversation from the index for good"
                    onClick={() => forgetConversation(open.id)}
                  >
                    Forget
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      )}

      {page === "insights" && insightsTab && (
        <section className="page insights-hub">
          <div className="page-head">
            <h2>Insights</h2>
            <p className="page-lede">
              What your history says about how you work. The scores, focus and activity are
              computed here with no model; the weekly review and analyses use your model.
            </p>
          </div>
            <div className="settings-tabs insights-tabs" role="tablist" aria-label="Insights">
              {(
                [
                  ["overview", "How you work"],
                  ["weekly", "Weekly review"],
                  ["themes", "Themes"],
                  ["analyses", "Analyses"],
                ] as [InsightsTab, string][]
              ).map(([tab, label]) => (
                <button
                  key={tab}
                  role="tab"
                  aria-selected={insightsTab === tab}
                  className={insightsTab === tab ? "on" : ""}
                  onClick={() => go("insights", tab)}
                >
                  {label}
                  {tab === "weekly" && weeklyUnseen && <span className="tab-dot" aria-label="new" />}
                </button>
              ))}
            </div>

            {insightsTab === "overview" && (
              <div className="insights-pane">
            <p className="doc-desc">
              Four sides of working with an agent, scored from your own sessions, with the change since the previous 30 days. Computed locally; no model.
            </p>
            {fluencyBusy && <p className="doc-desc">Computing…</p>}
            {fluencyError && <div className="error">{fluencyError}</div>}
            {!fluencyBusy && !fluencyError && fluency && fluency.sessions === 0 && (
              <p className="doc-desc">No sessions in the last {fluency.window_days} days yet.</p>
            )}
            {!fluencyBusy && !fluencyError && fluency && fluency.sessions > 0 && (
              <div className="fluency">
                <div className="fluency-overall">
                  <span className="fluency-overall-score">{fluency.overall}</span>
                  <span className="fluency-overall-cap">/100 overall</span>
                  <span className="fluency-overall-meta">
                    {fluency.sessions} session{fluency.sessions === 1 ? "" : "s"} · last{" "}
                    {fluency.window_days} days
                  </span>
                </div>
                <div className="fluency-grid">
                  {fluency.dimensions.map((d) => (
                    <div key={d.key} className="fluency-card">
                      <div className="fluency-card-head">
                        <span className="fluency-card-label">{d.label}</span>
                        <span className="fluency-card-score">
                          {d.score}
                          {d.delta != null && d.delta !== 0 && (
                            <span
                              className={`fluency-delta ${d.delta > 0 ? "up" : "down"}`}
                              title="Change vs. the previous window"
                            >
                              {d.delta > 0 ? `▲ +${d.delta}` : `▼ ${d.delta}`}
                            </span>
                          )}
                        </span>
                      </div>
                      <div className="fluency-meter">
                        <div
                          className="fluency-meter-fill"
                          style={{ width: `${d.score}%` }}
                        />
                      </div>
                      <p className="fluency-blurb">{d.blurb}</p>
                      <p className="fluency-read">{d.read}</p>
                      <div className="fluency-components">
                        {d.components.map(([k, v]) => (
                          <span key={k} className="fluency-chip">
                            {k}: <strong>{v}</strong>
                          </span>
                        ))}
                      </div>
                    </div>
                  ))}
                </div>
                {fluency.reflection && (
                  <div className="fluency-reflection">
                    <div className="fluency-reflection-head">A question to sit with</div>
                    <p className="fluency-reflection-prompt">{fluency.reflection.prompt}</p>
                    <button
                      className="secondary"
                      onClick={() => {
                        setAskQuery(fluency.reflection!.prompt);
                        go("search", "ask");
                      }}
                      title="Talk it through with your history"
                    >
                      💬 Reflect with Ask
                    </button>
                  </div>
                )}
              </div>
            )}
                <div className="insights-section-head">
                  <h3>Your focus</h3>
                  <button
                    className="secondary"
                    onClick={() => openOptimize(true)}
                    disabled={optimizeBusy}
                    title="Grade the current focus now instead of waiting ~7 days"
                  >
                    {optimizeBusy ? "…" : "↻ Grade now"}
                  </button>
                </div>
            <p className="doc-desc">
              The one change most worth making now, whether the last one worked, and how your numbers are trending.
            </p>
            {optimizeBusy && <p className="doc-desc">Computing…</p>}
            {optimizeError && <div className="error">{optimizeError}</div>}
            {!optimizeBusy && !optimizeError && optimize && (
              <pre className="insight-body">{optimize.markdown}</pre>
            )}
                <div className="insights-section-head">
                  <h3>Activity</h3>
                  <div className="settings-tabs">
                    <button
                      className={activityBucket === "day" ? "on" : ""}
                      onClick={() => openActivity("day")}
                    >
                      Day
                    </button>
                    <button
                      className={activityBucket === "week" ? "on" : ""}
                      onClick={() => openActivity("week")}
                    >
                      Week
                    </button>
                  </div>
                </div>
            {!activity && <p className="doc-desc">Loading…</p>}
            {activity && activity.length === 0 && (
              <p className="doc-desc">No dated sessions yet — index some first.</p>
            )}
            {activity && activity.length > 0 && (() => {
              const total = activity.reduce((s, p) => s + p.total, 0);
              const max = Math.max(...activity.map((p) => p.total), 1);
              const busiest = activity.reduce((a, b) => (b.total > a.total ? b : a));
              // Aggregate per-tool totals across all periods, busiest first.
              const toolTotals = new Map<string, number>();
              for (const p of activity)
                for (const [t, n] of p.by_tool)
                  toolTotals.set(t, (toolTotals.get(t) ?? 0) + n);
              const tools = [...toolTotals.entries()].sort((a, b) => b[1] - a[1]);
              const color = (t: string) =>
                `hsl(${(tools.findIndex(([x]) => x === t) * 67 + 210) % 360} 65% 60%)`;
              return (
                <>
                  <div className="activity-summary">
                    <span><b>{total.toLocaleString()}</b> sessions</span>
                    <span><b>{activity.length}</b> active {activityBucket}s</span>
                    <span>busiest <b>{busiest.period}</b> ({busiest.total})</span>
                    <span><b>{(total / activity.length).toFixed(1)}</b> avg/{activityBucket}</span>
                  </div>
                  <div className="activity-legend">
                    {tools.map(([t, n]) => (
                      <span key={t} className="activity-legend-item" title={`${n} sessions`}>
                        <span className="activity-swatch" style={{ background: color(t) }} />
                        {toolLabel(t)} <span className="activity-legend-count">{n}</span>
                      </span>
                    ))}
                  </div>
                  <div className="activity-chart">
                    {activity.map((p) => (
                      <div key={p.period} className="activity-row">
                        <span className="activity-period">{p.period}</span>
                        <span className="activity-bar-wrap">
                          <span className="activity-bar" style={{ width: `${(p.total / max) * 100}%` }}>
                            {p.by_tool.map(([t, n]) => (
                              <span
                                key={t}
                                className="activity-seg"
                                style={{ background: color(t), flexGrow: n }}
                                title={`${toolLabel(t)}: ${n}`}
                              />
                            ))}
                          </span>
                        </span>
                        <span className="activity-count">{p.total}</span>
                      </div>
                    ))}
                  </div>
                </>
              );
            })()}
              </div>
            )}

            {insightsTab === "weekly" && (
              <div className="insights-pane">
                <div className="insights-section-head">
                  <span />
                  <button
                    className="secondary"
                    onClick={refreshWeekly}
                    disabled={weeklyBusy}
                    title="Regenerate from the last 7 days"
                  >
                    {weeklyBusy ? "Refreshing…" : "↻ Refresh"}
                  </button>
                </div>
            {weekly && (
              <p className="doc-desc">
                Week of {weekly.period_start}
                {weekly.llm_used ? "" : " — metrics only (add a model for a narrative)"}.
              </p>
            )}
            {weeklyBusy && <p className="doc-desc">Generating…</p>}
            {weekly && !weeklyBusy && <pre className="insight-body">{weekly.markdown}</pre>}
            {!weekly && !weeklyBusy && (
              <p className="doc-desc">No review yet — index some sessions first.</p>
            )}
              </div>
            )}

            {insightsTab === "themes" && (
              <div className="insights-pane">
                <div className="insights-section-head">
                  <span />
                  <button
                    className="secondary"
                    onClick={nameThemes}
                    disabled={namingThemes || !themes || themes.length === 0}
                    title="Label each theme with a short AI-generated name (needs a model)"
                  >
                    {namingThemes ? "Naming…" : "✨ Name with AI"}
                  </button>
                </div>
            <p className="doc-desc">
              Your indexed sessions clustered by topic — what you've been working on,
              across tools. Click a session to open it.
            </p>
            {!themes && <p className="doc-desc">Clustering…</p>}
            {themes && themes.length === 0 && (
              <p className="doc-desc">
                No themes yet — index some sessions first (semantic themes need
                embeddings).
              </p>
            )}
            {themes && themes.length > 0 && (
              <div className="theme-grid">
                {themes.map((t, i) => {
                  // Scale the card by cluster size (sqrt keeps big themes sane).
                  const max = Math.max(...themes.map((x) => x.size));
                  const weight = Math.sqrt(t.size / max);
                  return (
                    <div
                      key={i}
                      className="theme-card"
                      style={{ flexGrow: 1 + weight * 3, minWidth: `${180 + weight * 120}px` }}
                    >
                      <div className="theme-card-head">
                        <span className="theme-label">{t.label}</span>
                        <span className="theme-size">{t.size}</span>
                      </div>
                      <div className="theme-tools">
                        {t.tools.slice(0, 3).map(([name, n]) => (
                          <span key={name} className="theme-tool-chip">
                            {name} {n}
                          </span>
                        ))}
                      </div>
                      <ul className="theme-samples">
                        {t.samples.slice(0, 4).map((s) => (
                          <li key={s.id}>
                            <button
                              className="linklike"
                              title={s.title ?? ""}
                              onClick={() => openConversation(s.id)}
                            >
                              {(s.title ?? "(untitled)").trim()}
                            </button>
                          </li>
                        ))}
                      </ul>
                    </div>
                  );
                })}
              </div>
            )}
              </div>
            )}

            {insightsTab === "analyses" && (
              <div className="insights-pane">
            {metrics && metrics.sessions > 0 && (
              <div className="metric-strip">
                <span><b>{metrics.sessions}</b> sessions</span>
                <span><b>{metrics.median_user_turns}</b> median turns</span>
                <span><b>{Math.round(metrics.correction_rate * 100)}%</b> rework</span>
                <span><b>{Math.round(metrics.abandonment_rate * 100)}%</b> unresolved</span>
                <span><b>{Math.round(metrics.specificity_rate * 100)}%</b> specific prompts</span>
                <span><b>{metrics.avg_tool_actions.toFixed(1)}</b> actions/session</span>
                {metrics.median_session_minutes > 0 && (
                  <span><b>{Math.round(metrics.median_session_minutes)}</b> min median</span>
                )}
                {metrics.top_languages[0] && (
                  <span>top lang <b>{metrics.top_languages[0][0]}</b></span>
                )}
              </div>
            )}
            <p className="doc-desc">
              Each of these reads your history and writes a short report with your configured
              model. They run only when you click one.
            </p>
            {ANALYSIS_GROUPS.map(([group, kinds]) => (
              <div key={group} className="analysis-group">
                <h3>{group}</h3>
                <div className="analysis-grid">
                  {kinds.map(([k, label, blurb]) => (
                    <button
                      key={k}
                      className={`analysis-card ${insightKind === k && (insightMd || insightBusyKind) ? "on" : ""}`}
                      onClick={() => runInsight(k)}
                      disabled={insightBusyKind !== null}
                    >
                      <span className="analysis-label">{label}</span>
                      <span className="analysis-blurb">{blurb}</span>
                    </button>
                  ))}
                </div>
              </div>
            ))}
            {insightBusyKind && <p className="doc-desc">Writing it up… this takes a little while.</p>}
            {insightError && /no usable model credential/i.test(insightError) && (
              <div className="needs-model">
                <p>
                  <strong>These need a model.</strong> Sign into Claude Code or Codex on this
                  machine, or paste an API key, and Crossthreads will use it. Nothing is sent
                  anywhere else.
                </p>
                <button
                  onClick={() => go("settings", "models")}
                >
                  Set up a model
                </button>
              </div>
            )}
            {insightError && !/no usable model credential/i.test(insightError) && (
              <div className="error">{insightError}</div>
            )}
            {!insightBusyKind && !insightError && insightMd && (
              <>
                <pre className="insight-body">{insightMd}</pre>
                {insightSources > 0 && (
                  <p className="doc-desc">Drawn from {insightSources} session(s).</p>
                )}
              </>
            )}
              </div>
            )}
        </section>
      )}

      {showSettings && (
        <section className="page settings-page">
          <div className="page-head">
            <h2>Settings</h2>
          </div>
          <div className="settings-tabs page-tabs" role="tablist" aria-label="Settings">
            {(
              [
                ["start", "Get started"],
                ["devices", "Devices"],
                ["phone", "Phone"],
                ["models", "Models"],
                ["index", "Index"],
                ["help", "Help"],
              ] as [SettingsTab, string][]
            ).map(([tab, label]) => (
              <button
                key={tab}
                role="tab"
                aria-selected={settingsTab === tab}
                className={settingsTab === tab ? "on" : ""}
                onClick={() => go("settings", tab)}
              >
                {label}
                {tab === "start" && setupLeft > 0 && <span className="tab-dot" aria-label="steps left" />}
              </button>
            ))}
          </div>

            {settingsTab === "start" && (
              <div className="settings-body">
                <SetupChecklist
                  setup={setup}
                  sessions={sessionCount}
                  reindexing={reindexing}
                  onReindex={async () => {
                    await runReindex();
                    refreshSetup();
                  }}
                  go={go}
                  onChanged={refreshSetup}
                />
              </div>
            )}

            {settingsTab === "index" && (
              <div className="settings-body">
                <section className="settings-section">
                  <h3>Your index</h3>
                  <p className="settings-hint">
                    Crossthreads keeps one local database of the sessions it found. It updates on
                    its own as you work; re-index if something seems missing.
                  </p>
                  {status && (
                    <p className="settings-hint">
                      {sessionCount.toLocaleString()} sessions from {toolChips.length} tool
                      {toolChips.length === 1 ? "" : "s"} · {status.embeddings.toLocaleString()}{" "}
                      embeddings · search model: {status.embedder}
                    </p>
                  )}
                  {toolChips.length > 0 && (
                    <div className="tool-chips" aria-label="Indexed tools">
                      {toolChips.map(([tool, sessions, skills]) => (
                        <span
                          key={tool}
                          className="tool-chip"
                          title={`${sessions} session${sessions === 1 ? "" : "s"}${
                            skills > 0 ? ` and ${skills} skill${skills === 1 ? "" : "s"}` : ""
                          } from ${toolLabel(tool)}`}
                        >
                          <span className="tool-chip-icon">{toolIcon(tool)}</span>
                          {toolLabel(tool)}
                          <span className="tool-chip-count">{sessions.toLocaleString()}</span>
                        </span>
                      ))}
                    </div>
                  )}
                  <button onClick={runReindex} disabled={reindexing}>
                    {reindexing ? "Re-indexing…" : "Re-index now"}
                  </button>
                </section>
              </div>
            )}

            {settingsTab === "help" && (
              <div className="settings-body">
                <section className="settings-section">
                  <h3>Help and documentation</h3>
                  <p className="settings-hint">
                    Guides, references and troubleshooting. Links open the docs on GitHub.
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
              </div>
            )}

            {settingsTab === "phone" && (
              <div className="settings-body">
                <PhoneSettings onError={reportError} />
              </div>
            )}

            {settingsTab === "models" && (
              <div className="settings-body">
                <section className="settings-section">
                  <h3>Models</h3>
                  <p className="settings-hint">
                    Optional AI features (e.g. naming themes) reuse your existing
                    Claude Code / Codex login when possible. Or paste an API key
                    below to bring your own — stored in your OS keychain, never
                    sent anywhere but the provider.
                  </p>
                  {!llmConfig && <p className="doc-desc">Loading…</p>}
                  {llmConfig?.providers.map((p) => (
                    <div className="settings-section" key={p.id}>
                      <label className="fed-row">
                        <span className="fed-label">{p.label}</span>
                        <span className="fed-token-state">
                          {p.methods.length > 0
                            ? `available · ${p.methods.join(", ")}`
                            : "no credentials found"}
                        </span>
                      </label>
                      <div className="fed-row">
                        <span className="fed-label">key</span>
                        <span className="fed-token-state">
                          {p.has_stored_key ? "set · in keychain" : "not set"}
                        </span>
                        <input
                          type="password"
                          placeholder={`paste a ${p.label.split(" ")[0]} API key`}
                          value={keyInputs[p.id] ?? ""}
                          onChange={(e) =>
                            setKeyInputs((m) => ({ ...m, [p.id]: e.target.value }))
                          }
                        />
                        <button
                          className="secondary"
                          disabled={!(keyInputs[p.id] ?? "").trim()}
                          onClick={async () => {
                            setLlmConfig(await setLlmKey(p.id, (keyInputs[p.id] ?? "").trim()));
                            setKeyInputs((m) => ({ ...m, [p.id]: "" }));
                          }}
                        >
                          Save
                        </button>
                        {p.has_stored_key && (
                          <button
                            className="linklike"
                            onClick={async () => setLlmConfig(await setLlmKey(p.id, ""))}
                          >
                            clear
                          </button>
                        )}
                      </div>
                    </div>
                  ))}
                  {llmConfig && (
                    <div className="fed-row">
                      <span className="fed-label">use</span>
                      <select
                        value={llmConfig.active ?? ""}
                        onChange={async (e) =>
                          setLlmConfig(await setLlmProvider(e.target.value))
                        }
                      >
                        <option value="">Auto (first available)</option>
                        {llmConfig.providers.map((p) => (
                          <option key={p.id} value={p.id}>
                            {p.label}
                          </option>
                        ))}
                      </select>
                      <span className="doc-desc">
                        Which provider AI features prefer.
                      </span>
                    </div>
                  )}
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
                        Turn on <strong>Can message</strong> on both devices to let their
                        agents talk to each other in Rooms.
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
                              <label
                                className="device-grant"
                                title="Agents here and on this device can talk in Rooms. Turn it on on both devices."
                              >
                                <input
                                  type="checkbox"
                                  checked={!!d.rooms}
                                  onChange={(e) => toggleRooms(d.name, e.target.checked)}
                                />
                                Can message
                              </label>
                            )}
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

                {federation && fedConfig && (
                  <>
                    <section className="settings-section">
                      <h3>This device</h3>
                      <label className="fed-row">
                        <span className="fed-label">name</span>
                        <input
                          defaultValue={fedConfig.device}
                          key={`name-${fedConfig.device}`}
                          onBlur={(e) => {
                            const v = e.target.value.trim();
                            if (v && v !== fedConfig.device) saveFedConfig({ device: v });
                          }}
                        />
                      </label>
                      <div className="fed-row">
                        <span className="fed-label">token</span>
                        <span className="fed-token-state">
                          {fedConfig.has_token
                            ? `set${fedConfig.token_in_keyring ? " · in keychain" : " · in config file"}`
                            : "not set"}
                        </span>
                        <input
                          type="password"
                          placeholder="new shared token"
                          value={tokenInput}
                          onChange={(e) => setTokenInput(e.target.value)}
                        />
                        <button
                          className="secondary"
                          disabled={!tokenInput.trim()}
                          onClick={() => {
                            saveFedConfig({ token: tokenInput.trim() });
                            setTokenInput("");
                          }}
                        >
                          Set
                        </button>
                        <button
                          className="linklike"
                          title="Make a strong random token. Other devices then join with this device's pairing code."
                          onClick={() => {
                            const bytes = crypto.getRandomValues(new Uint8Array(24));
                            const token = btoa(String.fromCharCode(...bytes))
                              .replace(/\+/g, "-")
                              .replace(/\//g, "_")
                              .replace(/=+$/, "");
                            saveFedConfig({ token });
                          }}
                        >
                          generate
                        </button>
                        {fedConfig.has_token && (
                          <button className="linklike" onClick={() => saveFedConfig({ token: "" })}>
                            clear
                          </button>
                        )}
                      </div>
                      <div className="fed-row">
                        <span className="fed-label">pairing</span>
                        {fedConfig.pairing_code ? (
                          <div className="fed-pairing">
                            <code className="fed-pairing-code">
                              {showPairing
                                ? fedConfig.pairing_code
                                : "•".repeat(28)}
                            </code>
                            <button
                              className="linklike"
                              onClick={() => setShowPairing((s) => !s)}
                            >
                              {showPairing ? "Hide" : "Show"}
                            </button>
                            <button
                              className="secondary"
                              title="Copy this device's code; paste it on another device's “Add a device by code”"
                              onClick={() =>
                                navigator.clipboard?.writeText(fedConfig.pairing_code!)
                              }
                            >
                              Copy
                            </button>
                            <span className="doc-desc">
                              This device's code — paste it on another device to join them.
                              Treat it like a password.
                            </span>
                          </div>
                        ) : (
                          <span className="doc-desc">
                            Set a token (above) and run with Tailscale connected — the
                            launcher binds to your tailnet automatically, and this
                            device's pairing code appears here.
                          </span>
                        )}
                      </div>
                    </section>

                    <section className="settings-section">
                      <h3>Don't share with peers</h3>
                      <p className="settings-hint">
                        Tools or projects this device won't serve to your other devices.
                        Comma-separated.
                      </p>
                      <label className="fed-row">
                        <span className="fed-label">tools</span>
                        <input
                          defaultValue={fedConfig.exclude_tools.join(", ")}
                          key={`et-${fedConfig.exclude_tools.join(",")}`}
                          placeholder="e.g. cursor, codex"
                          onBlur={(e) =>
                            saveFedConfig({ exclude_tools: splitCsv(e.target.value) })
                          }
                        />
                      </label>
                      <label className="fed-row">
                        <span className="fed-label">projects</span>
                        <input
                          defaultValue={fedConfig.exclude_projects.join(", ")}
                          key={`ep-${fedConfig.exclude_projects.join(",")}`}
                          placeholder="path substrings"
                          onBlur={(e) =>
                            saveFedConfig({ exclude_projects: splitCsv(e.target.value) })
                          }
                        />
                      </label>
                    </section>

                    <section className="settings-section">
                      <h3>Add a device by code</h3>
                      <p className="settings-hint">
                        Paste a pairing code from another device to adopt its token and add it.
                      </p>
                      <div className="fed-row">
                        <input
                          placeholder="pairing code…"
                          value={pairCode}
                          onChange={(e) => setPairCode(e.target.value)}
                        />
                        <button
                          className="secondary"
                          disabled={!pairCode.trim()}
                          onClick={submitPairCode}
                        >
                          Add device
                        </button>
                      </div>
                    </section>
                  </>
                )}
              </div>
            )}
        </section>
      )}
      </main>
    </div>
  );
}

// Split a comma-separated input into trimmed, non-empty values.
function splitCsv(s: string): string[] {
  return s
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);
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

// The daemon wraps matched terms in STX…ETX control chars (sentinels that can't
// occur in content, unlike literal [brackets]); render those as <mark>.
function highlight(snippet: string): string {
  const escaped = snippet
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
  return escaped.replace(/([^]*)/g, "<mark>$1</mark>");
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
