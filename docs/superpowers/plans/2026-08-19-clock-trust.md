# The Clock at Boot — Implementation Plan

**Goal:** No row SESH writes can silently claim an instant the box did not know.
Timestamps that are recorded against an unsynced clock say so; everything that
is really a *duration* stops reading the wall clock at all.

**Spec:** `docs/superpowers/specs/2026-08-19-clock-trust.md` (approved
2026-08-19; the open question answered **mark, do not block**).

**Approach:** One organizing rule, applied everywhere:

> **Durations use a monotonic clock. Recorded instants use the wall clock, and
> carry a marker when the wall clock was not yet trustworthy.**

Nearly every bug in the spec is a duration computed from two wall-clock reads
straddling the NTP jump. Those do not need a marker, they need the right clock,
and the fix removes the failure rather than annotating it. What is genuinely an
instant — when a thing happened, for a human reading the log later — keeps the
wall clock and gains the marker.

---

## Constraints

Beyond the repo-wide gate in `CLAUDE.md`:

- **Nothing blocks on the clock.** The spec's answer is load-bearing. No handler
  returns an error, no surface degrades, no request waits. The single permitted
  wait is inside `seshd`'s own startup, described in Task 4.
- **The `events` table stays append-only**, and the marker goes in the
  **payload**, not a new column. `exit_observed` is the precedent; payload keeps
  the schema untouched and keeps the marker next to the claim it qualifies.
- **`Room::record` remains the only write path.** The marker is applied inside
  `Store::append`, which is already the one place that stamps time — so no
  caller can forget it and no caller may bypass it.
- **`now_ms()` keeps its name and meaning.** It is the wall clock. The point is
  not to make it lie less; it is to stop using it for things that were never
  about wall time.
- **No new dependency.** `/run/systemd/timesync/synchronized` and
  `std::time::Instant` are both already available.
- Soft ceiling of 300 lines per file; `clock.rs` is a new module.

## Tasks

- [ ] **Task 1: The `Clock` seam**

  New `crates/seshd/src/clock.rs`, declared in `lib.rs`. Module doc stating the
  rule above and why a Pi with no RTC forces it.

  ```rust
  pub trait Clock: Send + Sync {
      fn now_ms(&self) -> i64;      // wall clock, for recorded instants
      fn mono_ms(&self) -> i64;     // monotonic, for durations
      fn synced(&self) -> bool;     // is the wall clock trustworthy yet
  }
  ```

  `SystemClock` is the production impl: `now_ms` as today; `mono_ms` from an
  `Instant` captured at construction; `synced` from the existence of
  `/run/systemd/timesync/synchronized`.

  **`synced` latches.** Once true it never re-reads the filesystem — the marker
  answers "was the clock trustworthy when this was written", and a `/run` that
  vanishes later must not retroactively change that answer. It also makes the
  common path a bool read rather than a `stat` per event.

  `TestClock` gives tests direct control of all three, so the unsynced path is a
  unit test rather than a reboot. Same reasoning as the `Platform` trait: the
  condition only reproduces on a cold Pi otherwise.

  Tests: `synced` latches once true; `mono_ms` is monotonic across a wall-clock
  jump backwards; the path is checked, not assumed, for a box with no
  `timesyncd` at all (absent file → not synced, forever, and that is correct —
  such a box's clock genuinely is not known good).

- [ ] **Task 2: The store stamps and marks**

  `Store` gains a `Clock`. `Store::append` sets `ts_ms` from `clock.now_ms()` as
  today, and when `!clock.synced()` inserts into the payload:

  ```json
  { "clock_synced": false }
  ```

  Absent means synced. That keeps every row written by a healthy box byte-identical
  to today's, so the marker's presence is itself the signal and the log does not
  grow noise for the ordinary case.

  The key is only ever *added*, never overwritten — if a caller has somehow
  already set it, that is a bug worth failing a test over rather than silently
  clobbering.

  Tests: a synced clock produces no key; an unsynced one produces `false`; the
  key survives a round-trip through `read_since`; an event whose payload is not
  a JSON object is still handled (today's `NewEvent` always builds an object,
  but the store must not panic if that changes).

- [ ] **Task 3: `joined_ms` gets the same treatment**

  `store/people.rs` stamps `joined_ms` from the same clock. `people` is a table,
  not the log, so there is no payload to mark — instead, **the roster's order
  stops depending on it**: `ORDER BY joined_ms ASC, rowid ASC` becomes
  `ORDER BY rowid ASC`.

  This is the Task 1 rule applied to sorting. `rowid` is the order people
  actually joined in, always, with no clock involved; `joined_ms` stays as the
  human-readable record of *when*. Sorting by a measurement when the sequence is
  already recorded was the bug.

  Tests: two people inserted while unsynced, then a third after sync, still list
  in join order.

- [ ] **Task 4: Startup reconciliation waits, briefly**

  In `main.rs`, immediately before `close_unfinished_launches` (currently line
  186, after `Room::new` and before the listener binds): if the clock is not
  synced, wait for it, polling at 250ms, to a ceiling of **10 seconds**.

  Measured on TatePi the gap was 9.2s from `seshd` start to NTP, and
  reconciliation ran 0.5s in — so a 10s ceiling covers the observed case with
  room, and the wait normally costs nothing because the marker is usually
  already set. On timeout it proceeds anyway and the row it writes carries
  `clock_synced: false`, which is the whole point: late is better than wrong,
  and wrong-but-marked is better than blocked.

  `tracing::warn!` when it waits and again if it times out. This should be
  visible — a box that routinely times out here is telling you its network is
  slow to come up, which is worth knowing.

  This is the one permitted wait. It has no caller: nothing is bound, no phone
  can be talking to it, and the room is not yet usable regardless.

  Tests: with a `TestClock` that never syncs, startup completes and the closing
  row is marked; with one that syncs partway, it waits and the row is unmarked.

- [ ] **Task 5: Durations move to monotonic**

  Three sites, all currently computing an elapsed time from two wall-clock
  reads. Each keeps its parameterised, pure shape — the caller passes the
  number, so the existing `T0 + WINDOW_MS` style tests keep working unchanged —
  only the caller's source changes from `now_ms()` to `clock.mono_ms()`.

  - `join.rs` — `ROTATE_MS` / `GRACE_MS`. This is the user-visible one: on the
    wall clock the forward jump blows past `ROTATE_MS + GRACE_MS` at once and
    kills the code the TV is displaying mid-scan. On a monotonic timer the jump
    is invisible and rotation just keeps ticking.
  - `presence.rs` — `WINDOW_MS` and the sweep. A jump larger than the 10-minute
    window would sweep out every phone in the room and write spurious
    `presence.left`. Note `Presence::seeded` takes a starting instant too, and it
    must come from the same monotonic source or the first sweep compares two
    different clocks.
  - `player/spotify.rs` — `expires_at_ms`. Benign today (a token refreshed early
    is not a bug) but it is the same mistake, and leaving one instance of a
    pattern behind is how it grows back.

  This deletes the failure rather than recording it, which is why these get no
  marker.

  Tests: for each, a wall clock that jumps forward 13 minutes mid-sequence
  changes nothing about the outcome.

- [ ] **Task 6: Correct `reconcile.rs`'s doc comment**

  It currently claims its `ts_ms` "is the upper bound on when the app died — by
  the time this is recorded, it certainly has." With Task 4 that is true when
  the marker is absent and false when it is present, and the comment must say
  so. A doc comment that overstates a guarantee is how the next person rebuilds
  the same bug.

  Same for `docs/arc1-followups.md` item 4, which repeats the claim.

- [ ] **Task 7: Verify on hardware**

  The bug only exists on a cold boot, so the proof has to be one:

  1. Launch an app through the API and leave it running.
  2. `sudo reboot`.
  3. After boot, read the log. The reconciliation `app.exited` must be present,
     and — depending on whether NTP beat the 10s ceiling — must either carry no
     marker and a `ts_ms` above its own `last_alive_ms`, or carry
     `clock_synced: false`. **What must never appear is an unmarked row whose
     `ts_ms` is below its `last_alive_ms`.** That is the assertion.
  4. Scan the QR within the first few seconds of the surface appearing and
     confirm the join succeeds — the Task 5 `join.rs` fix, checked the way a
     guest would.

  `~/sesh-boot-verify/` already re-arms an `@reboot` run and already diffs the
  log across the power cycle, so step 3's assertion belongs in `verify.sh`
  rather than in a one-off script.

## Definition of Done

- All five gate commands green.
- No handler, surface, or request path can fail or stall because of the clock —
  re-read against the spec's answer before calling this done.
- A cold boot with an app left running produces a reconciliation row that is
  either correct or marked, never neither.
- `reconcile.rs` and `docs/arc1-followups.md` no longer claim a guarantee the
  hardware does not provide.

## Verification record

To be filled in after Task 7 runs on TatePi, following the pattern of
`docs/arc2-phase1-verification.md`. Predictions in this plan that the hardware
contradicts get corrected here rather than quietly dropped.

## Not solved here

- **The 10-second ceiling is a guess informed by one measurement.** One boot on
  one network. If the verification record shows it timing out, the answer is
  probably to look at why `NetworkManager-wait-online` took 14.3s this boot, not
  to raise the ceiling.
- **Rows already written stand.** The log is append-only. Nothing backfills.
- **An RTC module would remove the window entirely** and is a perfectly good
  answer at the hardware level. It does not remove the need for the software to
  be honest on a box that lacks one.
