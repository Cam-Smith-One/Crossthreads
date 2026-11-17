import { useState } from "react";
import { ChevronDown, ClipboardCopy, Play, Send, Sparkles, Terminal } from "lucide-react";
import {
  continueThread,
  getResume,
  getThreadHandoff,
  moveThread,
  type Device,
  type ThreadSummary,
  type ToolId,
} from "@/api";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

const TOOLS: [ToolId, string][] = [
  ["claude-code", "Claude Code"],
  ["codex", "Codex"],
  ["grok-build", "Grok Build"],
];

/** The tool to offer first: the one the thread was last worked in, when it can continue. */
function preferredTool(t: ThreadSummary): ToolId {
  const last = [...t.tools].reverse().find((x) => TOOLS.some(([id]) => id === x));
  return (last as ToolId) ?? "claude-code";
}

/**
 * Continue a thread: one click opens the last tool in a terminal on this
 * computer, briefed (or resuming). The menu has the other tool, a fresh
 * session, the handoff to paste anywhere, and sending it to another device.
 */
export function ContinueMenu({
  thread,
  devices,
  localDevice,
  latestConversation,
  onDone,
  onError,
  size = "sm",
  className,
}: {
  thread: ThreadSummary;
  devices: Device[];
  localDevice?: string;
  /** The latest session's conversation id, for the resume command. */
  latestConversation?: string | null;
  onDone: (message: string) => void;
  onError: (message: string) => void;
  size?: "sm" | "default";
  className?: string;
}) {
  const [busy, setBusy] = useState(false);
  const tool = preferredTool(thread);
  const label = TOOLS.find(([id]) => id === tool)![1];
  const others = devices.filter((d) => !d.local);

  async function run(f: () => Promise<string>) {
    setBusy(true);
    try {
      onDone(await f());
    } catch (e) {
      onError(String(e).replace(/^Error:\s*/, ""));
    } finally {
      setBusy(false);
    }
  }
  const go = (t: ToolId, fresh = false) =>
    run(async () => {
      await continueThread(thread.id, thread.device, t, fresh);
      return `Opened ${TOOLS.find(([id]) => id === t)![1]} in a terminal`;
    });

  return (
    <div className={cn("inline-flex", className)} onClick={(e) => e.stopPropagation()}>
      <Button size={size} className="rounded-r-none" disabled={busy} onClick={() => go(tool)}>
        <Play /> Continue in {label}
      </Button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button size={size} className="rounded-l-none border-l border-primary-foreground/20 px-2" disabled={busy} aria-label="More ways to continue">
            <ChevronDown />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-64">
          <DropdownMenuLabel>On this computer</DropdownMenuLabel>
          {TOOLS.map(([id, name]) => (
            <DropdownMenuItem key={id} onSelect={() => go(id)}>
              <Terminal /> Continue in {name}
            </DropdownMenuItem>
          ))}
          <DropdownMenuItem onSelect={() => go(tool, true)}>
            <Sparkles /> New {label} session, briefed
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            onSelect={() =>
              run(async () => {
                await navigator.clipboard.writeText(await getThreadHandoff(thread.id, thread.device ?? null));
                return "Handoff copied: paste it into any tool or agent";
              })
            }
          >
            <ClipboardCopy /> Copy handoff
          </DropdownMenuItem>
          {latestConversation && (!thread.device || thread.device === localDevice) && (
            <DropdownMenuItem
              onSelect={() =>
                run(async () => {
                  const plan = await getResume(latestConversation);
                  if (plan?.action.kind !== "resume") throw new Error("This session's tool can't be resumed from a terminal");
                  await navigator.clipboard.writeText(plan.action.display);
                  return "Resume command copied";
                })
              }
            >
              <ClipboardCopy /> Copy resume command
            </DropdownMenuItem>
          )}
          {others.length > 0 && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuLabel>Continue on another computer</DropdownMenuLabel>
              {others.map((d) =>
                TOOLS.map(([id, name]) => (
                  <DropdownMenuItem
                    key={`${d.name}-${id}`}
                    onSelect={() =>
                      run(async () => {
                        await moveThread(thread.id, thread.device, d.name, id);
                        return `Sent to ${d.name}: it's waiting at the top of Home there`;
                      })
                    }
                  >
                    <Send /> {d.name} · {name}
                  </DropdownMenuItem>
                )),
              )}
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
