// "Get started": what Crossthreads can do on this machine, what's already set
// up, and the one command or click that turns on each remaining piece.

import { useState } from "react";
import type { SetupStatus } from "./api";

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
  action?: { label: string; run: () => void };
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
  const mcpCommands = [
    ...(s.claude_present || !s.codex_present
      ? ['claude mcp add crossthreads -s user -- "$(command -v ct-mcp)"']
      : []),
    ...(s.codex_present
      ? [
          `# Codex: add to ~/.codex/config.toml\n[mcp_servers.crossthreads]\ncommand = "<output of: command -v ct-mcp>"`,
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
        commands: ["crossthreads hooks install"],
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
        commands: ["crossthreads skill install"],
      },
    ],
    optional: [
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
        {step.action && (!step.done || step.id === "index") && (
          <button className="secondary" onClick={step.action.run}>
            {step.action.label}
          </button>
        )}
        {step.note && <p className="setup-note">{step.note}</p>}
      </div>
    </li>
  );
}

/** Settings → Get started. */
export function SetupChecklist({
  setup,
  sessions,
  reindexing,
  onReindex,
  go,
}: {
  setup: SetupStatus | null;
  sessions: number;
  reindexing: boolean;
  onReindex: () => void;
  go: Go;
}) {
  if (!setup) return <p className="settings-hint">Checking what's set up…</p>;
  const { essentials, optional } = setupSteps(setup, sessions, onReindex, go);
  const done = essentials.filter((s) => s.done).length;
  return (
    <div className="setup">
      <section className="settings-section">
        <h3>
          Essentials <span className="setup-progress">{done} of {essentials.length} done</span>
        </h3>
        <p className="settings-hint">
          Run these once in a terminal on this computer. This list checks again each time you open
          it.
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
