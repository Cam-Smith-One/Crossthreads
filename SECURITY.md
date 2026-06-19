# Security Policy

Crossthreads is **local-first**: it indexes your AI coding sessions into a SQLite
database on your machine and performs no network I/O except, optionally, a
one-time embedding-model download. There is no telemetry and no server component.
Because the index can contain sensitive content from your conversations, we take
data-handling issues seriously.

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
- The local API/daemon being reachable beyond loopback, or accepting commands it
  shouldn't.
- Injection via maliciously crafted session files that leads to code execution.

## Out of scope

- The native Tauri shell's platform webview internals (report upstream).
- Issues requiring an already-compromised local account with full disk access.

## Hardening notes

- The daemon binds to loopback by default and is the single writer to the index.
- Static file serving guards against path traversal.
- Treat the index database as sensitive — it mirrors your conversation history.
