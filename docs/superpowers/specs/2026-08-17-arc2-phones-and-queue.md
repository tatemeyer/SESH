# SESH Arc 2 — Phones & the Shared Queue

**Date:** 2026-08-17
**Status:** Proposed. Needs approval before a plan is written.
**Supersedes the build order in:** `2026-08-15-sesh-vision-design.md`, which put
Attract Mode second.

---

## Why this arc, and not Attract Mode

The vision's build order puts Attract Mode at Arc 2, defining it as "a
broadcast-style screen driven by the trophy case — standings ticker, streaks,
head-to-head records, recent clips on loop." Every one of those inputs comes
from Arc 4 or Arc 5.

The log today holds eight events of two kinds, no actors on any of them, and
zero rows in `people`. Attract Mode built now would be a broadcast channel with
nothing to broadcast.

This arc is the smallest one that changes what the room *is* rather than what it
looks like. It is also the one that starts filling the log with **social** events
— every `music.queued` carries an actor — so Attract Mode and the trophy case
have something real to read when their turn comes. Arc 4's manual score
reporting rides in free on the phone surface built here.

## Scope

**In:** QR handoff, per-person tokens, the phone surface, the shared music
queue with majority veto, Spotify playback through the Pi, music out to a
Bluetooth speaker.

**Out, deliberately:** BLE presence detection and the vinyl handoff. Both belong
to the audio/presence subsystems and neither is needed for a queue to work. See
*Deferred* at the end.

## Identity and trust

Unchanged from the vision: no accounts, no passwords. Identity is a name and a
face in this house, and the LAN is the trust boundary.

- The TV shows a QR encoding `http://<pi>:7373/join?c=<one-time code>`.
- Scanning opens the phone surface. The code is exchanged once for a
  per-person token, then burned.
- First join asks for a display name and writes a `people` row.
- The token is stored on the phone and sent on every request.

`people` gains `token` and `joined_ms` columns. This is an `ALTER TABLE`, which
Arc 1 explicitly permits: `people` is an identity registry — source data, not a
projection — so it does not fall under the "rebuildable from the log"
invariant, and the append-only rule covers `events` only.

**The QR must not encode the token itself.** A QR on a TV is visible to a
camera through a window and to every photo taken of the room. A one-time code
that is burned on exchange bounds that exposure to the moment it is displayed.

Note from Arc 1's follow-ups: `http://<pi>:7373` is not a secure context, so no
service workers and no `getUserMedia`. Neither is needed — QR scanning happens
in the phone's own camera app, not in our page.

## The queue is events

| Kind | Actor | Subject | Payload |
|---|---|---|---|
| `person.joined` | the person | — | display name |
| `music.queued` | who added it | track uri | title, artist, duration |
| `music.vetoed` | who voted | track uri | — |
| `music.skipped` | — | track uri | why: `vetoed` / `finished` / `manual` |
| `music.started` | — | track uri | — |

The playing order, who added what, veto tallies, and "what did we play the night
Marcus came over" are all projections over these. Nothing about the queue is
stored anywhere else.

## The load-bearing constraint: SESH owns the queue, Spotify holds one track

This is the decision the whole arc turns on.

The obvious implementation — push every queued track to Spotify with
`POST /me/player/queue` — **makes veto impossible.** The Web API can add to
Spotify's queue but cannot reorder it or remove from it. Once a track is in
Spotify's queue it is going to play.

So SESH keeps the authoritative queue in its own projection and hands Spotify
exactly one track at a time, topping it up as the current track nears its end.
Veto then means:

- **Not yet playing** → drop it from SESH's queue. Spotify never hears about it.
- **Currently playing** → `POST /me/player/next`, and record `music.skipped`
  with `why: vetoed`.

The cost is that SESH must watch playback state to know when to push the next
track — a poll of `GET /me/player` every few seconds while something is
playing, idle otherwise. That is the price of being able to change your mind,
and it is worth paying: veto is the feature that solves the aux-cord problem.

## Majority of whom?

Veto is "majority veto skips." Without BLE presence, the roster is whoever has
an active phone token — which the vision already anticipated as the degraded
mode: *"presence dies → roster falls back to whoever is on the phone app."*

A phone counts as present if it has been seen within a rolling window. The
threshold is a strict majority of those phones, minimum two, so a single person
cannot veto alone in a room of one — that is just skipping, and there is a skip
button for it.

## Playback

The Web API does not play audio. It controls playback on a Spotify Connect
device, so the Pi has to *be* one.

```
phones ──► seshd ──► Spotify Web API ──► the Pi's Connect device
                                              (raspotify/librespot)
                                                     │
                                              PipeWire ──► BT speaker
```

- **raspotify** (packaging librespot) runs on the Pi as the Connect endpoint.
  Actively maintained in 2026, and the current maintainer is a librespot
  contributor.
- **seshd** holds one house Spotify account, authorizes once via the
  Authorization Code flow, and stores the refresh token in a `0600` file
  outside the event log. Secrets never go in the log — it is append-only, it is
  served over an unauthenticated LAN endpoint, and it is meant to be readable
  forever.
- On startup, and whenever playback is lost, seshd transfers playback to the
  Pi's device with `PUT /me/player`.

A `Player` trait keeps all of this behind a seam, exactly as `Platform` does for
the launcher, so the queue logic is unit-testable with no network and no
Spotify account.

## What the February 2026 Spotify changes mean for this

Checked against the current API rather than assumed, because Spotify made
breaking changes effective 2026-03-09.

**Everything this arc needs survived:** `GET /me/player`, `POST /me/player/queue`,
`PUT /me/player`, `POST /me/player/next`, and `GET /search` are all still live.
The removals were metadata and browse endpoints — artist top tracks, new
releases, the batch "Get Several" family, other users' profiles — none of which
a queue needs.

Two real constraints:

1. **Search returns at most 10 results** (the `limit` maximum dropped from 50,
   the default from 20 to 5). Fine for "queue a song" on a phone, where nobody
   scrolls past ten anyway, but it rules out any design that leans on deep
   search paging.
2. **A Development Mode app allows five authorized Spotify users and one client
   ID per developer, and requires Premium.**

That second point is worth dwelling on, because it **retroactively justifies the
house-account design and kills the alternative.** Had each friend authorized
Spotify individually, the sixth person through the door would have been refused
by Spotify. Because only the house account ever authorizes, the cap is never
approached no matter how many people are on the couch.

## Honesty about the terms of service

librespot's own disclaimer says using it to connect to Spotify's API "is
probably forbidden by them," and both it and raspotify state they are for
personal private use, not commercial or public presentation. A house of friends
in a living room is squarely personal use, and the house account is a paid
Premium subscription. Recording it here so the decision is deliberate rather
than discovered later.

## Failure behaviour

Per the vision, every subsystem degrades to *"the room still plays media"*:

- **Spotify unreachable** → the queue still accepts adds and records them; the
  TV says music is offline. Nothing about launching Kodi is affected.
- **The BT speaker drops** → music stops; the queue is intact and resumes on
  reconnect. (Arc 1's lesson: the disconnect itself is a signal worth recording,
  and is what the vinyl handoff will later hang off.)
- **The token file is lost** → re-authorize once. No event data is lost, because
  none of it lives there.
- **A phone loses its token** → rescan the QR. Their history stays attached to
  their `people` row.

## Definition of Done

- Scanning the QR on the TV puts a working queue in your hand with no app
  install and no typing beyond a display name.
- Two phones can both add tracks, and the tracks play in the order they were
  added.
- A majority veto skips the current track, and the log says who voted.
- Music comes out of the Bluetooth speaker while a game's audio still goes to
  the TV.
- `GET /api/events` shows the whole night, actors attached, and survives a
  reboot.
- The five-command gate stays green, and the queue logic is tested with no
  network.

## Deferred, with reasons

- **BLE presence.** The hardest hardware piece in the original Arc 3 and not
  required for a queue. Phones give a good-enough roster today, and pulling BLE
  out keeps this arc shippable. It stays the headline of a later arc.
- **The vinyl handoff and content-type audio routing.** Genuinely clever — the
  speaker disconnect *is* the signal — but it needs the audio router, and this
  arc only needs "music goes to the speaker." Better done when there is a
  turntable in the room to test against.
- **Voting on anything but music.** `vote.cast` exists in the vision for
  brackets and picks. The queue's veto is deliberately its own kind, so the
  general voting model can be designed when Arc 4 needs it.

## Open questions for approval

1. **Is there a Bluetooth speaker to pair yet?** None is paired today — only a
   mouse and a DualShock 4 — and the only audio sink is HDMI. The queue can be
   built and tested against HDMI, but the "music to the good speaker" half of
   the Definition of Done cannot be verified without one.
2. **Does the house Spotify account exist, and is it Premium?** Premium is
   required twice over: by librespot for Connect, and by Development Mode.
3. **Should the TV show a now-playing card while an app is running?** It is a
   small addition to the Arc 1 surface, but it changes the home screen from
   "grid of tiles" to "grid plus status," which is the first step toward
   Attract Mode and worth deciding on purpose.
