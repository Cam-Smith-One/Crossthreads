import { useCallback, useEffect, useState } from "react";
import {
  getGlance,
  openMainWindow,
  type Glance as GlanceData,
  type ProviderUsage,
} from "./api";

// Short, friendly tool labels for the compact popover.
const TOOL_LABEL: Record<string, string> = {
  "claude-code": "Claude Code",
  codex: "Codex",
  cursor: "Cursor",
  copilot: "Copilot",
  gemini: "Gemini",
  windsurf: "Windsurf",
  aider: "Aider",
  cline: "Cline",
  "open-webui": "Open WebUI",
};

function label(tool: string): string {
  return TOOL_LABEL[tool] ?? tool;
}

/** Countdown like "2h 12m" until an ISO instant, or null if past/invalid. */
function untilReset(iso: string): string | null {
  const ms = new Date(iso).getTime() - Date.now();
  if (!Number.isFinite(ms) || ms <= 0) return null;
  const mins = Math.floor(ms / 60000);
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return h > 0 ? `${h}h ${m}m` : `${m}m`;
}

function UsageRow({ p }: { p: ProviderUsage }) {
  // Meter: today's sessions vs. your own typical day (capped at 2× baseline so a
  // busy day reads as "full", not off-scale). Quota, when present, wins the meter.
  let pct: number;
  let right: string;
  if (p.quota && p.quota.limit > 0) {
    pct = Math.min(100, (p.quota.used / p.quota.limit) * 100);
    const reset = untilReset(p.quota.resets_at);
    right = reset ? `${Math.round(pct)}% · ${reset}` : `${Math.round(pct)}%`;
  } else {
    const base = Math.max(p.avg_daily_sessions, 0.5);
    pct = Math.min(100, (p.today_sessions / (base * 2)) * 100);
    right = `${p.today_sessions} today`;
  }
  return (
    <div className="glance-usage">
      <div className="glance-usage-top">
        <span className="glance-usage-name">{label(p.tool)}</span>
        <span className="glance-usage-right">{right}</span>
      </div>
      <div className="glance-meter">
        <div className="glance-meter-fill" style={{ width: `${pct}%` }} />
      </div>
      <div className="glance-usage-sub">
        {p.quota
          ? `${p.week_sessions} sessions this week`
          : `${p.week_sessions} this week · typically ${p.avg_daily_sessions.toFixed(1)}/day`}
      </div>
    </div>
  );
}

export function Glance() {
  const [g, setG] = useState<GlanceData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setBusy(true);
    try {
      setG(await getGlance());
      setError(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    load();
    const id = window.setInterval(load, 60_000);
    return () => window.clearInterval(id);
  }, [load]);

  return (
    <div className="glance">
      <div className="glance-head">
        <span className="glance-brand">
          Cross<span className="brand-threads">threads</span>
        </span>
        <div className="glance-head-actions">
          <button className="glance-icon-btn" onClick={load} disabled={busy} title="Refresh">
            ↻
          </button>
          <button className="glance-icon-btn" onClick={() => openMainWindow()} title="Open Crossthreads">
            ⤢
          </button>
        </div>
      </div>

      {error && <div className="glance-error">{error}</div>}
      {!error && !g && <div className="glance-empty">Loading…</div>}

      {g && (
        <>
          <div className="glance-today">
            <strong>{g.today_sessions}</strong> session{g.today_sessions === 1 ? "" : "s"} ·{" "}
            <strong>{g.today_projects}</strong> project{g.today_projects === 1 ? "" : "s"} today
            <span className="glance-today-week">{g.week_sessions} this week</span>
          </div>

          <div className="glance-section-label">Usage</div>
          {g.providers.length === 0 ? (
            <div className="glance-empty">No sessions this week yet.</div>
          ) : (
            g.providers.map((p) => <UsageRow key={p.tool} p={p} />)
          )}

          <div className="glance-section-label">How it's going</div>
          <div className="glance-tiles">
            <div className="glance-tile">
              <span className="glance-tile-value">{g.fluency_overall}</span>
              <span className="glance-tile-label">
                fluency
                {g.fluency_weakest ? ` · ${g.fluency_weakest[0].toLowerCase()} lowest` : ""}
              </span>
            </div>
            <div className="glance-tile">
              <span className="glance-tile-value">{g.unresolved}</span>
              <span className="glance-tile-label">
                unresolved thread{g.unresolved === 1 ? "" : "s"}
              </span>
            </div>
          </div>

          {g.focus && (
            <div className="glance-focus">
              <span className="glance-focus-label">Focus</span>
              <span className="glance-focus-text">{g.focus}</span>
            </div>
          )}
        </>
      )}
    </div>
  );
}
