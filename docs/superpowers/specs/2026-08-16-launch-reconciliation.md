# SESH — Launch Reconciliation on Startup

**Date:** 2026-08-16
**Status:** Proposed. Needs approval before implementation.
**Found on:** TatePi, during Arc 1 bring-up

---

## The problem

`seshd` records `app.launched` when it starts an app and `app.exited` when its
reaper observes that app go away. The pairing of the two lives entirely in
memory, in `Launcher`'s `current`.

Restart `seshd` while an app is running and that memory is gone. The reaper
never observes the exit, so it never records one. The log keeps an
`app.launched` that no `app.exited` will ever close, and because the log is
append-only, the gap is permanent.

Observed on the Pi tonight:

```
 3 23:24:22 app.launched   retroarch     <- never closed
 4 23:52:00 app.launched   moonlight
 5 23:52:45 app.exited     moonlight
```

RetroArch was not running. `seshd` had been restarted at 23:51:44 to pick up an
edited `apps.toml`.

## Why this matters more than one bad row

The architecture's load-bearing claim is that **all derived state is
rebuildable from the log alone**. Rebuild "what is running" from the log above
and you get *RetroArch, since 23:24, forever*. The in-memory `current` says
`null` and is right; the log says otherwise and is wrong. The one that is
supposed to be authoritative is the one that lies.

It also compounds. The vision promises stats invented years from now applying
retroactively to every night ever hosted. Any future question with a duration
in it — longest session, most-played app, what was running when Sam arrived —
reads these dangling launches. One unclosed launch per restart, accumulating
across the life of the house.

## This is normal operation, not an edge case

Three routine events produce it, and only the first is avoidable:

1. **Restarting `seshd`** — required to pick up an `apps.toml` change, and will
   be required by every future upgrade.
2. **A crash.** `Restart=always` brings the daemon back; the app it launched is
   gone and unrecorded.
3. **Pulling the plug.** This is a living-room appliance. Someone will cut
   power mid-Kodi, and that is not misuse.

## A dangling launch always means the app is dead

Worth establishing, because it removes the hard case from the design.

`seshd.service` runs with the systemd default `KillMode=control-group`, and
launched apps are ordinary children, so they sit inside the unit's cgroup —
confirmed on hardware, where a streaming `moonlight-qt` logged 606 lines under
`journalctl --user -u seshd`. When the unit stops, systemd signals the entire
cgroup. **Restarting `seshd` therefore kills every app it launched.**

So at startup, an open `app.launched` never means "this might still be
running." The app is gone. What `seshd` does not know is *when* it went.

## Constraints

From `CLAUDE.md`, all load-bearing:

- The `events` table is append-only. No `UPDATE`, no `DELETE`, ever. The bad
  row cannot be repaired, only explained by a later row.
- `Room::record` is the only write path.
- Derived state must be rebuildable from the log alone.
- Projections are pure functions over event sequences, and must stay unit
  testable off-Pi.

## Options

### A. Record a plain `app.exited` at startup

Cheapest. Every existing projection keeps working with no changes, because
`app.exited` is what they already understand.

Rejected. It asserts an observation `seshd` never made, at a timestamp it
invented. The exit could have happened three hours before the restart; writing
it at boot time silently inflates that session's duration. The log's one job is
to be honest about what happened, and this option makes it confidently wrong in
a way nothing downstream can detect.

### B. A new event kind, e.g. `app.abandoned`

Honest about the epistemics. But every projection that currently reasons about
"is something running" must learn the new kind, and any that is not updated
keeps believing the app is running — which is precisely the bug being fixed,
now with extra steps. It also puts the burden on every future projection
author to remember a kind they have never seen fire.

### C. `app.exited`, carrying an explicit marker that the exit was not observed *(recommended)*

Record `app.exited` so that "the app ended" reaches every consumer through the
channel they already read, and put the uncertainty in the payload:

```json
{
  "kind": "app.exited",
  "subject": "retroarch",
  "payload": {
    "exit_observed": false,
    "reason": "seshd restarted while this app was running",
    "last_alive_ms": 1786937604000
  }
}
```

- Projections that only ask *did it end* are correct with no change.
- Projections that compute durations can test `exit_observed` and treat the
  session as bounded-but-unknown rather than quietly reporting a wrong number.
- Nothing is invented: the app genuinely did exit, which the cgroup argument
  above establishes. Only the instant is unknown, and the payload says so.

**Timestamp.** Use the daemon's startup time — the earliest moment SESH can
prove the app was gone — and carry `last_alive_ms` as the latest moment it can
prove the app was alive. The true exit lies between the two, and both bounds
are recorded rather than guessed.

## Behaviour

At startup, before serving any request:

1. Scan back through the log for the most recent `app.launched` with no
   subsequent `app.exited` for that subject.
2. If found, record the reconciling event through `Room::record` — the sole
   write path, so it is appended, applied to projections, and published on the
   WS feed like anything else.
3. Serve.

Idempotent by construction: the reconciling event closes the launch, so the
next startup finds nothing dangling. A log with no open launch is untouched.

Defensive on multiplicity: SESH runs one app at a time, but the scan should
close *every* open launch it finds rather than assume there is at most one, so
a log damaged by an older build heals on next boot.

## Testing

This is a pure function over an event sequence, which is where the vision says
the logic belongs and where it is cheapest to test. No hardware required:

- launched, then exited → no reconciling event
- launched, never exited → exactly one `app.exited` with `exit_observed: false`
- empty log → no event
- launched / exited / launched → closes only the second
- two open launches → closes both
- run reconciliation twice → the second is a no-op

One hardware check to confirm the premise rather than trust it: launch an app,
`systemctl --user restart seshd`, and assert the log closes itself and
`/api/apps` reports `current: null`.

## Not in scope

- **Repairing tonight's RetroArch row.** It stays. The table is append-only and
  the discipline is worth more than a tidy log. Reconciliation is
  forward-looking; if closing the historical row is wanted, it is a deliberate
  one-off `POST /api/events` and should be argued separately.
- **Surviving a restart without killing apps.** Keeping Kodi alive across a
  `seshd` restart would mean moving launched apps out of the unit's cgroup
  (`systemd-run --scope`, or `setsid` plus adoption by pid). That is a larger
  change to the launcher's process model, it interacts with the open
  process-group follow-up, and it does not remove the need for reconciliation —
  a power cut still severs the pairing. Worth its own spec if wanted.

## Open question for approval

Does `exit_observed: false` belong in the payload, or should the honesty marker
be a top-level column on `events`? Payload keeps the schema untouched and costs
nothing today; a column would make the distinction queryable in SQL without
JSON extraction, which may matter once the trophy case is doing real work over
years of history. Recommendation is payload now, on the grounds that the
projections are the only consumer and they are Rust, not SQL.
