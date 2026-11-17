import { useCallback, useEffect, useState } from "react";
import {
  Bell,
  Home as HomeIcon,
  Laptop,
  Menu,
  Moon,
  Search as SearchIcon,
  Settings as SettingsIcon,
  Sun,
  Users,
  X,
} from "lucide-react";
import {
  getDevices,
  getSession,
  getSetupStatus,
  getStatus,
  getThreads,
  getWeeklyReview,
  SignInRequiredError,
  UnpairedError,
  type Device,
  type Session,
  type SetupStatus,
  type Status,
  type ThreadSummary,
} from "./api";
import { Button } from "@/components/ui/button";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ThreadSheet } from "@/components/ThreadSheet";
import { NewThreadDialog } from "@/components/NewThreadDialog";
import { TranscriptSheet } from "@/components/TranscriptSheet";
import { HomePage } from "@/pages/Home";
import { DevicesPage } from "@/pages/Devices";
import { SearchPage } from "@/pages/Search";
import { InsightsPage } from "@/pages/Insights";
import { SettingsPage, SetupNudge, setupSteps, staleClientRecently } from "@/pages/Settings";
import { RoomsPage } from "@/pages/Rooms";
import { PairPhone } from "./Phone";
import { SignIn } from "./SignIn";
import { cn } from "@/lib/utils";

/** The app's places, each with its own address (#/search, …). */
type Page = "threads" | "rooms" | "search" | "insights" | "devices" | "settings";
const PAGES: Page[] = ["threads", "rooms", "search", "insights", "devices", "settings"];
type InsightsTab = "overview" | "weekly" | "themes" | "analyses";
type SettingsTab = "start" | "models" | "index" | "help";

interface Toast {
  id: number;
  text: string;
  error?: boolean;
}

export function App() {
  const [page, setPage] = useState<Page>("threads");
  const [sub, setSub] = useState<string>("");
  const [status, setStatus] = useState<Status | null>(null);
  const [devices, setDevices] = useState<Device[]>([]);
  const [setup, setSetup] = useState<SetupStatus | null>(null);
  const [session, setSession] = useState<Session | null>(null);
  const [unpaired, setUnpaired] = useState(false);
  const [signedOut, setSignedOut] = useState(false);
  const [theme, setTheme] = useState(() => document.documentElement.dataset.theme || "dark");
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [boardKey, setBoardKey] = useState(0);
  const [openThread, setOpenThread] = useState<ThreadSummary | { id: string; device?: string | null } | null>(null);
  const [threads, setThreads] = useState<ThreadSummary[]>([]);
  const [transcript, setTranscript] = useState<{ id: string; device?: string | null } | null>(null);
  const [newThread, setNewThread] = useState(false);
  const [roomsListening, setRoomsListening] = useState(false);
  const [weeklyUnseen, setWeeklyUnseen] = useState(false);
  const [navOpen, setNavOpen] = useState(false);
  const [nudgeDismissed, setNudgeDismissed] = useState(() => {
    try {
      return localStorage.getItem("ct-setup-nudge") === "dismissed";
    } catch {
      return false;
    }
  });
  const phone = !!session?.phone;
  const localDevice = devices.find((d) => d.local)?.name;

  const toast = useCallback((text: string, error = false) => {
    const id = Date.now() + Math.random();
    setToasts((t) => [...t.slice(-3), { id, text, error }]);
    window.setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), error ? 8000 : 4000);
  }, []);
  const onError = useCallback((m: string) => toast(m.replace(/^Error:\s*/, ""), true), [toast]);
  const onToast = useCallback((m: string) => toast(m), [toast]);

  const refreshSetup = useCallback(() => {
    getSetupStatus().then(setSetup).catch(() => {});
  }, []);

  useEffect(() => {
    if (window.location.pathname === "/login" || window.location.pathname === "/pair") return;
    getSession()
      .then(setSession)
      .catch((e) => {
        if (e instanceof UnpairedError) setUnpaired(true);
        if (e instanceof SignInRequiredError) setSignedOut(true);
      });
    getStatus().then(setStatus).catch(() => {});
    getDevices()
      .then((d) => setDevices(d.devices))
      .catch(() => {});
    getWeeklyReview(false)
      .then((r) => {
        let seen = "";
        try {
          seen = localStorage.getItem("ct-weekly-seen") ?? "";
        } catch {
          /* ignore */
        }
        if (r.generated_at && r.generated_at !== seen && !r.markdown.startsWith("No sessions")) setWeeklyUnseen(true);
      })
      .catch(() => {});
  }, []);
  useEffect(() => {
    if (!phone) refreshSetup();
  }, [phone, refreshSetup]);

  // Follow the address: #/threads, #/search[/ask], #/insights/<tab>, …
  const route = useCallback(() => {
    const [p, s] = window.location.hash.replace(/^#\/?/, "").split("/");
    let next: Page = PAGES.includes(p as Page) ? (p as Page) : "threads";
    if (next === "settings" && (s === "devices" || s === "phone")) next = "devices";
    if (phone && (next === "insights" || next === "settings" || next === "devices")) next = "threads";
    setPage(next);
    setSub(s ?? "");
    setNavOpen(false);
    if (next === "insights" && s === "weekly") {
      setWeeklyUnseen(false);
    }
    document.querySelector("main")?.scrollTo({ top: 0 });
  }, [phone]);
  useEffect(() => {
    route();
    window.addEventListener("hashchange", route);
    return () => window.removeEventListener("hashchange", route);
  }, [route]);
  const go = useCallback((p: string, s?: string) => {
    const hash = `#/${p}${s ? `/${s}` : ""}`;
    if (window.location.hash !== hash) window.location.hash = hash;
  }, []);

  // The thread list, shared with the thread panel (merge choices).
  useEffect(() => {
    getThreads()
      .then((r) => setThreads(r.threads))
      .catch(() => {});
  }, [boardKey, openThread]);

  function toggleTheme() {
    const next = theme === "dark" ? "light" : "dark";
    setTheme(next);
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem("ct-theme", next);
    } catch {
      /* ignore */
    }
  }

  const setupLeft = setup ? setupSteps(setup, status?.conversations ?? 0, () => {}, () => {}).essentials.filter((s) => !s.done).length : 0;

  if (signedOut || window.location.pathname === "/login") return <SignIn />;
  if (unpaired || window.location.pathname === "/pair") return <PairPhone />;

  const nav: [Page, React.ReactNode, string, React.ReactNode?][] = [
    ["threads", <HomeIcon key="h" />, "Home", roomsListening ? <Dot live /> : null],
    ...(phone ? ([["rooms", <Users key="r" />, "Rooms"]] as [Page, React.ReactNode, string][]) : []),
    ["search", <SearchIcon key="s" />, "Search", weeklyUnseen ? <Dot /> : null],
    ...(phone
      ? []
      : ([
          ["devices", <Laptop key="d" />, "Devices"],
          ["settings", <SettingsIcon key="g" />, "Settings", setupLeft > 0 ? <Count n={setupLeft} /> : null],
        ] as [Page, React.ReactNode, string, React.ReactNode?][])),
  ];
  const active = (p: Page) => page === p || (p === "search" && page === "insights") || (p === "threads" && page === "rooms" && !phone);

  const sidebar = (
    <div className="flex h-full flex-col gap-1 p-3">
      <a href="#/threads" className="mb-4 flex items-center gap-2 px-2 pt-1" aria-label="Crossthreads home">
        <img src="/mark.png" alt="" className="h-7 w-7" />
        <span className="text-[15px] font-semibold tracking-tight">
          Cross<span className="text-primary">threads</span>
        </span>
      </a>
      {nav.map(([p, icon, label, extra]) => (
        <a
          key={p}
          href={`#/${p}`}
          aria-current={active(p) ? "page" : undefined}
          className={cn(
            "flex items-center gap-2.5 rounded-md px-2.5 py-2 text-sm font-medium transition-colors [&>svg]:h-4 [&>svg]:w-4",
            active(p) ? "bg-accent text-accent-foreground" : "text-muted-foreground hover:bg-accent/60 hover:text-foreground",
          )}
        >
          {icon}
          <span className="flex-1">{label}</span>
          {extra}
        </a>
      ))}
      {!phone && (
        <div className="mt-4 space-y-1 border-t pt-4">
          <p className="px-2.5 pb-1 text-xs font-medium uppercase tracking-wide text-muted-foreground/70">More</p>
          <a href="#/rooms" className={cn("flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm transition-colors hover:bg-accent/60 [&>svg]:h-4 [&>svg]:w-4", page === "rooms" ? "text-foreground" : "text-muted-foreground")}>
            <Users /> <span className="flex-1">Agent rooms</span> {roomsListening && <Dot live />}
          </a>
          <a href="#/insights/weekly" className={cn("flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm transition-colors hover:bg-accent/60 [&>svg]:h-4 [&>svg]:w-4", page === "insights" && sub === "weekly" ? "text-foreground" : "text-muted-foreground")}>
            <Bell /> <span className="flex-1">Weekly review</span> {weeklyUnseen && <Dot />}
          </a>
        </div>
      )}
      <div className="mt-auto flex items-center justify-between gap-2 border-t px-1 pt-3">
        <a href={phone ? undefined : "#/settings/index"} className="truncate text-xs text-muted-foreground hover:text-foreground" title={status ? `${status.conversations.toLocaleString()} records · ${status.embedder}` : ""}>
          {status ? `${status.conversations.toLocaleString()} sessions · ${status.tools?.length ?? 0} tools` : "…"}
          {phone && session?.name ? ` · ${session.name}` : ""}
        </a>
        <Button variant="ghost" size="icon-sm" onClick={toggleTheme} aria-label="Toggle theme">
          {theme === "dark" ? <Sun /> : <Moon />}
        </Button>
      </div>
    </div>
  );

  const nudges = (
    <>
      {!phone && setup && (!nudgeDismissed || staleClientRecently(setup)) && (
        <SetupNudge
          setup={setup}
          sessions={status?.conversations ?? 0}
          onOpen={() => go("settings", "start")}
          onDismiss={() => {
            setNudgeDismissed(true);
            try {
              localStorage.setItem("ct-setup-nudge", "dismissed");
            } catch {
              /* ignore */
            }
          }}
        />
      )}
    </>
  );

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-full">
        {/* Sidebar: fixed on desktop, a drawer on narrow screens. */}
        <aside className="hidden w-60 shrink-0 border-r bg-sidebar text-sidebar-foreground md:block">{sidebar}</aside>
        {navOpen && (
          <div className="fixed inset-0 z-40 md:hidden" onClick={() => setNavOpen(false)}>
            <div className="absolute inset-0 bg-black/40" />
            <aside className="absolute inset-y-0 left-0 w-64 border-r bg-sidebar" onClick={(e) => e.stopPropagation()}>
              {sidebar}
            </aside>
          </div>
        )}

        <main className="relative flex-1 overflow-y-auto pb-20 md:pb-0">
          <div className="sticky top-0 z-30 flex items-center gap-2 border-b bg-background/80 px-4 py-2 backdrop-blur md:hidden">
            <Button variant="ghost" size="icon-sm" onClick={() => setNavOpen(true)} aria-label="Menu">
              <Menu />
            </Button>
            <img src="/mark.png" alt="" className="h-5 w-5" />
            <span className="font-semibold">Crossthreads</span>
          </div>
          <div className="mx-auto w-full max-w-6xl px-4 py-6 md:px-8 md:py-8">
            {page === "threads" && (
              <HomePage
                devices={devices}
                localDevice={localDevice}
                readOnly={phone}
                refreshKey={boardKey}
                onOpenThread={setOpenThread}
                onNewThread={() => setNewThread(true)}
                onToast={onToast}
                onError={onError}
                header={nudges}
              />
            )}
            {page === "search" && (
              <SearchPage
                phone={phone}
                mode={sub === "ask" ? "ask" : "find"}
                onMode={(m) => go("search", m)}
                onOpenSession={(id, device) => setTranscript({ id, device })}
                onShowThreadOf={(id) => setOpenThread({ id })}
                onError={onError}
              />
            )}
            {page === "insights" && (
              <InsightsPage
                tab={(["overview", "weekly", "themes", "analyses"].includes(sub) ? sub : "overview") as InsightsTab}
                onTab={(t) => go("insights", t)}
                onOpenSession={(id, device) => setTranscript({ id, device })}
              />
            )}
            {page === "devices" && <DevicesPage onError={onError} onToast={onToast} />}
            {page === "settings" && (
              <SettingsPage
                tab={(["start", "models", "index", "help"].includes(sub) ? sub : "start") as SettingsTab}
                onTab={(t) => go("settings", t)}
                go={go}
                onError={onError}
              />
            )}
            <div className={page === "rooms" ? "" : "hidden"}>
              <RoomsPage active={page === "rooms"} onError={onError} canPost={session?.can_post ?? true} onListening={setRoomsListening} />
            </div>
          </div>
        </main>

        {/* Phone: a bottom tab bar. */}
        <nav className="fixed inset-x-0 bottom-0 z-30 flex border-t bg-background/95 backdrop-blur md:hidden" aria-label="Main">
          {nav.map(([p, icon, label]) => (
            <a key={p} href={`#/${p}`} className={cn("flex flex-1 flex-col items-center gap-0.5 py-2 text-[11px] [&>svg]:h-5 [&>svg]:w-5", active(p) ? "text-primary" : "text-muted-foreground")}>
              {icon}
              {label}
            </a>
          ))}
        </nav>
      </div>

      <ThreadSheet
        target={openThread}
        threads={threads}
        devices={devices}
        localDevice={localDevice}
        readOnly={phone}
        onClose={() => setOpenThread(null)}
        onChanged={() => setBoardKey((k) => k + 1)}
        onOpenSession={(id, device) => setTranscript({ id, device })}
        onToast={onToast}
        onError={onError}
      />
      <TranscriptSheet
        id={transcript?.id ?? null}
        device={transcript?.device}
        onClose={() => setTranscript(null)}
        onShowThread={(id) => {
          setTranscript(null);
          setOpenThread({ id });
          go("threads");
        }}
        onError={onError}
      />
      <NewThreadDialog
        open={newThread}
        onOpenChange={setNewThread}
        onCreated={(_, m) => {
          onToast(m);
          setBoardKey((k) => k + 1);
        }}
        onError={onError}
      />

      {/* Notifications */}
      <div className="pointer-events-none fixed bottom-20 right-4 z-[60] flex w-[min(24rem,calc(100%-2rem))] flex-col gap-2 md:bottom-4">
        {toasts.map((t) => (
          <div
            key={t.id}
            role={t.error ? "alert" : "status"}
            className={cn(
              "pointer-events-auto flex items-start gap-2 rounded-lg border bg-popover p-3 text-sm shadow-lg animate-in slide-in-from-bottom-2",
              t.error && "border-destructive/50",
            )}
          >
            <span className={cn("mt-1.5 h-2 w-2 shrink-0 rounded-full", t.error ? "bg-destructive" : "bg-success")} />
            <span className="flex-1">{t.text}</span>
            <button className="text-muted-foreground hover:text-foreground" onClick={() => setToasts((x) => x.filter((y) => y.id !== t.id))} aria-label="Dismiss">
              <X className="h-4 w-4" />
            </button>
          </div>
        ))}
      </div>
    </TooltipProvider>
  );
}

function Dot({ live = false }: { live?: boolean }) {
  return <span className={cn("h-2 w-2 rounded-full", live ? "animate-pulse-dot bg-success" : "bg-primary")} />;
}

function Count({ n }: { n: number }) {
  return <span className="rounded-full bg-primary px-1.5 text-[10px] font-semibold leading-4 text-primary-foreground">{n}</span>;
}
