import { useCallback, useEffect, useState } from "react";
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
} from "./api";
import { PhoneSettings } from "./Phone";

type Adding = null | "choose" | "show" | "enter" | "phone";

/** The Devices page: your computers and phones, and one way to add another. */
export function Devices({ onError }: { onError: (message: string) => void }) {
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [config, setConfig] = useState<FederationConfig | null>(null);
  const [adding, setAdding] = useState<Adding>(null);
  const [pairCode, setPairCode] = useState<DevicePairCode | null>(null);
  const [left, setLeft] = useState(0);
  const [typed, setTyped] = useState("");
  const [addr, setAddr] = useState("");
  const [joining, setJoining] = useState(false);
  const [joined, setJoined] = useState<{ name: string; note?: string | null } | null>(null);
  const [joinError, setJoinError] = useState<string | null>(null);

  const load = useCallback(() => {
    getDevices()
      .then((d) => setDevices(d.devices))
      .catch((e) => onError(String(e)));
    getFederationConfig()
      .then(setConfig)
      .catch(() => {});
  }, [onError]);
  useEffect(load, [load]);

  // While a code is showing, count down and watch for the new computer.
  useEffect(() => {
    if (adding !== "show" || !pairCode) return;
    const until = Date.now() + pairCode.expires_in * 1000;
    const known = new Set((devices ?? []).map((d) => d.name));
    const timer = window.setInterval(() => {
      const s = Math.max(0, Math.round((until - Date.now()) / 1000));
      setLeft(s);
      getDevices()
        .then((d) => {
          setDevices(d.devices);
          const fresh = d.devices.find((x) => !known.has(x.name));
          if (fresh) {
            setJoined({ name: fresh.name });
            setAdding(null);
            setPairCode(null);
          }
        })
        .catch(() => {});
      if (s === 0) setPairCode(null);
    }, 2000);
    return () => window.clearInterval(timer);
    // `devices` is read once, when the code is shown.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [adding, pairCode]);

  async function showCode() {
    setAdding("show");
    setJoined(null);
    try {
      const c = await devicePairStart();
      setPairCode(c);
      setLeft(c.expires_in);
    } catch (e) {
      onError(String(e));
      setAdding(null);
    }
  }

  async function join() {
    setJoining(true);
    setJoinError(null);
    try {
      const r = await devicePairJoin(typed, addr.trim() || undefined);
      setJoined(r);
      setAdding(null);
      setTyped("");
      setAddr("");
      load();
    } catch (e) {
      setJoinError(String(e).replace(/^Error:\s*/, ""));
    } finally {
      setJoining(false);
    }
  }

  function close() {
    if (adding === "show") devicePairCancel().catch(() => {});
    setAdding(null);
    setPairCode(null);
    setJoinError(null);
  }

  const others = (devices ?? []).filter((d) => !d.local);
  const me = (devices ?? []).find((d) => d.local);

  return (
    <div className="devices-page">
      <section className="settings-section">
        <div className="devices-head">
          <h3>Computers</h3>
          {adding === null && <button onClick={() => setAdding("choose")}>Add a device</button>}
        </div>
        <p className="settings-hint">
          Your computers search each other's history, share threads, and their agents can
          message each other. Everything stays on your devices: they talk directly over your
          Tailscale network.
        </p>

        {joined && (
          <div className="nudge">
            <div>
              <strong>✓ Added {joined.name}</strong>
              <span className="nudge-next">
                {joined.note ?? "Your history, threads and rooms now span both computers."}
              </span>
            </div>
            <button className="linklike" onClick={() => setJoined(null)}>
              Dismiss
            </button>
          </div>
        )}

        {adding !== null && (
          <div className="add-device">
            {adding === "choose" && (
              <>
                <p>What are you adding?</p>
                <div className="add-device-choices">
                  <button onClick={showCode}>
                    <strong>Another computer</strong>
                    <span>Show a code here, type it there</span>
                  </button>
                  <button onClick={() => setAdding("enter")}>
                    <strong>This computer</strong>
                    <span>I have a code from my other computer</span>
                  </button>
                  <button onClick={() => setAdding("phone")}>
                    <strong>A phone</strong>
                    <span>Scan a QR code to see your board</span>
                  </button>
                </div>
              </>
            )}

            {adding === "show" && (
              <>
                {pairCode?.problem && <p className="pair-error">{pairCode.problem}</p>}
                {pairCode ? (
                  <>
                    <p>
                      On your other computer, open Crossthreads → <strong>Devices</strong> →{" "}
                      <strong>Add a device</strong> → <strong>This computer</strong>, and type:
                    </p>
                    <code className="device-code">{pairCode.code}</code>
                    <p className="settings-hint">
                      Works once, for {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")}.
                      This page updates when it joins.
                      {pairCode.addr && (
                        <>
                          {" "}
                          If it can't find {pairCode.device}, also give it this address:{" "}
                          <code>{pairCode.addr.split(":")[0]}</code>
                        </>
                      )}
                    </p>
                  </>
                ) : (
                  <p className="settings-hint">
                    The code expired. <button className="linklike" onClick={showCode}>Show a new one</button>
                  </p>
                )}
              </>
            )}

            {adding === "enter" && (
              <form
                className="add-device-enter"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (!joining) join();
                }}
              >
                <p>Type the code your other computer is showing.</p>
                <input
                  autoFocus
                  className="device-code-input"
                  placeholder="K7PQ-4XMN"
                  value={typed}
                  maxLength={12}
                  onChange={(e) => setTyped(e.target.value.toUpperCase())}
                  aria-label="Code from your other computer"
                />
                {joinError && (
                  <>
                    <p className="pair-error">{joinError}</p>
                    <label className="fed-row">
                      <span className="fed-label">address</span>
                      <input
                        placeholder="its Tailscale address, e.g. 100.101.102.103"
                        value={addr}
                        onChange={(e) => setAddr(e.target.value)}
                      />
                    </label>
                  </>
                )}
                <button type="submit" disabled={joining || typed.replace(/[^A-Za-z0-9]/g, "").length !== 8}>
                  {joining ? "Looking for it…" : "Add"}
                </button>
              </form>
            )}

            {adding === "phone" && <PhoneSettings onError={onError} />}

            <button className="linklike add-device-cancel" onClick={close}>
              {adding === "phone" ? "Done" : "Cancel"}
            </button>
          </div>
        )}

        {devices === null ? (
          <p className="settings-hint">Loading…</p>
        ) : others.length === 0 ? (
          <p className="settings-hint">
            Just this computer{me ? ` (${me.name})` : ""} so far.
          </p>
        ) : (
          <ul className="device-list">
            {(devices ?? []).map((d) => (
              <li key={d.name} className="device-row">
                <span className={`device-dot ${d.online ? "online" : "offline"}`} />
                <span className="device-name">
                  {d.name}
                  {d.local && <span className="device-tag">this computer</span>}
                  {!d.local && !d.online && <span className="device-tag">offline</span>}
                </span>
                {!d.local && (
                  <label
                    className="device-grant"
                    title="Agents here and on this computer can message each other in rooms"
                  >
                    <input
                      type="checkbox"
                      checked={!!d.rooms}
                      onChange={(e) =>
                        setPeerRooms(d.name, e.target.checked)
                          .then(setDevices)
                          .catch((err) => onError(String(err)))
                      }
                    />
                    Agents can message
                  </label>
                )}
                {!d.local && (
                  <button
                    className="linklike"
                    onClick={() =>
                      removePeer(d.name)
                        .then(setDevices)
                        .catch((err) => onError(String(err)))
                    }
                  >
                    remove
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>

      {adding !== "phone" && (
        <details className="settings-section devices-phones">
          <summary>Phones</summary>
          <PhoneSettings onError={onError} />
        </details>
      )}

      {config && (
        <details className="settings-section">
          <summary>This computer's name, privacy and connection</summary>
          <label className="fed-row">
            <span className="fed-label">name</span>
            <input
              defaultValue={config.device}
              key={`name-${config.device}`}
              onBlur={(e) => {
                const v = e.target.value.trim();
                if (v && v !== config.device)
                  setFederationConfig({ device: v })
                    .then(setConfig)
                    .catch((err) => onError(String(err)));
              }}
            />
          </label>
          <p className="settings-hint">
            Keep some history to this computer: tools or project folders your other devices won't
            see. Comma-separated.
          </p>
          <label className="fed-row">
            <span className="fed-label">tools</span>
            <input
              defaultValue={config.exclude_tools.join(", ")}
              key={`et-${config.exclude_tools.join(",")}`}
              placeholder="e.g. cursor, codex"
              onBlur={(e) =>
                setFederationConfig({ exclude_tools: splitCsv(e.target.value) })
                  .then(setConfig)
                  .catch((err) => onError(String(err)))
              }
            />
          </label>
          <label className="fed-row">
            <span className="fed-label">projects</span>
            <input
              defaultValue={config.exclude_projects.join(", ")}
              key={`ep-${config.exclude_projects.join(",")}`}
              placeholder="path substrings"
              onBlur={(e) =>
                setFederationConfig({ exclude_projects: splitCsv(e.target.value) })
                  .then(setConfig)
                  .catch((err) => onError(String(err)))
              }
            />
          </label>
          <p className="settings-hint">
            No Tailscale? Devices can meet through a relay instead: it forwards only encrypted
            traffic it can't read. Run your own with <code>crossthreads-relay</code> (see the relay
            guide), enter its address here on each computer, and reopen Crossthreads.
          </p>
          <label className="fed-row">
            <span className="fed-label">relay</span>
            <input
              defaultValue={config.relay ?? ""}
              key={`relay-${config.relay ?? ""}`}
              placeholder="host:47200 (optional)"
              onBlur={(e) => {
                const v = e.target.value.trim();
                if (v !== (config.relay ?? ""))
                  setFederationConfig({ relay: v })
                    .then(setConfig)
                    .catch((err) => onError(String(err)));
              }}
            />
          </label>
          {config.relay && !config.relay_active && (
            <p className="settings-hint">Saved. Quit and reopen Crossthreads to start using it.</p>
          )}
          <p className="settings-hint">
            {config.listen_addr
              ? `Reachable on Tailscale at ${config.listen_addr}, only by your paired devices.`
              : config.relay_active
                ? `Reachable through the relay at ${config.relay}, only by your paired devices.`
                : "Not on Tailscale: install it and sign in (or set a relay), then reopen Crossthreads to add devices."}
          </p>
        </details>
      )}
    </div>
  );
}

function splitCsv(s: string): string[] {
  return s
    .split(",")
    .map((x) => x.trim())
    .filter(Boolean);
}
