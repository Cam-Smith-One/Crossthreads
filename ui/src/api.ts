// Thin client for the daemon's HTTP/JSON bridge (POST /api/rpc).

export type Mode = "lexical" | "semantic" | "hybrid";

export interface Hit {
  conversation_id: string;
  tool: string;
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

async function rpc<T>(body: unknown): Promise<T> {
  const res = await fetch("/api/rpc", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data = await res.json();
  if (data && data.type === "error") {
    throw new Error(data.message ?? "daemon error");
  }
  return data as T;
}

export function getStatus(): Promise<Status> {
  return rpc<Status>({ op: "status" });
}

export async function search(query: string, mode: Mode, limit = 20): Promise<Hit[]> {
  const data = await rpc<{ hits: Hit[] }>({ op: "search", query, mode, limit });
  return data.hits;
}

export function buildContext(
  query: string,
  mode: Mode,
  limit = 3,
  max_chars = 6000,
): Promise<ContextBlock> {
  return rpc<ContextBlock>({ op: "context", query, mode, limit, max_chars });
}
