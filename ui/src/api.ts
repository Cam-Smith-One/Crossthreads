// Thin client for the daemon's HTTP/JSON bridge (POST /api/rpc).

export type Mode = "lexical" | "semantic" | "hybrid";

export interface Filters {
  tool?: string;
  kind?: string;
  project?: string;
  since?: string;
  until?: string;
  tag?: string;
}

export interface Hit {
  conversation_id: string;
  tool: string;
  kind?: string;
  project?: string | null;
  title?: string | null;
  started_at?: string | null;
  snippet: string;
  score: number;
  source_path: string;
  bookmarked?: boolean;
  pinned?: boolean;
  tags?: string[];
  device?: string | null;
}

export interface Device {
  name: string;
  addr?: string | null;
  online: boolean;
  local: boolean;
  /** Room messages are exchanged with this device ("can message"). */
  rooms?: boolean;
}

export interface DiscoveredDevice {
  name: string;
  addr: string;
  already_approved: boolean;
}

export interface Status {
  conversations: number;
  embeddings: number;
  embedder: string;
  /** Detected tools with indexed counts: [tool, threads, skills], busiest first. */
  tools?: [string, number, number][];
}

export interface ContextBlock {
  markdown: string;
  sources: string[];
  token_estimate: number;
}

export interface StoredMessage {
  role: string;
  content: string;
}

export interface StoredConversation {
  id: string;
  tool: string;
  /** thread (a session) | skill | exchange (a room conversation). */
  kind?: string;
  project?: string | null;
  title?: string | null;
  started_at?: string | null;
  source_path?: string;
  bookmarked?: boolean;
  pinned?: boolean;
  note?: string;
  tags?: string[];
  messages: StoredMessage[];
}

/** A phone that hasn't been paired yet asked for something. */
export class UnpairedError extends Error {
  constructor() {
    super("pair this phone first");
  }
}

/** This browser hasn't been signed in on this computer yet. */
export class SignInRequiredError extends Error {
  constructor() {
    super("sign in first: run `crossthreads open`");
  }
}

/** On this computer: trade a one-time sign-in ticket for a session cookie. */
export async function redeemSignInTicket(ticket: string): Promise<void> {
  const res = await fetch("/api/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ ticket }),
  });
  if (!res.ok) {
    const err = await res.json().catch(() => null);
    throw new Error(err?.message ?? `sign-in failed (HTTP ${res.status})`);
  }
}

/** Who is using the app: this computer, or a paired phone. */
export interface Session {
  phone: boolean;
  name?: string | null;
  /** May post to rooms. Always true on the computer itself. */
  can_post: boolean;
}

export async function getSession(): Promise<Session> {
  return rpc<Session>({ op: "session" });
}

export interface PhoneInfo {
  id: string;
  name: string;
  created: number;
  last_seen: number;
  can_post: boolean;
}

export interface PhoneStatus {
  enabled: boolean;
  listening: boolean;
  url?: string | null;
  error?: string | null;
  phones: PhoneInfo[];
}

export interface PhonePairing {
  code: string;
  url: string;
  qr_svg?: string | null;
  expires_secs: number;
}

/** What's installed and turned on here, for the Get started checklist. */
export interface SetupStatus {
  hooks_claude: boolean;
  hooks_codex: boolean;
  mcp_claude: boolean;
  mcp_codex: boolean;
  skill_claude: boolean;
  skill_codex: boolean;
  model: boolean;
  devices: number;
  devices_can_message: number;
  /** Agents get the thread's context when a session starts. */
  agent_context?: boolean;
  /** Grok Build is on this computer, and connected. */
  grok_present?: boolean;
  mcp_grok?: boolean;
  skill_grok?: boolean;
  /** Your model writes where threads stand. */
  auto_standing?: boolean;
  phone_enabled: boolean;
  phones: number;
  claude_present: boolean;
  codex_present: boolean;
  /** When a client without the local key last tried to connect (epoch
   * seconds): an older ct-mcp or CLI is still in use. */
  keyless_client_at?: number | null;
}

export async function getSetupStatus(): Promise<SetupStatus> {
  const data = await rpc<{ setup: SetupStatus }>({ op: "setup_status" });
  return data.setup;
}

export const phoneStatus = () => rpc<PhoneStatus>({ op: "phone_status" });
export const setPhoneEnabled = (enabled: boolean) =>
  rpc<PhoneStatus>({ op: "phone_set_enabled", enabled });
export const pairPhoneStart = () => rpc<PhonePairing>({ op: "phone_pair" });
export const revokePhone = (id: string) => rpc<PhoneStatus>({ op: "phone_revoke", id });
export const setPhonePost = (id: string, can_post: boolean) =>
  rpc<PhoneStatus>({ op: "phone_set_post", id, can_post });

/** On the phone: trade a pairing code for this browser's session cookie. */
export async function redeemPairingCode(code: string, name: string): Promise<void> {
  const res = await fetch("/api/pair", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code, name }),
  });
  if (!res.ok) {
    const err = await res.json().catch(() => null);
    throw new Error(err?.message ?? `pairing failed (HTTP ${res.status})`);
  }
}

async function rpc<T>(body: unknown): Promise<T> {
  let data: any;
  // In the native Tauri shell, forward through the `rpc` command (which talks
  // to the daemon over the local protocol). In a browser, POST to the HTTP
  // bridge. Same UI, both surfaces.
  const tauri = (window as unknown as { __TAURI__?: any }).__TAURI__;
  if (tauri?.core?.invoke) {
    const text: string = await tauri.core.invoke("rpc", { request: JSON.stringify(body) });
    data = JSON.parse(text);
  } else {
    const res = await fetch("/api/rpc", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!res.ok) {
      // The phone listener explains a refusal in JSON; keep its words.
      const err = await res.json().catch(() => null);
      if (res.status === 401 && err?.unpaired) throw new UnpairedError();
      if (res.status === 401 && err?.login) throw new SignInRequiredError();
      if (err?.message) throw new Error(err.message);
      throw new Error(`daemon unreachable (HTTP ${res.status} ${res.statusText})`);
    }
    try {
      data = await res.json();
    } catch {
      throw new Error("daemon returned a malformed (non-JSON) response");
    }
  }
  if (data && data.type === "error") {
    // An older daemon answers newer requests with a serde "unknown variant"
    // error; say what that means instead of showing it raw.
    if (/unknown variant `/.test(data.message ?? "")) {
      throw new Error(
        "The Crossthreads service running on this computer is older than this app. Quit Crossthreads, run `pkill crossthreadsd` in Terminal, and open Crossthreads again. If it comes back, update the terminal install too (see Help).",
      );
    }
    throw new Error(data.message ?? "daemon error");
  }
  return data as T;
}

// Drop empty filter fields so the daemon's #[serde(default)] applies.
function clean(f: Filters): Filters {
  const out: Filters = {};
  if (f.tool) out.tool = f.tool;
  if (f.kind) out.kind = f.kind;
  if (f.project) out.project = f.project;
  if (f.since) out.since = f.since;
  if (f.until) out.until = f.until;
  if (f.tag) out.tag = f.tag;
  return out;
}

export function getStatus(): Promise<Status> {
  return rpc<Status>({ op: "status" });
}

export interface Reindexed {
  inserted: number;
  duplicate: number;
  embedded: number;
}

export function reindex(): Promise<Reindexed> {
  return rpc<Reindexed>({ op: "reindex" });
}

export async function getFacets(): Promise<{ tools: string[]; tags: string[] }> {
  const data = await rpc<{ tools: string[]; tags?: string[] }>({ op: "facets" });
  return { tools: data.tools ?? [], tags: data.tags ?? [] };
}

export async function setNote(id: string, note: string): Promise<boolean> {
  const data = await rpc<{ ok: boolean }>({ op: "set_note", id, note });
  return data.ok;
}

export async function setTags(id: string, tags: string[]): Promise<boolean> {
  const data = await rpc<{ ok: boolean }>({ op: "set_tags", id, tags });
  return data.ok;
}

export async function search(
  query: string,
  mode: Mode,
  filters: Filters,
  limit = 25,
  devices?: string[] | null,
): Promise<{ hits: Hit[]; unreachable: string[] }> {
  const data = await rpc<{ hits: Hit[]; unreachable?: string[] }>({
    op: "search",
    query,
    mode,
    limit,
    filters: clean(filters),
    ...(devices && devices.length ? { devices } : {}),
  });
  return { hits: data.hits, unreachable: data.unreachable ?? [] };
}

export interface FederationConfig {
  device: string;
  has_token: boolean;
  token_in_keyring: boolean;
  listen_addr?: string | null;
  exclude_tools: string[];
  exclude_projects: string[];
  pairing_code?: string | null;
  /** The relay saved for this device, and whether it's in use now. */
  relay?: string | null;
  relay_active?: boolean;
}

export function getFederationConfig(): Promise<FederationConfig> {
  return rpc<FederationConfig>({ op: "get_federation_config" });
}

export function setFederationConfig(patch: {
  device?: string;
  token?: string;
  exclude_tools?: string[];
  exclude_projects?: string[];
  relay?: string;
}): Promise<FederationConfig> {
  return rpc<FederationConfig>({ op: "set_federation_config", ...patch });
}

// --- Models (LLM providers) ---------------------------------------------

export interface ProviderStatus {
  id: string;
  label: string;
  /** Resolved credential methods, most-preferred first (secret-free). */
  methods: string[];
  has_stored_key: boolean;
}

export interface LlmConfig {
  providers: ProviderStatus[];
  /** Pinned active provider id, if any. */
  active?: string | null;
}

export function getLlmConfig(): Promise<LlmConfig> {
  return rpc<LlmConfig>({ op: "get_llm_config" });
}

// --- Themes (clusters) --------------------------------------------------

export interface ThemeSample {
  id: string;
  title?: string | null;
  tool: string;
}

export interface Theme {
  label: string;
  size: number;
  tools: [string, number][];
  samples: ThemeSample[];
  conversation_ids: string[];
}

export async function getThemes(k?: number, name = false): Promise<Theme[]> {
  const data = await rpc<{ themes: Theme[] }>({ op: "themes", k, name });
  return data.themes;
}

// --- Insights (LLM synthesis over recent history) -----------------------

export type InsightKind =
  | "open_loops"
  | "knowledge_cards"
  | "decision_log"
  | "recurrence"
  | "work_dna"
  | "gaps"
  | "prompt_coach"
  | "process_miner";

export interface Insight {
  markdown: string;
  sources: string[];
}

export function getInsight(kind: InsightKind, limit?: number): Promise<Insight> {
  return rpc<Insight>({ op: "insight", kind, limit });
}

// --- Ask (RAG over your history) ----------------------------------------

export interface Answer {
  markdown: string;
  sources: string[];
  llm_used: boolean;
}

export function ask(
  question: string,
  filters: Filters = {},
  limit = 12,
  devices?: string[] | null,
): Promise<Answer> {
  return rpc<Answer>({
    op: "ask",
    question,
    limit,
    filters: clean(filters),
    ...(devices && devices.length ? { devices } : {}),
  });
}

// --- Activity (temporal view) -------------------------------------------

export interface ActivityBucket {
  period: string;
  total: number;
  by_tool: [string, number][];
}

export async function getActivity(
  bucket: "day" | "week" = "day",
  filters: Filters = {},
): Promise<{ bucket: string; periods: ActivityBucket[] }> {
  const data = await rpc<{ bucket: string; periods: ActivityBucket[] }>({
    op: "activity",
    bucket,
    filters: clean(filters),
  });
  return { bucket: data.bucket, periods: data.periods ?? [] };
}

// --- Behavioral metrics ("how you work") --------------------------------

export interface WorkMetrics {
  sessions: number;
  by_tool: [string, number][];
  projects: number;
  median_user_turns: number;
  mean_user_turns: number;
  correction_rate: number;
  first_prompt_miss_rate: number;
  abandonment_rate: number;
  avg_first_prompt_chars: number;
  specificity_rate: number;
  code_paste_rate: number;
  error_paste_rate: number;
  test_mention_rate: number;
  commit_mention_rate: number;
  by_hour: [number, number][];
  high_friction_projects: [string, number][];
  avg_tool_actions: number;
  top_actions: [string, number][];
  top_languages: [string, number][];
  by_model: [string, number][];
  median_session_minutes: number;
}

export async function getMetrics(filters: Filters = {}): Promise<WorkMetrics | null> {
  const data = await rpc<{ metrics: WorkMetrics }>({ op: "metrics", filters: clean(filters) });
  return data.metrics ?? null;
}

// --- Weekly review (proactive) ------------------------------------------

export interface WeeklyReview {
  markdown: string;
  generated_at: string;
  period_start: string;
  llm_used: boolean;
  fresh: boolean;
}

export function getWeeklyReview(force = false): Promise<WeeklyReview> {
  return rpc<WeeklyReview>({ op: "weekly_review", force });
}

// --- Optimize / trends ("optimize how you work") ------------------------

export interface OptimizeReport {
  markdown: string;
  has_active: boolean;
}

export function getOptimize(force = false): Promise<OptimizeReport> {
  return rpc<OptimizeReport>({ op: "optimize", force });
}

export interface TrendRow {
  label: string;
  current: number;
  previous: number;
  delta: number;
  improved: boolean;
  is_pct: boolean;
}

export async function getTrends(days?: number): Promise<TrendRow[]> {
  const data = await rpc<{ rows: TrendRow[] }>({ op: "trends", days });
  return data.rows ?? [];
}

// --- Fluency (the 4D AI-fluency view + reflection) ----------------------

export interface FluencyDimension {
  key: string;
  label: string;
  blurb: string;
  score: number;
  delta: number | null;
  read: string;
  components: [string, string][];
}

export interface FluencyReflection {
  dimension: string;
  prompt: string;
}

export interface FluencyReport {
  sessions: number;
  window_days: number;
  overall: number;
  dimensions: FluencyDimension[];
  reflection: FluencyReflection | null;
  markdown: string;
}

export async function getFluency(windowDays?: number): Promise<FluencyReport> {
  const data = await rpc<{ report: FluencyReport }>({
    op: "fluency",
    window_days: windowDays,
  });
  return data.report;
}

// --- Glance (the menu-bar tray popover: usage + insight) ----------------

export interface QuotaWindow {
  used: number;
  limit: number;
  resets_at: string;
  source: string;
}

export interface ProviderUsage {
  tool: string;
  today_sessions: number;
  week_sessions: number;
  avg_daily_sessions: number;
  quota: QuotaWindow | null;
}

export interface Glance {
  generated_at: string;
  today_sessions: number;
  today_projects: number;
  week_sessions: number;
  providers: ProviderUsage[];
  fluency_overall: number;
  fluency_weakest: [string, number] | null;
  unresolved: number;
  focus: string | null;
}

export async function getGlance(): Promise<Glance> {
  const data = await rpc<{ glance: Glance }>({ op: "glance" });
  return data.glance;
}

/** True when running inside the native Tauri desktop shell. */
export function isDesktop(): boolean {
  return Boolean((window as unknown as { __TAURI__?: unknown }).__TAURI__);
}

/** From the tray popover, bring up the full Crossthreads window. In the browser
 * this just navigates to the full app (dropping the `?view=glance` param). */
export async function openMainWindow(): Promise<void> {
  const tauri = (window as unknown as { __TAURI__?: any }).__TAURI__;
  if (tauri?.core?.invoke) {
    await tauri.core.invoke("open_main");
  } else {
    window.location.href = "/";
  }
}

/** Store (non-empty) or clear (empty) a provider's BYO key. */
export function setLlmKey(provider: string, key: string): Promise<LlmConfig> {
  return rpc<LlmConfig>({ op: "set_llm_key", provider, key });
}

/** Pin the active provider (empty clears the pin). */
export function setLlmProvider(provider: string): Promise<LlmConfig> {
  return rpc<LlmConfig>({ op: "set_llm_provider", provider });
}

export async function pairWithCode(code: string): Promise<Device[]> {
  const data = await rpc<{ devices: Device[] }>({ op: "pair_with_code", code });
  return data.devices ?? [];
}

export async function getDevices(): Promise<{ devices: Device[]; federation: boolean }> {
  const data = await rpc<{ devices: Device[]; federation: boolean }>({ op: "devices" });
  return { devices: data.devices ?? [], federation: !!data.federation };
}

export async function discoverDevices(): Promise<DiscoveredDevice[]> {
  const data = await rpc<{ devices: DiscoveredDevice[] }>({ op: "discover_devices" });
  return data.devices ?? [];
}

export async function approvePeer(name: string, addr: string): Promise<Device[]> {
  const data = await rpc<{ devices: Device[] }>({ op: "approve_peer", name, addr });
  return data.devices ?? [];
}

export async function setPeerRooms(name: string, rooms: boolean): Promise<Device[]> {
  const data = await rpc<{ devices: Device[] }>({ op: "set_peer_rooms", name, rooms });
  return data.devices ?? [];
}

export async function removePeer(name: string): Promise<Device[]> {
  const data = await rpc<{ devices: Device[] }>({ op: "remove_peer", name });
  return data.devices ?? [];
}

export async function getConversation(
  id: string,
  device?: string | null,
): Promise<StoredConversation | null> {
  const data = await rpc<{ conversation: StoredConversation | null }>({
    op: "get_conversation",
    id,
    ...(device ? { device } : {}),
  });
  return data.conversation;
}

export async function setFlags(
  id: string,
  flags: { bookmarked?: boolean; pinned?: boolean },
): Promise<boolean> {
  const data = await rpc<{ ok: boolean }>({ op: "set_flags", id, ...flags });
  return data.ok;
}

export async function getSaved(): Promise<Hit[]> {
  const data = await rpc<{ hits: Hit[] }>({ op: "saved" });
  return data.hits;
}

// --- Resume (how to pick a session back up in its native tool) -----------

/** A runnable resume command, or a reveal-the-transcript fallback. Mirrors the
 * daemon `resume` op — the single source of truth for every surface. */
export type ResumeAction =
  | { kind: "resume"; program: string; args: string[]; cwd: string | null; display: string }
  | { kind: "reveal"; path: string };

export interface ResumePlan {
  tool: string;
  action: ResumeAction;
  source_path: string;
}

export async function getResume(id: string): Promise<ResumePlan | null> {
  const data = await rpc<{ resume: ResumePlan | null }>({ op: "resume", id });
  return data.resume;
}

export async function openSource(id: string): Promise<boolean> {
  const data = await rpc<{ ok: boolean }>({ op: "open_source", id });
  return data.ok;
}

export async function forget(id: string): Promise<boolean> {
  const data = await rpc<{ ok: boolean }>({ op: "forget", id });
  return data.ok;
}

export function buildContext(
  query: string,
  mode: Mode,
  filters: Filters,
  limit = 3,
  max_chars = 6000,
  devices?: string[] | null,
): Promise<ContextBlock> {
  return rpc<ContextBlock>({
    op: "context",
    query,
    mode,
    limit,
    max_chars,
    filters: clean(filters),
    ...(devices && devices.length ? { devices } : {}),
  });
}

// --- Threads (pieces of work: sessions linked across tools) ---------------

export type ThreadStatus = "active" | "parked" | "done";

/** What a session is doing right now. `inferred` = guessed from the transcript
 * file rather than reported by a hook, so it can't tell "needs you" from idle. */
export interface Live {
  state: "running" | "needs_you" | "listening" | "idle" | "ended";
  inferred: boolean;
  since?: string | null;
}

/** One session: a conversation in one tool, as its latest indexed version. */
export interface SessionRef {
  conversation_id: string;
  session_key: string;
  tool: string;
  project?: string | null;
  repo_key?: string | null;
  title?: string | null;
  started_at?: string | null;
  last_activity?: string | null;
  branch?: string | null;
  message_count: number;
  live?: Live | null;
  /** The device this session ran on, when the thread spans devices. */
  device?: string | null;
}

/** A thread on a particular device. */
export interface PartRef {
  device: string;
  id: string;
  title: string;
}

export type NoteKind = "decision" | "todo" | "done" | "context";

/** A note kept on a thread by you or an agent; shared with your devices. */
export interface ThreadNote {
  id: string;
  thread_id: string;
  kind: NoteKind;
  text: string;
  author: string;
  device: string;
  /** Epoch milliseconds. */
  created: number;
  removed: boolean;
  updated: number;
}

export interface ThreadSummary {
  id: string;
  title: string;
  status: ThreadStatus;
  repo_key?: string | null;
  repo_label?: string | null;
  tools: string[];
  branches: string[];
  session_count: number;
  message_count: number;
  first_activity?: string | null;
  last_activity?: string | null;
  pending_suggestions: number;
  /** The most pressing state among the thread's sessions. */
  live?: Live | null;
  /** The device the thread is on, when the list spans devices. */
  device?: string | null;
  /** Every device with part of this thread, when it spans more than one. */
  devices?: string[];
  /** Threads on other devices that look like the same work. */
  related?: PartRef[];
  /** Where the work stands. */
  standing?: Standing | null;
  /** Sent to a device to continue there, and in which tool. */
  moved_to?: string | null;
  moved_tool?: string | null;
}

/** Where a thread stands: read from its latest session, or by your model. */
export interface Standing {
  summary: string;
  next?: string | null;
  blocked?: string | null;
  last_ask?: string | null;
  by_model?: boolean;
  upto?: string | null;
}

export interface ThreadLink {
  from: string;
  to: string;
  kind: string;
  /** explicit | detected | manual */
  origin: string;
  confidence: number;
}

/** A link the detector proposed but did not apply. */
export interface ThreadSuggestion {
  /** The session in this thread. */
  with: string;
  /** The session elsewhere that may belong here. */
  session: SessionRef;
  thread_id: string;
  kind: string;
  confidence: number;
}

export interface ThreadDetail extends ThreadSummary {
  sessions: SessionRef[];
  links: ThreadLink[];
  suggestions: ThreadSuggestion[];
  notes?: ThreadNote[];
  /** This thread's parts on other devices. */
  parts?: PartRef[];
}

/** Threads on this device and, with federation set up, on every reachable
 * peer. `unreachable` names approved devices that didn't answer. */
export async function getThreads(): Promise<{ threads: ThreadSummary[]; unreachable: string[] }> {
  const data = await rpc<{ threads: ThreadSummary[]; unreachable?: string[] }>({
    op: "threads",
    filters: {},
  });
  return { threads: data.threads ?? [], unreachable: data.unreachable ?? [] };
}

/** `id` is a thread id or the conversation id of any session in it. `device`
 * fetches it from the peer it lives on. */
export async function getThread(id: string, device?: string | null): Promise<ThreadDetail | null> {
  const data = await rpc<{ thread: ThreadDetail | null }>({
    op: "thread",
    id,
    ...(device ? { device } : {}),
  });
  return data.thread;
}

/** Put two sessions in one thread (also accepts a suggestion). */
export async function linkSessions(a: string, b: string, kind?: string): Promise<string | null> {
  const data = await rpc<{ id: string | null }>({
    op: "link_sessions",
    a,
    b,
    ...(kind ? { kind } : {}),
  });
  return data.id;
}

export async function unlinkSession(id: string): Promise<boolean> {
  const data = await rpc<{ ok: boolean }>({ op: "unlink_session", id });
  return data.ok;
}

/** Dismiss a suggested link for good. */
export async function rejectLink(a: string, b: string): Promise<boolean> {
  const data = await rpc<{ ok: boolean }>({ op: "reject_link", a, b });
  return data.ok;
}

/** Rename a thread or set its status, on whichever device it lives; the
 * change reaches your other devices too. */
export async function setThread(
  id: string,
  change: { title?: string; status?: ThreadStatus },
  device?: string | null,
): Promise<string | null> {
  const data = await rpc<{ id: string | null }>({
    op: "set_thread",
    id,
    ...change,
    ...(device ? { device } : {}),
  });
  return data.id;
}

/** Make thread `part` (on `device`) part of thread `into` (on `intoDevice`). */
export async function joinThread(
  part: string,
  device: string | null | undefined,
  into: string,
  intoDevice: string | null | undefined,
): Promise<string | null> {
  const data = await rpc<{ id: string | null }>({
    op: "thread_join",
    part,
    into,
    ...(device ? { device } : {}),
    ...(intoDevice ? { into_device: intoDevice } : {}),
  });
  return data.id;
}

/** Undo a join. */
export const splitThread = (part: string, device?: string | null) =>
  rpc<{ ok: boolean }>({ op: "thread_split", part, ...(device ? { device } : {}) });

export async function addNote(
  thread: string,
  device: string | null | undefined,
  kind: NoteKind,
  text: string,
): Promise<ThreadNote> {
  const data = await rpc<{ note: ThreadNote }>({
    op: "thread_note_add",
    thread,
    kind,
    text,
    ...(device ? { device } : {}),
  });
  return data.note;
}

export async function editNote(id: string, change: { kind?: NoteKind; removed?: boolean }) {
  const data = await rpc<{ note: ThreadNote }>({ op: "thread_note_edit", id, ...change });
  return data.note;
}

// --- Starting, continuing and moving work ------------------------------------

export type ToolId = "claude-code" | "codex" | "grok-build";

/** Start a thread: a name, an optional goal, and its folder on this computer. */
export async function createThread(title: string, cwd: string, goal?: string): Promise<string | null> {
  const data = await rpc<{ id: string | null }>({ op: "thread_create", title, cwd, ...(goal ? { goal } : {}) });
  return data.id;
}

/** Open a terminal here running `tool`, continuing the thread. */
export async function continueThread(
  id: string,
  device: string | null | undefined,
  tool: ToolId,
  fresh = false,
): Promise<string> {
  const data = await rpc<{ text: string }>({
    op: "thread_continue",
    id,
    tool,
    fresh,
    ...(device ? { device } : {}),
  });
  return data.text;
}

/** Send a thread to another device to continue there (null `to` clears it). */
export const moveThread = (
  id: string,
  device: string | null | undefined,
  to: string | null,
  tool?: ToolId,
) =>
  rpc<{ id: string | null }>({
    op: "thread_move",
    id,
    to,
    ...(tool ? { tool } : {}),
    ...(device ? { device } : {}),
  });

/** Have your model write where a thread stands now (this device's threads). */
export async function summarizeThread(id: string): Promise<Standing> {
  const data = await rpc<{ standing: Standing }>({ op: "thread_summarize", id });
  return data.standing;
}

export async function recentFolders(): Promise<string[]> {
  const data = await rpc<{ folders: string[] }>({ op: "recent_folders" });
  return data.folders ?? [];
}

export const setAutoStanding = (on: boolean) => rpc<{ ok: boolean }>({ op: "set_auto_standing", on });

export const setAgentContext = (on: boolean) => rpc<{ ok: boolean }>({ op: "set_agent_context", on });

// --- Adding a device (a short code) ------------------------------------------

export interface DevicePairCode {
  code: string;
  expires_in: number;
  device: string;
  addr?: string | null;
  problem?: string | null;
}

export const devicePairStart = () => rpc<DevicePairCode>({ op: "device_pair_start" });
export const devicePairCancel = () => rpc<{ ok: boolean }>({ op: "device_pair_cancel" });
export const devicePairJoin = (code: string, addr?: string) =>
  rpc<{ name: string; note?: string | null }>({
    op: "device_pair_join",
    code,
    ...(addr ? { addr } : {}),
  });

// --- Rooms (agents and you, talking in one place) -------------------------

export interface RoomAgent {
  name: string;
  /** Epoch seconds of its last call. */
  seen: number;
  /** Blocked waiting for a message right now. */
  listening: boolean;
}

export interface RoomClaim {
  path: string;
  holder: string;
  room: string;
  /** Epoch seconds. */
  expires: number;
}

export interface RoomView {
  /** Room id: a repository key, or a plain name. */
  room: string;
  /** Short display name. */
  label: string;
  messages: number;
  /** Agent messages in the current exchange. */
  turns: number;
  /** Turn cap, or 0 for none. */
  max_turns: number;
  last_ts: number;
  agents: RoomAgent[];
  /** The latest agent message closed the exchange with DONE. */
  closed: boolean;
  claims: RoomClaim[];
  /** The room's task board: one agent leads, others take tasks. */
  tasks?: RoomTask[];
  /** Tasks with a request to start an agent waiting on this computer. */
  asks_here?: string[];
}

export type TaskStatus = "todo" | "doing" | "blocked" | "done" | "cancelled";

export interface RoomTask {
  id: string;
  room: string;
  title: string;
  brief: string;
  /** An agent name, or name@device; empty when unassigned. */
  owner: string;
  status: TaskStatus;
  result: string;
  files: string[];
  /** The agent that planned it. */
  created_by: string;
  device: string;
  /** Epoch milliseconds. */
  created: number;
  updated: number;
  /** The task's own git branch (ct/<id>), when it has one. */
  branch?: string;
  /** Its worktree folder (on worktree_device), or empty once removed. */
  worktree?: string;
  worktree_device?: string;
  /** A request to start an agent for it: tool, device, and where it stands. */
  launch_tool?: string;
  launch_device?: string;
  launch_state?: "" | "requested" | "started" | "declined";
}

/**
 * Answer a request to start an agent on this computer. Allowing opens the
 * tool in a terminal with the task; nothing starts without this.
 */
export const answerAgentRequest = (id: string, allow: boolean) =>
  rpc<{ text: string }>({ op: "room_launch", id, allow });

/** Change a task on a room's board, as you. The lead is told. */
export const updateRoomTask = (
  room: string,
  id: string,
  change: { status?: TaskStatus; owner?: string; result?: string },
) => rpc<{ text: string }>({ op: "room_task", me: "you", room, id, ...change });

export interface RoomMessage {
  id: number;
  /** Epoch seconds. */
  ts: number;
  room: string;
  author: string;
  to?: string | null;
  text: string;
  human: boolean;
  /** The device it was posted on; absent for this device. */
  device?: string | null;
}

export async function getRooms(): Promise<RoomView[]> {
  const data = await rpc<{ rooms: RoomView[] }>({ op: "rooms" });
  return data.rooms ?? [];
}

/** Messages in a room after id `after`, oldest first. */
export async function getRoomMessages(room: string, after = 0): Promise<RoomMessage[]> {
  const data = await rpc<{ messages: RoomMessage[] }>({
    op: "room_messages",
    room,
    after,
    limit: 300,
  });
  return data.messages ?? [];
}

/** Post as the human. Agents treat these as instructions that override theirs. */
export async function postToRoom(room: string, text: string, to?: string): Promise<number> {
  const data = await rpc<{ id: number }>({
    op: "room_post",
    room,
    text,
    ...(to ? { to } : {}),
  });
  return data.id;
}

/** A paste-ready brief for continuing a thread in another tool or agent: the
 * goal, every session, and where the latest one left off. */
export async function getThreadHandoff(id: string, device?: string | null): Promise<string> {
  const data = await rpc<{ markdown: string }>({
    op: "thread_handoff",
    id,
    ...(device ? { device } : {}),
  });
  return data.markdown;
}
