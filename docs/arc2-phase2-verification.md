# Arc 2 Phase 2 — verification record

Run on TatePi, 2026-08-17, against the real `seshd` binary built from this
branch, on a spare port with a throwaway database. Two phones joined the way a
real one does — by decoding the QR from the served SVG — then built one queue
between them.

Phase 2 needs no hardware by design, so this is a demonstration rather than a
discovery. It is here because two of its claims are much more convincing shown
than asserted.

## What the room did

```
roster: ["marcus","sam"]        votes needed to skip: 2

 5  Teenage Dirtbag    Wheatus            added by sam     vetoes=[]
 6  Mr. Brightside     The Killers        added by marcus  vetoes=[]
 7  Semi-Charmed       Third Eye Blind    added by sam     vetoes=[]

Sam vetoes entry 6      votes=1 needed=2 carried=false
Marcus agrees           votes=2 needed=2 carried=true
Sam votes again         votes=2            (one person, one vote)
```

Then the same song twice, vetoed separately — the case the spec got wrong:

```
entry 11  Africa  added by sam     vetoes=[]
entry 12  Africa  added by marcus  vetoes=['sam']
```

## The queue survives a restart

`seshd` killed and restarted against the same database:

```
PASS: the queue rebuilt identically from the log (5 tracks, vetoes intact)
```

This is the payoff of SESH holding the authoritative queue instead of pushing
it to Spotify. Spotify's own queue does not survive anything; this one is a
projection, so it comes back for free — including who added each track and who
had already voted against it.

## The log

```
 1  person.joined    sam      -                  {"name": "Sam"}
 2  presence.arrived sam      -                  {}
 3  person.joined    marcus   -                  {"name": "Marcus"}
 4  presence.arrived marcus   -                  {}
 5  music.queued     sam      spotify:track:aaa  {"title": "Teenage Dirtbag", ...}
 6  music.queued     marcus   spotify:track:bbb  {"title": "Mr. Brightside", ...}
 7  music.queued     sam      spotify:track:ccc  {"title": "Semi-Charmed", ...}
 8  music.vetoed     sam      spotify:track:bbb  {"entry": 6}
 9  music.vetoed     marcus   spotify:track:bbb  {"entry": 6}
10  music.vetoed     sam      spotify:track:bbb  {"entry": 6}
11  music.queued     sam      spotify:track:ddd  {"title": "Africa", ...}
12  music.queued     marcus   spotify:track:ddd  {"title": "Africa", ...}
13  music.vetoed     sam      spotify:track:ddd  {"entry": 12}
```

**Event 10 is Sam voting a second time, and it is deliberately kept.** The
tally did not move, but the vote was cast, and the log records what happened
rather than what mattered. Deduplication belongs in the projection, where it
is tested, and not in the log, which cannot be edited later if the rule turns
out to be wrong. Anyone tempted to "fix" this by dropping repeat votes at the
API should read that sentence twice.

Note also that `subject` is the track URI throughout, so the log reads as
"Sam wanted to skip `spotify:track:bbb`" with no join required, while
`payload.entry` carries the identity that makes two copies of Africa distinct.
That split is the whole of decision D1.

## Mutation testing found two tests that proved nothing

Worth recording, because both looked completely reasonable.

`veto.rs` was written implementation-first, so it was mutation-checked as a
matter of course. All four mutations were caught. The queue projection was
written red-first and checked anyway — and two mutations **survived**:

| Mutation | First run | After |
|---|---|---|
| Look entries up by track URI instead of entry id — the spec's original design | **SURVIVED** | caught by 2 |
| Only remember the most recent voter | caught by 1 | caught by 1 |
| Always drop the first pending entry, whichever was named | **SURVIVED** | caught by 2 |

The cause was the same in both cases: every test that could have caught them
named the **first** entry in the queue. Keying by URI finds the first match;
`remove(0)` removes the first element; naming entry one makes both mutations
into no-ops. The tests asserted the right things about the wrong scenario.

Fixed by adding cases that name the *second* entry — the only discriminating
one — in the projection and over HTTP. This is the second time in this project
that a check has looked convincing and measured nothing (after `pgrep -f`
matching its own command line), and the lesson generalises: a test whose
scenario is symmetric under the bug you fear is not a test of that bug.
