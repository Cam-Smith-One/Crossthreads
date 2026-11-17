import { useEffect, useState } from "react";
import { FolderGit2, Plus } from "lucide-react";
import { continueThread, createThread, recentFolders, type ToolId } from "@/api";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input, Textarea } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/select";

/** Start a piece of work: name it, say what it's for, pick the folder, go. */
export function NewThreadDialog({
  open,
  onOpenChange,
  onCreated,
  onError,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: (id: string, message: string) => void;
  onError: (message: string) => void;
}) {
  const [title, setTitle] = useState("");
  const [goal, setGoal] = useState("");
  const [folder, setFolder] = useState("");
  const [folders, setFolders] = useState<string[]>([]);
  const [tool, setTool] = useState<ToolId | "none">("claude-code");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    recentFolders()
      .then((f) => {
        setFolders(f);
        setFolder((cur) => cur || f[0] || "");
      })
      .catch(() => {});
  }, [open]);

  async function submit() {
    setBusy(true);
    try {
      const id = await createThread(title.trim(), folder.trim(), goal.trim() || undefined);
      if (!id) throw new Error("couldn't create the thread");
      let message = `Started “${title.trim()}”`;
      if (tool !== "none") {
        await continueThread(id, null, tool);
        message += ` and opened ${tool === "codex" ? "Codex" : "Claude Code"}`;
      }
      onCreated(id, message);
      setTitle("");
      setGoal("");
      onOpenChange(false);
    } catch (e) {
      onError(String(e).replace(/^Error:\s*/, ""));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>New thread</DialogTitle>
          <DialogDescription>
            A piece of work you'll carry across tools and computers. Sessions you start for it join it, and its notes go with it.
          </DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (!busy && title.trim() && folder.trim()) submit();
          }}
        >
          <div className="grid gap-1.5">
            <Label htmlFor="nt-title">Name</Label>
            <Input id="nt-title" autoFocus value={title} onChange={(e) => setTitle(e.target.value)} placeholder="e.g. Dark mode for settings" />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="nt-goal">Goal <span className="font-normal text-muted-foreground">(optional, your agent starts from it)</span></Label>
            <Textarea id="nt-goal" value={goal} onChange={(e) => setGoal(e.target.value)} placeholder="What done looks like, constraints, links…" />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="nt-folder">Folder on this computer</Label>
            <div className="relative">
              <FolderGit2 className="pointer-events-none absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
              <Input id="nt-folder" list="nt-folders" className="pl-8 font-mono text-xs" value={folder} onChange={(e) => setFolder(e.target.value)} placeholder="/path/to/your/repo" />
              <datalist id="nt-folders">
                {folders.map((f) => (
                  <option key={f} value={f} />
                ))}
              </datalist>
            </div>
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="nt-tool">Then</Label>
            <NativeSelect id="nt-tool" value={tool} onChange={(e) => setTool(e.target.value as ToolId | "none")}>
              <option value="claude-code">Open Claude Code on it</option>
              <option value="codex">Open Codex on it</option>
              <option value="none">Just add it to my board</option>
            </NativeSelect>
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" disabled={busy || !title.trim() || !folder.trim()}>
              <Plus /> {busy ? "Starting…" : "Start thread"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
