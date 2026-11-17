// Insights: what your history says about how you work. The scores, focus and
// activity are computed locally with no model; the weekly review, theme names
// and analyses use your model, and only when you ask.

import * as React from "react";
import {
  ArrowDownRight,
  ArrowUpRight,
  CalendarDays,
  Lightbulb,
  Loader2,
  MessageCircleQuestion,
  RefreshCw,
  Sparkles,
  Target,
  Wand2,
} from "lucide-react";
import {
  getActivity,
  getFluency,
  getInsight,
  getMetrics,
  getOptimize,
  getThemes,
  getWeeklyReview,
  type ActivityBucket,
  type FluencyReport,
  type Insight,
  type InsightKind,
  type OptimizeReport,
  type Theme,
  type WeeklyReview,
  type WorkMetrics,
} from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Empty } from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { Notice, PageHeader, Segmented, SectionHeader, SourceList, ToolChip } from "@/components/common";
import { Markdown } from "@/lib/markdown";
import { cn } from "@/lib/utils";
import { toolLabel } from "@/tools";
import { ASK_PREFILL_KEY } from "@/pages/Search";

type Tab = "overview" | "weekly" | "themes" | "analyses";

export const ANALYSIS_GROUPS: [string, [InsightKind, string, string][]][] = [
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

// One colour per tool, readable in both themes.
const PALETTE = [
  "hsl(221 83% 58%)",
  "hsl(160 60% 42%)",
  "hsl(35 92% 52%)",
  "hsl(280 60% 62%)",
  "hsl(350 75% 60%)",
  "hsl(190 75% 45%)",
  "hsl(90 50% 45%)",
  "hsl(15 80% 58%)",
  "hsl(250 50% 65%)",
  "hsl(0 0% 55%)",
];

const NEEDS_MODEL = /no usable model credential/i;

// Kept across visits, so switching tabs or pages doesn't recompute everything.
const cache: {
  fluency?: FluencyReport;
  optimize?: OptimizeReport;
  activity: Partial<Record<"day" | "week", ActivityBucket[]>>;
  metrics?: WorkMetrics | null;
  weekly?: WeeklyReview;
  themes?: Theme[];
  insights: Partial<Record<InsightKind, Insight>>;
  lastKind?: InsightKind;
} = { activity: {}, insights: {} };

function markWeeklySeen(r: WeeklyReview | null | undefined) {
  if (!r?.generated_at) return;
  try {
    localStorage.setItem("ct-weekly-seen", r.generated_at);
  } catch {
    /* ignore */
  }
}

export function InsightsPage({
  tab,
  onTab,
  onOpenSession,
}: {
  tab: "overview" | "weekly" | "themes" | "analyses";
  onTab: (t: string) => void;
  onOpenSession: (id: string, device?: string | null) => void;
}) {
  return (
    <div className="mx-auto w-full max-w-5xl space-y-6 px-4 py-6 sm:px-6 sm:py-8">
      <PageHeader
        title="How you work"
        description="What your history says about how you work. Scores, focus and activity are computed here with no model; the weekly review and analyses use your model."
      />
      <Segmented<Tab>
        label="Insights"
        value={tab}
        onChange={onTab}
        options={[
          { value: "overview", label: "Overview" },
          { value: "weekly", label: "Weekly review" },
          { value: "themes", label: "Themes" },
          { value: "analyses", label: "Analyses" },
        ]}
      />
      {tab === "overview" && <Overview />}
      {tab === "weekly" && <Weekly />}
      {tab === "themes" && <Themes onOpenSession={onOpenSession} />}
      {tab === "analyses" && <Analyses onOpenSession={onOpenSession} />}
    </div>
  );
}

// --- Overview ------------------------------------------------------------------

function Overview() {
  const [fluency, setFluency] = React.useState<FluencyReport | null>(cache.fluency ?? null);
  const [fluencyErr, setFluencyErr] = React.useState<string | null>(null);
  const [optimize, setOptimize] = React.useState<OptimizeReport | null>(cache.optimize ?? null);
  const [optimizeBusy, setOptimizeBusy] = React.useState(false);
  const [optimizeErr, setOptimizeErr] = React.useState<string | null>(null);
  const [metrics, setMetrics] = React.useState<WorkMetrics | null | undefined>(cache.metrics);

  const loadOptimize = React.useCallback(async (force: boolean) => {
    setOptimizeBusy(true);
    setOptimizeErr(null);
    try {
      const r = await getOptimize(force);
      cache.optimize = r;
      setOptimize(r);
    } catch (err) {
      setOptimizeErr(String(err));
    } finally {
      setOptimizeBusy(false);
    }
  }, []);

  React.useEffect(() => {
    getFluency()
      .then((r) => {
        cache.fluency = r;
        setFluency(r);
        setFluencyErr(null);
      })
      .catch((e) => setFluencyErr(String(e)));
    loadOptimize(false);
    getMetrics()
      .then((m) => {
        cache.metrics = m;
        setMetrics(m);
      })
      .catch(() => setMetrics(null));
  }, [loadOptimize]);

  return (
    <div className="space-y-10">
      {/* Fluency */}
      <section className="space-y-4">
        <SectionHeader
          title="Working with agents"
          description="Four sides of working with an agent, scored from your own sessions, with the change since the previous window."
        />
        {fluencyErr && <Notice tone="error" title="Couldn't compute the scores">{fluencyErr}</Notice>}
        {!fluency && !fluencyErr && (
          <div className="grid gap-3 sm:grid-cols-2">
            {Array.from({ length: 4 }).map((_, i) => (
              <Skeleton key={i} className="h-40" />
            ))}
          </div>
        )}
        {fluency && fluency.sessions === 0 && (
          <Empty icon={<Target />} title={`No sessions in the last ${fluency.window_days} days yet`}>
            Use your AI coding tools for a while and this fills in.
          </Empty>
        )}
        {fluency && fluency.sessions > 0 && <Fluency report={fluency} />}
      </section>

      {/* Metrics */}
      {metrics && metrics.sessions > 0 && (
        <section className="space-y-4">
          <SectionHeader title="Your numbers" description="Across everything indexed. No model involved." />
          <MetricTiles m={metrics} />
          {metrics.by_hour.some(([, n]) => n > 0) && <HourChart byHour={metrics.by_hour} />}
        </section>
      )}

      {/* Focus */}
      <section className="space-y-4">
        <SectionHeader
          title="Your focus"
          description="The one change most worth making now, whether the last one worked, and how your numbers are trending."
          actions={
            <Button
              variant="outline"
              size="sm"
              onClick={() => loadOptimize(true)}
              disabled={optimizeBusy}
              title="Grade the current focus now instead of waiting about a week"
            >
              <RefreshCw className={cn(optimizeBusy && "animate-spin")} />
              Grade now
            </Button>
          }
        />
        {optimizeErr && <Notice tone="error" title="Couldn't work out your focus">{optimizeErr}</Notice>}
        {!optimize && !optimizeErr && <ReportSkeleton />}
        {optimize && (
          <Card className={cn("p-5 shadow-none", optimizeBusy && "opacity-60")}>
            <Markdown source={optimize.markdown} />
          </Card>
        )}
      </section>

      {/* Activity */}
      <Activity />
    </div>
  );
}

function Fluency({ report }: { report: FluencyReport }) {
  return (
    <div className="space-y-3">
      <Card className="flex flex-wrap items-center gap-x-6 gap-y-2 p-5 shadow-none">
        <div className="flex items-baseline gap-1.5">
          <span className="text-4xl font-semibold tabular-nums tracking-tight">{report.overall}</span>
          <span className="text-sm text-muted-foreground">/ 100 overall</span>
        </div>
        <div className="h-2 min-w-[8rem] flex-1 overflow-hidden rounded-full bg-muted">
          <div className="h-full rounded-full bg-primary" style={{ width: `${report.overall}%` }} />
        </div>
        <span className="text-[13px] text-muted-foreground">
          {report.sessions} session{report.sessions === 1 ? "" : "s"} · last {report.window_days} days
        </span>
      </Card>
      <div className="grid gap-3 sm:grid-cols-2">
        {report.dimensions.map((d) => (
          <Card key={d.key} className="flex flex-col gap-3 p-4 shadow-none">
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0">
                <h3 className="text-sm font-semibold">{d.label}</h3>
                <p className="text-xs text-muted-foreground">{d.blurb}</p>
              </div>
              <div className="flex shrink-0 items-center gap-1.5">
                {d.delta != null && d.delta !== 0 && (
                  <Badge variant={d.delta > 0 ? "success" : "destructive"} title="Change since the previous window">
                    {d.delta > 0 ? <ArrowUpRight /> : <ArrowDownRight />}
                    {d.delta > 0 ? `+${d.delta}` : d.delta}
                  </Badge>
                )}
                <span className="text-2xl font-semibold tabular-nums">{d.score}</span>
              </div>
            </div>
            <div className="h-1.5 overflow-hidden rounded-full bg-muted" role="meter" aria-valuenow={d.score} aria-valuemin={0} aria-valuemax={100} aria-label={d.label}>
              <div
                className={cn("h-full rounded-full", d.score >= 70 ? "bg-success" : d.score >= 40 ? "bg-primary" : "bg-warning")}
                style={{ width: `${d.score}%` }}
              />
            </div>
            <p className="text-[13px] leading-relaxed">{d.read}</p>
            {d.components.length > 0 && (
              <div className="mt-auto flex flex-wrap gap-1.5">
                {d.components.map(([k, v]) => (
                  <span key={k} className="rounded-md bg-muted px-2 py-0.5 text-xs text-muted-foreground">
                    {k}: <span className="font-medium text-foreground">{v}</span>
                  </span>
                ))}
              </div>
            )}
          </Card>
        ))}
      </div>
      {report.reflection && (
        <Card className="flex flex-col gap-3 border-primary/25 bg-primary/5 p-4 shadow-none sm:flex-row sm:items-center">
          <Lightbulb className="size-5 shrink-0 text-primary" />
          <div className="min-w-0 flex-1">
            <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">A question to sit with</p>
            <p className="text-sm">{report.reflection.prompt}</p>
          </div>
          <Button
            variant="outline"
            size="sm"
            className="shrink-0"
            title="Talk it through with your history"
            onClick={() => {
              try {
                sessionStorage.setItem(ASK_PREFILL_KEY, report.reflection!.prompt);
              } catch {
                /* ignore */
              }
              window.location.hash = "#/search/ask";
            }}
          >
            <MessageCircleQuestion /> Reflect with Ask
          </Button>
        </Card>
      )}
    </div>
  );
}

function MetricTiles({ m }: { m: WorkMetrics }) {
  const pct = (x: number) => `${Math.round(x * 100)}%`;
  const tiles: [string, string, string?][] = [
    ["Sessions", m.sessions.toLocaleString()],
    ["Median turns", String(m.median_user_turns), "Your messages per session"],
    ["Rework", pct(m.correction_rate), "Sessions where you had to correct the agent"],
    ["Unresolved", pct(m.abandonment_rate), "Sessions that ended without a resolution"],
    ["Specific prompts", pct(m.specificity_rate), "Opening prompts that name files, errors or tests"],
    ["Actions / session", m.avg_tool_actions.toFixed(1), "Tool calls the agent made"],
  ];
  if (m.median_session_minutes > 0) tiles.push(["Median length", `${Math.round(m.median_session_minutes)} min`]);
  if (m.top_languages[0]) tiles.push(["Top language", m.top_languages[0][0]]);
  return (
    <div className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border bg-border sm:grid-cols-4">
      {tiles.map(([label, value, hint]) => (
        <div key={label} className="bg-card px-4 py-3" title={hint}>
          <p className="text-xs text-muted-foreground">{label}</p>
          <p className="mt-0.5 truncate text-lg font-semibold tabular-nums">{value}</p>
        </div>
      ))}
    </div>
  );
}

function HourChart({ byHour }: { byHour: [number, number][] }) {
  const counts = Array.from({ length: 24 }, (_, h) => byHour.find(([x]) => x === h)?.[1] ?? 0);
  const max = Math.max(...counts, 1);
  return (
    <Card className="p-4 shadow-none">
      <p className="mb-3 text-xs font-medium text-muted-foreground">When you work (local time)</p>
      <div className="flex h-20 items-end gap-[3px]">
        {counts.map((n, h) => (
          <div key={h} className="group relative flex h-full flex-1 items-end" title={`${String(h).padStart(2, "0")}:00 · ${n} session${n === 1 ? "" : "s"}`}>
            <div
              className={cn("w-full rounded-sm", n ? "bg-primary/70 group-hover:bg-primary" : "bg-muted")}
              style={{ height: n ? `${Math.max(6, (n / max) * 100)}%` : "3px" }}
            />
          </div>
        ))}
      </div>
      <div className="mt-1.5 flex justify-between text-[10px] tabular-nums text-muted-foreground">
        <span>00</span>
        <span>06</span>
        <span>12</span>
        <span>18</span>
        <span>23</span>
      </div>
    </Card>
  );
}

function Activity() {
  const [bucket, setBucket] = React.useState<"day" | "week">("week");
  const [data, setData] = React.useState<ActivityBucket[] | null>(cache.activity.week ?? null);

  React.useEffect(() => {
    let live = true;
    setData(cache.activity[bucket] ?? null);
    getActivity(bucket)
      .then((r) => {
        cache.activity[bucket] = r.periods;
        if (live) setData(r.periods);
      })
      .catch(() => live && setData([]));
    return () => {
      live = false;
    };
  }, [bucket]);

  return (
    <section className="space-y-4">
      <SectionHeader
        title="Activity"
        description="Sessions over time, by tool."
        actions={
          <Segmented<"day" | "week">
            size="sm"
            label="Group by"
            value={bucket}
            onChange={setBucket}
            options={[
              { value: "day", label: "Day" },
              { value: "week", label: "Week" },
            ]}
          />
        }
      />
      {!data && <Skeleton className="h-56" />}
      {data && data.length === 0 && (
        <Empty icon={<CalendarDays />} title="No dated sessions yet">
          Index some sessions first.
        </Empty>
      )}
      {data && data.length > 0 && <ActivityChart periods={data} bucket={bucket} />}
    </section>
  );
}

function ActivityChart({ periods, bucket }: { periods: ActivityBucket[]; bucket: "day" | "week" }) {
  const total = periods.reduce((s, p) => s + p.total, 0);
  const max = Math.max(...periods.map((p) => p.total), 1);
  const busiest = periods.reduce((a, b) => (b.total > a.total ? b : a));
  const toolTotals = new Map<string, number>();
  for (const p of periods) for (const [t, n] of p.by_tool) toolTotals.set(t, (toolTotals.get(t) ?? 0) + n);
  const tools = [...toolTotals.entries()].sort((a, b) => b[1] - a[1]);
  const color = (t: string) => PALETTE[Math.max(0, tools.findIndex(([x]) => x === t)) % PALETTE.length];
  // Oldest on the left. The daemon may send either order.
  const ordered = [...periods].sort((a, b) => a.period.localeCompare(b.period));
  const labelEvery = Math.max(1, Math.ceil(ordered.length / 8));
  const [hover, setHover] = React.useState<ActivityBucket | null>(null);
  const shownPeriod = hover ?? busiest;

  return (
    <Card className="space-y-4 p-4 shadow-none sm:p-5">
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Stat label="Sessions" value={total.toLocaleString()} />
        <Stat label={`Active ${bucket}s`} value={String(periods.length)} />
        <Stat label={`Average per ${bucket}`} value={(total / periods.length).toFixed(1)} />
        <Stat label="Busiest" value={`${busiest.period}`} sub={`${busiest.total} sessions`} />
      </div>
      <div className="overflow-x-auto scrollbar-thin">
        <div
          className="flex h-44 items-end gap-[2px]"
          style={{ minWidth: `${Math.max(ordered.length * (bucket === "day" ? 6 : 14), 100)}px` }}
          onMouseLeave={() => setHover(null)}
        >
          {ordered.map((p) => (
            <div
              key={p.period}
              className="flex h-full min-w-0 flex-1 flex-col justify-end"
              onMouseEnter={() => setHover(p)}
              title={`${p.period}: ${p.total} — ${p.by_tool.map(([t, n]) => `${toolLabel(t)} ${n}`).join(", ")}`}
            >
              <div
                className={cn("flex w-full flex-col-reverse overflow-hidden rounded-t-sm transition-opacity", hover && hover !== p && "opacity-50")}
                style={{ height: `${(p.total / max) * 100}%` }}
              >
                {p.by_tool.map(([t, n]) => (
                  <div key={t} style={{ background: color(t), flexGrow: n }} />
                ))}
              </div>
            </div>
          ))}
        </div>
        <div className="mt-1.5 flex h-5 gap-[2px] border-t pt-1.5" style={{ minWidth: `${Math.max(ordered.length * (bucket === "day" ? 6 : 14), 100)}px` }}>
          {ordered.map((p, i) => (
            <div key={p.period} className="relative min-w-0 flex-1">
              {i % labelEvery === 0 && (
                <span className="absolute left-0 whitespace-nowrap text-[10px] tabular-nums text-muted-foreground">
                  {p.period.length > 7 ? p.period.slice(5) : p.period}
                </span>
              )}
            </div>
          ))}
        </div>
      </div>
      <div className="flex flex-col gap-3 pt-3 sm:flex-row sm:items-start sm:justify-between">
        <div className="flex flex-wrap gap-x-4 gap-y-1.5">
          {tools.map(([t, n]) => (
            <span key={t} className="inline-flex items-center gap-1.5 text-xs">
              <span className="size-2.5 rounded-sm" style={{ background: color(t) }} />
              {toolLabel(t)}
              <span className="tabular-nums text-muted-foreground">{n.toLocaleString()}</span>
            </span>
          ))}
        </div>
        <p className="shrink-0 text-xs text-muted-foreground" aria-live="polite">
          <span className="font-medium text-foreground">{shownPeriod.period}</span> · {shownPeriod.total} session
          {shownPeriod.total === 1 ? "" : "s"}
        </p>
      </div>
    </Card>
  );
}

function Stat({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="min-w-0">
      <p className="text-xs text-muted-foreground">{label}</p>
      <p className="truncate text-lg font-semibold tabular-nums">{value}</p>
      {sub && <p className="text-xs text-muted-foreground">{sub}</p>}
    </div>
  );
}

function ReportSkeleton() {
  return (
    <Card className="space-y-2.5 p-5 shadow-none">
      <Skeleton className="h-4 w-1/3" />
      <Skeleton className="h-3 w-full" />
      <Skeleton className="h-3 w-11/12" />
      <Skeleton className="h-3 w-4/5" />
    </Card>
  );
}

// --- Weekly review -------------------------------------------------------------

function Weekly() {
  const [review, setReview] = React.useState<WeeklyReview | null>(cache.weekly ?? null);
  const [loading, setLoading] = React.useState(!cache.weekly);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    getWeeklyReview(false)
      .then((r) => {
        cache.weekly = r;
        setReview(r);
        markWeeklySeen(r);
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  async function refresh() {
    setBusy(true);
    setError(null);
    try {
      const r = await getWeeklyReview(true);
      cache.weekly = r;
      setReview(r);
      markWeeklySeen(r);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="space-y-4">
      <SectionHeader
        title={review ? `Week of ${review.period_start}` : "Weekly review"}
        description={
          review
            ? review.llm_used
              ? "How you worked, and one thing to try."
              : "Metrics only. Connect a model for a written review."
            : "How you worked this week, and one thing to try."
        }
        actions={
          <Button variant="outline" size="sm" onClick={refresh} disabled={busy} title="Write it again from the last 7 days">
            <RefreshCw className={cn(busy && "animate-spin")} />
            {busy ? "Writing…" : "Refresh"}
          </Button>
        }
      />
      {error && <ModelError error={error} />}
      {(loading || busy) && <ReportSkeleton />}
      {!loading && !busy && review && (
        <Card className="p-5 shadow-none sm:p-6">
          <Markdown source={review.markdown} />
          {review.generated_at && (
            <p className="mt-5 border-t pt-3 text-xs text-muted-foreground">
              Written {new Date(review.generated_at).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}
            </p>
          )}
        </Card>
      )}
      {!loading && !busy && !review && !error && (
        <Empty icon={<CalendarDays />} title="No review yet">
          Index some sessions first.
        </Empty>
      )}
    </section>
  );
}

// --- Themes --------------------------------------------------------------------

function Themes({ onOpenSession }: { onOpenSession: (id: string) => void }) {
  const [themes, setThemes] = React.useState<Theme[] | null>(cache.themes ?? null);
  const [naming, setNaming] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (cache.themes && cache.themes.length) return;
    getThemes(8)
      .then((t) => {
        cache.themes = t;
        setThemes(t);
      })
      .catch(() => setThemes([]));
  }, []);

  async function nameThem() {
    setNaming(true);
    setError(null);
    try {
      const t = await getThemes(8, true);
      cache.themes = t;
      setThemes(t);
    } catch (e) {
      setError(String(e));
    } finally {
      setNaming(false);
    }
  }

  const max = Math.max(1, ...(themes ?? []).map((t) => t.size));

  return (
    <section className="space-y-4">
      <SectionHeader
        title="What you've been working on"
        description="Your sessions clustered by topic, across tools. Open a session to read it."
        actions={
          <Button
            variant="outline"
            size="sm"
            onClick={nameThem}
            disabled={naming || !themes || themes.length === 0}
            title="Give each theme a short name with your model"
          >
            {naming ? <Loader2 className="animate-spin" /> : <Sparkles />}
            {naming ? "Naming…" : "Name them"}
          </Button>
        }
      />
      {error && <ModelError error={error} />}
      {!themes && (
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {Array.from({ length: 6 }).map((_, i) => (
            <Skeleton key={i} className="h-44" />
          ))}
        </div>
      )}
      {themes && themes.length === 0 && (
        <Empty icon={<Wand2 />} title="No themes yet">
          Index some sessions first. Themes group sessions by meaning, so they need the search model's embeddings.
        </Empty>
      )}
      {themes && themes.length > 0 && (
        <div className="flex flex-wrap gap-3">
          {themes.map((t, i) => {
            // Bigger themes take more room (sqrt keeps big ones sane).
            const weight = Math.sqrt(t.size / max);
            return (
              <Card
                key={i}
                className={cn("flex flex-col gap-3 p-4 shadow-none transition-opacity", naming && "opacity-60")}
                style={{ flexGrow: 1 + weight * 3, flexBasis: `${200 + weight * 120}px` }}
              >
                <div className="flex items-start justify-between gap-2">
                  <h3 className="text-sm font-semibold leading-snug">{t.label}</h3>
                  <Badge variant="secondary" className="shrink-0 tabular-nums" title={`${t.size} sessions`}>
                    {t.size}
                  </Badge>
                </div>
                <div className="h-1 overflow-hidden rounded-full bg-muted">
                  <div className="h-full rounded-full bg-primary/70" style={{ width: `${(t.size / max) * 100}%` }} />
                </div>
                {t.tools.length > 0 && (
                  <div className="flex flex-wrap gap-1">
                    {t.tools.slice(0, 3).map(([name, n]) => (
                      <ToolChip key={name} tool={name} count={n} />
                    ))}
                  </div>
                )}
                <ul className="-mx-1.5 space-y-0.5">
                  {t.samples.slice(0, 4).map((s) => (
                    <li key={s.id}>
                      <button
                        type="button"
                        onClick={() => onOpenSession(s.id)}
                        title={s.title ?? ""}
                        className="block w-full truncate rounded px-1.5 py-1 text-left text-[13px] text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      >
                        {(s.title ?? "").trim() || "(untitled)"}
                      </button>
                    </li>
                  ))}
                </ul>
              </Card>
            );
          })}
        </div>
      )}
    </section>
  );
}

// --- Analyses ------------------------------------------------------------------

function Analyses({ onOpenSession }: { onOpenSession: (id: string) => void }) {
  const [kind, setKind] = React.useState<InsightKind | null>(cache.lastKind ?? null);
  const [results, setResults] = React.useState<Partial<Record<InsightKind, Insight>>>(cache.insights);
  const [busy, setBusy] = React.useState<InsightKind | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [metrics, setMetrics] = React.useState<WorkMetrics | null | undefined>(cache.metrics);
  const resultRef = React.useRef<HTMLDivElement>(null);

  React.useEffect(() => {
    if (cache.metrics !== undefined) return;
    getMetrics()
      .then((m) => {
        cache.metrics = m;
        setMetrics(m);
      })
      .catch(() => {});
  }, []);

  async function run(k: InsightKind, force = false) {
    setKind(k);
    cache.lastKind = k;
    setError(null);
    if (results[k] && !force) return;
    setBusy(k);
    window.setTimeout(() => resultRef.current?.scrollIntoView({ behavior: "smooth", block: "nearest" }), 50);
    try {
      const r = await getInsight(k);
      cache.insights[k] = r;
      setResults({ ...cache.insights });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  const label = kind ? ANALYSIS_GROUPS.flatMap(([, ks]) => ks).find(([k]) => k === kind)?.[1] : null;
  const current = kind ? results[kind] : undefined;

  return (
    <div className="space-y-8">
      {metrics && metrics.sessions > 0 && <MetricTiles m={metrics} />}
      <p className="text-[13px] text-muted-foreground">
        Each of these reads your history and writes a short report with your model. They run only when you choose one.
      </p>
      {ANALYSIS_GROUPS.map(([group, kinds]) => (
        <section key={group} className="space-y-3">
          <h2 className="text-sm font-semibold">{group}</h2>
          <div className="grid gap-2.5 sm:grid-cols-2 lg:grid-cols-4">
            {kinds.map(([k, name, blurb]) => {
              const on = kind === k;
              return (
                <button
                  key={k}
                  type="button"
                  onClick={() => run(k)}
                  disabled={busy !== null}
                  aria-pressed={on}
                  className={cn(
                    "group flex flex-col items-start gap-1 rounded-lg border bg-card p-3.5 text-left transition-colors hover:border-foreground/20 hover:bg-accent/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60",
                    on && "border-primary/50 bg-primary/5 ring-1 ring-primary/25",
                  )}
                >
                  <span className="flex w-full items-center justify-between gap-2 text-sm font-medium">
                    {name}
                    {busy === k ? (
                      <Loader2 className="size-4 animate-spin text-primary" />
                    ) : results[k] ? (
                      <Badge variant="success" className="font-normal">
                        ready
                      </Badge>
                    ) : null}
                  </span>
                  <span className="text-xs leading-relaxed text-muted-foreground">{blurb}</span>
                </button>
              );
            })}
          </div>
        </section>
      ))}

      <div ref={resultRef} className="scroll-mt-4">
        {busy && (
          <Card className="space-y-3 p-5 shadow-none" aria-busy="true">
            <p className="flex items-center gap-2 text-sm text-muted-foreground">
              <Sparkles className="size-4 animate-pulse text-primary" /> Writing up {label}… this takes a little while.
            </p>
            <Skeleton className="h-3 w-full" />
            <Skeleton className="h-3 w-11/12" />
            <Skeleton className="h-3 w-3/4" />
          </Card>
        )}
        {!busy && error && <ModelError error={error} />}
        {!busy && !error && current && kind && (
          <Card className="overflow-hidden shadow-none">
            <div className="flex flex-wrap items-center justify-between gap-2 border-b px-5 py-3">
              <h2 className="text-sm font-semibold">{label}</h2>
              <Button variant="ghost" size="sm" onClick={() => run(kind, true)}>
                <RefreshCw /> Run again
              </Button>
            </div>
            <div className="p-5 sm:p-6">
              <Markdown source={current.markdown} />
            </div>
            {current.sources.length > 0 && (
              <details className="group border-t px-5 py-3">
                <summary className="cursor-pointer select-none text-[13px] text-muted-foreground hover:text-foreground">
                  Drawn from {current.sources.length} session{current.sources.length === 1 ? "" : "s"}
                </summary>
                <SourceList className="mt-3" ids={current.sources} onOpen={onOpenSession} />
              </details>
            )}
          </Card>
        )}
      </div>
    </div>
  );
}

/** A model-backed feature failed: offer to set one up when that's why. */
function ModelError({ error }: { error: string }) {
  if (NEEDS_MODEL.test(error))
    return (
      <Notice
        tone="info"
        title="This needs a model"
        action={
          <Button size="sm" onClick={() => (window.location.hash = "#/settings/models")}>
            Set up a model
          </Button>
        }
      >
        Sign in to Claude Code or Codex on this computer, or paste an API key, and Crossthreads will use it. Nothing is
        sent anywhere else.
      </Notice>
    );
  return <Notice tone="error" title="That didn't work">{error}</Notice>;
}
