// The board on a phone (SPEC FR-BRD-04): the pairing screen a phone sees
// first, and the Devices → Phones panel on the computer.

import { useCallback, useEffect, useState } from "react";
import { Loader2, QrCode, Smartphone, Trash2 } from "lucide-react";
import {
  pairPhoneStart,
  phoneStatus,
  redeemPairingCode,
  revokePhone,
  setPhoneEnabled,
  setPhonePost,
  type PhonePairing,
  type PhoneStatus,
} from "./api";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Brand, Notice, useLatest } from "@/components/common";

function guessName(): string {
  const ua = navigator.userAgent;
  if (/iPhone/.test(ua)) return "iPhone";
  if (/iPad/.test(ua)) return "iPad";
  if (/Android/.test(ua)) return "Android phone";
  return "phone";
}

/** Shown on a phone that hasn't been paired. The code can come in the link. */
export function PairPhone() {
  const [code, setCode] = useState(() => decodeURIComponent(window.location.hash.slice(1)));
  const [name, setName] = useState(guessName);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await redeemPairingCode(code, name);
      window.location.replace("/");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  }

  return (
    <div className="flex min-h-full flex-col items-center bg-muted/30 px-4 py-10 sm:justify-center">
      <Brand className="mb-8" />
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-lg">
            <Smartphone className="size-5 text-primary" /> Pair this phone
          </CardTitle>
          <CardDescription className="leading-relaxed">
            On your computer, open Crossthreads → Devices → Add a device → A phone →{" "}
            <strong className="text-foreground">Pair a phone</strong>, then scan the QR code or type the code it shows.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} className="space-y-4">
            <div className="space-y-1.5">
              <Label htmlFor="pair-code">Code</Label>
              <Input
                id="pair-code"
                value={code}
                onChange={(e) => setCode(e.target.value)}
                autoCapitalize="characters"
                autoComplete="one-time-code"
                autoFocus={!code}
                placeholder="e.g. K7QM-2XWD"
                required
                className="h-11 text-center font-mono text-lg tracking-[0.2em]"
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="pair-name">Name this phone</Label>
              <Input id="pair-name" value={name} onChange={(e) => setName(e.target.value)} maxLength={40} className="h-10" />
            </div>
            {error && <Notice tone="error">{error}</Notice>}
            <Button type="submit" className="h-10 w-full" disabled={busy || !code.trim()}>
              {busy && <Loader2 className="animate-spin" />}
              {busy ? "Pairing…" : "Pair"}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}

function ago(ts: number): string {
  if (!ts) return "never";
  const s = Math.max(0, Date.now() / 1000 - ts);
  if (s < 90) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

/** Devices → Phones, on the computer. */
export function PhoneSettings({ onError }: { onError: (message: string) => void }) {
  const [status, setStatus] = useState<PhoneStatus | null>(null);
  const [pairing, setPairing] = useState<PhonePairing | null>(null);
  const [busy, setBusy] = useState(false);
  const onErrorRef = useLatest(onError);

  const run = useCallback(
    async (f: () => Promise<PhoneStatus>) => {
      setBusy(true);
      try {
        setStatus(await f());
      } catch (err) {
        onErrorRef.current(String(err));
      } finally {
        setBusy(false);
      }
    },
    [onErrorRef],
  );

  useEffect(() => {
    run(phoneStatus);
  }, [run]);

  async function pair() {
    try {
      setPairing(await pairPhoneStart());
      setStatus(await phoneStatus());
    } catch (err) {
      onError(String(err));
    }
  }

  if (!status)
    return (
      <div className="space-y-3">
        <Skeleton className="h-28" />
        <Skeleton className="h-20" />
      </div>
    );

  return (
    <div className="space-y-4">
      <Card className="shadow-none">
        <div className="flex items-start justify-between gap-4 p-5">
          <div className="min-w-0 space-y-1">
            <Label htmlFor="phone-access" className="cursor-pointer text-sm">
              Your board on your phone
            </Label>
            <p className="text-[13px] leading-relaxed text-muted-foreground">
              Open your threads, rooms and search from your phone's browser. It only listens on your Tailscale network, so
              your phone and this computer both need Tailscale, signed in to the same account. Phones can read; posting to
              rooms is a separate switch per phone. Nothing goes through a hosted service.
            </p>
            {status.listening && status.url && (
              <p className="pt-1 text-[13px] text-muted-foreground">
                Listening at <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs text-foreground">{status.url}</code>
              </p>
            )}
          </div>
          <Switch
            id="phone-access"
            checked={status.enabled}
            disabled={busy}
            onCheckedChange={(on) => run(() => setPhoneEnabled(on))}
            className="mt-0.5"
          />
        </div>
        {status.error && (
          <div className="px-5 pb-5">
            <Notice tone="error">{status.error}</Notice>
          </div>
        )}
      </Card>

      {status.listening && (
        <Card className="shadow-none">
          <CardHeader className="pb-3">
            <CardTitle className="text-sm">Pair a phone</CardTitle>
          </CardHeader>
          <CardContent>
            {pairing ? (
              <div className="flex flex-col gap-5 sm:flex-row sm:items-start">
                {pairing.qr_svg && (
                  <div
                    className="w-44 shrink-0 self-center rounded-lg border bg-white p-2.5 [&_svg]:h-auto [&_svg]:w-full"
                    aria-label="QR code for the pairing link"
                    // Generated by the daemon from the pairing link; no user input.
                    dangerouslySetInnerHTML={{ __html: pairing.qr_svg }}
                  />
                )}
                <div className="min-w-0 space-y-3">
                  <p className="text-[13px] leading-relaxed text-muted-foreground">
                    Scan with your phone's camera, or open{" "}
                    <code className="break-all font-mono text-xs text-foreground">{pairing.url.split("#")[0]}</code> on your
                    phone and enter:
                  </p>
                  <p className="font-mono text-3xl font-semibold tracking-[0.18em]">
                    {pairing.code.slice(0, 4)}-{pairing.code.slice(4)}
                  </p>
                  <p className="text-xs text-muted-foreground">
                    It works once, for {Math.round(pairing.expires_secs / 60)} minutes.
                  </p>
                  <Button variant="outline" size="sm" onClick={() => setPairing(null)}>
                    Done
                  </Button>
                </div>
              </div>
            ) : (
              <Button onClick={pair}>
                <QrCode /> Pair a phone
              </Button>
            )}
          </CardContent>
        </Card>
      )}

      {status.phones.length > 0 && (
        <Card className="overflow-hidden shadow-none">
          <p className="border-b px-5 py-3 text-sm font-semibold">Paired phones</p>
          <ul className="divide-y">
            {status.phones.map((p) => (
              <li key={p.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-5 py-3">
                <Smartphone className="size-4 shrink-0 text-muted-foreground" />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-medium">{p.name}</p>
                  <p className="text-xs text-muted-foreground">Seen {ago(p.last_seen)}</p>
                </div>
                <label
                  className="flex items-center gap-2 text-[13px]"
                  title="Let this phone post messages into rooms, where agents read them as yours"
                >
                  <Switch checked={p.can_post} disabled={busy} onCheckedChange={(on) => run(() => setPhonePost(p.id, on))} />
                  Can post to rooms
                </label>
                <Button
                  variant="ghost"
                  size="sm"
                  className="text-muted-foreground hover:text-destructive"
                  disabled={busy}
                  onClick={() => run(() => revokePhone(p.id))}
                >
                  <Trash2 /> Remove
                </Button>
              </li>
            ))}
          </ul>
        </Card>
      )}
    </div>
  );
}
