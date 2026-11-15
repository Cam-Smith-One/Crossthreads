// Friendly label + glyph per tool slug, shared by the status chips, the
// activity breakdown and the board. Unknown tools fall back to the slug and a
// neutral dot.
const TOOL_META: Record<string, { label: string; icon: string }> = {
  "claude-code": { label: "Claude Code", icon: "✳️" },
  codex: { label: "Codex", icon: "◆" },
  cursor: { label: "Cursor", icon: "▱" },
  aider: { label: "Aider", icon: "✦" },
  cline: { label: "Cline", icon: "◈" },
  copilot: { label: "Copilot", icon: "⊕" },
  "gemini-cli": { label: "Gemini CLI", icon: "✶" },
  windsurf: { label: "Windsurf", icon: "🌊" },
  antigravity: { label: "Antigravity", icon: "🛰️" },
};

export function toolLabel(slug: string): string {
  return TOOL_META[slug]?.label ?? slug;
}

export function toolIcon(slug: string): string {
  return TOOL_META[slug]?.icon ?? "•";
}
