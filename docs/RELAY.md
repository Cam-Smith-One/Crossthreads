# The relay

Crossthreads devices can reach each other without Tailscale, through a
**relay**: a small server that matches connections up and copies bytes
between them. Everything it forwards is encrypted end to end between your
devices, so the relay can't read it, and can't change it without the devices
noticing. You can run your own; it's one binary with no state on disk.

Two crates implement it:

- `ct-tunnel`: the device side (identity key, pairing, `connect`, `listen`).
- `ct-relay`: the relay (library, plus the `crossthreads-relay` binary).

**In one line:** each device has a long-term key; you pair two devices once
with an 8-character code; after that they talk over Noise KK with both keys
pinned, and the relay in the middle sees only ciphertext.

## What the relay operator can and cannot see

| The relay operator **can** see | The relay operator **cannot** |
|---|---|
| The IP addresses of your devices, and when they connect | Read anything your devices send each other |
| Each device's public key (that's how it routes) | Change, replay, reorder or inject data without the receiving device rejecting it |
| Which public key connects to which, when, for how long | Pretend to be one of your devices (it doesn't have their secret keys) |
| How many bytes go each way, and their timing | Register a device under someone else's public key |
| That a pairing happened, and the first 4 characters of its code | Pair a device of its own with yours without the code |

What the operator *can* do is deny service: drop connections, refuse to
forward, or cut a connection short (the devices see an error, not a
truncated message that looks complete).

**Pairing and a malicious relay.** The code is 8 characters from a 31-letter
alphabet (about 39.6 bits). The first 4 name the rendezvous at the relay,
so the relay learns those. The whole code is the password for SPAKE2, a
password-authenticated key exchange: someone in the middle gets exactly one
guess at the remaining 4 characters per pairing, with no way to check
guesses offline. A relay that tries has a 1 in 923,521 chance, and when it
guesses wrong the pairing visibly fails on both devices. A failed pairing
should be treated as a warning: try again with a new code, and if it keeps
failing, use another relay.

**Out of scope:** malware on your devices (it can read the key file),
traffic analysis (sizes and timing), and availability. Public keys are not
secret; treat them like a phone number.

## How it works

### Identity

On first use each device creates a random X25519 key pair and saves the
secret to a file readable only by your account (`0600` on Unix; on Windows
it relies on the per-user folder). A corrupt key file is an error, never
silently replaced, because a new key unpairs the device everywhere. The
**fingerprint** (base32 of the first 80 bits of SHA-256 of the public key,
like `ABCD-EFGH-IJKL-MNOP`) is for people to compare.

### Framing

Every message on a relay connection is a frame: a 2-byte big-endian length,
then up to 65,535 bytes. The first frame on every new connection is a small
plaintext JSON *hello* (at most 1 KiB) saying what the connection is for:
`register`, `join`, `accept` or `pair`. The relay answers with a JSON reply
(`{"ok":true,"code":"spliced"}` or an error code such as `offline`,
`refused`, `busy`, `limit`).

### Registration (a device staying reachable)

A device that wants to be reachable keeps one control connection open:

1. Device → relay: `{"op":"register","key":"<public key, hex>"}`.
2. A Noise `KN` handshake, the relay as responder, with the claimed key as
   the device's static key. The relay's reply mixes in a Diffie-Hellman
   between its fresh ephemeral key and the claimed public key, so only the
   holder of that key's secret can derive the session keys.
3. Device → relay, encrypted: `ready`. This is the proof: the relay
   registers the key only if it decrypts. A replay is useless, since every
   handshake uses a fresh relay ephemeral.

The relay doesn't need a key of its own for this. The control connection
stays encrypted afterwards; the device pings every 30 seconds, and either
side drops a silent connection after 90–120 seconds. A device that
reconnects replaces its older registration. If the relay is unreachable the
device retries with backoff (1 s doubling to 60 s).

### Joining (one device connecting to another)

1. Initiator → relay: `{"op":"join","peer":"<B's key>","from":"<A's key>"}`.
2. The relay tells B, over B's encrypted control connection: `incoming`,
   with a random one-time token and A's key.
3. B checks A's key against its paired devices. Unknown: it declines and
   A gets `refused`. Known: B opens a new connection with
   `{"op":"accept","token":...}`.
4. The relay answers both with `spliced` and from then on copies bytes
   between the two connections, nothing else.
5. A and B run **Noise KK** (`Noise_KK_25519_ChaChaPoly_BLAKE2s`) over the
   spliced pipe. Both static keys were pinned at pairing; the prologue binds
   the protocol version and both keys. KK's first message only decrypts if
   the initiator holds the secret for the key B pinned, and the reply only
   if B holds its own, so each side authenticates the other. Knowing a
   paired device's public key is not enough.
6. The result is a `SecureStream`: `Read + Write`, so the daemon's
   newline-delimited JSON runs over it unchanged.

Inside the tunnel each Noise transport message carries at most 65,518 bytes
of data plus a 1-byte type (data or close); larger writes are split. A
connection that ends without the encrypted close message is reported as
cut (`UnexpectedEof`), not as a clean end of stream. A message that fails
authentication (anything flipped, replayed or reordered) is a
decryption error (`InvalidData`), and the stream stays failed: bad data is
never handed to the application.

### Pairing

The host shows a code like `K7QM-3XRB`; the other device types it in.

1. Host → relay: `{"op":"pair","slot":"K7QM","role":"host","ttl_secs":600}`.
   The relay holds the slot open (default 10 minutes, at most 15).
2. Joiner → relay: the same slot, `"role":"join"`. The relay splices the two
   connections and **burns the slot**: whatever happens next, that code
   can't be tried again.
3. Both run SPAKE2 (Ed25519 group, host as A, joiner as B) with the whole
   code as the password.
4. Keys are derived with HKDF-SHA256 from the SPAKE2 key, salted with a hash
   of the slot and both SPAKE2 messages.
5. **Key confirmation, before anything else:** the joiner sends an HMAC
   proving it has the same key; the host checks it (in constant time) and
   only then sends its own. A wrong code stops here on the host, and the
   joiner sees the host hang up. Both report "the code didn't match"; neither
   payload has been sent.
6. The joiner sends its static public key and payload, encrypted with
   ChaCha20-Poly1305 under its own derived key; the host answers the same
   way. Each side pins the other's public key.

The payload is opaque to the tunnel (up to 16 KiB); the daemon uses it for
the device name and its federation token.

### Limits and timeouts

On devices: control frames at most 4 KiB, Noise frames at most 65,535 bytes,
`recv_message` at most 1 MiB by default, a 15-second budget per handshake
step, 20 seconds for a join to be answered, 30 seconds per pairing step.
Every read of untrusted input is size-checked first and malformed input is
an error, never a panic.

On the relay (all flags; defaults in brackets): open connections in total
[4096] and per IP [64], new connections per IP per minute [600],
registrations [10,000], pairing slots [4096], joins waiting per device
[16], a 10-second budget for the first frame and handshake, 15 seconds for
a device to answer a join, tunnels closed after 10 minutes idle, each
direction of a tunnel limited to 8 MiB/s and 4 GiB. Pairing tunnels carry at
most 64 KiB each way and close after 60 seconds idle.

Someone who scans slots at random can spoil pairings in progress on a
public relay (each guess burns a slot), though with one guess per slot they
are very unlikely to complete one. The per-IP limits slow this down; a
self-hosted relay avoids it.

## Running your own

The relay keeps everything in memory and needs no configuration.

```sh
cargo build --release -p ct-relay
./target/release/crossthreads-relay --listen 0.0.0.0:47200
```

It's meant to face the internet: it only forwards traffic it can't read,
and nothing but the relay protocol is served on its port. Open TCP 47200 in
your firewall. Logs go to stderr: one line per registration, tunnel and
pairing, with IP addresses and the first 12 hex characters of public keys,
never content. `--quiet` turns them off.

```
crossthreads-relay [--listen ADDR] [--max-conns N] [--max-conns-per-ip N]
                   [--max-new-conns-per-min N] [--max-registrations N]
                   [--max-slots N] [--max-pair-ttl SECS] [--idle-timeout SECS]
                   [--rate BYTES_PER_SEC] [--max-bytes BYTES] [--quiet]
```

**TLS.** Not needed for confidentiality: the devices encrypt end to end.
The relay speaks plain TCP and the device side does not speak TLS, so a
TLS-terminating proxy in front won't work as is; a plain TCP proxy or load
balancer (e.g. HAProxy in `mode tcp`) does. Behind one, the relay sees the
proxy's address, so per-IP limits apply to the proxy as a whole; raise
`--max-conns-per-ip` and `--max-new-conns-per-min` accordingly.

### systemd

```ini
# /etc/systemd/system/crossthreads-relay.service
[Unit]
Description=Crossthreads relay
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/crossthreads-relay --listen 0.0.0.0:47200
DynamicUser=yes
Restart=always
RestartSec=2
LimitNOFILE=16384
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_INET AF_INET6
MemoryMax=512M

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl daemon-reload && sudo systemctl enable --now crossthreads-relay
journalctl -u crossthreads-relay -f
```

### Docker

```dockerfile
FROM rust:1-slim AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p ct-relay

FROM debian:stable-slim
COPY --from=build /src/target/release/crossthreads-relay /usr/local/bin/
USER nobody
EXPOSE 47200
ENTRYPOINT ["crossthreads-relay", "--listen", "0.0.0.0:47200"]
```

```sh
docker build -f Dockerfile.relay -t crossthreads-relay .
docker run -d --restart=always -p 47200:47200 crossthreads-relay
```

## Testing it

```sh
cargo test -p ct-tunnel -p ct-relay
```

The end-to-end tests in `crates/ct-relay/tests/e2e.rs` run a relay on
loopback and check: pairing swaps keys and payloads; a wrong code fails on
both sides, reveals nothing and burns the slot; a 250 KB message echoes
both ways; an unpaired device is declined; a device that knows a paired
public key but not its secret can't complete KK; a key can't be registered
without its secret; a byte flipped in transit is a decryption error, not
bad data; the device reconnects after the relay restarts.
