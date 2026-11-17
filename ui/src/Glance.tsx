// The desktop tray popover (360×440): today's usage per tool and how it's going.

import { useCallback, useEffect, useState } from "react";
import { Maximize2, RefreshCw, Target } from "lucide-react";
import { getGlance, openMainWindow, type Glance as GlanceData, type ProviderUsage } from "./api";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Brand } from "@/components/common";
import { cn } from "@/lib/utils";
import { toolIcon } from "./tools";

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
  // Meter: today's sessions vs. your own typical day (capped at 2× so a busy day
  // reads as "full", not off-scale). A quota, when known, wins the meter.
  let pct: number;
  let right: string;
  if (p.quota && p.quota.limit > 0) {
    pct = Math.min(100, (p.quota.used / p.quota.limit) * 100);
    const reset = untilReset(p.quota.resets_at);
    right = reset ? `${Math.round(pct)}% · resets in ${reset}` : `${Math.round(pct)}%`;
  } else {
    const base = Math.max(p.avg_daily_sessions, 0.5);
    pct = Math.min(100, (p.today_sessions / (base * 2)) * 100);
    right = `${p.today_sessions} today`;
  }
  return (
    <div className="space-y-1">
      <div className="flex items-baseline justify-between gap-2 text-[13px]">
        <span className="flex min-w-0 items-center gap-1.5 truncate font-medium">
          <span aria-hidden="true" className="text-[11px]">
            {toolIcon(p.tool)}
          </span>
          {label(p.tool)}
        </span>
        <span className="shrink-0 text-xs tabular-nums text-muted-foreground">{right}</span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-muted">
        <div
          className={cn("h-full rounded-full", p.quota && pct >= 85 ? "bg-warning" : "bg-primary")}
          style={{ width: `${pct}%` }}
        />
      </div>
      <p className="text-[11px] text-muted-foreground">
        {p.quota
          ? `${p.week_sessions} sessions this week`
          : `${p.week_sessions} this week · typically ${p.avg_daily_sessions.toFixed(1)}/day`}
      </p>
    </div>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return <p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">{children}</p>;
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
    <div className="flex h-full max-h-[440px] w-full flex-col overflow-hidden bg-background text-foreground">
      <header className="flex items-center justify-between border-b px-3.5 py-2.5">
        <Brand size="sm" />
        <div className="flex items-center gap-0.5">
          <Button variant="ghost" size="icon-sm" onClick={load} disabled={busy} title="Refresh" aria-label="Refresh">
            <RefreshCw className={cn(busy && "animate-spin")} />
          </Button>
          <Button variant="ghost" size="icon-sm" onClick={() => openMainWindow()} title="Open Crossthreads" aria-label="Open Crossthreads">
            <Maximize2 />
          </Button>
        </div>
      </header>

      <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-3.5 py-3 scrollbar-thin">
        {error && (
          <p role="alert" className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive">
            {error}
          </p>
        )}
        {!error && !g && (
          <div className="space-y-3">
            <Skeleton className="h-12" />
            <Skeleton className="h-10" />
            <Skeleton className="h-10" />
            <Skeleton className="h-16" />
          </div>
        )}

        {g && (
          <>
            <div className="rounded-lg border bg-card px-3 py-2.5">
              <div className="flex items-baseline gap-1.5">
                <span className="text-2xl font-semibold tabular-nums">{g.today_sessions}</span>
                <span className="text-[13px] text-muted-foreground">
                  session{g.today_sessions === 1 ? "" : "s"} · {g.today_projects} project{g.today_projects === 1 ? "" : "s"} today
                </span>
              </div>
              <p className="text-xs text-muted-foreground">{g.week_sessions} this week</p>
            </div>

            <section className="space-y-2.5">
              <SectionLabel>Usage</SectionLabel>
              {g.providers.length === 0 ? (
                <p className="text-[13px] text-muted-foreground">No sessions this week yet.</p>
              ) : (
                g.providers.map((p) => <UsageRow key={p.tool} p={p} />)
              )}
            </section>

            <section className="space-y-2">
              <SectionLabel>How it's going</SectionLabel>
              <div className="grid grid-cols-2 gap-2">
                <div className="rounded-lg border bg-card px-3 py-2">
                  <p className="text-xl font-semibold tabular-nums">{g.fluency_overall}</p>
                  <p className="text-[11px] leading-snug text-muted-foreground">
                    fluency{g.fluency_weakest ? ` · ${g.fluency_weakest[0].toLowerCase()} lowest` : ""}
                  </p>
                </div>
                <div className="rounded-lg border bg-card px-3 py-2">
                  <p className="text-xl font-semibold tabular-nums">{g.unresolved}</p>
                  <p className="text-[11px] leading-snug text-muted-foreground">
                    unresolved thread{g.unresolved === 1 ? "" : "s"}
                  </p>
                </div>
              </div>
            </section>

            {g.focus && (
              <div className="flex gap-2 rounded-lg border border-primary/25 bg-primary/5 px-3 py-2.5">
                <Target className="mt-0.5 size-4 shrink-0 text-primary" />
                <div className="min-w-0">
                  <SectionLabel>Focus</SectionLabel>
                  <p className="text-[13px] leading-snug">{g.focus}</p>
                </div>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
