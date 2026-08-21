# Audit — the suite passing for reasons unrelated to what it checks

**Date:** 2026-08-21
**Issue:** #48
**Method:** mechanical sweep of the two shapes the issue names, before proposing
any rule. The evidence sets the rule rather than the other way round.

---

## Why this was worth doing

Three instances surfaced in one day, and one of them reached users: the veto
failed in front of three people and they left for Spotify Jam. That bug was
invisible to a green suite because the Spotify stub answered `204` to every
command — the documented answer, and the only answer that could never catch a
client which mishandles any other one.

## Shape A — a double that only gives the documented answer

**Four instances. One was a live bug.**

| # | Double | Could not express | Consequence |
|---|---|---|---|
| 1 | `spotify_stub` | a 2xx whose body is not JSON | **#47**, in front of users |
| 2 | `MockSinks` | the query itself failing | **live bug, below** |
| 3 | `MockPlatform` | a spawn failing realistically | `launch`'s error path never ran |
| 4 | `FakeWs` (surfaces) | a malformed frame | the `catch` had no test |

### 2 is a bug, not a coverage gap

`PactlSinks::names` ignored the process exit status:

```rust
let output = Command::new("pactl").args([...]).output()?;
Ok(parse_sinks(&String::from_utf8_lossy(&output.stdout)))
```

A `pactl` that fails — PipeWire not up yet, socket gone, session not ours —
writes nothing to stdout. Empty stdout parses to an empty sink list. So
`Ok(vec![])` meant both *"the query failed"* and *"there are no sinks"*, and the
watcher turns the second into `audio.sink_lost`:

> **the room announcing the speaker left because it could not ask.**

That is the distinction Arc 3's fusion is built on — *no answer is not a
negative answer* — violated one layer down. The watch loop was already written
correctly (it warns and skips on `Err`), so the entire defect was the one `Ok`
that should have been an error. `MockSinks` could not fail, so nothing could
have caught it.

### A note on testing the fix

The first test written for this used `MockSinks` and therefore exercised the
*loop's* handling of `Err` — which was already correct. **It would have passed
against the broken code.** The status check was then extracted into
`interpret_pactl(success, stdout, stderr)` so the defect is reachable, and the
real test was confirmed by mutation: disabling the check makes it fail.

That near-miss is the audit's own subject appearing inside the audit, and it is
the strongest argument for the rule below.

## Shape B — assertions forced equal by a floor or constant

**One instance, already fixed. No survivors.**

Every clamp in `crates/seshd/src` was swept (`.max(`, `.min(`, `.clamp(`,
`saturating_`) and each assertion near one was checked for whether it spans both
sides of the clamp:

| Site | Verdict |
|---|---|
| `presence/tests.rs` denominator test | **was vacuous** — compared 2 with 2 because `MIN_VOTES` floored both sides. Fixed; now 5 present vs 2 attentive (3 vs 2) with an `assert_ne!` guard |
| `veto.rs:126–131` | sound — asserts 3 for four and five people, above the floor |
| `api/music.rs` needed-count | sound — walks 0→1→3→4 people and ends on 3 |
| `player/mod.rs` `remaining_ms().max(0)` | sound — 210_000 and 10_000 discriminate; the 0 cases test the clamp deliberately |

Shape B is real but rare, and the existing tests mostly handle it well. **It does
not justify a rule.**

## The rule the evidence sets

Shape A produced four instances and one user-visible outage. Shape B produced
one, already guarded. So the rule targets doubles:

> **A test double must be able to fail the way the real thing fails.**
>
> If the real dependency can return an error status, hand back malformed data,
> or fail the query outright, the double needs a mode for it and at least one
> test must use that mode. A double built from the documentation encodes what a
> service *promises*, not what it *does*, and every bug here has lived in that
> gap.

Two corollaries worth stating, both learned the expensive way:

- **Test the fix at the seam where the defect actually is.** If a test passes
  against the broken code, it is not testing the fix — see `interpret_pactl`.
- **Reproduce an observed failure verbatim** when there is one. The #47 test
  asserted the production error string before the fix existed.

## What changed

- `PactlSinks` checks the exit status; `interpret_pactl` makes it testable.
- `MockSinks` gains `fail_with` / `recover`.
- `MockPlatform` gains `fail_next_spawn`; `Launcher::launch`'s failure path is
  now tested.
- `FakeWs` is driven with a malformed frame, asserting the **next** frame still
  arrives — on a TV there is no console, so a silently dead feed looks exactly
  like a quiet room.
- `tokio`'s `test-util` feature in dev-dependencies, so timed loops can be
  tested at all. The audio watch ticks every 5s and the presence sweep every
  60s; `watch_loop` had no test whatsoever.

## Still open

`post_token` in `player/auth.rs` parses its response with no context, so a 2xx
non-JSON from the accounts service fails with a bare serde error naming no
operation — the opposite of #47, whose error was diagnosable precisely because
it named the call. Not the same class of bug, and not fixed here.
