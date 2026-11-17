// The board on a phone (SPEC FR-BRD-04): the pairing screen a phone sees
// first, and the Devices → Phones panel on the computer.

import { useCallback, useEffect, useState } from "react";
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

function guessName(): string {
  const ua = navigator.userAgent;
  if (/iPhone/.test(ua)) return "iPhone";
  if (/iPad/.test(ua)) return "iPad";
  if (/Android/.test(ua)) return "Android phone";
  return "phone";
}

/** Shown on a phone that hasn't been paired. The code can come in the link. */
export function PairPhone() {
  const [code, setCode] = useState(() =>
    decodeURIComponent(window.location.hash.slice(1)),
  );
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
      <form className="pair-card" onSubmit={submit}>
        <h2>Pair this phone</h2>
        <p className="settings-hint">
          On your computer, open Crossthreads → Devices → Add a device → A phone →{" "}
          <strong>Pair a phone</strong>, then scan the QR code or type the code
          it shows.
        </p>
        <label>
          Code
          <input
            value={code}
            onChange={(e) => setCode(e.target.value)}
            autoCapitalize="characters"
            autoComplete="one-time-code"
            placeholder="e.g. K7QM-2XWD"
            required
          />
        </label>
        <label>
          Name this phone
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            maxLength={40}
          />
        </label>
        {error && <p className="pair-error">{error}</p>}
        <button type="submit" disabled={busy || !code.trim()}>
          {busy ? "Pairing…" : "Pair"}
        </button>
      </form>
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
export function PhoneSettings({
  onError,
}: {
  onError: (message: string) => void;
}) {
  const [status, setStatus] = useState<PhoneStatus | null>(null);
  const [pairing, setPairing] = useState<PhonePairing | null>(null);
  const [busy, setBusy] = useState(false);

  const run = useCallback(
    async (f: () => Promise<PhoneStatus>) => {
      setBusy(true);
      try {
        setStatus(await f());
      } catch (err) {
        onError(String(err));
      } finally {
        setBusy(false);
      }
    },
    [onError],
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

  if (!status) return <p className="settings-hint">Loading…</p>;

  return (
    <>
      <section className="settings-section">
        <h3>Your board on your phone</h3>
        <p className="settings-hint">
          Open your threads, rooms and search from your phone's browser. It only
          listens on your Tailscale network, so your phone and this computer
          both need Tailscale, signed in to the same account. Phones can read;
          posting to rooms is a separate switch per phone. Nothing goes through
          a hosted service.
        </p>
        <label className="phone-toggle">
          <input
            type="checkbox"
            checked={status.enabled}
            disabled={busy}
            onChange={(e) => run(() => setPhoneEnabled(e.target.checked))}
          />
          Phone access
        </label>
        {status.listening && status.url && (
          <p className="settings-hint">
            Listening at <code>{status.url}</code>
          </p>
        )}
        {status.error && <p className="pair-error">{status.error}</p>}
      </section>

      {status.listening && (
        <section className="settings-section">
          <h3>Pair a phone</h3>
          {pairing ? (
            <div className="phone-pairing">
              {pairing.qr_svg && (
                <div
                  className="phone-qr"
                  aria-label="QR code for the pairing link"
                  // Generated by the daemon from the pairing link; no user input.
                  dangerouslySetInnerHTML={{ __html: pairing.qr_svg }}
                />
              )}
              <div>
                <p className="settings-hint">
                  Scan with your phone's camera, or open{" "}
                  <code>{pairing.url.split("#")[0]}</code> on your phone and
                  enter:
                </p>
                <code className="fed-pairing-code phone-code">
                  {pairing.code.slice(0, 4)}-{pairing.code.slice(4)}
                </code>
                <p className="settings-hint">
                  It works once, for {Math.round(pairing.expires_secs / 60)}{" "}
                  minutes.
                </p>
                <button className="secondary" onClick={() => setPairing(null)}>
                  Done
                </button>
              </div>
            </div>
          ) : (
            <button onClick={pair}>Pair a phone</button>
          )}
        </section>
      )}

      {status.phones.length > 0 && (
        <section className="settings-section">
          <h3>Paired phones</h3>
          <ul className="device-list">
            {status.phones.map((p) => (
              <li key={p.id} className="device-row">
                <span className="device-name">
                  {p.name}
                  <span className="device-tag">seen {ago(p.last_seen)}</span>
                </span>
                <label
                  className="device-grant"
                  title="Let this phone post messages into rooms, where agents read them as yours"
                >
                  <input
                    type="checkbox"
                    checked={p.can_post}
                    onChange={(e) =>
                      run(() => setPhonePost(p.id, e.target.checked))
                    }
                  />
                  Can post to rooms
                </label>
                <button
                  className="linklike"
                  onClick={() => run(() => revokePhone(p.id))}
                >
                  remove
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
}
