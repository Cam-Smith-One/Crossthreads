import { useState } from "react";
import { CheckCircle2, Circle, CircleDashed, CircleSlash, GitBranch, Loader2, OctagonAlert, Play, Users } from "lucide-react";
import { answerAgentRequest, updateRoomTask, type RoomTask, type TaskStatus } from "@/api";
import { Button } from "@/components/ui/button";
import { NativeSelect } from "@/components/ui/select";
import { cn } from "@/lib/utils";

const ICON: Record<TaskStatus, React.ReactNode> = {
  todo: <Circle className="text-muted-foreground" />,
  doing: <Loader2 className="animate-spin text-primary" />,
  blocked: <OctagonAlert className="text-warning" />,
  done: <CheckCircle2 className="text-success" />,
  cancelled: <CircleSlash className="text-muted-foreground" />,
};

const TOOL_NAME: Record<string, string> = {
  "claude-code": "Claude Code",
  codex: "Codex",
  "grok-build": "Grok Build",
};

/**
 * A room's crew: who leads, which agent has which task (on which computer),
 * and how each is going. You can reassign a task or change its status.
 */
export function CrewBoard({
  room,
  tasks,
  asksHere = [],
  agents,
  canEdit,
  onChanged,
  onError,
}: {
  room: string;
  tasks: RoomTask[];
  asksHere?: string[];
  agents: string[];
  canEdit: boolean;
  onChanged: () => void;
  onError: (m: string) => void;
}) {
  const [open, setOpen] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  if (tasks.length === 0) return null;
  const done = tasks.filter((t) => t.status === "done").length;
  const leads = [...new Set(tasks.map((t) => t.created_by))];
  const owners = [...new Set([...agents, ...tasks.map((t) => t.owner).filter(Boolean)])].sort();

  async function change(t: RoomTask, c: { status?: TaskStatus; owner?: string }) {
    setBusy(true);
    try {
      await updateRoomTask(room, t.id, c);
      onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function answer(t: RoomTask, allow: boolean) {
    setBusy(true);
    try {
      await answerAgentRequest(t.id, allow);
      onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const asks = tasks.filter((t) => asksHere.includes(t.id));

  return (
    <div className="space-y-2 rounded-lg border bg-muted/30 p-3">
      {asks.map((t) => (
        <div key={`ask-${t.id}`} className="space-y-2 rounded-md border border-primary/40 bg-primary/5 p-3 text-sm">
          <div className="flex items-center gap-2 font-medium">
            <Play className="h-4 w-4 text-primary" />
            {t.created_by} wants to start {TOOL_NAME[t.launch_tool ?? ""] ?? t.launch_tool} on this computer
          </div>
          <p className="text-xs text-muted-foreground">
            For “{t.title}” in <span className="font-mono">{room}</span>. It opens in a terminal, joins this room and
            takes the task. Nothing starts unless you allow it.
          </p>
          <div className="flex gap-2">
            <Button size="sm" disabled={busy} onClick={() => answer(t, true)}>
              Allow and start
            </Button>
            <Button size="sm" variant="outline" disabled={busy} onClick={() => answer(t, false)}>
              Decline
            </Button>
          </div>
        </div>
      ))}
      <div className="flex items-center gap-2 text-xs">
        <Users className="h-3.5 w-3.5 text-muted-foreground" />
        <span className="font-medium">Crew</span>
        <span className="text-muted-foreground">
          led by {leads.join(", ")} · {done}/{tasks.length} done
        </span>
        <div className="ml-auto h-1.5 w-24 overflow-hidden rounded-full bg-muted">
          <div className="h-full bg-success transition-all" style={{ width: `${(done / tasks.length) * 100}%` }} />
        </div>
      </div>
      <ul className="space-y-1">
        {tasks.map((t) => (
          <li key={t.id} className="rounded-md bg-background/60">
            <div
              role="button"
              tabIndex={0}
              onClick={() => setOpen(open === t.id ? null : t.id)}
              onKeyDown={(e) => e.key === "Enter" && setOpen(open === t.id ? null : t.id)}
              className="flex cursor-pointer items-center gap-2 px-2 py-1.5 text-sm [&_svg]:h-4 [&_svg]:w-4 [&_svg]:shrink-0"
            >
              {ICON[t.status] ?? <CircleDashed />}
              <span className={cn("min-w-0 flex-1 truncate", t.status === "cancelled" && "line-through text-muted-foreground")}>
                {t.title}
              </span>
              {t.branch && (
                <span className="hidden shrink-0 items-center gap-1 font-mono text-[11px] text-muted-foreground sm:inline-flex">
                  <GitBranch className="!h-3 !w-3" />
                  {t.branch}
                </span>
              )}
              {t.launch_state === "requested" && (
                <span className="shrink-0 text-[11px] text-warning">waiting for approval</span>
              )}
              <span className="shrink-0 rounded-full border px-2 py-0.5 text-[11px] text-muted-foreground">
                {t.owner || "unassigned"}
              </span>
            </div>
            {open === t.id && (
              <div className="space-y-2 border-t px-3 py-2 text-xs">
                {t.brief && <p className="whitespace-pre-wrap text-muted-foreground">{t.brief}</p>}
                {t.files.length > 0 && (
                  <p className="font-mono text-[11px] text-muted-foreground">{t.files.join("  ")}</p>
                )}
                {t.branch && (
                  <p className="text-muted-foreground">
                    Branch <span className="font-mono">{t.branch}</span>
                    {t.worktree
                      ? <> in <span className="font-mono">{t.worktree}</span>{t.worktree_device ? ` on ${t.worktree_device}` : ""}</>
                      : " (worktree removed; ready to merge)"}
                  </p>
                )}
                {t.launch_state && t.launch_state !== "requested" && (
                  <p className="text-muted-foreground">
                    {TOOL_NAME[t.launch_tool ?? ""] ?? t.launch_tool} on {t.launch_device}: {t.launch_state}
                  </p>
                )}
                {t.result && (
                  <p className="whitespace-pre-wrap">
                    <span className="font-medium">Result: </span>
                    {t.result}
                  </p>
                )}
                {canEdit && (
                  <div className="flex flex-wrap items-center gap-2">
                    <NativeSelect
                      className="h-7 text-xs"
                      value={t.status}
                      disabled={busy}
                      onChange={(e) => change(t, { status: e.target.value as TaskStatus })}
                      aria-label="Status"
                    >
                      {(["todo", "doing", "blocked", "done", "cancelled"] as TaskStatus[]).map((s) => (
                        <option key={s} value={s}>
                          {s}
                        </option>
                      ))}
                    </NativeSelect>
                    <NativeSelect
                      className="h-7 text-xs"
                      value={t.owner}
                      disabled={busy}
                      onChange={(e) => change(t, { owner: e.target.value })}
                      aria-label="Assigned to"
                    >
                      <option value="">unassigned</option>
                      {owners.map((o) => (
                        <option key={o} value={o}>
                          {o}
                        </option>
                      ))}
                    </NativeSelect>
                    <span className="text-muted-foreground">{t.id} · planned by {t.created_by}{t.device ? ` on ${t.device}` : ""}</span>
                  </div>
                )}
              </div>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
