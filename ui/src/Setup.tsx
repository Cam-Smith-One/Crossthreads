// "Get started": what Crossthreads can do on this machine, what's already set
// up, and the one command or click that turns on each remaining piece.

import { useEffect, useState } from "react";
import type { SetupStatus } from "./api";

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

/** The desktop app's bridge, when running inside it. */
function desktopInvoke(): Invoke | null {
  const t = (window as unknown as { __TAURI__?: { core?: { invoke?: Invoke } } }).__TAURI__;
  return t?.core?.invoke ?? null;
}

/** Inside the desktop app: where its bundled ct-mcp lives, and a way to run
 * a setup step with its bundled CLI. */
export interface Desktop {
  ctMcp: string | null;
  run: (step: string) => Promise<string>;
}

export function useDesktop(): Desktop | null {
  const invoke = desktopInvoke();
  const [ctMcp, setCtMcp] = useState<string | null>(null);
  useEffect(() => {
    if (!invoke) return;
    invoke("app_paths")
      .then((p) => setCtMcp((p as { ct_mcp?: string | null }).ct_mcp ?? null))
      .catch(() => {});
  }, [invoke]);
  if (!invoke) return null;
  return { ctMcp, run: (step) => invoke("setup_step", { step }) as Promise<string> };
}

type Go = (page: "settings", tab: string) => void;

interface Step {
  id: string;
  title: string;
  why: string;
  done: boolean;
  /** Per-tool state, shown when a step applies to Claude Code and Codex. */
  perTool?: [string, boolean][];
  /** Commands to run in a terminal. */
  commands?: string[];
  /** Or a place in the app. */
  action?: { label: string; run: () => void | Promise<void> };
  note?: string;
}

function CopyCommand({ cmd }: { cmd: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="setup-cmd">
      <code>{cmd}</code>
      <button
        className="secondary"
        onClick={() => {
          navigator.clipboard?.writeText(cmd);
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1500);
        }}
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

/** The essential steps and the optional ones, from what the daemon found. */
export function setupSteps(
  s: SetupStatus,
  sessions: number,
  reindex: () => void,
  go: Go,
  desktop: Desktop | null = null,
  done: () => void = () => {},
): { essentials: Step[]; optional: Step[] } {
  // In the desktop app, the bundled CLI does the work at the click of a button.
  const runner = (step: string) =>
    desktop ? { label: "Set it up for me", run: () => desktop.run(step).then(done) } : undefined;
  const ctMcp = desktop?.ctMcp ? `"${desktop.ctMcp}"` : '"$(command -v ct-mcp)"';
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
  const mcpCommands = [
    ...(s.claude_present || !s.codex_present
      ? [`claude mcp add crossthreads -s user -- ${ctMcp}`]
      : []),
    ...(s.codex_present
      ? [
          `# Codex: add to ~/.codex/config.toml\n[mcp_servers.crossthreads]\ncommand = ${
            desktop?.ctMcp ? ctMcp : '"<output of: command -v ct-mcp>"'
          }`,
        ]
      : []),
  ];
  return {
    essentials: [
      {
        id: "index",
        title: "Find your sessions",
        why: "Crossthreads reads the history Claude Code, Codex, Cursor and six other tools already keep, and keeps it up to date as you work.",
        done: sessions > 0,
        action: { label: sessions > 0 ? "Index again" : "Index now", run: reindex },
        note:
          sessions > 0
            ? `${sessions.toLocaleString()} sessions indexed.`
            : "Nothing found yet. Use one of your AI coding tools, then index again.",
      },
      {
        id: "hooks",
        title: "See what's running",
        why: "Your tools report each session's state, so the board shows what's running and pins what's waiting for you. It also wakes an agent mid-conversation in a room and stops agents editing a file another has claimed.",
        done: anyOf(hooks),
        perTool: hooks,
        commands: desktop ? undefined : ["crossthreads hooks install"],
        action: runner("hooks"),
        note: "Restart Claude Code and Codex afterwards. Codex asks you to trust the new hooks once.",
      },
      {
        id: "mcp",
        title: "Give your agents Crossthreads",
        why: "Your agents can search your past work, read a thread before continuing it, and talk to each other in rooms.",
        done: anyOf(mcp),
        perTool: mcp,
        commands: mcpCommands,
      },
      {
        id: "skill",
        title: "Teach them when to use it",
        why: "A short skill for Claude Code and a /crossthreads prompt for Codex: look for prior work before building, and how to lead or listen in a room.",
        done: anyOf(skill),
        perTool: skill,
        commands: desktop ? undefined : ["crossthreads skill install"],
        action: runner("skill"),
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
              action: { label: "Add the commands", run: () => desktop.run("cli").then(done) },
            },
          ]
        : []),
      {
        id: "devices",
        title: "Add your other computers",
        why: "One search and one board across all your machines, over your own Tailscale network.",
        done: s.devices > 0,
        action: { label: "Open Devices", run: () => go("settings", "devices") },
        note: s.devices > 0 ? `${s.devices} other device(s) connected.` : undefined,
      },
      ...(s.devices > 0
        ? [
            {
              id: "rooms",
              title: "Let agents talk across computers",
              why: "Tick Can message for a device, on both machines, and their agents share your repositories' rooms.",
              done: s.devices_can_message > 0,
              action: { label: "Open Devices", run: () => go("settings", "devices") },
            },
          ]
        : []),
      {
        id: "phone",
        title: "Open your board on your phone",
        why: "Check what's running and what needs you from anywhere on your Tailscale network. Read-only unless you allow more.",
        done: s.phones > 0,
        action: { label: "Open Phone", run: () => go("settings", "phone") },
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

function StepCard({ step, n }: { step: Step; n: number }) {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  async function act() {
    if (!step.action) return;
    setBusy(true);
    setResult(null);
    try {
      const out = await step.action.run();
      if (typeof out === "string") setResult(out);
    } catch (e) {
      setResult(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <li className={`setup-step ${step.done ? "done" : ""}`}>
      <span className="setup-check" aria-label={step.done ? "done" : "not done"}>
        {step.done ? "✓" : n}
      </span>
      <div className="setup-step-body">
        <div className="setup-step-title">
          <strong>{step.title}</strong>
          {step.perTool && step.perTool.length > 0 && (
            <span className="setup-tools">
              {step.perTool.map(([tool, on]) => (
                <span key={tool} className={`setup-tool ${on ? "on" : ""}`}>
                  {on ? "✓" : "–"} {tool}
                </span>
              ))}
            </span>
          )}
        </div>
        <p className="setup-why">{step.why}</p>
        {!step.done && step.commands?.map((c) => <CopyCommand key={c} cmd={c} />)}
        {step.action && (!step.done || step.id === "index" || step.id === "cli") && (
          <button className="secondary" onClick={act} disabled={busy}>
            {busy ? "Working…" : step.action.label}
          </button>
        )}
        {result && <pre className="setup-result">{result.trim()}</pre>}
        {step.note && <p className="setup-note">{step.note}</p>}
      </div>
    </li>
  );
}

/** True when an older tool was refused in the last day. */
export function staleClientRecently(s: SetupStatus): boolean {
  return !!s.keyless_client_at && Date.now() / 1000 - s.keyless_client_at < 24 * 3600;
}

/** An older ct-mcp or CLI keeps connecting without the local key: say what
 * that is and how to fix it. */
function StaleClientWarning({ setup, ctMcp }: { setup: SetupStatus; ctMcp: string | null }) {
  if (!staleClientRecently(setup)) return null;
  const when = new Date((setup.keyless_client_at ?? 0) * 1000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
  const path = ctMcp ?? "$(command -v ct-mcp)";
  return (
    <section className="settings-section stale-client">
      <h3>⚠ An older Crossthreads tool is still connecting</h3>
      <p className="settings-hint">
        At {when}, something connected without this computer's key and was refused. It is almost
        always Codex or Claude Code still running an older <code>ct-mcp</code>, so its Crossthreads
        tools (search, threads, rooms) fail. Quit and reopen Codex and Claude Code. If it happens
        again, point them at this version:
      </p>
      <CopyCommand
        cmd={`claude mcp remove crossthreads -s user; claude mcp add crossthreads -s user -- ${
          ctMcp ? `"${ctMcp}"` : path
        }`}
      />
      <CopyCommand
        cmd={`# Codex: in ~/.codex/config.toml\n[mcp_servers.crossthreads]\ncommand = ${
          ctMcp ? `"${ctMcp}"` : '"<output of: command -v ct-mcp>"'
        }`}
      />
      <p className="setup-note">
        Check which version a tool uses with <code>ct-mcp --version</code> (0.22.2 or later
        understands it).
      </p>
    </section>
  );
}

/** Settings → Get started. */
export function SetupChecklist({
  setup,
  sessions,
  reindexing,
  onReindex,
  go,
  onChanged,
}: {
  setup: SetupStatus | null;
  sessions: number;
  reindexing: boolean;
  onReindex: () => void;
  go: Go;
  /** Re-check after a step ran. */
  onChanged?: () => void;
}) {
  const desktop = useDesktop();
  if (!setup) return <p className="settings-hint">Checking what's set up…</p>;
  const { essentials, optional } = setupSteps(setup, sessions, onReindex, go, desktop, () =>
    onChanged?.(),
  );
  const done = essentials.filter((s) => s.done).length;
  return (
    <div className="setup">
      <StaleClientWarning setup={setup} ctMcp={desktop?.ctMcp ?? null} />
      <section className="settings-section">
        <h3>
          Essentials <span className="setup-progress">{done} of {essentials.length} done</span>
        </h3>
        <p className="settings-hint">
          {desktop
            ? "Each takes one click. This list checks again each time you open it."
            : "Run these once in a terminal on this computer. This list checks again each time you open it."}
        </p>
        <ol className="setup-steps">
          {essentials.map((s, i) => (
            <StepCard key={s.id} step={reindexing && s.id === "index" ? { ...s, note: "Indexing…" } : s} n={i + 1} />
          ))}
        </ol>
      </section>
      <section className="settings-section">
        <h3>When you want more</h3>
        <ol className="setup-steps">
          {optional.map((s, i) => (
            <StepCard key={s.id} step={s} n={i + 1} />
          ))}
        </ol>
      </section>
    </div>
  );
}

/** A one-line nudge on the Threads page while essentials are missing. */
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
      <div className="nudge">
        <div>
          <strong>⚠ An older Crossthreads tool is being refused</strong>
          <span className="nudge-next">
            Codex or Claude Code is likely running an old ct-mcp, so their Crossthreads tools fail.
          </span>
        </div>
        <span className="nudge-actions">
          <button onClick={onOpen}>Fix it</button>
        </span>
      </div>
    );
  }
  if (left.length === 0) return null;
  const next = left[0];
  return (
    <div className="nudge">
      <div>
        <strong>
          Finish setting up · {essentials.length - left.length} of {essentials.length} done
        </strong>
        <span className="nudge-next">Next: {next.title.toLowerCase()}.</span>
      </div>
      <span className="nudge-actions">
        <button onClick={onOpen}>Show me how</button>
        <button className="linklike" onClick={onDismiss}>
          Not now
        </button>
      </span>
    </div>
  );
}
