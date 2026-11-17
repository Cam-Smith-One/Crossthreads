// Settings: Get started (what's set up on this computer and the one click or
// command that turns on the rest), Models, Index and Help. Also the slim
// set-up banner the Home page shows while essentials are missing.

import * as React from "react";
import {
  AlertTriangle,
  ArrowRight,
  BookOpen,
  Bot,
  Bug,
  Check,
  Database,
  ExternalLink,
  KeyRound,
  Layers,
  Loader2,
  Network,
  RefreshCw,
  Rocket,
  X,
} from "lucide-react";
import {
  getLlmConfig,
  getSetupStatus,
  getStatus,
  reindex,
  setAgentContext,
  setAutoStanding,
  setLlmKey,
  setLlmProvider,
  type LlmConfig,
  type Reindexed,
  type SetupStatus,
  type Status,
} from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { CommandLine, Kbd, Notice, PageHeader, Segmented, ToolChip, useLatest } from "@/components/common";
import { cn } from "@/lib/utils";

// --- The desktop bridge --------------------------------------------------------

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

/** The desktop app's bridge, when running inside it. */
function desktopInvoke(): Invoke | null {
  const t = (window as unknown as { __TAURI__?: { core?: { invoke?: Invoke } } }).__TAURI__;
  return t?.core?.invoke ?? null;
}

/** Inside the desktop app: where its bundled ct-mcp lives, and a way to run a
 * setup step with its bundled CLI. */
export interface Desktop {
  ctMcp: string | null;
  run: (step: string) => Promise<string>;
}

export function useDesktop(): Desktop | null {
  const invoke = desktopInvoke();
  const [ctMcp, setCtMcp] = React.useState<string | null>(null);
  React.useEffect(() => {
    if (!invoke) return;
    invoke("app_paths")
      .then((p) => setCtMcp((p as { ct_mcp?: string | null }).ct_mcp ?? null))
      .catch(() => {});
  }, [invoke]);
  if (!invoke) return null;
  return { ctMcp, run: (step) => invoke("setup_step", { step }) as Promise<string> };
}

// --- The set-up steps ----------------------------------------------------------

type Go = (page: "settings" | "devices", tab?: string) => void;

export interface Step {
  id: string;
  title: string;
  why: string;
  done: boolean;
  /** Per-tool state, shown when a step applies to Claude Code and Codex. */
  perTool?: [string, boolean][];
  /** Commands to run in a terminal. */
  commands?: string[];
  /** Or a place in the app. */
  action?: { label: string; run: () => void | Promise<unknown> };
  note?: string;
}

/** The essential steps and the optional ones, from what the daemon found. */
export function setupSteps(
  s: SetupStatus,
  sessions: number,
  reindexNow: () => void,
  go: Go,
  desktop: Desktop | null = null,
  done: () => void = () => {},
): { essentials: Step[]; optional: Step[] } {
  const tools = (claude: boolean, codex: boolean): [string, boolean][] => {
    const out: [string, boolean][] = [];
    if (s.claude_present || !s.codex_present) out.push(["Claude Code", claude]);
    if (s.codex_present) out.push(["Codex", codex]);
    return out;
  };
  const anyOf = (pairs: [string, boolean][]) => pairs.some(([, on]) => on);
  const hooks = tools(s.hooks_claude, s.hooks_codex);
  const mcp = tools(s.mcp_claude, s.mcp_codex);
  const skill = tools(s.skill_claude, s.skill_codex);
  return {
    essentials: [
      {
        id: "index",
        title: "Find your sessions",
        why: "Crossthreads reads the history Claude Code, Codex, Cursor and six other tools already keep, and keeps it up to date as you work.",
        done: sessions > 0,
        action: { label: sessions > 0 ? "Index again" : "Index now", run: reindexNow },
        note:
          sessions > 0
            ? `${sessions.toLocaleString()} sessions indexed.`
            : "Nothing found yet. Use one of your AI coding tools, then index again.",
      },
      {
        id: "connect",
        title: "Connect your AI tools",
        why: "Your agents get Crossthreads: they see what's running, read a thread's decisions and todos before they start, leave notes for the next agent, search your past work, and talk to each other in rooms.",
        done: anyOf(hooks) && anyOf(mcp) && anyOf(skill),
        perTool: tools(
          s.hooks_claude && s.mcp_claude && s.skill_claude,
          s.hooks_codex && s.mcp_codex && s.skill_codex,
        ),
        commands: desktop ? undefined : ["crossthreads connect"],
        action: desktop
          ? {
              label: anyOf(hooks) || anyOf(mcp) ? "Connect again" : "Connect Claude Code and Codex",
              run: () =>
                desktop.run("connect").then((out) => {
                  done();
                  return out;
                }),
            }
          : undefined,
        note: "Restart Claude Code and Codex afterwards. Codex asks you to trust the new hooks once.",
      },
    ],
    optional: [
      ...(desktop
        ? [
            {
              id: "cli",
              title: "Use Crossthreads from your terminal",
              why: "Adds the crossthreads, crossthreadsd and ct-mcp commands to ~/.local/bin, so the commands in the docs work as written.",
              done: false,
              action: {
                label: "Add the commands",
                run: () =>
                  desktop.run("cli").then((out) => {
                    done();
                    return out;
                  }),
              },
            },
          ]
        : []),
      {
        id: "devices",
        title: "Add your other computers",
        why: "One search and one set of threads across all your computers, and agents that can message each other. Type a short code on the new computer; it talks to the others over your own Tailscale network.",
        done: s.devices > 0,
        action: { label: "Add a device", run: () => go("devices") },
        note: s.devices > 0 ? `${s.devices} other device${s.devices === 1 ? "" : "s"} connected.` : undefined,
      },
      {
        id: "phone",
        title: "Open your board on your phone",
        why: "Check what's running and what needs you from anywhere on your Tailscale network. Read-only unless you allow more.",
        done: s.phones > 0,
        action: { label: "Add a phone", run: () => go("devices") },
      },
      {
        id: "model",
        title: "Connect a model for insights",
        why: "Ask questions of your history, get a weekly review, and run the analyses. Uses the Claude Code, Codex or Gemini login you already have, or a key you add.",
        done: s.model,
        action: { label: "Open Models", run: () => go("settings", "models") },
      },
    ],
  };
}

/** True when an older tool was refused in the last day. */
export function staleClientRecently(s: SetupStatus): boolean {
  return !!s.keyless_client_at && Date.now() / 1000 - s.keyless_client_at < 24 * 3600;
}

// --- The Home banner -----------------------------------------------------------

/** A slim banner on the Home page while essentials are missing. */
export function SetupNudge({
  setup,
  sessions,
  onOpen,
  onDismiss,
}: {
  setup: SetupStatus;
  sessions: number;
  onOpen: () => void;
  onDismiss: () => void;
}) {
  const { essentials } = setupSteps(setup, sessions, () => {}, () => {});
  const left = essentials.filter((s) => !s.done);
  if (staleClientRecently(setup)) {
    return (
      <div role="status" className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-warning/30 bg-warning/10 px-3.5 py-2.5 text-sm">
        <AlertTriangle className="size-4 shrink-0 text-warning" />
        <p className="min-w-0 flex-1">
          <span className="font-medium">An older Crossthreads tool is being refused.</span>{" "}
          <span className="text-muted-foreground">
            Codex or Claude Code is likely running an old ct-mcp, so their Crossthreads tools fail.
          </span>
        </p>
        <Button size="sm" onClick={onOpen}>
          Fix it <ArrowRight />
        </Button>
      </div>
    );
  }
  if (left.length === 0) return null;
  const doneCount = essentials.length - left.length;
  const next = left[0];
  return (
    <div role="status" className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-primary/25 bg-primary/5 px-3.5 py-2.5 text-sm">
      <Rocket className="size-4 shrink-0 text-primary" />
      <p className="min-w-0 flex-1">
        <span className="font-medium">
          Finish setting up · {doneCount} of {essentials.length} done
        </span>
        <span className="text-muted-foreground"> · Next: {next.title.charAt(0).toLowerCase() + next.title.slice(1)}.</span>
      </p>
      <div className="flex items-center gap-1">
        <Button size="sm" onClick={onOpen}>
          Show me how <ArrowRight />
        </Button>
        <Button size="sm" variant="ghost" onClick={onDismiss}>
          Not now
        </Button>
      </div>
    </div>
  );
}

// --- The page --------------------------------------------------------------------

type Tab = "start" | "models" | "index" | "help";

export function SettingsPage({
  tab,
  onTab,
  go,
  onError,
}: {
  tab: "start" | "models" | "index" | "help";
  onTab: (t: string) => void;
  go: (page: string, sub?: string) => void;
  onError: (msg: string) => void;
}) {
  const [status, setStatus] = React.useState<Status | null>(null);
  const [setup, setSetup] = React.useState<SetupStatus | null>(null);
  const [reindexing, setReindexing] = React.useState(false);
  const [lastIndex, setLastIndex] = React.useState<Reindexed | null>(null);
  const onErrorRef = useLatest(onError);

  const refreshStatus = React.useCallback(() => {
    getStatus().then(setStatus).catch((e) => onErrorRef.current(String(e)));
  }, [onErrorRef]);
  const refreshSetup = React.useCallback(() => {
    getSetupStatus().then(setSetup).catch(() => {});
  }, []);

  React.useEffect(refreshStatus, [refreshStatus]);
  React.useEffect(() => {
    if (tab === "start") refreshSetup();
  }, [tab, refreshSetup]);

  const toolChips = (status?.tools ?? []).filter(([tool, n]) => tool !== "room" && n > 0).sort((a, b) => b[1] - a[1]);
  const sessions = toolChips.reduce((n, [, s]) => n + s, 0);
  const setupLeft = setup ? setupSteps(setup, sessions, () => {}, () => {}).essentials.filter((s) => !s.done).length : 0;

  const runReindex = React.useCallback(async () => {
    setReindexing(true);
    try {
      setLastIndex(await reindex());
      refreshStatus();
      refreshSetup();
    } catch (err) {
      onErrorRef.current(String(err));
    } finally {
      setReindexing(false);
    }
  }, [onErrorRef, refreshStatus, refreshSetup]);

  return (
    <div className="mx-auto w-full max-w-3xl space-y-6 px-4 py-6 sm:px-6 sm:py-8">
      <PageHeader title="Settings" description="Connect your tools, choose a model, and look after your index." />
      <Segmented<Tab>
        label="Settings"
        value={tab}
        onChange={onTab}
        options={[
          { value: "start", label: "Get started", dot: setupLeft > 0 },
          { value: "models", label: "Models" },
          { value: "index", label: "Index" },
          { value: "help", label: "Help" },
        ]}
      />
      {tab === "start" && (
        <GetStarted
          setup={setup}
          sessions={sessions}
          reindexing={reindexing}
          onReindex={runReindex}
          go={go}
          onChanged={refreshSetup}
          onError={onError}
        />
      )}
      {tab === "models" && <Models onError={onError} />}
      {tab === "index" && (
        <IndexTab status={status} sessions={sessions} toolChips={toolChips} reindexing={reindexing} lastIndex={lastIndex} onReindex={runReindex} />
      )}
      {tab === "help" && <Help />}
    </div>
  );
}

// --- Get started -----------------------------------------------------------------

function StepRow({ step, n, busyLabel }: { step: Step; n: number; busyLabel?: string }) {
  const [busy, setBusy] = React.useState(false);
  const [result, setResult] = React.useState<{ ok: boolean; text: string } | null>(null);
  async function act() {
    if (!step.action) return;
    setBusy(true);
    setResult(null);
    try {
      const out = await step.action.run();
      if (typeof out === "string" && out.trim()) setResult({ ok: true, text: out.trim() });
    } catch (e) {
      setResult({ ok: false, text: String(e) });
    } finally {
      setBusy(false);
    }
  }
  const working = busy || !!busyLabel;
  const showAction = step.action && (!step.done || ["index", "cli", "connect"].includes(step.id));
  return (
    <li className="flex gap-3.5 px-4 py-4 sm:px-5">
      <span
        aria-label={step.done ? "Done" : `Step ${n}, not done`}
        className={cn(
          "mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-full border text-xs font-medium tabular-nums",
          step.done ? "border-success bg-success text-white" : "border-input text-muted-foreground",
        )}
      >
        {step.done ? <Check className="size-3.5" strokeWidth={3} /> : n}
      </span>
      <div className="min-w-0 flex-1 space-y-2">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <h3 className={cn("text-sm font-medium", step.done && "text-muted-foreground")}>{step.title}</h3>
          {step.perTool?.map(([tool, on]) => (
            <Badge key={tool} variant={on ? "success" : "outline"} className="font-normal">
              {on ? <Check /> : <X />} {tool}
            </Badge>
          ))}
        </div>
        <p className="text-[13px] leading-relaxed text-muted-foreground">{step.why}</p>
        {!step.done && step.commands?.map((c) => <CommandLine key={c} cmd={c} />)}
        {(showAction || step.note) && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2 pt-0.5">
            {showAction && (
              <Button size="sm" variant={step.done ? "outline" : "default"} onClick={act} disabled={working}>
                {working && <Loader2 className="animate-spin" />}
                {working ? "Working…" : step.action!.label}
              </Button>
            )}
            {(busyLabel || step.note) && <p className="text-xs text-muted-foreground">{busyLabel ?? step.note}</p>}
          </div>
        )}
        {result && (
          <pre
            className={cn(
              "max-h-48 overflow-auto whitespace-pre-wrap rounded-md border p-2.5 font-mono text-xs leading-relaxed scrollbar-thin",
              result.ok ? "bg-muted/50" : "border-destructive/30 bg-destructive/10 text-destructive",
            )}
          >
            {result.text}
          </pre>
        )}
      </div>
    </li>
  );
}

/** An older ct-mcp or CLI keeps connecting without the local key: say what that
 * is and how to fix it. */
function StaleClientWarning({ setup, ctMcp }: { setup: SetupStatus; ctMcp: string | null }) {
  if (!staleClientRecently(setup)) return null;
  const when = new Date((setup.keyless_client_at ?? 0) * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return (
    <Card className="border-warning/40 bg-warning/5 shadow-none">
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center gap-2 text-sm">
          <AlertTriangle className="size-4 text-warning" /> An older Crossthreads tool is still connecting
        </CardTitle>
        <CardDescription className="text-[13px] leading-relaxed">
          At {when}, something connected without this computer's key and was refused. It is almost always Codex or Claude
          Code still running an older <code className="font-mono text-foreground">ct-mcp</code>, so its Crossthreads tools
          (search, threads, rooms) fail. Quit and reopen Codex and Claude Code. If it happens again, use{" "}
          <strong className="text-foreground">Connect again</strong> below (or run{" "}
          <code className="font-mono text-foreground">crossthreads connect</code>), which points them at this version, or do
          it by hand:
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-2">
        <CommandLine
          cmd={`claude mcp remove crossthreads -s user; claude mcp add crossthreads -s user -- ${
            ctMcp ? `"${ctMcp}"` : "$(command -v ct-mcp)"
          }`}
        />
        <CommandLine
          cmd={`# Codex: in ~/.codex/config.toml\n[mcp_servers.crossthreads]\ncommand = ${
            ctMcp ? `"${ctMcp}"` : '"<output of: command -v ct-mcp>"'
          }`}
        />
        <p className="text-xs text-muted-foreground">
          Check which version a tool uses with <code className="font-mono">ct-mcp --version</code> (0.22.2 or later
          understands it).
        </p>
      </CardContent>
    </Card>
  );
}

function GetStarted({
  setup,
  sessions,
  reindexing,
  onReindex,
  go,
  onChanged,
  onError,
}: {
  setup: SetupStatus | null;
  sessions: number;
  reindexing: boolean;
  onReindex: () => void;
  go: (page: string, sub?: string) => void;
  onChanged: () => void;
  onError: (msg: string) => void;
}) {
  const desktop = useDesktop();
  // Switches answer at once; the daemon catches up.
  const [agentContext, setAgentCtx] = React.useState<boolean | null>(null);
  const [autoStanding, setAutoStand] = React.useState<boolean | null>(null);
  React.useEffect(() => {
    if (!setup) return;
    setAgentCtx(setup.agent_context ?? true);
    setAutoStand(setup.auto_standing ?? true);
  }, [setup]);

  if (!setup)
    return (
      <div className="space-y-3" aria-busy="true">
        <Skeleton className="h-6 w-40" />
        <Skeleton className="h-40" />
        <Skeleton className="h-28" />
      </div>
    );

  const { essentials, optional } = setupSteps(setup, sessions, onReindex, go as Go, desktop, onChanged);
  const done = essentials.filter((s) => s.done).length;

  async function flip(which: "context" | "standing", on: boolean) {
    const set = which === "context" ? setAgentCtx : setAutoStand;
    set(on);
    try {
      await (which === "context" ? setAgentContext(on) : setAutoStanding(on));
      onChanged();
    } catch (err) {
      set(!on);
      onError(String(err));
    }
  }

  return (
    <div className="space-y-6">
      <StaleClientWarning setup={setup} ctMcp={desktop?.ctMcp ?? null} />

      <Card className="overflow-hidden shadow-none">
        <CardHeader className="border-b pb-4">
          <div className="flex items-center justify-between gap-3">
            <CardTitle className="text-sm">Essentials</CardTitle>
            <span className="text-xs tabular-nums text-muted-foreground">
              {done} of {essentials.length} done
            </span>
          </div>
          <div className="h-1 overflow-hidden rounded-full bg-muted">
            <div className="h-full rounded-full bg-success transition-all" style={{ width: `${(done / essentials.length) * 100}%` }} />
          </div>
          <CardDescription className="pt-1 text-[13px]">
            {desktop
              ? "Each takes one click. This list checks again each time you open it."
              : "Run these once in a terminal on this computer. This list checks again each time you open it."}
          </CardDescription>
        </CardHeader>
        <ol className="divide-y">
          {essentials.map((s, i) => (
            <StepRow key={s.id} step={s} n={i + 1} busyLabel={reindexing && s.id === "index" ? "Indexing…" : undefined} />
          ))}
        </ol>
      </Card>

      <Card className="shadow-none">
        <CardHeader className="pb-2">
          <CardTitle className="flex items-center gap-2 text-sm">
            <Bot className="size-4 text-muted-foreground" /> Agents
          </CardTitle>
        </CardHeader>
        <CardContent className="divide-y p-0">
          <SwitchRow
            id="agent-context"
            label="Brief agents when a session starts"
            checked={agentContext ?? true}
            onChange={(on) => flip("context", on)}
          >
            When Claude Code starts in a folder whose thread has notes, or work in another tool or on another computer, it
            gets a short summary: decisions, open todos, and who else is working on it. Agents can always ask for it with{" "}
            <code className="font-mono text-foreground">crossthreads_thread_context</code>.
          </SwitchRow>
          <SwitchRow
            id="auto-standing"
            label="Write thread summaries with my model"
            checked={autoStanding ?? true}
            onChange={(on) => flip("standing", on)}
          >
            When a session goes quiet, your model writes where its thread stands: what's done, the next step, and what's blocking it. Without this, the board reads it from the latest session.
            {!setup.model && (
              <>
                {" "}
                <button type="button" className="font-medium text-primary hover:underline" onClick={() => go("settings", "models")}>
                  Connect a model
                </button>{" "}
                first.
              </>
            )}
          </SwitchRow>
        </CardContent>
      </Card>

      <section className="space-y-3">
        <div>
          <h2 className="text-sm font-semibold">When you want more</h2>
          <p className="text-[13px] text-muted-foreground">Optional, and you can come back to them any time.</p>
        </div>
        <Card className="overflow-hidden shadow-none">
          <ol className="divide-y">
            {optional.map((s, i) => (
              <StepRow key={s.id} step={s} n={i + 1} />
            ))}
          </ol>
        </Card>
      </section>
    </div>
  );
}

function SwitchRow({
  id,
  label,
  checked,
  onChange,
  children,
}: {
  id: string;
  label: string;
  checked: boolean;
  onChange: (on: boolean) => void;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-start justify-between gap-4 px-5 py-4">
      <div className="min-w-0 space-y-1">
        <Label htmlFor={id} className="cursor-pointer text-sm">
          {label}
        </Label>
        <p id={`${id}-desc`} className="text-[13px] leading-relaxed text-muted-foreground">
          {children}
        </p>
      </div>
      <Switch id={id} checked={checked} onCheckedChange={onChange} aria-describedby={`${id}-desc`} className="mt-0.5" />
    </div>
  );
}

// --- Models ------------------------------------------------------------------------

function Models({ onError }: { onError: (msg: string) => void }) {
  const [config, setConfig] = React.useState<LlmConfig | null>(null);
  const [keys, setKeys] = React.useState<Record<string, string>>({});
  const [busy, setBusy] = React.useState<string | null>(null);
  const onErrorRef = useLatest(onError);

  React.useEffect(() => {
    getLlmConfig()
      .then(setConfig)
      .catch((e) => onErrorRef.current(String(e)));
  }, [onErrorRef]);

  async function apply(id: string, f: () => Promise<LlmConfig>) {
    setBusy(id);
    try {
      setConfig(await f());
    } catch (err) {
      onError(String(err));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="space-y-6">
      <p className="text-[13px] leading-relaxed text-muted-foreground">
        AI features (Ask, the weekly review, theme names, analyses, thread summaries) reuse your existing Claude Code or Codex
        login when they can. Or paste an API key to bring your own: it's stored in your system keychain and sent only to the
        provider.
      </p>

      {!config && (
        <div className="space-y-3">
          <Skeleton className="h-28" />
          <Skeleton className="h-28" />
        </div>
      )}

      {config && (
        <>
          <Card className="shadow-none">
            <div className="flex flex-col gap-3 p-4 sm:flex-row sm:items-center sm:justify-between sm:p-5">
              <div>
                <Label htmlFor="llm-active" className="text-sm">
                  Preferred provider
                </Label>
                <p className="text-[13px] text-muted-foreground">Which provider AI features use first.</p>
              </div>
              <NativeSelect
                id="llm-active"
                value={config.active ?? ""}
                disabled={busy === "active"}
                onChange={(e) => apply("active", () => setLlmProvider(e.target.value))}
                className="sm:w-56"
              >
                <option value="">Automatic (first available)</option>
                {config.providers.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label}
                  </option>
                ))}
              </NativeSelect>
            </div>
          </Card>

          <div className="space-y-3">
            {config.providers.map((p) => {
              const available = p.methods.length > 0;
              const draft = keys[p.id] ?? "";
              const active = config.active === p.id;
              return (
                <Card key={p.id} className={cn("shadow-none", active && "border-primary/40")}>
                  <div className="space-y-3 p-4 sm:p-5">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <div className="flex items-center gap-2">
                        <h3 className="text-sm font-medium">{p.label}</h3>
                        {active && <Badge variant="info">preferred</Badge>}
                      </div>
                      <Badge variant={available ? "success" : "outline"} className="font-normal">
                        {available ? <Check /> : <X />}
                        {available ? `Available · ${p.methods.join(", ")}` : "No credentials found"}
                      </Badge>
                    </div>
                    <form
                      className="flex flex-col gap-2 sm:flex-row sm:items-center"
                      onSubmit={(e) => {
                        e.preventDefault();
                        if (!draft.trim()) return;
                        apply(p.id, async () => {
                          const c = await setLlmKey(p.id, draft.trim());
                          setKeys((m) => ({ ...m, [p.id]: "" }));
                          return c;
                        });
                      }}
                    >
                      <div className="relative flex-1">
                        <KeyRound className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                        <Input
                          type="password"
                          autoComplete="off"
                          value={draft}
                          onChange={(e) => setKeys((m) => ({ ...m, [p.id]: e.target.value }))}
                          placeholder={p.has_stored_key ? "Key saved in your keychain · paste to replace" : `Paste a ${p.label.split(" ")[0]} API key`}
                          aria-label={`${p.label} API key`}
                          className="pl-8"
                        />
                      </div>
                      <div className="flex gap-2">
                        <Button type="submit" variant="secondary" disabled={!draft.trim() || busy === p.id}>
                          {busy === p.id && <Loader2 className="animate-spin" />}
                          Save key
                        </Button>
                        {p.has_stored_key && (
                          <Button
                            type="button"
                            variant="ghost"
                            disabled={busy === p.id}
                            onClick={() => apply(p.id, () => setLlmKey(p.id, ""))}
                          >
                            Remove key
                          </Button>
                        )}
                      </div>
                    </form>
                  </div>
                </Card>
              );
            })}
          </div>
        </>
      )}
    </div>
  );
}

// --- Index -------------------------------------------------------------------------

function IndexTab({
  status,
  sessions,
  toolChips,
  reindexing,
  lastIndex,
  onReindex,
}: {
  status: Status | null;
  sessions: number;
  toolChips: [string, number, number][];
  reindexing: boolean;
  lastIndex: Reindexed | null;
  onReindex: () => void;
}) {
  return (
    <div className="space-y-6">
      <p className="text-[13px] leading-relaxed text-muted-foreground">
        Crossthreads keeps one local database of the sessions it found. It updates on its own as you work; index again if
        something seems missing.
      </p>
      {!status ? (
        <Skeleton className="h-24" />
      ) : (
        <div className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border bg-border sm:grid-cols-4">
          {(
            [
              ["Sessions", sessions.toLocaleString(), Layers],
              ["Records", status.conversations.toLocaleString(), Database],
              ["Embeddings", status.embeddings.toLocaleString(), Network],
              ["Search model", status.embedder, Bot],
            ] as [string, string, typeof Layers][]
          ).map(([label, value, Icon]) => (
            <div key={label} className="bg-card px-4 py-3">
              <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
                <Icon className="size-3.5" /> {label}
              </p>
              <p className="mt-0.5 truncate text-lg font-semibold tabular-nums" title={value}>
                {value}
              </p>
            </div>
          ))}
        </div>
      )}

      {toolChips.length > 0 && (
        <section className="space-y-2">
          <h2 className="text-sm font-semibold">By tool</h2>
          <Card className="divide-y overflow-hidden shadow-none">
            {toolChips.map(([tool, n, skills]) => (
              <div key={tool} className="flex items-center gap-3 px-4 py-2.5">
                <ToolChip tool={tool} />
                <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-muted">
                  <div className="h-full rounded-full bg-primary/60" style={{ width: `${(n / toolChips[0][1]) * 100}%` }} />
                </div>
                <span className="w-28 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
                  {n.toLocaleString()} session{n === 1 ? "" : "s"}
                  {skills > 0 && ` · ${skills} skill${skills === 1 ? "" : "s"}`}
                </span>
              </div>
            ))}
          </Card>
        </section>
      )}

      <div className="flex flex-wrap items-center gap-3">
        <Button onClick={onReindex} disabled={reindexing}>
          <RefreshCw className={cn(reindexing && "animate-spin")} />
          {reindexing ? "Indexing…" : "Index again"}
        </Button>
        {lastIndex && !reindexing && (
          <p className="text-[13px] text-muted-foreground" aria-live="polite">
            {lastIndex.inserted.toLocaleString()} new · {lastIndex.duplicate.toLocaleString()} unchanged ·{" "}
            {lastIndex.embedded.toLocaleString()} embedded
          </p>
        )}
      </div>
    </div>
  );
}

// --- Help --------------------------------------------------------------------------

// The UI is served from a local origin where relative doc paths don't exist, so link out.
const DOCS_BASE = "https://github.com/Cam-Smith-One/Crossthreads/blob/main/docs/";
const REPO_BASE = "https://github.com/Cam-Smith-One/Crossthreads/blob/main/";
const DOC_LINKS: { title: string; href: string; desc: string; icon: typeof BookOpen }[] = [
  { title: "Getting started", href: `${REPO_BASE}README.md`, desc: "Install, index, and run your first search.", icon: Rocket },
  { title: "Multi-device setup", href: `${DOCS_BASE}MULTI_DEVICE_SETUP.md`, desc: "Connect your other computers for one search and one set of threads.", icon: Network },
  { title: "Troubleshooting", href: `${DOCS_BASE}TROUBLESHOOTING.md`, desc: "Common issues, fixes, and a bug-report checklist.", icon: AlertTriangle },
  { title: "Agents and MCP", href: `${DOCS_BASE}AGENT_API.md`, desc: "Wire Crossthreads into Claude Code, Codex and other agents.", icon: Bot },
  { title: "Architecture", href: `${DOCS_BASE}ARCHITECTURE.md`, desc: "How the daemon, index and connectors fit together.", icon: Layers },
  { title: "Report an issue", href: "https://github.com/Cam-Smith-One/Crossthreads/issues", desc: "Found a bug or missing a tool? Let us know.", icon: Bug },
];

const SHORTCUTS: [React.ReactNode, string][] = [
  [<><Kbd>/</Kbd> or <Kbd>Ctrl</Kbd> <Kbd>K</Kbd></>, "Search from anywhere"],
  [<><Kbd>j</Kbd> <Kbd>k</Kbd> or <Kbd>↑</Kbd> <Kbd>↓</Kbd></>, "Move through search results"],
  [<Kbd>↵</Kbd>, "Open the selected result"],
  [<><Kbd>Ctrl</Kbd> <Kbd>F</Kbd></>, "Find inside an open transcript"],
  [<Kbd>Esc</Kbd>, "Close a transcript or dialog"],
];

function Help() {
  return (
    <div className="space-y-8">
      <section className="space-y-3">
        <div>
          <h2 className="text-sm font-semibold">Guides and documentation</h2>
          <p className="text-[13px] text-muted-foreground">Links open the docs on GitHub.</p>
        </div>
        <div className="grid gap-2.5 sm:grid-cols-2">
          {DOC_LINKS.map((d) => (
            <a
              key={d.href}
              href={d.href}
              target="_blank"
              rel="noreferrer noopener"
              className="group flex items-start gap-3 rounded-lg border bg-card p-3.5 transition-colors hover:border-foreground/20 hover:bg-accent/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <d.icon className="mt-0.5 size-4 shrink-0 text-muted-foreground group-hover:text-primary" />
              <span className="min-w-0 flex-1">
                <span className="flex items-center gap-1 text-sm font-medium">
                  {d.title}
                  <ExternalLink className="size-3 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100" />
                </span>
                <span className="text-[13px] text-muted-foreground">{d.desc}</span>
              </span>
            </a>
          ))}
        </div>
      </section>
      <Separator />
      <section className="space-y-3">
        <h2 className="text-sm font-semibold">Keyboard shortcuts</h2>
        <Card className="divide-y overflow-hidden shadow-none">
          {SHORTCUTS.map(([keys, what], i) => (
            <div key={i} className="flex items-center justify-between gap-4 px-4 py-2.5 text-[13px]">
              <span className="text-muted-foreground">{what}</span>
              <span className="flex shrink-0 items-center gap-1">{keys}</span>
            </div>
          ))}
        </Card>
      </section>
      <Notice tone="info" title="Your data stays on your computers">
        Crossthreads indexes locally and talks only to your own devices over your Tailscale network. A model is used only
        for the features that say so, with the login or key you choose.
      </Notice>
    </div>
  );
}
