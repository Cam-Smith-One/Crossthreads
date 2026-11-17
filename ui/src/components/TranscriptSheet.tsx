// The transcript viewer: one past session, opened from search, a thread, a
// theme or a cited source. A right-hand sheet on a computer, full screen on a
// phone.

import * as React from "react";
import {
  ChevronDown,
  Download,
  FileSearch,
  FolderOpen,
  GitBranch,
  MoreHorizontal,
  Search,
  Star,
  Terminal,
  Trash2,
  X,
} from "lucide-react";
import {
  forget,
  getConversation,
  getDevices,
  getResume,
  openSource,
  setFlags,
  setNote,
  setTags,
  type ResumePlan,
  type StoredConversation,
} from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input, Textarea } from "@/components/ui/input";
import { Sheet, SheetContent, SheetDescription, SheetTitle } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { CopyButton, useLatest, highlightFind, markClass, matchesFind, Notice, shortPath, ToolChip } from "@/components/common";
import { cn } from "@/lib/utils";

/** Messages longer than this start folded (unfolded while finding). */
const FOLD_AT = 3000;

// Cached across opens: the name of this computer, to tell remote sessions apart.
let localDeviceName: Promise<string | null> | null = null;
function localDevice(): Promise<string | null> {
  if (!localDeviceName)
    localDeviceName = getDevices()
      .then(({ devices }) => devices.find((d) => d.local)?.name ?? null)
      .catch(() => null);
  return localDeviceName;
}

function conversationMarkdown(c: StoredConversation): string {
  const head = `# ${c.title ?? "(untitled)"}\n\n_${c.tool}${c.project ? ` · ${c.project}` : ""}${
    c.started_at ? ` · ${c.started_at.slice(0, 10)}` : ""
  }_\n\n`;
  return head + c.messages.map((m) => `**${m.role}:** ${m.content.trim()}`).join("\n\n");
}

function download(name: string, text: string, type: string) {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  // Defer the revoke so the browser can start the download first.
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

function exportConversation(c: StoredConversation, fmt: "md" | "json") {
  const stem = (c.title ?? c.id).replace(/[^\w.-]+/g, "_").slice(0, 60) || "conversation";
  if (fmt === "md") download(`${stem}.md`, conversationMarkdown(c), "text/markdown");
  else download(`${stem}.json`, JSON.stringify(c, null, 2), "application/json");
}

function roleTone(role: string): "user" | "assistant" | "other" {
  const r = role.toLowerCase();
  if (r === "user" || r === "human") return "user";
  if (r === "assistant" || r === "model" || r === "ai") return "assistant";
  return "other";
}

function Turn({ role, content, find }: { role: string; content: string; find: string }) {
  const tone = roleTone(role);
  const long = content.length > FOLD_AT;
  const [open, setOpen] = React.useState(false);
  const folded = long && !open && !find.trim();
  const html = React.useMemo(
    () => highlightFind(folded ? content.slice(0, FOLD_AT) : content, find),
    [content, find, folded],
  );
  return (
    <div className={cn("group flex flex-col gap-1", tone === "user" && "items-end")}>
      <span
        className={cn(
          "px-1 text-[11px] font-medium uppercase tracking-wide",
          tone === "user" ? "text-primary" : "text-muted-foreground",
        )}
      >
        {role}
      </span>
      <div
        className={cn(
          "max-w-full overflow-hidden rounded-lg border px-3.5 py-2.5",
          tone === "user" && "border-primary/20 bg-primary/[0.07] sm:max-w-[85%]",
          tone === "assistant" && "w-full bg-card",
          tone === "other" && "w-full border-dashed bg-muted/40",
        )}
      >
        <div
          className={cn(
            "whitespace-pre-wrap break-words",
            tone === "other" ? "font-mono text-xs leading-relaxed text-muted-foreground" : "text-[13.5px] leading-relaxed",
            markClass,
          )}
          dangerouslySetInnerHTML={{ __html: html + (folded ? "…" : "") }}
        />
        {long && !find.trim() && (
          <button
            type="button"
            onClick={() => setOpen((v) => !v)}
            className="mt-2 inline-flex items-center gap-1 text-xs font-medium text-primary hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <ChevronDown className={cn("size-3.5 transition-transform", open && "rotate-180")} />
            {open ? "Show less" : `Show all (${Math.round(content.length / 1000)}k characters)`}
          </button>
        )}
      </div>
    </div>
  );
}

export function TranscriptSheet({
  id,
  device,
  onClose,
  onShowThread,
  onError,
}: {
  id: string | null;
  device?: string | null;
  onClose: () => void;
  onShowThread: (conversationId: string) => void;
  onError: (m: string) => void;
}) {
  const [convo, setConvo] = React.useState<StoredConversation | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [missing, setMissing] = React.useState(false);
  // Set when the session lives on another computer: it can't be revealed or resumed here.
  const [remote, setRemote] = React.useState<string | null>(null);
  const [resume, setResume] = React.useState<ResumePlan | null>(null);
  const [find, setFind] = React.useState("");
  const [tagsDraft, setTagsDraft] = React.useState("");
  const [noteDraft, setNoteDraft] = React.useState("");
  const [savedHint, setSavedHint] = React.useState<string | null>(null);
  const findInput = React.useRef<HTMLInputElement>(null);
  const onErrorRef = useLatest(onError);

  // Load the conversation whenever the id changes.
  React.useEffect(() => {
    if (!id) {
      setConvo(null);
      return;
    }
    let live = true;
    setLoading(true);
    setMissing(false);
    setConvo(null);
    setResume(null);
    setFind("");
    (async () => {
      const local = device ? await localDevice() : null;
      const away = device && device !== local ? device : null;
      try {
        const c = await getConversation(id, device);
        if (!live) return;
        setRemote(away);
        setConvo(c);
        setMissing(!c);
        setTagsDraft((c?.tags ?? []).join(", "));
        setNoteDraft(c?.note ?? "");
        if (c && !away) getResume(c.id).then((r) => live && setResume(r)).catch(() => {});
      } catch (err) {
        if (live) {
          setMissing(true);
          onErrorRef.current(String(err));
        }
      } finally {
        if (live) setLoading(false);
      }
    })();
    return () => {
      live = false;
    };
  }, [id, device, onErrorRef]);

  const turns = React.useMemo(() => {
    if (!convo) return [];
    const term = find.trim();
    return convo.messages
      .map((m, i) => ({ m, i }))
      .filter(({ m }) => !term || matchesFind(m.content, term));
  }, [convo, find]);

  const saved = !!(convo?.bookmarked || convo?.pinned);

  async function toggleSaved() {
    if (!convo) return;
    const patch = saved ? { bookmarked: false, pinned: false } : { bookmarked: true };
    try {
      await setFlags(convo.id, patch);
      setConvo((c) => (c ? { ...c, ...patch } : c));
    } catch (err) {
      onError(String(err));
    }
  }

  function flash(msg: string) {
    setSavedHint(msg);
    window.setTimeout(() => setSavedHint(null), 1500);
  }

  async function saveTags() {
    if (!convo) return;
    const tags = Array.from(new Set(tagsDraft.split(",").map((t) => t.trim().toLowerCase()).filter(Boolean)));
    if (tags.join(",") === (convo.tags ?? []).join(",")) return;
    try {
      await setTags(convo.id, tags);
      setConvo((c) => (c ? { ...c, tags } : c));
      setTagsDraft(tags.join(", "));
      flash("Tags saved");
    } catch (err) {
      onError(String(err));
    }
  }

  async function saveNote() {
    if (!convo || noteDraft === (convo.note ?? "")) return;
    try {
      await setNote(convo.id, noteDraft);
      setConvo((c) => (c ? { ...c, note: noteDraft } : c));
      flash("Note saved");
    } catch (err) {
      onError(String(err));
    }
  }

  async function reveal() {
    if (!convo) return;
    try {
      if (!(await openSource(convo.id))) onError("Couldn't open the source file.");
    } catch (err) {
      onError(String(err));
    }
  }

  async function forgetIt() {
    if (!convo) return;
    if (!window.confirm("Forget this conversation? It's removed from the index and won't be re-added.")) return;
    try {
      await forget(convo.id);
      onClose();
    } catch (err) {
      onError(String(err));
    }
  }

  const canThread = !!convo && !remote && convo.kind !== "skill" && convo.kind !== "exchange";
  const resumeCmd = resume?.action.kind === "resume" ? resume.action.display : null;

  return (
    <Sheet open={!!id} onOpenChange={(o) => !o && onClose()}>
      <SheetContent
        className="gap-0 p-0 sm:max-w-3xl xl:max-w-4xl [&>button:last-child]:hidden"
        onOpenAutoFocus={(e) => e.preventDefault()}
        onKeyDown={(e) => {
          // Ctrl/Cmd+F finds inside the transcript instead of the page.
          if (e.key.toLowerCase() === "f" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            findInput.current?.focus();
            findInput.current?.select();
          }
        }}
      >
        {/* Header */}
        <div className="flex items-start gap-3 border-b px-4 py-3 sm:px-6 sm:py-4">
          <div className="min-w-0 flex-1 space-y-1.5">
            <div className="flex flex-wrap items-center gap-1.5">
              {convo && <ToolChip tool={convo.tool} />}
              {convo?.kind === "skill" && <Badge variant="info">skill</Badge>}
              {convo?.kind === "exchange" && <Badge variant="info">room exchange</Badge>}
              {remote && <Badge variant="outline">on {remote}</Badge>}
              {convo?.started_at && (
                <span className="text-xs text-muted-foreground">
                  {new Date(convo.started_at).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}
                </span>
              )}
            </div>
            <SheetTitle className="line-clamp-2 text-base leading-snug sm:text-lg">
              {loading ? "Loading…" : convo?.title?.trim() || (missing ? "Session not found" : "(untitled)")}
            </SheetTitle>
            <SheetDescription className="truncate font-mono text-xs" title={convo?.project ?? undefined}>
              {convo?.project ? shortPath(convo.project) : convo ? `${convo.messages.length} messages` : " "}
            </SheetDescription>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            {convo && (
              <Button
                variant="ghost"
                size="icon"
                onClick={toggleSaved}
                aria-pressed={saved}
                aria-label={saved ? "Remove from saved" : "Save for later"}
                title={saved ? "Remove from saved" : "Save for later"}
              >
                <Star className={cn(saved && "fill-warning text-warning")} />
              </Button>
            )}
            {convo && (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="ghost" size="icon" aria-label="More actions">
                    <MoreHorizontal />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-52">
                  {!remote && convo.source_path && (
                    <DropdownMenuItem onSelect={reveal}>
                      <FolderOpen /> Reveal source file
                    </DropdownMenuItem>
                  )}
                  {convo.source_path && (
                    <DropdownMenuItem onSelect={() => navigator.clipboard?.writeText(convo.source_path!)}>
                      <FileSearch /> Copy file path
                    </DropdownMenuItem>
                  )}
                  <DropdownMenuItem onSelect={() => exportConversation(convo, "md")}>
                    <Download /> Export as Markdown
                  </DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => exportConversation(convo, "json")}>
                    <Download /> Export as JSON
                  </DropdownMenuItem>
                  {!remote && (
                    <>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem onSelect={forgetIt} className="text-destructive focus:text-destructive">
                        <Trash2 /> Forget this session
                      </DropdownMenuItem>
                    </>
                  )}
                </DropdownMenuContent>
              </DropdownMenu>
            )}
            <Button variant="ghost" size="icon" onClick={onClose} aria-label="Close">
              <X />
            </Button>
          </div>
        </div>

        {/* Actions + find */}
        {convo && (
          <div className="flex flex-wrap items-center gap-2 border-b bg-muted/30 px-4 py-2.5 sm:px-6">
            <div className="relative min-w-[12rem] flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                ref={findInput}
                value={find}
                onChange={(e) => setFind(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape" && find) {
                    e.stopPropagation();
                    setFind("");
                  }
                }}
                placeholder="Find in this conversation"
                aria-label="Find in this conversation"
                className="h-8 pl-8 pr-20"
              />
              {find.trim() && (
                <span className="absolute right-2.5 top-1/2 -translate-y-1/2 text-xs tabular-nums text-muted-foreground" aria-live="polite">
                  {turns.length} match{turns.length === 1 ? "" : "es"}
                </span>
              )}
            </div>
            {resumeCmd && (
              <CopyButton text={resumeCmd} label="Copy resume command" title={resumeCmd} className="h-8" icon={<Terminal />} />
            )}
            {canThread && (
              <Button
                variant="outline"
                size="sm"
                className="h-8"
                title="See every session that is part of the same piece of work"
                onClick={() => {
                  onShowThread(convo.id);
                  onClose();
                }}
              >
                <GitBranch /> Show its thread
              </Button>
            )}
          </div>
        )}

        {/* Body */}
        <div className="min-h-0 flex-1 overflow-y-auto scrollbar-thin">
          <div className="space-y-4 px-4 py-5 sm:px-6">
            {loading &&
              Array.from({ length: 4 }).map((_, i) => (
                <div key={i} className={cn("space-y-2", i % 2 === 0 && "flex flex-col items-end")}>
                  <Skeleton className="h-3 w-16" />
                  <Skeleton className={cn("h-16", i % 2 === 0 ? "w-2/3" : "w-full")} />
                </div>
              ))}
            {!loading && missing && (
              <Notice tone="warning" title="This session couldn't be loaded">
                It may have been forgotten, or the computer it's on is offline.
              </Notice>
            )}
            {convo && turns.length === 0 && (
              <p className="py-10 text-center text-sm text-muted-foreground">
                {find.trim() ? `Nothing in this conversation matches “${find.trim()}”.` : "This conversation has no messages."}
              </p>
            )}
            {convo && turns.map(({ m, i }) => <Turn key={i} role={m.role} content={m.content} find={find} />)}
          </div>

          {/* Your notes on it */}
          {convo && !remote && (
            <div className="space-y-3 border-t bg-muted/20 px-4 py-4 sm:px-6">
              <div className="flex items-center justify-between">
                <h3 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Your notes</h3>
                {savedHint && <span className="text-xs text-success" aria-live="polite">{savedHint}</span>}
              </div>
              <div className="grid gap-3 sm:grid-cols-[6rem_1fr] sm:items-start">
                <label htmlFor="ts-tags" className="pt-2 text-[13px] text-muted-foreground">
                  Tags
                </label>
                <Input
                  id="ts-tags"
                  value={tagsDraft}
                  onChange={(e) => setTagsDraft(e.target.value)}
                  onBlur={saveTags}
                  onKeyDown={(e) => e.key === "Enter" && (e.currentTarget as HTMLInputElement).blur()}
                  placeholder="comma, separated"
                />
                <label htmlFor="ts-note" className="pt-2 text-[13px] text-muted-foreground">
                  Note
                </label>
                <Textarea
                  id="ts-note"
                  value={noteDraft}
                  onChange={(e) => setNoteDraft(e.target.value)}
                  onBlur={saveNote}
                  placeholder="A private note, saved when you click away"
                  rows={2}
                />
              </div>
              {convo.source_path && (
                <p className="truncate font-mono text-[11px] text-muted-foreground" title={convo.source_path}>
                  {convo.source_path}
                </p>
              )}
            </div>
          )}
          {convo && remote && convo.source_path && (
            <p className="truncate border-t px-4 py-3 font-mono text-[11px] text-muted-foreground sm:px-6" title={convo.source_path}>
              source on {remote}: {convo.source_path}
            </p>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}
