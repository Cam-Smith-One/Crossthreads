// Signing the web app in on this computer. The daemon answers only clients
// that present this account's local key; a browser can't read it, so
// `crossthreads open` gets a one-time ticket and opens /login#<ticket>.

import { useEffect, useState } from "react";
import { Loader2, LockKeyhole } from "lucide-react";
import { redeemSignInTicket } from "./api";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Brand, CommandLine, Notice } from "@/components/common";

export function SignIn() {
  const ticket = window.location.pathname === "/login" ? window.location.hash.slice(1) : "";
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!ticket) return;
    // Drop the ticket from the address bar and history straight away.
    window.history.replaceState(null, "", "/login");
    redeemSignInTicket(ticket)
      .then(() => window.location.replace("/"))
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, [ticket]);

  return (
    <div className="flex min-h-full flex-col items-center bg-muted/30 px-4 py-10 sm:justify-center">
      <Brand className="mb-8" />
      <Card className="w-full max-w-md">
        {ticket && !error ? (
          <CardContent className="flex items-center justify-center gap-3 py-12 text-sm text-muted-foreground">
            <Loader2 className="size-5 animate-spin text-primary" /> Signing in…
          </CardContent>
        ) : (
          <>
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-lg">
                <LockKeyhole className="size-5 text-primary" /> Open Crossthreads from your computer
              </CardTitle>
              <CardDescription className="leading-relaxed">
                Your history is private to your account, so this page needs a sign-in link. Run this in a terminal on this
                computer:
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <CommandLine cmd="crossthreads open" />
              <p className="text-[13px] leading-relaxed text-muted-foreground">
                It opens this app signed in. <code className="font-mono text-foreground">crossthreads-up</code> does this
                for you when it starts Crossthreads.
              </p>
              {error && <Notice tone="error" title="That sign-in link didn't work">{error}</Notice>}
            </CardContent>
          </>
        )}
      </Card>
    </div>
  );
}
