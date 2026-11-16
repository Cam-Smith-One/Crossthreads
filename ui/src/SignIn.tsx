// Signing the web app in on this computer. The daemon answers only clients
// that present this account's local key; a browser can't read it, so
// `crossthreads open` gets a one-time ticket and opens /login#<ticket>.

import { useEffect, useState } from "react";
import { redeemSignInTicket } from "./api";

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
    <div className="app pair-phone">
      <header>
        <div className="brand-row">
          <img className="brand-mark" src="/mark.png" alt="Crossthreads" />
          <h1 className="brand-name">
            <span className="brand-cross">Cross</span>
            <span className="brand-threads">threads</span>
          </h1>
        </div>
      </header>
      <div className="pair-card">
        {ticket && !error ? (
          <h2>Signing in…</h2>
        ) : (
          <>
            <h2>Open Crossthreads from your computer</h2>
            <p className="settings-hint">
              Your history is private to your account, so this page needs a sign-in link. Run this
              in a terminal on this computer:
            </p>
            <code className="signin-cmd">crossthreads open</code>
            <p className="settings-hint">
              It opens this app signed in. <code>crossthreads-up</code> does this for you when it
              starts Crossthreads.
            </p>
            {error && <p className="pair-error">{error}</p>}
          </>
        )}
      </div>
    </div>
  );
}
