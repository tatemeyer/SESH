# SESH — The Clock Cannot Be Trusted at Boot

**Date:** 2026-08-19
**Status:** Proposed. Needs approval before implementation.

---

## The problem

A Raspberry Pi 5 has no battery-backed RTC. On every cold boot the clock is
wrong, and `systemd-timesyncd` fixes it in two steps: first it restores the
timestamp it recorded at last shutdown, then — once the network is up — it
contacts an NTP server and jumps the clock to the truth.

`seshd` starts between those two steps. Measured on TatePi, boot of
2026-08-19:

```
[  84.398586] systemd-timesyncd: System clock time unset or jumped backwards,
                                 restoring from recorded timestamp: 12:37:45
[ 135.964374] systemd: Started seshd.service
[ 136.509124] seshd: reconciled with the music source wait=15s online=true
[ 145.165950] systemd-timesyncd: Contacted time server 66.118.230.14:123
[ 145.166230] systemd-timesyncd: Initial clock synchronization to 12:52:05
```

`seshd` came up at monotonic 136.0s and NTP landed at 145.2s. For **9.2
seconds** the daemon was running with a clock that was **13 minutes 28 seconds
slow**, and its startup reconciliation ran at 136.5s — inside the window.

Two numbers matter here and they are not the same number:

- **The window** is short. It is however long the network takes after `seshd`
  starts; on this boot, nine seconds.
- **The error** is not short and is not bounded by anything in our control. The
  restored timestamp is the last shutdown, so the error is roughly *how long the
  Pi was powered off*. Nine seconds of exposure at thirteen minutes of skew this
  time; nine seconds at twelve hours after a Pi left off overnight.

`store::now_ms()` is a bare `SystemTime::now()`, and every timestamp SESH writes
comes from it.

## Why this matters more than a slightly-off row

**It writes a self-contradicting event into an append-only log.** Startup
reconciliation is the highest-consequence write `seshd` makes, it runs
unconditionally at startup, and it lands inside the window every cold boot.
`reconcile.rs` documents its own guarantee:

> The store assigns each returned event its `ts_ms`, which is the upper bound on
> when the app died — by the time this is recorded, it certainly has.

That is false on this hardware. The `app.exited` it writes carries `ts_ms` from
the skewed clock and `last_alive_ms` copied from the previous session's last
event, which was stamped when the clock was correct. So `ts_ms <
last_alive_ms`: the upper bound sits below the lower bound, in the same payload.
On a reboot after an evening with an app left running, this is guaranteed rather
than a race.

The log is append-only. A row like that cannot be corrected later, only
explained.

**It is not confined to reconciliation.** Everything downstream of `now_ms()`
is exposed for the same window:

| Site | Effect of a backdated read |
|---|---|
| `store/events.rs` — every `ts_ms` | Events backdated by the full skew |
| `store/people.rs` — `joined_ms` | Roster order is wrong; Arc 2 sorts `joined_ms ASC, rowid ASC` |
| `join.rs` — code rotation | **User-visible.** Codes rotate on a 60s window; the forward jump instantly expires the code the TV is displaying, so a scan during the window fails |
| `presence.rs` — the 10-minute window | A jump larger than the window sweeps every present phone out and writes spurious `presence.left` |
| `player/spotify.rs` — `expires_at_ms` | Token believed expired ~13 minutes early; benign, refreshes sooner than needed |

**What is safe, and worth stating so nobody "fixes" it.** `GET /api/events`
reads `ORDER BY id ASC`, not by `ts_ms`. Log *ordering* is immune to clock
jumps and must stay that way — the id is the sequence, the timestamp is a
measurement.

## This is normal operation, not an edge case

The one thing SESH is for is being the box in the living room that is always
on and remembers. It gets unplugged, it gets rebooted after an update, the
power blips. Every one of those is a cold boot into this window, and Arc 1's
Definition of Done was specifically that the box comes up unattended. The whole
design assumes reboots are routine, which makes the boot-time clock a routine
condition rather than an exotic one.

## Constraints

- **The log is append-only.** No fix may `UPDATE` a written `ts_ms`. Whatever we
  record has to be right when it is recorded, or honest about not being.
- **Boot must not get slower.** This is a TV appliance; the room should be usable
  when someone walks in. `systemd-time-wait-sync` would block the boot until NTP
  answers, which is the obvious fix and the wrong one — it trades a nine-second
  data-quality window for an unbounded delay before the TV shows anything, on
  the exact failure (no network) where the room should still work.
- **`seshd` is a user unit ordered on `graphical-session.target`**, deliberately,
  so apps it spawns have a display. It cannot simply be reordered after a system
  target without breaking that.
- **No new dependency** should be needed for this.

## Options

### A. Order `seshd` after time sync

Add `After=time-sync.target` and enable `systemd-time-wait-sync`. Rejected: it
is the constraint above. It also fails open in the worst way — a Pi with no
internet never syncs, so the room never starts.

### B. Stamp events from a monotonic anchor

Record `CLOCK_MONOTONIC` at startup alongside one wall-clock reading, and derive
`ts_ms` from the offset. Rejected: it makes every timestamp wrong by whatever the
anchor was wrong by, permanently, and *hides* the error instead of recording it.
A backdated row you can spot is better than a plausible one you cannot.

### C. Record whether the clock was trustworthy when the row was written *(recommended)*

`systemd-timesyncd` creates `/run/systemd/timesync/synchronized` on first
successful sync — an existing, dependency-free, file-existence check.

- `now_ms()` gains a companion that reports whether the clock is synced.
- Events written while it is not synced carry an explicit marker in the payload,
  the same shape as `exit_observed: false` from the launch-reconciliation spec —
  a measurement SESH could not make, recorded as such rather than guessed.
- Startup reconciliation additionally **defers**: it is the one write with no
  deadline, so it waits for the marker (bounded, then proceeds with the marker
  set) rather than racing it. Nothing else in the system needs to block.
- `join.rs` rotation switches to a monotonic `Instant` for its 60-second window.
  Rotation is a *duration*, not a *time*; it never needed the wall clock, and on
  a monotonic timer the forward jump stops killing the displayed code.

C is the only option that leaves the log honest. It is also consistent with how
Arc 1 already handled the same class of problem: `exit_observed: false` exists
because SESH would rather write down what it did not see than write a confident
lie.

## Behaviour

To be settled in the plan, but the shape:

- A `Clock` seam, so tests can drive both synced and unsynced states without
  touching `/run`. Same reasoning as the `Platform` trait — the condition only
  reproduces on a cold Pi otherwise.
- The marker name and payload key. `exit_observed` is the precedent; something
  like `clock_synced: false` reads consistently with it.
- How long startup reconciliation waits before giving up, and what it records
  when it does.
- Whether `presence.rs` should treat a detected jump as a reason to *not* sweep,
  rather than sweeping everyone out.

## Testing

- The `Clock` seam makes the unsynced path a unit test rather than a reboot.
- One test must pin the contradiction directly: reconcile with a clock behind
  the previous session's last event, and assert the written row is marked rather
  than silently self-contradicting.
- The real proof is a cold boot with an app left running, checked against the
  log afterwards. The existing `~/sesh-boot-verify` harness already re-arms for
  a reboot and already diffs the log across the power cycle, so it is the right
  place to add the assertion.

## Not in scope

- Adding an RTC module to the Pi. Hardware is a fine answer to this and does not
  remove the need for the software to be honest when it is missing.
- Backfilling or correcting rows already written. The log is append-only; the
  eight Arc 1 events and everything since stand as they are.
- Anything about time *zones*. This is about whether the instant is known, not
  how it is displayed.

## Open question for approval

**Should an unsynced clock block `POST /api/events` and the phone surface, or
only mark what it writes?** The recommendation above is mark-only: a room that
refuses to work for the first nine seconds after boot is a worse room, and the
open ingest port is an architectural invariant. But this is the call that
decides whether the marker is advisory or load-bearing, and it belongs to Tate
rather than to the plan.
