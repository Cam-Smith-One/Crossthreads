// Thin client for the daemon's HTTP/JSON bridge (POST /api/rpc).

export type Mode = "lexical" | "semantic" | "hybrid";

export interface Filters {
  tool?: string;
  kind?: string;
  project?: string;
  since?: string;
  until?: string;
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
  return out;
}

export function getStatus(): Promise<Status> {
  return rpc<Status>({ op: "status" });
}

export async function getFacets(): Promise<string[]> {
  const data = await rpc<{ tools: string[] }>({ op: "facets" });
  return data.tools;
}

export async function search(
  query: string,
  mode: Mode,
  filters: Filters,
  limit = 25,
): Promise<Hit[]> {
  const data = await rpc<{ hits: Hit[] }>({
    op: "search",
    query,
    mode,
    limit,
    filters: clean(filters),
  });
  return data.hits;
}

export async function getConversation(id: string): Promise<StoredConversation | null> {
  const data = await rpc<{ conversation: StoredConversation | null }>({
    op: "get_conversation",
    id,
  });
  return data.conversation;
}

export function buildContext(
  query: string,
  mode: Mode,
  filters: Filters,
  limit = 3,
  max_chars = 6000,
): Promise<ContextBlock> {
  return rpc<ContextBlock>({
    op: "context",
    query,
    mode,
    limit,
    max_chars,
    filters: clean(filters),
  });
}
