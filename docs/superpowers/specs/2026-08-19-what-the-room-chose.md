# SESH — What The Room Chose

**Date:** 2026-08-19
**Status:** Proposed. Needs approval before a plan is written.
**Blocks:** Attract Mode, and the trophy case behind it.

---

## The measurement

The live log on TatePi, three days after Arc 1 first wrote to it:

```
total events: 147
span: 2026-08-16 21:23 -> 2026-08-19 18:07

    63  music.started
    62  music.skipped
     5  app.launched
     5  app.exited
     3  audio.sink_found
     2  presence.arrived
     2  presence.left
     2  audio.sink_lost
     2  music.queued
     1  person.joined

events with an actor: 7 / 147
music.started with a queue entry (i.e. someone chose it): 2/63
```

**85% of the log is music, and 61 of 63 tracks were chosen by Spotify rather
than by anyone in the room.** Two were ours. The rows are indistinguishable:
same kind, same shape, no actor on either.

## Why this blocks the next arc

The vision puts Attract Mode next and describes it as "a broadcast-style screen
driven by the trophy case". The Arc 2 spec deferred it with a good argument:

> The log today holds eight events of two kinds, no actors on any of them, and
> zero rows in `people`. Attract Mode built now would be a broadcast channel
> with nothing to broadcast.

Arc 2 fixed that, and created the opposite problem. There is plenty to
broadcast now and almost all of it is **wrong**. An attract screen reading this
log truthfully would say the room played sixty-three songs on a night it chose
two. A "top artists" panel would rank whatever Spotify's recommender liked.

This is worse than having no data. An empty screen is obviously empty; a
confident screen full of Spotify's taste presented as the house's is a lie the
room tells about itself, and it gets more expensive the longer it runs.

## Why it happens, and why it is not a bug

The conductor implements D8: believe the speaker, not the log. librespot lives
outside `seshd`'s cgroup, so after a restart the music really is still playing,
and the conductor's job is to correct the log to match reality. When the queue
empties Spotify keeps going on its own, the conductor sees a new track on the
speaker, and it records `music.started` — correctly. Something *is* playing, and
the log should say so.

**The defect is not that autoplay is recorded. It is that the record does not
say where the choice came from.** `music.queued` carries an actor; the
`music.started` that follows it does not distinguish itself from the sixty-one
that nobody asked for.

## Constraints

- **The log is append-only.** The 147 rows stand. Whatever this changes applies
  from the change forward, and any reader must cope with a prefix that predates
  it.
- **D8 stays.** Recording what is actually on the speaker is correct and is the
  difference between the conductor and Arc 1's launcher reaper. Nothing here may
  turn into "stop recording things SESH did not initiate".
- **`POST /api/events` stays open.** A future producer must be able to post a
  `music.started` without knowing about any of this.
- **Additive.** A reader that does not know the new field must still be right
  about everything it did know.

## Options

### A. Stop recording autoplay

Only record `music.started` for tracks that came from the queue. Rejected: it
breaks D8 and it makes the log lie by omission — the room genuinely played those
sixty-one songs, and "what was playing when Marcus walked in" is a question the
log should be able to answer.

### B. Turn autoplay off at the source

Spotify can be told not to continue when the queue empties. Worth doing or not
on its own merits — a room that goes silent when the queue runs dry is a
different room, and that is Tate's call rather than a logging decision. But it
does not fix the log: it reduces how often the ambiguity arises without ever
resolving it, and every row already written stays ambiguous.

### C. Say where the choice came from *(recommended)*

`music.started` carries how the track got there:

```json
{ "entry": 118, "source": "queue" }
{ "source": "autoplay" }
```

Absent means unknown, which is exactly what the existing 147 rows are, so the
prefix reads correctly without pretending.

This is the third time this project has answered a question this shape, and the
shape is now a house idiom worth naming: `exit_observed: false` for an exit SESH
did not see, `clock_synced: false` for a timestamp it could not trust, and now a
`source` for a choice it did not make. **Record what you know about how you
know it, rather than flattening it into the thing you wish you knew.**

## Behaviour

To be settled in the plan:

- The vocabulary. `queue` and `autoplay` are the two that exist today;
  `manual` — someone drove Spotify from their own phone, which the conductor
  also sees — is a likely third and worth deciding now rather than discovering.
- Whether `music.skipped` needs the same treatment. It is paired with a
  `started`, so a reader can follow the pair; carrying it on both is cheap and
  saves the join.
- Whether the phone and TV surfaces should show it. The TV strip already reads
  `added by tate` when there is an entry; the question is what it says when
  there is not.

## Testing

- The conductor already distinguishes these cases internally — it knows whether
  it pushed the track. The test is that the distinction reaches the row.
- One test must pin the **absent** case: a `music.started` posted through
  `POST /api/events` by something that knows nothing about `source` is still
  valid, and readers must treat it as unknown rather than as autoplay.
- The real check is the live log: after a night, `source` should partition the
  music rows with none left unknown except the pre-change prefix.

## Not in scope

- **Backfilling the 147.** Append-only. Anything that wants to reason about the
  prefix treats absent as unknown, which it is.
- **Attract Mode itself.** This exists so that arc has something true to read.
- **Whether autoplay should be on at all.** Option B, above — a real question,
  a separate one.

## Open question for approval

**Should the room keep playing when the queue runs dry?**

The recommendation above deliberately does not answer it, because it is a
question about what kind of room this is rather than about the log. Autoplay
keeps the evening going without anyone tending it; silence is an honest signal
that nobody has queued anything and hands the room back to the people in it.

The logging change is worth making either way — with autoplay off, `source`
still separates the queue from whatever anyone starts by hand — but the answer
decides whether 61-in-63 stays the normal ratio or becomes rare.
