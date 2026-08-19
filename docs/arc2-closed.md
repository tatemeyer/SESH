# Closing Arc 2 — phones and the shared queue

Arc 2 shipped over 2026-08-17 to 08-19 in six phases, PRs #11 through #26. This
is its Definition of Done checked item by item against what was actually
exercised, and — following Arc 1's closure — what is still only claimed.

## The list

| # | Item | Status |
|---|---|---|
| 1 | Scanning the QR puts a working queue in your hand | **Done**, in the room |
| 2 | Two phones both add tracks; they play in the order added | **Partly** — see below |
| 3 | A majority veto skips, and the log says who voted | **Partly** — see below |
| 4 | Music from the Victrola while game audio goes to the TV | **Partly** — see below |
| 5 | `/api/events` shows the night with actors, and survives a reboot | **Done** |
| 6 | Gate green at every phase boundary; queue, veto and conductor tested with no network | **Done** |
| 7 | The pending queue survives a `seshd` restart | **Done**, on hardware |
| 8 | Unplugging the speaker mid-song does not lose the queue, and the log says when the sink went away | **Partly** — see below |

### 1 — verified in the living room, 2026-08-19

Scanned from the couch with an ordinary camera app, no walking up. Details and
one falsified prediction in `docs/arc2-phase5-verification.md`.

### 7 — verified on hardware, 2026-08-19

Two tracks queued, the daemon killed, then restarted. Both came back in order
with their attribution intact, rebuilt from **three rows** — one
`person.joined` and two `music.queued`. There is no queue table and no cached
copy; the projection is the log, read forward.

This is the clearest demonstration in the system of why the log is the design,
which is why the plan asked for it on hardware rather than in a test.

### 2, 3 — the logic is proven; two physical phones are not

Two identities, both present, both voting, were driven against the real binary
on this Pi:

```
Sam votes  -> {"votes": 1, "needed": 2, "carried": false}
Jess votes -> {"votes": 2, "needed": 2, "carried": true}
```

and the log named both voters against the track's URI. Queue ordering and
`added_by` survive a restart (item 7).

**What that does not cover** is the conductor acting on the carried veto, and
tracks actually playing in order, because the instance used had no music source.
`Conductor::honour_vetoes` is unit-tested and mutation-tested, but the chain
from "the room votes" to "the speaker changes song" has not been run end to end
with two real phones.

### 4 — proven at the routing level, not with a game

`librespot - SESH` sits on the `bluez_output` sink while the default sink stays
`alsa_output…hdmi`, so music and game audio take independent routes and the
split is structural rather than incidental. Music out of the Victrola was
confirmed by ear.

Nobody has launched RetroArch over a playing track. That is the two-minute
check that would close it.

### 8 — the signal works; the mid-song case is untested

Disconnecting and reconnecting the speaker produced exactly the expected pair,
naming the sink:

```
112  16:53:43  audio.sink_lost   bluez_output.2D_D4_65_45_03_4D.1
114  16:55:08  audio.sink_found  bluez_output.2D_D4_65_45_03_4D.1
```

It was not done mid-song with tracks pending, which is the case the item names.

## What the arc cost that the plan did not predict

The plan ranked its risks: Spotify Development Mode friction first, Bluetooth
audio second. **Neither was what actually went wrong.**

Bluetooth was almost uneventful — the Victrola is a true A2DP sink
(`0000110b`), it paired, and it reconnects on its own. What cost the time was a
different class of thing entirely: **four bugs that a green test suite could not
see, all found by running the real thing against the real service.**

- `ServeDir` served `/join` and `/phone` as a **404 carrying the right page**
  (#15). Rust suite green, DOM suite green; neither layer speaks HTTP.
- Spotify **411s any POST or PUT without `Content-Length`** (#25), which reqwest
  omits for a bodyless request. That had silently disabled D7's pre-push since
  it was written. The stub answered `204` to anything — laxer than the real
  service, so it certified a request Spotify rejects.
- The phone's **search box closed the keyboard mid-word** (#26). Every DOM test
  passed: they assert on markup, and the markup was right. What was wrong was
  the *identity* of the element between renders.
- `install.sh` **unpaired the speaker** by restarting `bluetoothd` on every run
  (#24), including runs that changed nothing.

The pattern is one thing, worth naming: *every one of these was a correct
component behind a wrong interaction with something outside the test boundary* —
an HTTP status, a header, DOM identity, a daemon restart. The suite is not weak;
it is bounded, and the boundary is where these live.

Three of the four were found in a single evening of someone sitting in the room
using it.

## Deliberately not done

- **The vinyl handoff itself.** Arc 2 records `audio.sink_lost`; making the room
  *react* is a later arc reading events that are now already in the log.
- **BLE presence.** Phones heartbeat instead, which the vision named as the
  degraded mode and which is enough for a veto denominator.

## One decision the next arc should make first

With the queue empty, Spotify autoplay picks the next track and the conductor
records `music.started` with no `added_by`. That is correct under D8 — believe
the speaker, not the log — but the log then cannot distinguish *the room chose
this* from *Spotify chose this*.

Arc 4's trophy case will want that distinction, and the rows are accumulating
now. Cheaper to decide before a season of them exists than to reinterpret them
afterwards.
