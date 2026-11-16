# Security Policy

Crossthreads is **local-first**: it indexes your AI coding sessions into a SQLite
database on your machine. There is no telemetry, no account and no hosted
service. Network traffic happens only when you turn it on: between your own
devices over Tailscale, to a paired phone, to a model provider you configured,
or a one-time embedding-model download. Because the index mirrors your
conversation history, and room messages reach agents that can run commands, we
treat security issues as high priority.

**How Crossthreads is protected, and against what:** see the
[security model](docs/SECURITY_MODEL.md).

## Reporting a vulnerability

Please report security issues **privately** — do not open a public issue.

- Use GitHub's **private vulnerability reporting** (Security → Report a
  vulnerability) on this repository, or
- email the maintainer at the address in the repository's profile.

Include a description, reproduction steps, affected version/commit, and impact.
We aim to acknowledge within a few days and to coordinate a fix and disclosure
timeline with you.

## What we consider in scope

- Path traversal or arbitrary file read/write (e.g. via the HTTP bridge or
  connectors).
- Unintended network egress of indexed content.
- The local API/daemon being reachable beyond loopback, accepting commands it
  shouldn't, or answering another account on the same computer.
- Bypassing the local key, the federation token, the *Can message* grant, phone
  pairing, or the phone's read-only allowlist.
- Getting a message into a room without the grant that should be required.
- Injection via maliciously crafted session files that leads to code execution.

## Out of scope

- The native Tauri shell's platform webview internals (report upstream).
- Issues requiring an already-compromised local account with full disk access.

## Hardening notes

See [docs/SECURITY_MODEL.md](docs/SECURITY_MODEL.md) for every boundary and how
it is enforced. In short: every client on the machine presents a key only your
account can read; the network listener serves only the token-checked peer
protocol; every cross-device capability is a separate opt-in grant; files are
written owner-only; and the web app sends a strict content security policy.
