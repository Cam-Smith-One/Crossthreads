// Small pieces shared by the pages: tool chips, copy buttons, page headers,
// inline notices, and the snippet/find highlighters.

import * as React from "react";
import { AlertTriangle, Check, Copy, Info, X } from "lucide-react";
import { getConversation } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button, type ButtonProps } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { toolIcon, toolLabel } from "@/tools";

/** A ref that always holds the latest value: lets effects call a callback
 * prop without re-running whenever the parent passes a new function. */
export function useLatest<T>(value: T): React.MutableRefObject<T> {
  const ref = React.useRef(value);
  ref.current = value;
  return ref;
}

/** The last two segments of a path: enough to recognise a project. */
export function shortPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length <= 2 ? path : `…/${parts.slice(-2).join("/")}`;
}

export function ToolChip({ tool, count, className }: { tool: string; count?: number; className?: string }) {
  return (
    <Badge variant="secondary" className={cn("shrink-0 gap-1 font-normal", className)} title={toolLabel(tool)}>
      <span aria-hidden="true" className="text-[11px] leading-none">
        {toolIcon(tool)}
      </span>
      {toolLabel(tool)}
      {count !== undefined && <span className="ml-0.5 tabular-nums text-muted-foreground">{count.toLocaleString()}</span>}
    </Badge>
  );
}

/** Copy text to the clipboard, with a brief "Copied" confirmation. */
export function CopyButton({
  text,
  label = "Copy",
  copiedLabel = "Copied",
  iconOnly = false,
  icon,
  ...props
}: {
  text: string | (() => string | Promise<string>);
  label?: string;
  copiedLabel?: string;
  iconOnly?: boolean;
  icon?: React.ReactNode;
} & Omit<ButtonProps, "onClick" | "children">) {
  const [copied, setCopied] = React.useState(false);
  async function copy() {
    try {
      const value = typeof text === "function" ? await text() : text;
      await navigator.clipboard?.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard refused: nothing useful to say */
    }
  }
  return (
    <Button
      variant="outline"
      size={iconOnly ? "icon-sm" : "sm"}
      onClick={copy}
      aria-label={iconOnly ? label : undefined}
      title={iconOnly ? label : props.title}
      {...props}
    >
      {copied ? <Check className="text-success" /> : (icon ?? <Copy />)}
      {!iconOnly && (copied ? copiedLabel : label)}
    </Button>
  );
}

/** A command on one line with a copy button. */
export function CommandLine({ cmd, className }: { cmd: string; className?: string }) {
  return (
    <div className={cn("flex items-start gap-2 rounded-md border bg-muted/50 p-2 pl-3", className)}>
      <code className="min-w-0 flex-1 whitespace-pre-wrap break-all py-1 font-mono text-xs leading-relaxed">{cmd}</code>
      <CopyButton text={cmd} iconOnly label="Copy command" className="shrink-0" />
    </div>
  );
}

export function PageHeader({
  title,
  description,
  actions,
  className,
}: {
  title: React.ReactNode;
  description?: React.ReactNode;
  actions?: React.ReactNode;
  className?: string;
}) {
  return (
    <header className={cn("flex flex-wrap items-start justify-between gap-x-6 gap-y-3", className)}>
      <div className="min-w-0 space-y-1">
        <h1 className="text-xl font-semibold tracking-tight">{title}</h1>
        {description && <p className="max-w-2xl text-sm text-muted-foreground">{description}</p>}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </header>
  );
}

export function SectionHeader({
  title,
  description,
  actions,
  className,
}: {
  title: React.ReactNode;
  description?: React.ReactNode;
  actions?: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex flex-wrap items-end justify-between gap-x-4 gap-y-2", className)}>
      <div className="min-w-0 space-y-0.5">
        <h2 className="text-sm font-semibold">{title}</h2>
        {description && <p className="text-[13px] text-muted-foreground">{description}</p>}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </div>
  );
}

/** An inline notice: info, warning or error. */
export function Notice({
  tone = "info",
  title,
  children,
  action,
  onDismiss,
  className,
}: {
  tone?: "info" | "warning" | "error";
  title?: React.ReactNode;
  children?: React.ReactNode;
  action?: React.ReactNode;
  onDismiss?: () => void;
  className?: string;
}) {
  const Icon = tone === "info" ? Info : AlertTriangle;
  return (
    <div
      role={tone === "error" ? "alert" : "status"}
      className={cn(
        "flex items-start gap-3 rounded-lg border px-3.5 py-3 text-sm",
        tone === "info" && "border-primary/25 bg-primary/5",
        tone === "warning" && "border-warning/30 bg-warning/10",
        tone === "error" && "border-destructive/30 bg-destructive/10",
        className,
      )}
    >
      <Icon
        className={cn(
          "mt-0.5 size-4 shrink-0",
          tone === "info" && "text-primary",
          tone === "warning" && "text-warning",
          tone === "error" && "text-destructive",
        )}
      />
      <div className="min-w-0 flex-1 space-y-1">
        {title && <p className="font-medium">{title}</p>}
        {children && <div className="text-[13px] text-muted-foreground [&_code]:font-mono [&_code]:text-foreground">{children}</div>}
      </div>
      {action && <div className="flex shrink-0 items-center gap-2">{action}</div>}
      {onDismiss && (
        <Button variant="ghost" size="icon-sm" onClick={onDismiss} aria-label="Dismiss" className="-mr-1 -mt-0.5 shrink-0">
          <X />
        </Button>
      )}
    </div>
  );
}

/** The Crossthreads wordmark. */
export function Brand({ className, size = "md" }: { className?: string; size?: "sm" | "md" }) {
  return (
    <span className={cn("inline-flex items-center gap-2", className)}>
      <img src="/mark.png" alt="" className={size === "sm" ? "h-5 w-5" : "h-7 w-7"} />
      <span className={cn("font-semibold tracking-tight", size === "sm" ? "text-sm" : "text-lg")}>
        Cross<span className="text-primary">threads</span>
      </span>
    </span>
  );
}

/** A small round status light. */
export function Dot({ tone = "muted", pulse, className }: { tone?: "success" | "warning" | "muted" | "primary"; pulse?: boolean; className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "inline-block size-2 shrink-0 rounded-full",
        tone === "success" && "bg-success",
        tone === "warning" && "bg-warning",
        tone === "primary" && "bg-primary",
        tone === "muted" && "bg-muted-foreground/40",
        pulse && "animate-pulse-dot",
        className,
      )}
    />
  );
}

export function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="inline-flex h-5 min-w-5 items-center justify-center rounded border bg-muted px-1 font-mono text-[11px] font-medium text-muted-foreground">
      {children}
    </kbd>
  );
}

/** `<mark>` styling for highlighted matches, as a class for a container. */
export const markClass =
  "[&_mark]:rounded-[3px] [&_mark]:bg-yellow-300/50 [&_mark]:px-0.5 [&_mark]:text-foreground dark:[&_mark]:bg-yellow-400/25";

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** The daemon wraps matched terms in STX…ETX control characters (sentinels
 * that can't occur in content); escape the rest and render those as <mark>. */
export function highlightSnippet(snippet: string): string {
  return escapeHtml(snippet).replace(/\u0002([^\u0003]*)\u0003/g, "<mark>$1</mark>");
}

export function matchesFind(content: string, find: string): boolean {
  return content.toLowerCase().includes(find.trim().toLowerCase());
}

/** Escape the content, then wrap case-insensitive matches of `find` in <mark>. */
export function highlightFind(content: string, find: string): string {
  const escaped = escapeHtml(content);
  const term = find.trim();
  if (!term) return escaped;
  return escaped.replace(new RegExp(escapeRegex(escapeHtml(term)), "gi"), "<mark>$&</mark>");
}

/** Titles for session ids, looked up once per id (for cited sources). */
const titleCache = new Map<string, Promise<{ title: string; tool: string } | null>>();
function lookupTitle(id: string) {
  let p = titleCache.get(id);
  if (!p) {
    p = getConversation(id)
      .then((c) => (c ? { title: (c.title ?? "").trim() || "(untitled)", tool: c.tool } : null))
      .catch(() => null);
    titleCache.set(id, p);
  }
  return p;
}

/** A numbered list of the sessions a report or answer was drawn from. */
export function SourceList({
  ids,
  onOpen,
  className,
}: {
  ids: string[];
  onOpen: (id: string) => void;
  className?: string;
}) {
  const [meta, setMeta] = React.useState<Record<string, { title: string; tool: string } | null>>({});
  React.useEffect(() => {
    let live = true;
    ids.slice(0, 20).forEach((id) =>
      lookupTitle(id).then((m) => live && setMeta((old) => ({ ...old, [id]: m }))),
    );
    return () => {
      live = false;
    };
  }, [ids]);
  if (ids.length === 0) return null;
  return (
    <ol className={cn("grid gap-1.5 sm:grid-cols-2", className)}>
      {ids.map((id, i) => {
        const m = meta[id];
        return (
          <li key={id}>
            <button
              type="button"
              onClick={() => onOpen(id)}
              className="flex w-full items-center gap-2 rounded-md border bg-card px-2.5 py-2 text-left text-[13px] transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <span className="flex size-5 shrink-0 items-center justify-center rounded bg-muted text-[11px] font-medium tabular-nums text-muted-foreground">
                {i + 1}
              </span>
              {m?.tool && (
                <span aria-hidden="true" className="text-xs" title={toolLabel(m.tool)}>
                  {toolIcon(m.tool)}
                </span>
              )}
              <span className="min-w-0 flex-1 truncate">
                {m === undefined ? <span className="text-muted-foreground">Loading…</span> : m ? m.title : <span className="font-mono text-xs text-muted-foreground">{id.slice(0, 12)}…</span>}
              </span>
            </button>
          </li>
        );
      })}
    </ol>
  );
}

/** A small spinner for buttons and loading rows. */
export function Spinner({ className }: { className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn("inline-block size-4 animate-spin rounded-full border-2 border-current border-r-transparent opacity-70", className)}
    />
  );
}

/** A segmented control (radio-like buttons), keyboard friendly. */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  size = "default",
  className,
}: {
  value: T | null;
  options: { value: T; label: React.ReactNode; dot?: boolean }[];
  onChange: (v: T) => void;
  label: string;
  size?: "default" | "sm";
  className?: string;
}) {
  return (
    <div role="tablist" aria-label={label} className={cn("inline-flex max-w-full items-center overflow-x-auto rounded-lg bg-muted p-1 text-muted-foreground", className)}>
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="tab"
          aria-selected={value === o.value}
          onClick={() => onChange(o.value)}
          className={cn(
            "relative inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-md font-medium transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring [&_svg]:size-4",
            size === "sm" ? "h-6 px-2.5 text-xs" : "h-7 px-3 text-sm",
            value === o.value ? "bg-background text-foreground shadow" : "hover:text-foreground",
          )}
        >
          {o.label}
          {o.dot && <span aria-label="new" className="size-1.5 rounded-full bg-primary" />}
        </button>
      ))}
    </div>
  );
}
