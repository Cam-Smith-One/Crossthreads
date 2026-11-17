import { CircleDot, Hourglass, Laptop, Pause, Radio } from "lucide-react";
import type { Live } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { toolIcon, toolLabel } from "@/tools";
import { cn } from "@/lib/utils";

/** A tool's glyph and name. `compact` shows only the glyph, with a tooltip. */
export function ToolBadge({ tool, compact = false }: { tool: string; compact?: boolean }) {
  if (compact) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <span className="inline-flex h-5 w-5 items-center justify-center rounded border bg-background text-[11px]">
            {toolIcon(tool)}
          </span>
        </TooltipTrigger>
        <TooltipContent>{toolLabel(tool)}</TooltipContent>
      </Tooltip>
    );
  }
  return (
    <Badge variant="outline" className="font-normal">
      <span className="text-[11px]">{toolIcon(tool)}</span>
      {toolLabel(tool)}
    </Badge>
  );
}

/** What a session (or the busiest session of a thread) is doing now. */
export function LiveBadge({ live, className }: { live?: Live | null; className?: string }) {
  if (!live || live.state === "ended" || live.state === "idle") return null;
  const tip = live.inferred ? "Guessed from the transcript; install the hooks to know for sure" : undefined;
  const body = (() => {
    switch (live.state) {
      case "needs_you":
        return (
          <Badge variant="warning" className={className}>
            <Hourglass /> Needs you
          </Badge>
        );
      case "running":
        return (
          <Badge variant="info" className={className}>
            <span className="h-1.5 w-1.5 animate-pulse-dot rounded-full bg-primary" /> Running
          </Badge>
        );
      case "listening":
        return (
          <Badge variant="secondary" className={className}>
            <Radio /> Listening
          </Badge>
        );
      default:
        return (
          <Badge variant="outline" className={className}>
            <CircleDot /> {live.state}
          </Badge>
        );
    }
  })();
  if (!tip) return body;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span>{body}</span>
      </TooltipTrigger>
      <TooltipContent>{tip}</TooltipContent>
    </Tooltip>
  );
}

export function DeviceBadge({ devices, className }: { devices: string[]; className?: string }) {
  if (devices.length === 0) return null;
  return (
    <Badge variant="outline" className={cn("font-normal", className)}>
      <Laptop /> {devices.join(" + ")}
    </Badge>
  );
}

export function StatusBadge({ status }: { status: string }) {
  if (status === "active") return null;
  return (
    <Badge variant={status === "done" ? "success" : "secondary"} className="capitalize">
      {status === "parked" && <Pause />}
      {status}
    </Badge>
  );
}
