# Launch Reconciliation — Implementation Plan

**Goal:** A launch that the log never saw end gets closed at the next startup,
honestly, so "what is running" rebuilt from the log always matches reality.

**Spec:** `docs/superpowers/specs/2026-08-16-launch-reconciliation.md` (approved
2026-08-17, with the honesty marker in the event payload rather than a column).

**Approach:** A pure function over an event slice decides what to close; a thin
wrapper records the result through `Room::record`. All the logic lives in the
pure half, where it is testable without hardware, a database, or a process.

## Constraints

Beyond the repo-wide gate in `CLAUDE.md`:

- **The `events` table stays append-only.** Tonight's dangling RetroArch row is
  not repaired — it is closed by a *later* row, like everything else.
- **`Room::record` remains the only write path.** Reconciliation is not a
  special case that reaches past it into `Store`.
- **The pure function takes `&[Event]` and returns `Vec<NewEvent>`.** No clock,
  no I/O, no `Room`. Timestamps come from the store, as they do for every other
  event.
- **Development now happens on the Pi**, not Windows as the Arc 1 plan states.
  Nothing here is platform-bound regardless — no process spawning is involved.
- Soft ceiling of 300 lines per file. `reconcile.rs` is a new module rather
  than an addition to `room.rs`, which is already carrying the write path.

## Tasks

- [x] **Task 1: The pure reconciler**

  New `crates/seshd/src/reconcile.rs`, declared in `lib.rs`. Module doc
  explaining what an unclosed launch is and why it happens.

  `pub fn unfinished_launches(events: &[Event]) -> Vec<NewEvent>`

  Walks the slice in order, tracking the last-seen `app.launched` per subject
  and clearing it on `app.exited` for that subject. Whatever is still open at
  the end becomes one `app.exited` carrying:

  ```json
  { "exit_observed": false,
    "reason": "seshd restarted while this app was running",
    "last_alive_ms": <ts of the newest event in the log> }
  ```

  `last_alive_ms` is the latest instant SESH can prove it was watching and had
  not recorded an exit. The store assigns the new event's own `ts_ms`, which is
  the other bound: by then the app was certainly gone.

  Ordering is deterministic — by the subject's launch id — so two runs over the
  same log produce the same events. Launches with no subject are skipped; there
  is nothing to close.

  Tests, all pure, per the spec's list:
  - launched then exited → nothing
  - launched, never exited → exactly one closing event, right subject, marker present
  - empty log → nothing
  - launched / exited / launched → closes only the second
  - two subjects open → closes both, deterministic order
  - unrelated kinds ignored
  - re-running over a log that already contains the closing event → nothing
    (idempotency, the property the whole design rests on)

- [x] **Task 2: Record them at startup**

  `pub fn close_unfinished_launches(room: &Room) -> Result<Vec<String>>` in the
  same module: read history via `room.events_since(0, -1)`, feed it to
  `unfinished_launches`, record each through `room.record`, return the subjects
  closed so the caller can log them.

  Tests against an in-memory `Room`:
  - a room whose log ends in `app.launched` gains exactly one `app.exited`
  - calling it twice appends nothing the second time
  - the closing event is visible to `events_since`, i.e. it went through the
    real write path

- [x] **Task 3: Wire it into `main.rs`**

  Call it after `Room::new` and before the listener binds, so no client can
  observe the inconsistent log. `tracing::warn!` naming the apps closed — this
  should be visible, not silent, because it means the previous run did not shut
  down cleanly.

- [x] **Task 4: Verify on hardware**

  Launch an app through the API, `systemctl --user restart seshd`, then assert
  the log closed itself and `/api/apps` reports `current: null`. This is the
  premise the spec rests on — that a restart kills the app — so it gets checked
  against the real daemon rather than trusted.

## Definition of Done

- All five gate commands green.
- The six-plus pure cases and the three `Room` cases pass.
- On the Pi: restarting `seshd` under a running app leaves a closed pair in the
  log, marked `exit_observed: false`, with no second closing event on a further
  restart.
- `docs/arc1-followups.md` follow-up 4 marked resolved.

## Verification record (2026-08-17, TatePi)

All five gate commands green: 72 unit tests (59 before this change), 3 `ws_feed`,
clippy clean under `-D warnings`, `fmt --check`, 26 vitest, `npm run build`.

Because tests and implementation were written together rather than strictly
red-first, the suite was mutation-checked to prove it has teeth:

| Mutation | Caught by |
|---|---|
| `app.exited` stops closing an open launch | 5 tests, including the idempotency guard |
| honesty marker flipped to `exit_observed: true` | 2 tests |

On hardware, against the installed daemon:

1. First start after the change closed the historical RetroArch launch —
   event 6, `exit_observed: false` — and logged
   `WARN closed launches left open by a previous run apps=["retroarch"]`.
2. A second restart appended nothing. Idempotent.
3. Launched Kodi (event 7), `systemctl --user restart seshd` mid-run: the
   restart killed Kodi, startup closed the launch (event 8,
   `exit_observed: false`), and `/api/apps` reported `current: null`.
4. A further restart appended nothing, and a scan of the whole log now finds
   zero unclosed launches.

**Testing note worth keeping.** The first pass at step 3 reported "kodi STILL
RUNNING" and appeared to falsify the spec's premise. It was a false positive:
`pgrep -f kodi` matched the shell command that contained the word `kodi`. This
is the exact footgun recorded at the end of the Pi bring-up handoff, walked
into again. Check for a process by exact name (`pgrep -x kodi.bin`,
`ps -eo pid,comm`) when the pattern could appear in your own command line.

## Known limitation, deliberately not solved here

The spec's "a dangling launch always means the app is dead" rests on
`KillMode=control-group`, which holds for the systemd deployment. Run `seshd`
by hand from a shell — the dev loop — and kill it, and its children are
reparented and survive; reconciliation would then record an exit for something
still running.

Closing that properly means recording the pid on `app.launched` and checking
liveness at startup, which drags in pid reuse and interacts with the open
process-group follow-up. Out of scope for this change; worth its own spec if
the dev loop ever starts mattering as much as the appliance.
