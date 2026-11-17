// A small, safe Markdown renderer for the reports the daemon writes (insights,
// the weekly review, Ask answers). It builds React elements — never HTML
// strings — so nothing in a report can inject markup, and it needs no
// dependency. Covers what the reports use: headings, paragraphs, bullet and
// numbered lists (nested by indentation), task boxes, block quotes, fenced
// code, pipe tables, rules, and inline code / bold / italic / links.

import * as React from "react";
import { cn } from "@/lib/utils";

type Block =
  | { t: "h"; level: number; text: string }
  | { t: "p"; text: string }
  | { t: "code"; lang: string; text: string }
  | { t: "quote"; blocks: Block[] }
  | { t: "list"; ordered: boolean; start: number; items: ListItem[] }
  | { t: "table"; head: string[]; rows: string[][] }
  | { t: "hr" };

interface ListItem {
  text: string;
  checked: boolean | null;
  children: Block[];
}

const LIST_RE = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;

function parse(src: string): Block[] {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  const out: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i++;
      continue;
    }
    // Fenced code.
    const fence = line.match(/^\s*(```|~~~)\s*([\w+-]*)/);
    if (fence) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith(fence[1])) body.push(lines[i++]);
      i++;
      out.push({ t: "code", lang: fence[2], text: body.join("\n") });
      continue;
    }
    const h = line.match(/^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/);
    if (h) {
      out.push({ t: "h", level: h[1].length, text: h[2] });
      i++;
      continue;
    }
    if (/^\s{0,3}([-*_])(\s*\1){2,}\s*$/.test(line)) {
      out.push({ t: "hr" });
      i++;
      continue;
    }
    if (/^\s{0,3}>/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^\s{0,3}>/.test(lines[i])) body.push(lines[i++].replace(/^\s{0,3}>\s?/, ""));
      out.push({ t: "quote", blocks: parse(body.join("\n")) });
      continue;
    }
    // Pipe table: a header row, then a |---|---| separator.
    if (line.includes("|") && i + 1 < lines.length && /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/.test(lines[i + 1])) {
      const cells = (l: string) =>
        l
          .trim()
          .replace(/^\|/, "")
          .replace(/\|$/, "")
          .split("|")
          .map((c) => c.trim());
      const head = cells(line);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i].includes("|") && lines[i].trim()) rows.push(cells(lines[i++]));
      out.push({ t: "table", head, rows });
      continue;
    }
    const li = line.match(LIST_RE);
    if (li) {
      const [block, next] = parseList(lines, i);
      out.push(block);
      i = next;
      continue;
    }
    // Paragraph: until a blank line or the start of another block.
    const para: string[] = [];
    while (
      i < lines.length &&
      lines[i].trim() &&
      !/^\s*(```|~~~|#{1,6}\s|>)/.test(lines[i]) &&
      !LIST_RE.test(lines[i])
    ) {
      para.push(lines[i++].trim());
    }
    out.push({ t: "p", text: para.join("\n") });
  }
  return out;
}

function parseList(lines: string[], start: number): [Block, number] {
  const first = lines[start].match(LIST_RE)!;
  const indent = first[1].length;
  const ordered = /\d/.test(first[2]);
  const items: ListItem[] = [];
  let i = start;
  while (i < lines.length) {
    const m = lines[i].match(LIST_RE);
    if (!m || m[1].length !== indent || /\d/.test(m[2]) !== ordered) {
      if (m && m[1].length < indent) break;
      if (!m) {
        // A continuation line (indented text) or a blank between items.
        if (!lines[i].trim()) {
          const nm = lines[i + 1]?.match(LIST_RE);
          if (nm && nm[1].length >= indent) {
            i++;
            continue;
          }
          break;
        }
        if (/^\s+/.test(lines[i]) && items.length) {
          items[items.length - 1].text += "\n" + lines[i].trim();
          i++;
          continue;
        }
        break;
      }
      if (m[1].length > indent && items.length) {
        // A nested list belongs to the previous item.
        const [child, next] = parseList(lines, i);
        items[items.length - 1].children.push(child);
        i = next;
        continue;
      }
      break;
    }
    let text = m[3];
    let checked: boolean | null = null;
    const task = text.match(/^\[([ xX])\]\s+(.*)$/);
    if (task) {
      checked = task[1] !== " ";
      text = task[2];
    }
    items.push({ text, checked, children: [] });
    i++;
  }
  return [{ t: "list", ordered, start: ordered ? parseInt(first[2], 10) || 1 : 1, items }, i];
}

// Inline: `code`, **bold**, *italic* / _italic_, [text](http…), bare URLs.
const INLINE_RE =
  /(`+)([^`]+?)\1|\*\*([^*]+?)\*\*|__([^_]+?)__|\*([^*\s][^*]*?)\*|\b_([^_\s][^_]*?)_\b|\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)|(https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"])/g;

function inline(text: string, key = "i"): React.ReactNode[] {
  const out: React.ReactNode[] = [];
  let last = 0;
  let n = 0;
  for (const m of text.matchAll(INLINE_RE)) {
    const at = m.index ?? 0;
    if (at > last) out.push(...breaks(text.slice(last, at), `${key}t${n}`));
    const k = `${key}-${n++}`;
    if (m[2] !== undefined)
      out.push(
        <code key={k} className="rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]">
          {m[2]}
        </code>,
      );
    else if (m[3] !== undefined || m[4] !== undefined)
      out.push(
        <strong key={k} className="font-semibold">
          {inline(m[3] ?? m[4], k)}
        </strong>,
      );
    else if (m[5] !== undefined || m[6] !== undefined) out.push(<em key={k}>{inline(m[5] ?? m[6], k)}</em>);
    else if (m[7] !== undefined)
      out.push(
        <a key={k} href={m[8]} target="_blank" rel="noreferrer noopener" className="text-primary underline-offset-4 hover:underline">
          {inline(m[7], k)}
        </a>,
      );
    else if (m[9] !== undefined)
      out.push(
        <a key={k} href={m[9]} target="_blank" rel="noreferrer noopener" className="break-all text-primary underline-offset-4 hover:underline">
          {m[9]}
        </a>,
      );
    last = at + m[0].length;
  }
  if (last < text.length) out.push(...breaks(text.slice(last), `${key}e`));
  return out;
}

/** Keep single newlines inside a paragraph as line breaks (reports use them). */
function breaks(s: string, key: string): React.ReactNode[] {
  const parts = s.split("\n");
  return parts.flatMap((p, i) => (i === 0 ? [p] : [<br key={`${key}b${i}`} />, p]));
}

const HEADING = [
  "",
  "text-lg font-semibold tracking-tight mt-6 first:mt-0",
  "text-base font-semibold tracking-tight mt-6 first:mt-0",
  "text-sm font-semibold mt-5 first:mt-0",
  "text-sm font-semibold mt-4 first:mt-0",
  "text-sm font-medium mt-4 first:mt-0",
  "text-sm font-medium mt-4 first:mt-0 text-muted-foreground",
];

function renderBlocks(blocks: Block[], key: string): React.ReactNode[] {
  return blocks.map((b, i) => {
    const k = `${key}.${i}`;
    switch (b.t) {
      case "h": {
        const Tag = `h${Math.min(b.level + 1, 6)}` as "h2";
        return (
          <Tag key={k} className={HEADING[b.level]}>
            {inline(b.text, k)}
          </Tag>
        );
      }
      case "p":
        return (
          <p key={k} className="leading-relaxed">
            {inline(b.text, k)}
          </p>
        );
      case "code":
        return (
          <pre key={k} className="overflow-x-auto rounded-md border bg-muted/60 p-3 font-mono text-xs leading-relaxed">
            <code>{b.text}</code>
          </pre>
        );
      case "quote":
        return (
          <blockquote key={k} className="space-y-2 border-l-2 border-primary/40 pl-3 text-muted-foreground">
            {renderBlocks(b.blocks, k)}
          </blockquote>
        );
      case "hr":
        return <hr key={k} className="my-2 border-border" />;
      case "table":
        return (
          <div key={k} className="overflow-x-auto rounded-md border">
            <table className="w-full text-left text-[13px]">
              <thead className="bg-muted/60">
                <tr>
                  {b.head.map((c, j) => (
                    <th key={j} className="px-3 py-1.5 font-medium">
                      {inline(c, `${k}h${j}`)}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {b.rows.map((r, ri) => (
                  <tr key={ri} className="border-t">
                    {r.map((c, j) => (
                      <td key={j} className="px-3 py-1.5 align-top">
                        {inline(c, `${k}r${ri}c${j}`)}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        );
      case "list": {
        const Tag = b.ordered ? "ol" : "ul";
        return (
          <Tag
            key={k}
            start={b.ordered && b.start !== 1 ? b.start : undefined}
            className={cn("space-y-1 pl-5", b.ordered ? "list-decimal" : "list-disc", "marker:text-muted-foreground")}
          >
            {b.items.map((it, j) => (
              <li key={j} className={cn("leading-relaxed", it.checked !== null && "-ml-5 list-none")}>
                {it.checked !== null && (
                  <span
                    aria-label={it.checked ? "done" : "not done"}
                    className={cn(
                      "mr-2 inline-flex h-3.5 w-3.5 translate-y-0.5 items-center justify-center rounded-sm border text-[10px]",
                      it.checked ? "border-primary bg-primary text-primary-foreground" : "border-input",
                    )}
                  >
                    {it.checked ? "✓" : ""}
                  </span>
                )}
                {inline(it.text, `${k}.${j}`)}
                {it.children.length > 0 && <div className="mt-1 space-y-1">{renderBlocks(it.children, `${k}.${j}c`)}</div>}
              </li>
            ))}
          </Tag>
        );
      }
    }
  });
}

/** Render a Markdown report. */
export function Markdown({ source, className }: { source: string; className?: string }) {
  const blocks = React.useMemo(() => parse(source ?? ""), [source]);
  return <div className={cn("space-y-3 text-sm text-foreground", className)}>{renderBlocks(blocks, "m")}</div>;
}
