import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  ArrowRight,
  Check,
  GitBranch,
  Laptop,
  Link2,
  Pencil,
  RefreshCw,
  Send,
  Sparkles,
  Trash2,
  Unlink,
  X,
} from "lucide-react";
import {
  addNote,
  editNote,
  getThread,
  joinThread,
  linkSessions,
  moveThread,
  rejectLink,
  setThread,
  splitThread,
  summarizeThread,
  unlinkSession,
  type Device,
  type NoteKind,
  type ThreadDetail,
  type ThreadStatus,
  type ThreadSummary,
} from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Sheet, SheetContent, SheetDescription, SheetTitle } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { ContinueMenu } from "@/components/ContinueMenu";
import { DeviceBadge, LiveBadge, ToolBadge } from "@/components/badges";
import { ago, cn } from "@/lib/utils";
import { toolLabel } from "@/tools";

const STATUSES: ThreadStatus[] = ["active", "parked", "done"];

/** One thread, in full: where it stands, how to continue, notes, sessions. */
export function ThreadSheet({
  target,
  threads,
  devices,
  localDevice,
  readOnly,
  onClose,
  onChanged,
  onOpenSession,
  onToast,
  onError,
}: {
  /** The thread to show (from the list), or null when closed. */
  target: ThreadSummary | { id: string; device?: string | null } | null;
  threads: ThreadSummary[];
  devices: Device[];
  localDevice?: string;
  readOnly: boolean;
  onClose: () => void;
  onChanged: () => void;
  onOpenSession: (conversationId: string, device?: string | null) => void;
  onToast: (message: string) => void;
  onError: (message: string) => void;
}) {
  const [t, setT] = useState<ThreadDetail | null>(null);
  const [busy, setBusy] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [noteKind, setNoteKind] = useState<NoteKind>("decision");
  const [noteText, setNoteText] = useState("");

  const related = target && "related" in target ? (target.related ?? []) : [];
  const key = target ? `${target.device ?? ""}|${target.id}` : "";
  useEffect(() => {
    setT(null);
    setRenaming(null);
    if (!target) return;
    getThread(target.id, target.device ?? null)
      .then((d) => setT(d && { ...d, related }))
      .catch((e) => onError(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  async function reload(id = t?.id, device = t?.device) {
    if (!id) return;
    const d = await getThread(id, device ?? null);
    setT(d && { ...d, related: t?.related ?? [] });
  }

  /** Run a change, then reload this thread (or the one `anchor` lands in). */
  async function change(action: () => Promise<unknown>, anchor?: string) {
    setBusy(true);
    try {
      await action();
      onChanged();
      if (anchor) {
        const d = await getThread(anchor);
        setT(d);
      } else await reload();
    } catch (e) {
      onError(String(e).replace(/^Error:\s*/, ""));
    } finally {
      setBusy(false);
    }
  }

  const isRemote = (d?: string | null) => !!d && !!localDevice && d !== localDevice;
  const latest = t?.sessions.length ? t.sessions[t.sessions.length - 1] : undefined;
  const notes = t?.notes ?? [];
  const todos = notes.filter((n) => n.kind === "todo" || n.kind === "done");
  const decisions = notes.filter((n) => n.kind === "decision");
  const context = notes.filter((n) => n.kind === "context");
  const mergeable = useMemo(
    () =>
      t
        ? threads
            .filter((x) => x.id !== t.id && (x.repo_key ?? null) === (t.repo_key ?? null))
            .slice(0, 40)
        : [],
    [threads, t],
  );

  async function mergeWith(keyStr: string) {
    if (!t || !keyStr) return;
    const other = threads.find((x) => `${x.device ?? ""}|${x.id}` === keyStr);
    if (!other) return;
    await change(() => joinThread(other.id, other.device, t.id, t.device));
  }

  return (
    <Sheet open={!!target} onOpenChange={(o) => !o && onClose()}>
      <SheetContent className="sm:max-w-2xl">
        {!t ? (
          <div className="space-y-4 p-6">
            <SheetTitle className="sr-only">Loading thread</SheetTitle>
            <Skeleton className="h-7 w-2/3" />
            <Skeleton className="h-4 w-1/3" />
            <Skeleton className="h-28 w-full" />
            <Skeleton className="h-40 w-full" />
          </div>
        ) : (
          <div className="flex h-full flex-col">
            {/* Header */}
            <div className="space-y-3 border-b p-6 pb-4 pr-12">
              {renaming === null ? (
                <div className="flex items-start gap-2">
                  <SheetTitle className="text-xl leading-snug">{t.title}</SheetTitle>
                  {!readOnly && (
                    <Button variant="ghost" size="icon-sm" className="mt-0.5 shrink-0" onClick={() => setRenaming(t.title)} aria-label="Rename">
                      <Pencil />
                    </Button>
                  )}
                </div>
              ) : (
                <form
                  className="flex gap-2"
                  onSubmit={(e) => {
                    e.preventDefault();
                    const title = renaming;
                    setRenaming(null);
                    change(() => setThread(t.id, { title }, t.device));
                  }}
                >
                  <Input autoFocus value={renaming} onChange={(e) => setRenaming(e.target.value)} placeholder="Name (empty: use the first session's title)" />
                  <Button type="submit" size="icon" aria-label="Save"><Check /></Button>
                  <Button type="button" variant="ghost" size="icon" onClick={() => setRenaming(null)} aria-label="Cancel"><X /></Button>
                </form>
              )}
              <SheetDescription asChild>
                <div className="flex flex-wrap items-center gap-2 text-sm">
                  <span className="font-medium text-foreground/80">{t.repo_label ?? "No repository"}</span>
                  {t.branches.length > 0 && (
                    <span className="inline-flex items-center gap-1 font-mono text-xs">
                      <GitBranch className="h-3.5 w-3.5" />
                      {t.branches[t.branches.length - 1]}
                    </span>
                  )}
                  <span>·</span>
                  <span>
                    {t.session_count} session{t.session_count === 1 ? "" : "s"}
                  </span>
                  {t.last_activity && <span>· {ago(t.last_activity)}</span>}
                  <LiveBadge live={t.live} />
                  <DeviceBadge devices={(t.devices?.length ?? 0) > 1 ? t.devices! : isRemote(t.device) ? [t.device!] : []} />
                </div>
              </SheetDescription>
              <div className="flex flex-wrap items-center gap-3">
                {!readOnly && (
                  <ContinueMenu
                    thread={t}
                    devices={devices}
                    localDevice={localDevice}
                    latestConversation={latest && !isRemote(latest.device) ? latest.conversation_id : null}
                    onDone={(m) => {
                      onToast(m);
                      onChanged();
                    }}
                    onError={onError}
                  />
                )}
                {readOnly ? (
                  <Badge variant="secondary" className="capitalize">{t.status}</Badge>
                ) : (
                  <Tabs value={t.status} onValueChange={(s) => change(() => setThread(t.id, { status: s as ThreadStatus }, t.device))}>
                    <TabsList className="h-8">
                      {STATUSES.map((s) => (
                        <TabsTrigger key={s} value={s} disabled={busy} className="h-6 px-2.5 text-xs capitalize">
                          {s}
                        </TabsTrigger>
                      ))}
                    </TabsList>
                  </Tabs>
                )}
              </div>
              {t.moved_to && (
                <div className="flex items-center justify-between gap-2 rounded-md border border-primary/30 bg-primary/5 px-3 py-2 text-sm">
                  <span className="inline-flex items-center gap-2">
                    <Send className="h-4 w-4 text-primary" />
                    Waiting on <strong>{t.moved_to}</strong>
                    {t.moved_tool ? ` in ${toolLabel(t.moved_tool)}` : ""}
                  </span>
                  {!readOnly && (
                    <Button variant="ghost" size="sm" onClick={() => change(() => moveThread(t.id, t.device, null))}>
                      Clear
                    </Button>
                  )}
                </div>
              )}
            </div>

            <div className="flex-1 space-y-6 overflow-y-auto p-6 scrollbar-thin">
              {/* Where it stands */}
              <section className="space-y-2">
                <div className="flex items-center justify-between">
                  <h3 className="text-sm font-semibold">Where it stands</h3>
                  {!readOnly && !isRemote(t.device) && t.sessions.length > 0 && (
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={busy}
                      onClick={() =>
                        change(async () => {
                          await summarizeThread(t.id);
                          onToast("Summary updated");
                        })
                      }
                    >
                      <RefreshCw /> Update with my model
                    </Button>
                  )}
                </div>
                {t.standing ? (
                  <div className="space-y-2 rounded-lg border bg-muted/40 p-4 text-sm">
                    <p className="leading-relaxed">{t.standing.summary}</p>
                    {t.standing.next && (
                      <p className="flex gap-2 text-foreground/90">
                        <ArrowRight className="mt-0.5 h-4 w-4 shrink-0 text-primary" />
                        <span><span className="font-medium">Next:</span> {t.standing.next}</span>
                      </p>
                    )}
                    {t.standing.blocked && (
                      <p className="flex gap-2 text-warning">
                        <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
                        <span><span className="font-medium">Blocked:</span> {t.standing.blocked}</span>
                      </p>
                    )}
                    <p className="flex items-center gap-1 text-xs text-muted-foreground">
                      {t.standing.by_model ? <><Sparkles className="h-3 w-3" /> Summarised by your model</> : "From the latest session"}
                      {t.standing.last_ask && !t.standing.by_model && <> · last asked: “{t.standing.last_ask}”</>}
                    </p>
                  </div>
                ) : (
                  <p className="text-sm text-muted-foreground">
                    {t.sessions.length === 0
                      ? "No sessions yet. Continue it in Claude Code or Codex to start."
                      : "Nothing to show yet."}
                  </p>
                )}
              </section>

              {/* Notes */}
              <section className="space-y-3">
                <div>
                  <h3 className="text-sm font-semibold">Notes</h3>
                  <p className="text-xs text-muted-foreground">
                    Decisions, todos and context. Your agents read these when they start, in any tool, on any of your computers, and add their own.
                  </p>
                </div>
                {notes.length === 0 && <p className="text-sm text-muted-foreground">No notes yet.</p>}
                {[
                  ["Decisions", decisions],
                  ["Todos", todos],
                  ["Context", context],
                ].map(([label, list]) =>
                  (list as typeof notes).length === 0 ? null : (
                    <div key={label as string} className="space-y-1.5">
                      <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">{label as string}</p>
                      <ul className="space-y-1">
                        {(list as typeof notes).map((n) => (
                          <li key={n.id} className="group flex items-start gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-muted/60">
                            {n.kind === "todo" || n.kind === "done" ? (
                              <input
                                type="checkbox"
                                className="mt-1 h-4 w-4 accent-[hsl(var(--primary))]"
                                checked={n.kind === "done"}
                                disabled={busy || readOnly}
                                onChange={(e) => change(() => editNote(n.id, { kind: e.target.checked ? "done" : "todo" }))}
                                aria-label="Done"
                              />
                            ) : (
                              <span className={cn("mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full", n.kind === "decision" ? "bg-primary" : "bg-muted-foreground/50")} />
                            )}
                            <span className={cn("flex-1 leading-relaxed", n.kind === "done" && "text-muted-foreground line-through")}>{n.text}</span>
                            <span className="shrink-0 pt-0.5 text-xs text-muted-foreground">
                              {n.author || "you"}
                              {n.device ? ` · ${n.device}` : ""}
                            </span>
                            {!readOnly && (
                              <button
                                className="shrink-0 rounded p-0.5 text-muted-foreground opacity-0 transition hover:text-destructive group-hover:opacity-100"
                                onClick={() => change(() => editNote(n.id, { removed: true }))}
                                aria-label="Remove note"
                              >
                                <Trash2 className="h-3.5 w-3.5" />
                              </button>
                            )}
                          </li>
                        ))}
                      </ul>
                    </div>
                  ),
                )}
                {!readOnly && (
                  <form
                    className="flex gap-2"
                    onSubmit={(e) => {
                      e.preventDefault();
                      const text = noteText.trim();
                      if (!text) return;
                      setNoteText("");
                      change(() => addNote(t.id, t.device, noteKind, text));
                    }}
                  >
                    <NativeSelect value={noteKind} onChange={(e) => setNoteKind(e.target.value as NoteKind)} aria-label="Kind of note" className="w-32">
                      <option value="decision">Decision</option>
                      <option value="todo">Todo</option>
                      <option value="context">Note</option>
                    </NativeSelect>
                    <Input value={noteText} onChange={(e) => setNoteText(e.target.value)} placeholder="e.g. Keep the v1 API until the mobile app ships" />
                    <Button type="submit" variant="secondary" disabled={busy || !noteText.trim()}>Add</Button>
                  </form>
                )}
              </section>

              <Separator />

              {/* Sessions */}
              <section className="space-y-3">
                <h3 className="text-sm font-semibold">Sessions</h3>
                {t.sessions.length === 0 && <p className="text-sm text-muted-foreground">None yet.</p>}
                <ol className="relative space-y-3 border-l pl-5">
                  {t.sessions.map((s) => {
                    const remote = isRemote(s.device);
                    const stays = t.sessions.find((x) => x.conversation_id !== s.conversation_id);
                    return (
                      <li key={s.session_key} className="relative">
                        <span className="absolute -left-[25px] top-2 h-2.5 w-2.5 rounded-full border-2 border-background bg-muted-foreground/40" />
                        <div className="rounded-lg border p-3 transition hover:border-foreground/20">
                          <div className="flex flex-wrap items-center gap-2">
                            <ToolBadge tool={s.tool} />
                            {(t.devices?.length ?? 0) > 1 && s.device && (
                              <Badge variant="outline" className="font-normal"><Laptop /> {s.device}</Badge>
                            )}
                            <LiveBadge live={s.live} />
                            <span className="ml-auto text-xs text-muted-foreground">{ago(s.last_activity)}</span>
                          </div>
                          <p className="mt-2 line-clamp-2 text-sm">{s.title ?? "(untitled)"}</p>
                          <div className="mt-2 flex items-center gap-2 text-xs text-muted-foreground">
                            <span>{s.message_count} messages</span>
                            {s.branch && <span className="font-mono">⎇ {s.branch}</span>}
                            <span className="ml-auto flex gap-1">
                              <Button variant="ghost" size="sm" className="h-7" onClick={() => onOpenSession(s.conversation_id, remote ? s.device : null)}>
                                Open
                              </Button>
                              {stays && !readOnly && !remote && (
                                <Button variant="ghost" size="sm" className="h-7" disabled={busy} title="Take this session out of the thread" onClick={() => change(() => unlinkSession(s.conversation_id), stays.conversation_id)}>
                                  <Unlink /> Remove
                                </Button>
                              )}
                            </span>
                          </div>
                        </div>
                      </li>
                    );
                  })}
                </ol>
              </section>

              {/* Other computers, suggestions, merging */}
              {!readOnly && (t.parts?.length ?? 0) > 0 && (
                <section className="space-y-2">
                  <h3 className="text-sm font-semibold">On other computers</h3>
                  {t.parts!.map((p) => (
                    <div key={`${p.device}|${p.id}`} className="flex items-center gap-2 rounded-md border px-3 py-2 text-sm">
                      <Badge variant="outline" className="font-normal"><Laptop /> {p.device}</Badge>
                      <span className="flex-1 truncate">{p.title}</span>
                      <Button variant="ghost" size="sm" disabled={busy} onClick={() => change(() => splitThread(p.id, p.device))}>
                        Separate
                      </Button>
                    </div>
                  ))}
                </section>
              )}
              {!readOnly && (t.related?.length ?? 0) > 0 && (
                <section className="space-y-2 rounded-lg border border-dashed p-4">
                  <h3 className="text-sm font-semibold">Same work on another computer?</h3>
                  <p className="text-xs text-muted-foreground">Same repository and branch, active around the same time.</p>
                  {t.related!.map((r) => (
                    <div key={`${r.device}|${r.id}`} className="flex items-center gap-2 text-sm">
                      <Badge variant="outline" className="font-normal"><Laptop /> {r.device}</Badge>
                      <span className="flex-1 truncate">{r.title}</span>
                      <Button size="sm" variant="secondary" disabled={busy} onClick={() => change(() => joinThread(r.id, r.device, t.id, t.device))}>
                        <Link2 /> Join
                      </Button>
                    </div>
                  ))}
                </section>
              )}
              {!readOnly && t.suggestions.length > 0 && (
                <section className="space-y-2 rounded-lg border border-dashed p-4">
                  <h3 className="text-sm font-semibold">Possibly the same work</h3>
                  <p className="text-xs text-muted-foreground">Started in another tool around the same time. Not linked until you say so.</p>
                  {t.suggestions.map((sg) => (
                    <div key={sg.session.session_key} className="flex flex-wrap items-center gap-2 text-sm">
                      <ToolBadge tool={sg.session.tool} />
                      <button className="flex-1 truncate text-left text-primary hover:underline" onClick={() => onOpenSession(sg.session.conversation_id)}>
                        {sg.session.title ?? "(untitled)"}
                      </button>
                      <Button size="sm" variant="secondary" disabled={busy} onClick={() => change(() => linkSessions(sg.with, sg.session.conversation_id, sg.kind), sg.with)}>
                        Add
                      </Button>
                      <Button size="sm" variant="ghost" disabled={busy} onClick={() => change(() => rejectLink(sg.with, sg.session.conversation_id), sg.with)}>
                        Not related
                      </Button>
                    </div>
                  ))}
                </section>
              )}
              {!readOnly && mergeable.length > 0 && (
                <section className="space-y-2">
                  <h3 className="text-sm font-semibold">Merge another thread into this one</h3>
                  <NativeSelect className="w-full" value="" disabled={busy} onChange={(e) => mergeWith(e.target.value)}>
                    <option value="">Choose a thread in {t.repo_label ?? "no repository"}…</option>
                    {mergeable.map((x) => (
                      <option key={`${x.device ?? ""}|${x.id}`} value={`${x.device ?? ""}|${x.id}`}>
                        {ago(x.last_activity)} · {x.tools.map(toolLabel).join(" + ")}
                        {isRemote(x.device) ? ` · on ${x.device}` : ""} · {x.title}
                      </option>
                    ))}
                  </NativeSelect>
                </section>
              )}
            </div>
          </div>
        )}
      </SheetContent>
    </Sheet>
  );
}
