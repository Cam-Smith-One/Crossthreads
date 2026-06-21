# Set up other devices (cross-device search)

Crossthreads indexes the AI-coding history on **one** machine. This guide walks
you through making several of your own machines searchable together, so a search
on your laptop also surfaces threads from your desktop.

> **Status.** Cross-device search is query **federation** over a private tunnel
> ([ADR-010](DECISIONS.md#adr-010-cross-device-search--query-federation-over-a-private-tunnel),
> full design in [CROSS_DEVICE_SEARCH.md](CROSS_DEVICE_SEARCH.md)) and is
> **implemented end to end**: discover and approve devices in **Settings →
> Devices**, choose which to search with the device picker, and see each result
> tagged with its origin. This guide is the full walkthrough.

Nothing here uploads your history anywhere. Each device keeps its own index;
searches travel only between *your* devices over *your* encrypted tunnel — no
server, no account, no telemetry.

---

## Before you start

- **2+ machines** you own (laptop, desktop, work box…), each running macOS,
  Linux, or Windows.
- Crossthreads installed on each (below).
- ~10 minutes.

---

## Step 1 — Install Crossthreads on every device

On each machine, install and confirm it indexes that machine's history:

```sh
# from a release
./install.sh
# …or from source
scripts/start.sh
```

Open the UI (`http://127.0.0.1:47101`) and confirm the status line shows a
conversation count. Repeat on every device you want to search. Each device now
has its own local index — the thing we're about to connect.

## Step 2 — Put every device on one private network (Tailscale)

Crossthreads delegates the hard parts of "securely reach my other PCs"
(encryption, stable addressing, NAT traversal) to
[Tailscale](https://tailscale.com) (WireGuard). It's the recommended path and
free for personal use.

1. Install Tailscale on **each** device: <https://tailscale.com/download>.
2. Sign in to the **same account** on every device — that's your *tailnet*.
3. Verify they see each other. On one machine:
   ```sh
   tailscale status
   ```
   You should see each other device with a `100.x.y.z` address and a MagicDNS
   name like `mac-mini` or `linux-desktop`. Note those names — you'll use them.

> **LAN-only / no Tailscale?** You can instead use raw `host:port` peers on a
> trusted local network, but then you own the firewall/NAT yourself. Tailscale is
> strongly recommended for anything beyond one LAN.

## Step 3 — Make each device searchable on the tailnet

By default the daemon listens on loopback only (`127.0.0.1:47100`), which other
devices can't reach.

**The easy way (recommended).** Set a shared token once in **Settings →
Devices**, then just run `crossthreads-up`. The launcher passes `--addr auto`,
which binds to this device's Tailscale IP **automatically — but only once a
token is set** (so the daemon is never exposed on the tailnet unauthenticated).
This device's **pairing code** then appears in Settings → Devices; reveal it with
**Show** and paste it on your other device's "Add a device by code".

**The manual way.** If you run the daemon yourself, bind it to that device's
tailnet address with `--addr` (or `--addr auto`), give it a `--device-name`
(shown on results), and set a shared `--fed-token` (the same secret on every
device):

```sh
# replace with this device's own Tailscale IP from `tailscale status`
crossthreadsd \
  --addr 100.x.y.z:47100 \
  --device-name linux-desktop \
  --fed-token "your-shared-secret" \
  --http 127.0.0.1:47101 --ui ui/dist
```

```sh
# or via env (e.g. in your start script)
CROSSTHREADS_ADDR=100.x.y.z:47100 \
CROSSTHREADS_DEVICE_NAME=linux-desktop \
CROSSTHREADS_FED_TOKEN=your-shared-secret \
  scripts/start.sh
```

Bind to the **tailnet interface, never `0.0.0.0`** — that keeps the daemon
reachable only from your own tailnet, not the open internet. The token is a
defence-in-depth check on top of the tailnet; pick anything and reuse it
verbatim on every device.

## Step 4 — Discover and approve your devices

On your **main** device (the one you search from), open Crossthreads and:

1. Click **⚙️ Settings → Devices**.
2. Click **Discover my devices**. Crossthreads runs a one-shot
   `tailscale status` scan, probes each machine for a running daemon, and lists
   the ones it finds.
3. Click **Approve** next to each device you want to search. Approved devices are
   **persisted** (to `federation.json`) and reloaded on restart — you approve
   once per machine.

Discovery is **on-demand**: Crossthreads never scans your network in the
background. Re-run **Discover my devices** whenever you add a machine.

> **Prefer the command line?** You can list peers explicitly instead of
> discovering them — add a `--peer NAME=ADDR` per device (or set
> `CROSSTHREADS_PEERS="linux-desktop=100.x.y.z:47100,work-laptop=100.p.q.r:47100"`):
>
> ```sh
> crossthreadsd --addr 100.a.b.c:47100 --device-name mac-mini \
>   --fed-token "your-shared-secret" \
>   --peer linux-desktop=100.x.y.z:47100 --http 127.0.0.1:47101 --ui ui/dist
> ```

## Step 5 — Search across everything

Run a search as usual. Results now span every reachable peer, merged into one
ranked list. Each result shows a **device chip** so you know where it came from:

```
📁 linux-desktop · Cursor · 2026-05-20
```

- A device that's **offline** is skipped automatically — your search still
  returns instantly from the devices that are up, and a small **"N devices
  unreachable"** banner tells you results may be partial.
- Click a result from another device to read its **full transcript** (fetched on
  demand); its source row shows "source on `<device>`".
- The same thread that exists on two machines collapses to a single result.
- Agent tools (`crossthreads_search`, `recall`, `build_context`) span devices
  too — see [AGENT_API](AGENT_API.md).

## More in Settings → Devices

- **Pair a device the easy way.** Instead of discovery, copy the **pairing code**
  from one device (it encodes the shared token + that device's address) and paste
  it into **"Add a device by code"** on another. The second device adopts the
  token and approves the first in one step. (A pairing code only appears once the
  device has a token and is bound to a tailnet address.)
- **Set your name + token in the app.** You don't have to use flags — set this
  device's name and shared token right in the panel. The token is stored in your
  **OS keychain** when available (it falls back to the config file on headless
  machines), never shown again after it's set.
- **Don't share everything.** Under "Don't share with peers", list tools or
  project substrings this device should keep private; peers won't see those
  threads in search *or* be able to open them.

---

## Troubleshooting

**A device doesn't show up in "Discover my devices."**
Confirm, on that device: (a) `tailscale status` lists it and the others; (b) the
daemon was started with `--addr 100.x.y.z:47100` (its own tailnet IP), not just
loopback; (c) no firewall is blocking port `47100` on the tailnet interface.

**It's found but searches skip it.**
The daemon may be down or the shared token may not match. Restart the daemon on
that device and re-approve it. Offline peers are skipped by design — a brief
timeout, never a hang.

**I'm on a LAN without Tailscale.**
Use raw `host:port` peers and make sure each daemon binds the LAN interface and
the port is open. This is best-effort and not recommended off a trusted LAN.

**Is my history leaving my machine?**
Only matching results travel, and only between your own devices over your own
encrypted tailnet. No third party, no telemetry. See the trust-boundary section
in [CROSS_DEVICE_SEARCH.md](CROSS_DEVICE_SEARCH.md#trust-boundary-be-explicit).

For anything else, see [TROUBLESHOOTING.md](TROUBLESHOOTING.md).
