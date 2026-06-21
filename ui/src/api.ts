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
    data = await res.json();
  }
  if (data && data.type === "error") {
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
}

export function getFederationConfig(): Promise<FederationConfig> {
  return rpc<FederationConfig>({ op: "get_federation_config" });
}

export function setFederationConfig(patch: {
  device?: string;
  token?: string;
  exclude_tools?: string[];
  exclude_projects?: string[];
}): Promise<FederationConfig> {
  return rpc<FederationConfig>({ op: "set_federation_config", ...patch });
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
