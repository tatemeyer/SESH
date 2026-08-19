# Phase 5 surfaces — verified in the living room, 2026-08-19

PR #15 landed the phone, join and TV surfaces with this admission:

> **every test here is a DOM or HTTP assertion, and nothing has confirmed how
> any of it looks.** Whether the strip fits under the grid on a real 16:9 TV,
> whether the QR scans at 8vw from couch distance, whether the veto buttons are
> thumb-sized in a dark room — all unverified.

Tate sat in the room and checked. This is what that produced.

## Confirmed

**The TV reads correctly from the couch.** The now-playing strip shows song,
artist, `added by <name>`, and a waiting count. No clipping, and the layout was
not a problem in person.

**The QR scans from the couch**, with an ordinary camera app, no walking up.

**The queue works end to end from a phone**: search, tap to add, and the track
comes out of the Victrola while the default sink stays HDMI. Confirmed at the
routing level — `librespot - SESH` on the `bluez_output` sink, everything else
on `alsa_output` — and by ear in the room.

## What the room found that no test had

**The search box closed the keyboard mid-word** (#26). `renderPhone` rebuilt the
whole subtree on every draw, and `search()` draws twice per keystroke on top of
the 3s poll, so the `<input>` was destroyed as it was being typed in. Every DOM
test passed throughout: they assert on markup, and the markup was right. What
was wrong was the *identity* of the element between renders, which is invisible
to an assertion that only reads the latest output.

**Spotify 411'd every bodyless call** (#25), found in the same session's log
rather than by looking. `enqueue` is D7's pre-push, so the seamless handoff had
never worked against the real API; `play` sends a JSON body and covered for it.

## A prediction the room falsified

Before the check, from a screenshot measurement: the QR card is 307px, which is
8.0vw, about 9.8 cm on a 55" panel. Applying the common rule that a code scans
at roughly ten times its own width gave ~1 m, and the written expectation was
that it would fail from a couch.

**It scans fine.** The 10× rule is a conservative floor for cheap fixed-focus
scanners, not a description of a modern phone camera with autofocus and digital
zoom. Worth keeping because the arithmetic was right and the conclusion was
still wrong: the measurement did not need a rule of thumb bolted onto it when
someone was sitting three metres away willing to look.

## Still not verified

- **The majority veto**, which needs two phones. `needed` is 2 with one person
  present — the deliberate `MIN_VOTES` floor from `veto.rs`, not a bug: "one
  person in a room of one is not a majority overruling anyone."
- **Thumb-sized targets in a dark room**, beyond the search box.
- **Game audio to the TV while music plays**, end to end. The routing is proven
  at the sink level, but nobody has launched RetroArch over a playing track.

## Loose end worth a decision

With the queue empty, Spotify autoplay picks the next track and the conductor
records it as `music.started` with no `added_by`. That follows D8 — believe the
speaker, not the log — but it means the log cannot distinguish "the room chose
this" from "Spotify chose this", and the trophy case in a later arc will want
to. Not urgent; worth deciding before the log fills with it.
