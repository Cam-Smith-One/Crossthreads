# Add your other computers

Each computer keeps its own index of its own AI coding history. Add your other
computers and, on any of them:

- **search** also finds sessions from the others,
- **threads** span them: one row per piece of work, with the same name,
  status and notes everywhere, editable from any of them, and
- **agents on different computers** can message each other in the same
  repository's room.

For your phone, see [PHONE.md](PHONE.md) (or Devices → Add a device → A phone).

Nothing is uploaded anywhere. Devices talk directly to each other, over your
own [Tailscale](https://tailscale.com) network or, if you don't use Tailscale,
through an end-to-end encrypted relay you run yourself ([RELAY.md](RELAY.md)).
Transcripts stay on the computer that has them and are fetched when you open
one; only search results, thread names, status and notes travel.

---

## You need

- Crossthreads on each computer: the app ([README](../README.md#-quick-start))
  or the terminal install.
- **Tailscale** on each, signed in to the **same account**
  (<https://tailscale.com/download>), and connected. Open Crossthreads after
  Tailscale is connected; if you connect it later, quit and reopen Crossthreads.
  - No Tailscale? Use a relay instead: see [Without Tailscale](#without-tailscale).

## Add a computer

1. On a computer that already has Crossthreads, open **Devices → Add a
   device → Another computer**. It shows a code like `K7PQ-4XMN` for ten
   minutes.
2. On the new computer, open **Devices → Add a device → This computer** and
   type the code.

That's all. The new computer finds the one showing the code on your Tailscale
network, they exchange the shared token, and each adds the other, with
messaging between their agents switched on. The page on the first computer
updates when it joins.

From a terminal the same thing is:

```sh
crossthreads devices add                 # on the first computer: shows the code
crossthreads devices join K7PQ-4XMN      # on the new one
crossthreads devices                     # list them, online or not
```

If the new computer says it can't find the other one (some Tailscale installs
have no `tailscale` command for it to ask), the first computer shows its
Tailscale address under the code: type that too.

To add a third computer, show a code on either of the first two.

## What you get

- **Search** fans out to every computer that's online, merges the results and
  tags each with its computer. A computer that's offline is skipped and named
  ("1 device unreachable"), never waited on.
- **Threads.** The board lists every computer's threads. Work that moved
  between computers is one thread:
  - copy a thread's **handoff** on one computer and paste it into a tool on
    another, and the session it starts joins the thread by itself;
  - in a thread, **Same work on another computer?** suggests threads on the
    same repository and branch active around the same time: **Join** them;
  - or pick any thread under **Merge another thread into this one**.

  **Separate** undoes a join. Rename a thread, change its status or keep notes
  from any computer; the others pick it up within seconds (straight away when
  online, else when they're next reachable).
- **Rooms.** An agent on the laptop can ask `codex@linux-desktop` for a review
  in the same repository's room and get the answer back, as on one machine.
  Untick **Agents can message** next to a computer to stop it. File claims only
  cover the computer they were made on, since each has its own checkout.
- **Agents' shared context** spans computers: a thread's notes and its sessions
  everywhere are in what `crossthreads_thread_context` returns and in the brief
  Claude Code gets at session start.

## Keep things private

In **Devices → This computer's name, privacy and connection**, list tools or
project folders this computer won't share. Your other computers won't see
those sessions in search or threads, or be able to open them.

## Without Tailscale

Run `crossthreads-relay` on a machine both computers can reach (a small VPS,
or a box on your LAN) and set its address on each computer, in Devices → This
computer's name, privacy and connection → **relay** (or `--relay host:47200` /
`CROSSTHREADS_RELAY`), then reopen Crossthreads. Add a computer with a code as
above: when Tailscale can't find the other computer, pairing goes through the
relay.

Everything between your computers is end-to-end encrypted with keys fixed when
you add the device; the relay forwards ciphertext and can't read or change it,
or learn the token. It does see IP addresses, timing and sizes. Details,
hosting and the threat model: [RELAY.md](RELAY.md).

## How it works

- The app gives each computer a strong random token on first run. Peers must
  present it for every request; the code exchange is how a new computer gets
  it without anyone typing or seeing it. Whichever computer already has other
  devices keeps its token.
- Each computer listens for its other devices only on its Tailscale address
  (and through the relay, if set), and serves them only the peer protocol:
  search, threads, transcripts, room messages, shared thread state. Settings,
  pairing and everything else are refused unless the request comes from the
  computer itself.
- Shared thread state (names, status, which threads are joined, notes) is
  compared every 15 seconds and sent straight away after a change. Each field
  keeps the latest change, so every computer ends up with the same answer.

More: [SECURITY_MODEL.md](SECURITY_MODEL.md), and the design in
[CROSS_DEVICE_SEARCH.md](CROSS_DEVICE_SEARCH.md).

## Troubleshooting

**"This computer isn't on Tailscale yet."** Tailscale isn't connected, or
was connected after Crossthreads started. Connect it, then quit and reopen
Crossthreads.

**"No other Crossthreads found on your Tailscale network."** Make sure the
other computer has Crossthreads open and Tailscale connected, and that both are
signed in to the same Tailscale account. Or type the other computer's Tailscale
address, shown under its code.

**"That code didn't match."** Codes change each time you open Add a device, and
five wrong tries cancel one: show a new code.

**A computer shows offline.** It's asleep, Crossthreads isn't running on it, or
it's not on Tailscale right now. Its threads come back when it does.

**I set things up with a token and pairing code in an older version.** Those
devices keep working; nothing to redo.

For anything else, see [TROUBLESHOOTING.md](TROUBLESHOOTING.md).
