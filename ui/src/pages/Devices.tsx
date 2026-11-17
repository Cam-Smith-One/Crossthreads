import { useCallback, useEffect, useState } from "react";
import { Check, ChevronRight, Laptop, Monitor, Plus, ShieldCheck, Smartphone, Trash2 } from "lucide-react";
import {
  devicePairCancel,
  devicePairJoin,
  devicePairStart,
  getDevices,
  getFederationConfig,
  removePeer,
  setFederationConfig,
  setPeerRooms,
  type Device,
  type DevicePairCode,
  type FederationConfig,
} from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { PhoneSettings } from "@/Phone";
import { cn } from "@/lib/utils";

type Step = "choose" | "show" | "enter" | "phone";

/** Your computers and phones, and one way to add another. */
export function DevicesPage({ onError, onToast }: { onError: (m: string) => void; onToast: (m: string) => void }) {
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [config, setConfig] = useState<FederationConfig | null>(null);
  const [step, setStep] = useState<Step | null>(null);
  const [code, setCode] = useState<DevicePairCode | null>(null);
  const [left, setLeft] = useState(0);
  const [typed, setTyped] = useState("");
  const [addr, setAddr] = useState("");
  const [joining, setJoining] = useState(false);
  const [joinError, setJoinError] = useState<string | null>(null);

  const load = useCallback(() => {
    getDevices()
      .then((d) => setDevices(d.devices))
      .catch((e) => onError(String(e)));
    getFederationConfig().then(setConfig).catch(() => {});
  }, [onError]);
  useEffect(load, [load]);

  // While a code shows: count down, and notice the new computer arriving.
  useEffect(() => {
    if (step !== "show" || !code) return;
    const until = Date.now() + code.expires_in * 1000;
    const known = new Set((devices ?? []).map((d) => d.name));
    const timer = window.setInterval(() => {
      const s = Math.max(0, Math.round((until - Date.now()) / 1000));
      setLeft(s);
      getDevices()
        .then((d) => {
          setDevices(d.devices);
          const fresh = d.devices.find((x) => !known.has(x.name));
          if (fresh) {
            onToast(`Added ${fresh.name}. Your history, threads and rooms now span both computers.`);
            close(false);
          }
        })
        .catch(() => {});
      if (s === 0) setCode(null);
    }, 2000);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [step, code]);

  async function showCode() {
    setStep("show");
    try {
      const c = await devicePairStart();
      setCode(c);
      setLeft(c.expires_in);
    } catch (e) {
      onError(String(e));
      setStep(null);
    }
  }

  async function join() {
    setJoining(true);
    setJoinError(null);
    try {
      const r = await devicePairJoin(typed, addr.trim() || undefined);
      onToast(r.note ?? `Added ${r.name}. Your history, threads and rooms now span both computers.`);
      setTyped("");
      setAddr("");
      close(false);
      load();
    } catch (e) {
      setJoinError(String(e).replace(/^Error:\s*/, ""));
    } finally {
      setJoining(false);
    }
  }

  function close(cancel = true) {
    if (cancel && step === "show") devicePairCancel().catch(() => {});
    setStep(null);
    setCode(null);
    setJoinError(null);
  }

  const others = (devices ?? []).filter((d) => !d.local);
  const me = (devices ?? []).find((d) => d.local);
  const save = (patch: Parameters<typeof setFederationConfig>[0]) =>
    setFederationConfig(patch)
      .then(setConfig)
      .catch((e) => onError(String(e)));

  return (
    <div className="max-w-3xl space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Devices</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Your computers share search, threads and agent rooms, talking directly over your own network. Nothing leaves your devices.
          </p>
        </div>
        <Button onClick={() => setStep("choose")}>
          <Plus /> Add a device
        </Button>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Computers</CardTitle>
        </CardHeader>
        <CardContent className="space-y-2">
          {devices === null ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : (
            (devices ?? []).map((d) => (
              <div key={d.name} className="flex items-center gap-3 rounded-lg border px-3 py-2.5">
                <span className={cn("h-2 w-2 shrink-0 rounded-full", d.online ? "bg-success" : "bg-muted-foreground/40")} />
                <Monitor className="h-4 w-4 text-muted-foreground" />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-medium">
                    {d.name}
                    {d.local && <Badge variant="secondary" className="ml-2">This computer</Badge>}
                    {!d.local && !d.online && <Badge variant="outline" className="ml-2">Offline</Badge>}
                  </p>
                </div>
                {!d.local && (
                  <label className="flex items-center gap-2 text-xs text-muted-foreground" title="Agents here and there can message each other in rooms">
                    <Switch
                      checked={!!d.rooms}
                      onCheckedChange={(on) =>
                        setPeerRooms(d.name, on)
                          .then(setDevices)
                          .catch((e) => onError(String(e)))
                      }
                    />
                    Agents can message
                  </label>
                )}
                {!d.local && (
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Remove ${d.name}`}
                    onClick={() =>
                      removePeer(d.name)
                        .then(setDevices)
                        .catch((e) => onError(String(e)))
                    }
                  >
                    <Trash2 />
                  </Button>
                )}
              </div>
            ))
          )}
          {devices !== null && others.length === 0 && (
            <p className="pt-1 text-sm text-muted-foreground">
              Just {me ? me.name : "this computer"} so far. <button className="text-primary hover:underline" onClick={() => setStep("choose")}>Add another</button> to see all your work in one place.
            </p>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base"><Smartphone className="h-4 w-4" /> Phones</CardTitle>
          <CardDescription>Check what's running and what needs you from your phone. Read-only unless you allow more.</CardDescription>
        </CardHeader>
        <CardContent>
          <PhoneSettings onError={onError} />
        </CardContent>
      </Card>

      {config && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base"><ShieldCheck className="h-4 w-4" /> This computer</CardTitle>
            <CardDescription>
              {config.listen_addr
                ? `Reachable on Tailscale at ${config.listen_addr}, only by your paired devices.`
                : config.relay_active
                  ? `Reachable through your relay at ${config.relay}, only by your paired devices.`
                  : "Not on Tailscale yet: install it and sign in (or set a relay), then reopen Crossthreads to add devices."}
            </CardDescription>
          </CardHeader>
          <CardContent className="grid gap-4 sm:grid-cols-2">
            <Field label="Name" hint="How your other devices see it.">
              <Input defaultValue={config.device} key={`n-${config.device}`} onBlur={(e) => e.target.value.trim() && e.target.value.trim() !== config.device && save({ device: e.target.value.trim() })} />
            </Field>
            <Field label="Relay" hint="Only without Tailscale: a crossthreads-relay you run. Reopen to apply.">
              <Input placeholder="host:47200" defaultValue={config.relay ?? ""} key={`r-${config.relay ?? ""}`} onBlur={(e) => e.target.value.trim() !== (config.relay ?? "") && save({ relay: e.target.value.trim() })} />
            </Field>
            <Field label="Keep these tools private" hint="Comma-separated, e.g. cursor, codex.">
              <Input defaultValue={config.exclude_tools.join(", ")} key={`t-${config.exclude_tools.join(",")}`} onBlur={(e) => save({ exclude_tools: csv(e.target.value) })} />
            </Field>
            <Field label="Keep these folders private" hint="Parts of a path, comma-separated.">
              <Input defaultValue={config.exclude_projects.join(", ")} key={`p-${config.exclude_projects.join(",")}`} onBlur={(e) => save({ exclude_projects: csv(e.target.value) })} />
            </Field>
          </CardContent>
        </Card>
      )}

      <Dialog open={step !== null} onOpenChange={(o) => !o && close()}>
        <DialogContent className={cn(step === "phone" ? "sm:max-w-2xl" : "sm:max-w-md")}>
          <DialogHeader>
            <DialogTitle>
              {step === "show" ? "Type this on your other computer" : step === "enter" ? "Join your other computer" : step === "phone" ? "Add a phone" : "Add a device"}
            </DialogTitle>
            <DialogDescription>
              {step === "choose" && "Both need Crossthreads open and Tailscale (or your relay) connected."}
              {step === "show" && "On the other computer: Devices → Add a device → This computer."}
              {step === "enter" && "Type the code your other computer is showing."}
            </DialogDescription>
          </DialogHeader>

          {step === "choose" && (
            <div className="grid gap-2">
              <Choice icon={<Laptop />} title="Another computer" text="Show a code here, type it there" onClick={showCode} />
              <Choice icon={<Monitor />} title="This computer" text="I have a code from my other computer" onClick={() => setStep("enter")} />
              <Choice icon={<Smartphone />} title="A phone" text="Scan a QR code to see your board" onClick={() => setStep("phone")} />
            </div>
          )}

          {step === "show" && (
            <div className="space-y-4">
              {code?.problem && <p className="rounded-md bg-warning/10 p-3 text-sm text-warning">{code.problem}</p>}
              {code ? (
                <>
                  <div className="select-all rounded-xl border bg-muted/50 py-6 text-center font-mono text-4xl font-semibold tracking-[0.2em]">{code.code}</div>
                  <p className="text-center text-sm text-muted-foreground">
                    Works once, for {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")}. This closes when it joins.
                  </p>
                  {code.addr && (
                    <p className="text-center text-xs text-muted-foreground">
                      If it can't find {code.device}, also give it <span className="font-mono">{code.addr.split(":")[0]}</span>
                    </p>
                  )}
                </>
              ) : (
                <div className="text-center">
                  <p className="mb-2 text-sm text-muted-foreground">The code expired.</p>
                  <Button variant="secondary" onClick={showCode}>Show a new code</Button>
                </div>
              )}
            </div>
          )}

          {step === "enter" && (
            <form
              className="space-y-4"
              onSubmit={(e) => {
                e.preventDefault();
                if (!joining) join();
              }}
            >
              <Input
                autoFocus
                className="h-14 text-center font-mono text-2xl uppercase tracking-[0.2em]"
                placeholder="K7PQ-4XMN"
                maxLength={12}
                value={typed}
                onChange={(e) => setTyped(e.target.value.toUpperCase())}
                aria-label="Code"
              />
              {joinError && (
                <div className="space-y-2">
                  <p className="text-sm text-destructive">{joinError}</p>
                  <Input placeholder="Its Tailscale address, e.g. 100.101.102.103 (optional)" value={addr} onChange={(e) => setAddr(e.target.value)} />
                </div>
              )}
              <Button type="submit" className="w-full" disabled={joining || typed.replace(/[^A-Za-z0-9]/g, "").length !== 8}>
                {joining ? "Looking for it…" : <><Check /> Add</>}
              </Button>
            </form>
          )}

          {step === "phone" && <PhoneSettings onError={onError} />}
        </DialogContent>
      </Dialog>
    </div>
  );
}

function Field({ label, hint, children }: { label: string; hint: string; children: React.ReactNode }) {
  return (
    <div className="grid gap-1.5">
      <Label>{label}</Label>
      {children}
      <p className="text-xs text-muted-foreground">{hint}</p>
    </div>
  );
}

function Choice({ icon, title, text, onClick }: { icon: React.ReactNode; title: string; text: string; onClick: () => void }) {
  return (
    <button onClick={onClick} className="flex items-center gap-3 rounded-lg border p-4 text-left transition hover:border-primary hover:bg-accent/50 [&>svg]:h-5 [&>svg]:w-5 [&>svg]:text-muted-foreground">
      {icon}
      <span className="flex-1">
        <span className="block font-medium">{title}</span>
        <span className="block text-sm text-muted-foreground">{text}</span>
      </span>
      <ChevronRight className="!h-4 !w-4" />
    </button>
  );
}

function csv(s: string): string[] {
  return s
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);
}
