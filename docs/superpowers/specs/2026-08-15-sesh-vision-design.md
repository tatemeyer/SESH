# SESH — Vision & Architecture

**Date:** 2026-08-15
**Status:** Approved (vision). Arc 1 spec pending.

---

## What this is

`SESH` turns a living room into a shared social system. A Raspberry Pi 5 on
the TV runs the room: it knows who's on the couch, gives everyone's phone a
say in what happens, keeps a permanent record of what the house has done, and
reacts to it.

It is deliberately **not** another media box. Kodi, RetroArch, and Moonlight
already do their jobs well and are launched as apps. What does not exist —
what this project builds — is the layer that ties the room, the people in it,
and everything that has ever happened in it into one thing.

## Audience

A senior-year college house. Friends over most nights, games on the TV, music
on the speaker, a rotating cast of people who all know each other. This drove
real design decisions, not just tone: the trophy case is competitive rather
than sentimental, the shared music queue is a first-class feature rather than
an afterthought, and the leaderboard covers pong and poker alongside Smash.

## Hardware

| Device | Role |
|---|---|
| **Raspberry Pi 5** | The front door. Runs `SESH`. Native retro emulation, personal media, all custom surfaces. |
| **Gaming PC** (on LAN) | The muscle. Streamed to the couch via Sunshine → Moonlight: Dolphin/Wii, modern games, and a full browser (which is also the working path to DRM streaming at real 4K). |
| **Smart TV / stick** | Fallback. Native streaming apps for when the PC is off. |
| **Bluetooth speaker** | Music output. |
| **Bluetooth turntable** | Independent audio source that competes for the speaker (see *Audio*). |
| **Smart bulbs** *(optional)* | Driven as a reactor. |

### Why the Pi is the brain and not the muscle

The two headline wants — DRM streaming services and Wii emulation — are the
two things a Raspberry Pi is worst at. Widevine on ARM Linux caps out around
720p and breaks on key rotations; a Pi 5 is borderline for GameCube and
effectively cannot do Wii. Both problems dissolve once the PC handles them and
the Pi handles the room. The Pi never fights a battle it loses.

## Core insight

Every feature in scope is the same thing wearing a different hat:

- **Trophy case** — who won, across all nights
- **Bit button** — what happened, at this moment
- **Resume night** — where did *this group* leave off
- **Tournament mode** — who beat who, in order
- **Attract mode** — show me the interesting parts of all of the above
- **Lights** — react to what is happening right now

All of them are **queries over an append-only log of things that happened in
this room, keyed by who was present.** Build that core once and five features
stop being five projects and become five views of one.

## Architecture

```
┌─ Raspberry Pi 5 ────────────────────────────────────────┐
│                                                          │
│   seshd  ──  SQLite event log + projections              │
│      │       HTTP/WS API on the LAN                      │
│      │                                                   │
│   ┌──┴──────┬──────────┬───────────┬──────────┬────────┐ │
│   │         │          │           │          │        │ │
│ presence  launcher  surfaces    reactors   capture  audio│
│ (BLE)     (apps)    (web app)   (lights)  (ringbuf) (rtr)│
│                         │                              │ │
│              ┌──────────┴──────────┐            ┌──────┴┐│
│              │                     │            │       ││
│         TV (kiosk browser)    phones (LAN)   HDMI    BT  ││
│                                              (TV)  (spkr)││
│                                                          │
│   launched apps:  Kodi │ RetroArch │ Moonlight ──────────┼──▶ Gaming PC
└──────────────────────────────────────────────────────────┘
```

**Base:** Raspberry Pi OS Lite (64-bit), no desktop. **labwc** as the Wayland
compositor, hosting a kiosk Chromium plus whatever app the launcher brings to
the front.

**Process isolation:** every box above is a separate process talking to `seshd`
over the local API. If the lights daemon crashes, movie night does not notice.

**Stack:** `seshd` in Rust (`axum` + `rusqlite` + `btleplug`) — a single static
binary is the right shape for an always-on daemon on a Pi. Surfaces in
TypeScript, one bundle serving both the TV route and the phone route.

## Components

| Component | Owns | Why it is isolated |
|---|---|---|
| **seshd** | Event store, projections, API | Knows nothing about TVs, phones, or bulbs |
| **presence** | BLE scan → `presence.*` events | BLE is flaky; must never take the core down |
| **launcher** | Start/stop Kodi, RetroArch, Moonlight; compositor focus | Most likely subsystem to wedge |
| **surfaces** | TV route + phone route (one bundle) | Both are pure clients of the WS feed |
| **reactors** | Lights, recap cutter | Side-effect subscribers; individually disableable |
| **capture** | Rolling A/V ring buffer | Only process doing continuous work |
| **audio** | Output routing, BT source, queue playback | Owns the one piece of genuinely stateful hardware |
| **ingest port** | `POST /events` for game results | Deliberately unimplemented — see *Deferred* |

Each component can be described in one sentence, tested on its own, and killed
without taking the room down.

## Audio

The room has three sources (Pi, turntable, phones) and two output paths with
opposite requirements.

- **HDMI → TV** for anything with a picture. Game audio needs tight sync;
  Bluetooth's ~150 ms latency would make Smash unplayable.
- **Bluetooth → speaker** for music-only content. Latency is irrelevant and the
  good speaker is the right destination.

Routing is chosen by **content type, not by user selection** — games and video
go to the TV, the shared queue goes to the speaker. This is automatic and
never surfaced as a setting.

### Vinyl handoff

Most Bluetooth speakers hold exactly one active connection. When someone starts
a record, the turntable claims the speaker and **disconnects the Pi**. That
disconnect is a reliable signal that vinyl has taken over — no sensor, no extra
hardware, no user action. On disconnect, `SESH` pauses the queue, emits
`music.vinyl_started`, and shows a "now spinning" card on the TV. When the Pi
reconnects, the queue resumes. The failure mode is the feature.

## Data model

One append-only table:

```
events(id, ts, kind, actors[], subject, payload)
```

Event kinds include `presence.arrived`, `presence.left`, `session.started`,
`session.ended`, `app.launched`, `match.result`, `moment.captured`,
`bracket.advanced`, `vote.cast`, `music.queued`, `music.vetoed`,
`music.vinyl_started`.

**Everything else is a projection.** The roster, leaderboards, streaks,
head-to-head records, resume-night state, and the recap reel are all derived
views — cached for speed, rebuildable from the log at any time, never
authoritative.

The payoff: a stat invented two years from now applies retroactively to every
night ever hosted. *"Longest gap between Sam visits." "Games only ever played
when Marcus is here."* Nobody has to know today what will be worth measuring
later. The log only has to be honest about what happened.

`people` is a small table — display name, avatar, BLE identifiers, phone token.

## Identity and trust

No accounts, no passwords. Identity is a name and a face in this house. Phones
authenticate by scanning a QR code on the TV, which issues a per-person token
scoped to the LAN. This is a friendly-network trust model, chosen deliberately:
auth theater in your own living room costs real usability and buys nothing.

## Failure behavior

The event log is the only thing that must survive. It is append-only and
fsync'd; projections rebuild from zero on corruption.

Every subsystem degrades to *"the room still plays media"*:

- presence dies → roster falls back to whoever is on the phone app
- lights die → nothing visible happens
- audio router dies → sound falls back to HDMI
- launcher wedges → watchdog restarts the session

## Testing

- **Projections** are pure functions over event sequences. Seed a synthetic
  night, assert the resulting trophy case. This is where the logic lives, and
  it is all trivially unit-testable.
- **Presence, launcher, capture, audio** are hardware-bound and verified
  manually.
- **TV surface** must be looked at — screenshot review on the real Pi, judged
  at ten feet, not in a desktop browser.

## Feature set

### In scope

1. **The room knows who is on the couch** — BLE presence detection. An ambient
   roster, not a login. Load-bearing for everything social.
2. **Nobody holds the remote** — QR code on the TV, no app install, every phone
   becomes a controller. Queue, veto, vote, report scores.
3. **Attract mode** — the TV is never a black rectangle and never a menu when
   nobody is using it. What it actually shows is decided in Arc 2's spec; the
   leading candidate is a broadcast-style screen driven by the trophy case
   (standings ticker, streaks, head-to-head records, recent clips on loop).
4. **The trophy case** — persistent cross-night, cross-game records for the
   house. Head-to-head records, streaks, standings.
5. **The bit button** — rolling 30-second buffer; anyone can save the last 30
   seconds from their phone. End of night, an auto-cut highlight reel.
6. **Resume night** — session state, not save state. *"Last time you five were
   here you were 40 minutes into Ocarina and had just gotten the slingshot."*
7. **Tournament mode** — bracket on the TV, phones for seeding and reporting.
8. **The room reacts** — smart bulbs as a subscriber to the event feed.
9. **Shared music queue** — everyone adds from their phone, majority veto
   skips. Solves the aux-cord problem: nobody is hostage to one playlist and
   nobody hands over an unlocked phone. Likely the most-used feature here.
10. **House leaderboard covers everything** — pong, poker, darts, and Catan
    land in the same trophy case as Smash. A free consequence of manual score
    reporting: the log does not care whether the game was on the TV.
11. **House rules and timers** — on-screen round timers, countdowns, power-hour
    playlists, and custom house rules displayed on the TV.

### Cut, and why

- **The always-on broadcast schedule** — a generated "channel" of what to watch
  tonight. Heavy content-metadata work for a problem this house does not have.
- **Guest NFC cartridges** — physical tokens per friend. Charming, but presence
  detection plus the phone QR already covers identity.
- **Home-video interstitials and doorbell picture-in-picture** — wrong register
  for this room.

## Build order

Six Arcs, each independently shippable. Each gets its own spec and plan.

**Arc 1 — The Log & The Room.** `seshd`, the SQLite event log, the `people`
table, the WS feed, a kiosk shell that boots to a fullscreen web app, and a
launcher that can start, stop, and return from Kodi, RetroArch, and Moonlight.
*Done when:* the Pi boots, lands on its own screen, and launches and quits all
three apps with a controller. Nothing above this is interesting until it exists.

**Arc 2 — Attract Mode.** The ambient screen. First thing that makes the room
feel different, and pure surface work on Arc 1's foundation.

**Arc 3 — Presence & Phones.** BLE watcher, QR handoff, phone surface, live
roster, shared music queue, audio routing and the vinyl handoff.

> **Reordered 2026-08-17.** Arcs 2 and 3 have swapped, and Arc 3 has been
> split. Attract Mode as specified above is driven by the trophy case, which
> is Arc 4 — with a log of eight events, no actors, and an empty `people`
> table, it would have been a broadcast screen with nothing to broadcast.
> The phone half of Arc 3 goes first instead, since it is what starts filling
> the log with social events. BLE presence and the vinyl handoff stay behind
> for a later arc; phones give a good-enough roster meanwhile, which is the
> degraded mode *Failure behavior* already describes.
> See `2026-08-17-arc2-phones-and-queue.md`.

**Arc 4 — The Record.** Trophy case, resume-night sessions, brackets, house
rules and timers. All projections plus surfaces, fed by manual reporting from
Arc 3's phone app.

**Arc 5 — The Bit Button.** Ring buffer, capture, auto-cut recap. Most
technically involved; deliberately last.

**Arc 6 — The Room Reacts.** Lights as a reactor. Small, and best once there
are rich events to react to.

Note that **Arc 4 gets manual score reporting for free** from Arc 3's phone
surface, which is why the deferred decision below costs nothing.

## Deferred decisions

**How the log learns what happened in a game.** Three strategies were
considered — friends reporting on their phones, the Pi watching the screen with
a vision model, and reading emulator RAM directly. The decision is deferred.

The core exposes a documented **event-ingest port** (`POST /events`) that any
future producer can post to. Manual reporting arrives free with Arc 3, so a
working trophy case exists before the automatic-capture question ever needs an
answer, and nothing built before then has to change when it does.

## Rejected alternatives

**Extend Kodi/OSMC.** Media handling and remote support come free, but attract
mode, the trophy case, and brackets would all be born as Kodi addons fighting
Kodi's skinning engine — a hostile toolkit for bespoke UI — and the phone
experience would be a second codebase.

**Batocera / EmulationStation-DE as the base.** Ships with emulation, Kodi, and
Moonlight preconfigured; fastest path to games on the couch. But it is closed
at the top, so building a custom shell over it means fighting the thing that
made it fast.

**Pi does everything.** Rejected on the hardware analysis above.

**One codebase for the whole system in TypeScript.** Viable, and simpler to
hack on late at night. Rust was chosen for `seshd` specifically because a
single static binary is a better shape for an always-on daemon; the surfaces
stay TypeScript, and the seam between them is plain HTTP/WS.
