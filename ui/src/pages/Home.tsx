import { useCallback, useEffect, useMemo, useState } from "react";
import { AlertTriangle, ArrowRight, GitBranch, Hourglass, Inbox, Plus, Search, Send, Sparkles, WifiOff } from "lucide-react";
import { getThreads, type Device, type ThreadStatus, type ThreadSummary } from "@/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Empty } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ContinueMenu } from "@/components/ContinueMenu";
import { DeviceBadge, LiveBadge, StatusBadge, ToolBadge } from "@/components/badges";
import { ago, cn } from "@/lib/utils";
import { toolLabel } from "@/tools";

type StatusFilter = ThreadStatus | "all";
const REFRESH_MS = 10_000;
const NO_REPO = "No repository";

/** Home: what's waiting for you, then every piece of work, by repository. */
export function HomePage({
  devices,
  localDevice,
  readOnly,
  refreshKey,
  onOpenThread,
  onNewThread,
  onToast,
  onError,
  header,
}: {
  devices: Device[];
  localDevice?: string;
  readOnly: boolean;
  refreshKey: number;
  onOpenThread: (t: ThreadSummary) => void;
  onNewThread: () => void;
  onToast: (m: string) => void;
  onError: (m: string) => void;
  /** Banners shown above the board (setup nudge, weekly review). */
  header?: React.ReactNode;
}) {
  const [threads, setThreads] = useState<ThreadSummary[] | null>(null);
  const [unreachable, setUnreachable] = useState<string[]>([]);
  const [status, setStatus] = useState<StatusFilter>("active");
  const [tool, setTool] = useState("");
  const [text, setText] = useState("");
  const [shown, setShown] = useState(60);

  const load = useCallback(
    (quiet = false) =>
      getThreads()
        .then(({ threads, unreachable }) => {
          setThreads(threads);
          setUnreachable(unreachable);
        })
        .catch((e) => !quiet && onError(String(e))),
    [onError],
  );
  useEffect(() => {
    load();
  }, [load, refreshKey]);
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") load(true);
    }, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const counts = useMemo(() => {
    const c: Record<StatusFilter, number> = { active: 0, parked: 0, done: 0, all: 0 };
    for (const t of threads ?? []) {
      c[t.status] += 1;
      c.all += 1;
    }
    return c;
  }, [threads]);
  const tools = useMemo(() => [...new Set((threads ?? []).flatMap((t) => t.tools))].sort(), [threads]);

  const visible = useMemo(() => {
    const want = text.trim().toLowerCase();
    return (threads ?? []).filter(
      (t) =>
        (status === "all" || t.status === status) &&
        (!tool || t.tools.includes(tool)) &&
        (!want ||
          t.title.toLowerCase().includes(want) ||
          (t.repo_label ?? "").toLowerCase().includes(want) ||
          (t.standing?.summary ?? "").toLowerCase().includes(want) ||
          t.branches.some((b) => b.toLowerCase().includes(want))),
    );
  }, [threads, status, tool, text]);

  const waitingHere = useMemo(
    () => (threads ?? []).filter((t) => t.moved_to && localDevice && t.moved_to === localDevice),
    [threads, localDevice],
  );
  const needsYou = useMemo(() => visible.filter((t) => t.live?.state === "needs_you"), [visible]);
  const groups = useMemo(() => {
    const out = new Map<string, ThreadSummary[]>();
    for (const t of visible.filter((t) => t.live?.state !== "needs_you").slice(0, shown)) {
      const k = t.repo_label ?? NO_REPO;
      out.set(k, [...(out.get(k) ?? []), t]);
    }
    return [...out.entries()];
  }, [visible, shown]);

  const card = (t: ThreadSummary, highlight?: "waiting" | "needs") => (
    <ThreadCard
      key={`${t.device ?? ""}:${t.id}`}
      t={t}
      highlight={highlight}
      devices={devices}
      localDevice={localDevice}
      readOnly={readOnly}
      onOpen={() => onOpenThread(t)}
      onToast={onToast}
      onError={onError}
      onChanged={() => load(true)}
    />
  );

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Your work</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            One thread per piece of work, across your tools{devices.length > 1 ? " and computers" : ""}. Pick any of it up in Claude Code or Codex.
          </p>
        </div>
        {!readOnly && (
          <Button onClick={onNewThread}>
            <Plus /> New thread
          </Button>
        )}
      </div>

      {header}

      {unreachable.length > 0 && (
        <div className="flex items-center gap-2 rounded-md border border-warning/40 bg-warning/10 px-3 py-2 text-sm">
          <WifiOff className="h-4 w-4 text-warning" />
          {unreachable.join(", ")} {unreachable.length > 1 ? "are" : "is"} offline: {unreachable.length > 1 ? "their" : "its"} threads are missing for now.
        </div>
      )}

      {waitingHere.length > 0 && (
        <Section icon={<Send className="text-primary" />} title="Waiting for you here" count={waitingHere.length}>
          {waitingHere.map((t) => card(t, "waiting"))}
        </Section>
      )}
      {needsYou.length > 0 && (
        <Section icon={<Hourglass className="text-warning" />} title="Needs you" count={needsYou.length}>
          {needsYou.map((t) => card(t, "needs"))}
        </Section>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <Tabs value={status} onValueChange={(s) => setStatus(s as StatusFilter)}>
          <TabsList>
            {(["active", "parked", "done", "all"] as StatusFilter[]).map((s) => (
              <TabsTrigger key={s} value={s} className="capitalize">
                {s} <span className="text-xs text-muted-foreground">{counts[s]}</span>
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
        <div className="relative min-w-[200px] flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
          <Input className="pl-8" placeholder="Filter by name, repository, branch or status" value={text} onChange={(e) => setText(e.target.value)} />
        </div>
        {tools.length > 1 && (
          <NativeSelect value={tool} onChange={(e) => setTool(e.target.value)} aria-label="Tool">
            <option value="">All tools</option>
            {tools.map((x) => (
              <option key={x} value={x}>{toolLabel(x)}</option>
            ))}
          </NativeSelect>
        )}
      </div>

      {threads === null ? (
        <div className="grid gap-3 md:grid-cols-2">
          {[0, 1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-36" />
          ))}
        </div>
      ) : threads.length === 0 ? (
        <Empty icon={<Inbox />} title="No work yet">
          Use Claude Code, Codex, Cursor or another AI coding tool and your sessions appear here, grouped into threads.
          {!readOnly && " Or start one with New thread."}
        </Empty>
      ) : visible.length === 0 ? (
        <Empty title={`No ${status === "all" ? "" : status + " "}threads match`} />
      ) : (
        groups.map(([repo, list]) => (
          <Section key={repo} icon={<GitBranch className="text-muted-foreground" />} title={repo} count={list.length}>
            {list.map((t) => card(t))}
          </Section>
        ))
      )}

      {visible.length - needsYou.length > shown && (
        <div className="flex justify-center">
          <Button variant="outline" onClick={() => setShown(shown + 60)}>
            Show more ({visible.length - needsYou.length - shown} left)
          </Button>
        </div>
      )}
    </div>
  );
}

function Section({ icon, title, count, children }: { icon: React.ReactNode; title: string; count: number; children: React.ReactNode }) {
  return (
    <section className="space-y-3">
      <h2 className="flex items-center gap-2 text-sm font-semibold [&_svg]:h-4 [&_svg]:w-4">
        {icon}
        {title}
        <span className="font-normal text-muted-foreground">{count}</span>
      </h2>
      <div className="grid gap-3 md:grid-cols-2 2xl:grid-cols-3">{children}</div>
    </section>
  );
}

function ThreadCard({
  t,
  highlight,
  devices,
  localDevice,
  readOnly,
  onOpen,
  onToast,
  onError,
  onChanged,
}: {
  t: ThreadSummary;
  highlight?: "waiting" | "needs";
  devices: Device[];
  localDevice?: string;
  readOnly: boolean;
  onOpen: () => void;
  onToast: (m: string) => void;
  onError: (m: string) => void;
  onChanged: () => void;
}) {
  const remote = !!t.device && !!localDevice && t.device !== localDevice;
  const spans = (t.devices?.length ?? 0) > 1 ? t.devices! : remote ? [t.device!] : [];
  const branch = t.branches[t.branches.length - 1];
  return (
    <Card
      role="button"
      tabIndex={0}
      onClick={onOpen}
      onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onOpen())}
      className={cn(
        "group flex cursor-pointer flex-col gap-3 p-4 transition hover:border-foreground/20 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        highlight === "needs" && "border-warning/50",
        highlight === "waiting" && "border-primary/50",
      )}
    >
      <div className="flex items-start gap-2">
        <h3 className="line-clamp-2 flex-1 font-medium leading-snug">{t.title}</h3>
        <span className="shrink-0 text-xs text-muted-foreground" title={t.last_activity ?? ""}>
          {ago(t.last_activity)}
        </span>
      </div>

      {t.standing ? (
        <div className="space-y-1.5 text-sm">
          <p className="line-clamp-2 text-muted-foreground">{t.standing.summary}</p>
          {t.standing.next && (
            <p className="flex gap-1.5 text-foreground/85">
              <ArrowRight className="mt-0.5 h-3.5 w-3.5 shrink-0 text-primary" />
              <span className="line-clamp-1">{t.standing.next}</span>
            </p>
          )}
          {t.standing.blocked && (
            <p className="flex gap-1.5 text-warning">
              <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
              <span className="line-clamp-1">{t.standing.blocked}</span>
            </p>
          )}
        </div>
      ) : (
        <p className="text-sm text-muted-foreground">{t.session_count === 0 ? "Not started yet." : "—"}</p>
      )}

      {highlight === "waiting" && (
        <p className="flex items-center gap-1.5 text-xs text-primary">
          <Send className="h-3.5 w-3.5" /> Sent here{t.moved_tool ? ` to continue in ${toolLabel(t.moved_tool)}` : ""}
        </p>
      )}

      <div className="mt-auto flex flex-wrap items-center gap-1.5">
        <LiveBadge live={t.live} />
        <StatusBadge status={t.status} />
        <span className="flex items-center gap-1">
          {t.tools.map((x) => (
            <ToolBadge key={x} tool={x} compact />
          ))}
        </span>
        <DeviceBadge devices={spans} />
        {branch && (
          <span className="inline-flex max-w-[12rem] items-center gap-1 truncate font-mono text-[11px] text-muted-foreground">
            <GitBranch className="h-3 w-3 shrink-0" />
            {branch}
          </span>
        )}
        {t.session_count > 1 && <span className="text-xs text-muted-foreground">{t.session_count} sessions</span>}
        {t.standing?.by_model && <Sparkles className="h-3 w-3 text-muted-foreground" aria-label="Summarised by your model" />}
        {!readOnly && (
          <ContinueMenu
            className="ml-auto opacity-100 transition md:opacity-0 md:group-hover:opacity-100 md:group-focus-within:opacity-100"
            thread={t}
            devices={devices}
            localDevice={localDevice}
            onDone={(m) => {
              onToast(m);
              onChanged();
            }}
            onError={onError}
          />
        )}
      </div>
    </Card>
  );
}
