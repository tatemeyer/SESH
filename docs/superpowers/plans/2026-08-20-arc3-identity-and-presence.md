# Arc 3 — Identity & Presence — Implementation Plan

**Goal:** The room can tell *who is in it* from *who is looking at their phone*,
and it says which one it knows. Presence stops being a single fragile proxy and
becomes a fusion of signals, each of which records how it knows — with one real
non-heartbeat signal built, so the fusion is exercised rather than theoretical.

**Spec:** `docs/superpowers/specs/2026-08-19-identity-and-presence.md`
(merged as PR #32, approved 2026-08-20).

**Status:** **Approved 2026-08-20**, and open. Phases 1–3 landed on master the
same day. **Q3 was answered "carried tags", then reversed to "bonded phones" —
see the correction below before reading anything about tags as current.** Phase
4 is next and no longer waits on hardware being bought; Phase 5 waits on an
ordinary evening.

---

## Scope, stated first because this spec invites sprawl

The spec describes six beats, a voice, tiers, profiles and the Stage. **This arc
builds the shape underneath all of them and none of them.**

| In | Out — and which arc it belongs to |
|---|---|
| `via` on presence events | The roster surface — later arc |
| The fusion projection (signals → who is here) | Attract Mode's first draft — later arc |
| Attention and presence as separate questions | Per-person profiles, mascots, cosmetics — Arc 4+ |
| One real BLE signal, in the room | Tiers and what they gate — deferred, see below |
| `asserted` as an ingest path | The Stage — Arc 4 |

The roster, Attract Mode and profiles are the things this arc **unblocks**. Each
becomes cheap once presence is trustworthy, and each is dishonest before then —
a roster built on today's presence would be a list of who has a tab open.

**The voice, the rarity law and the six beats are not implemented here.** They
are design constraints on surfaces this arc does not build. They are quoted in
the spec and stay there until there is a surface to apply them to.

## The open questions

The spec leaves three. **Only one gates this arc.**

**Q1 — what do the tiers gate?** *Not needed.* Tiers govern the veto denominator
and admin powers, and this arc changes neither. It changes what *presence* means,
which is an input to the denominator; the denominator's own policy is a separate
decision that gets easier once presence is real. Deferred with the roster.

**Q2 — how deep does profile customization go?** *Not needed.* Profiles are out
of scope entirely.

**Q3 — which BLE shape does the house adopt?** **Answered 2026-08-20 as "carried
tags first", then reversed the same day to "bonded phones first".** It gated
Phase 4, which is planned around bonded phones. Both the original reasoning and
the reversal are below, kept because a decision is only as good as what it rests
on — and that includes showing when what it rested on stopped being true.

### Q3, with a measurement rather than an argument

The spec states, flagged explicitly as a design constraint rather than verified
fact, that modern phones cannot be passively identified over BLE. **Measured on
TatePi, 2026-08-20, 19 minutes of passive observation:**

```
addresses that advertised          340
  resolvable-private (rotating)    189   56%
  random-static                    106
  public                            30
  non-resolvable                    15
ever advertised a name              18 of 340
```

The rotation signature is the *arrival pattern*, not the raw count:

```
new addresses first seen, per 10-minute window
                public   resolvable-private
  0–10 min          30                  163
 10–20 min           0                   26
```

**Addresses that cannot rotate saturate; addresses that can, never do.** Every
public address in range was found in the first ten minutes and then no more
appeared, while resolvable-private addresses kept arriving at a steady rate. If
the churn were people walking past, new public addresses would keep arriving
too. They do not.

That is the spec's constraint, confirmed with an in-dataset control rather than
assumed: **a passive scan cannot hold a stable per-person identity.** Something
must be bonded or carried.

> **Superseded — read the reversal that follows this section before acting on
> anything in it.** Kept because the hole in its reasoning is the useful part.

**Recommendation: tags first, bonded phones as an optional second `via`.**

- A tag advertises a stable address and needs no cooperation from a phone, no
  pairing flow per guest, and no app.
- Bonding every guest's phone puts the arc's foundation on the shakiest ground
  in this system. BlueZ bonding has now cost this project time **twice** — the
  `ClassicBondedOnly` rejection in Arc 1, and the agent-lifetime defect fixed in
  #35, where pairing reported success and silently stored no key for weeks.
  It works now, and it is still not what a guest should have to do at the door.
- The resident case is where a tag plainly wins: keys are already in a pocket.
- Guests keep the heartbeat as the floor, which is what the fusion is *for*.

**Decided 2026-08-20: tags first.** Accepted on the reasoning above — twice-bitten
BlueZ bonding, and a pairing ritual at the front door being the wrong thing to ask
a guest to do. **Bonded phones remain an available second `via` and are added only
if tags prove insufficient**, which is a question Phase 5 can answer and no
argument can.

> ### Reversed, 2026-08-20 — bonded phones lead, tags are the option
>
> **The decision above is superseded.** It is left standing rather than edited
> away, because the interesting part is not which shape won but that the
> argument for the loser had a hole in it.
>
> **1. My strongest objection had already expired when I made it.** The case
> against bonding rested on BlueZ having cost this project time twice: the
> `ClassicBondedOnly` rejection in Arc 1, and the agent-lifetime defect where
> pairing reported success and silently stored no key for weeks. But the second
> of those was fixed **in #35, by me, hours earlier in the same session**. I
> counted a bug as evidence against an approach at the moment I removed it. The
> evidence was gathered before the cause was known, and I did not re-weigh it
> after fixing it — which is the same failure as quoting a measurement taken
> under conditions that no longer hold.
>
> **2. Nobody has to buy or carry anything.** Every guest already walks in
> holding the thing. A tag is only free for a resident whose keys are in their
> pocket; for anybody else it is an object they do not own, and the plan as
> written blocked Phase 4 on hardware that does not exist yet. "No pairing at
> the door" was traded against "no phase until something is purchased", and the
> second cost is larger and was not stated.
>
> **3. The pairing ritual is a real cost, accepted deliberately.** It is not
> free and it is not being waved away. It is **one-time per person, not per
> visit**, which is a materially different thing from what "a ritual at the
> front door" implied.
>
> **Tags are not dropped.** They remain a supported `via` for anyone who prefers
> one — the resident case where keys are already in a pocket is still the case
> where a tag is genuinely nicer. What changes is which one Phase 4 builds
> first.
>
> **Phases 1–3 are unaffected and need no rework.** A bonded phone is
> `via: ble` exactly as a tag was, so the `via` vocabulary, the
> attention/presence split and the fusion windows all stand untouched. That
> they survive a reversal of the signal source is the strongest evidence so far
> that the seam was drawn in the right place.

## Shape of the work

Five phases, each landing as its own PR behind a green gate. Ordered so the
three phases with no hardware dependency come first and the whole model is
provable before a radio is involved — the inverse of Arc 2, which learned this
the hard way.

| # | Phase | Lands | Hardware? |
|---|---|---|---|
| 1 | `via` on presence | Every presence row says how it knows | No |
| 2 | Attention ≠ presence | The two questions separate, callers choose | No |
| 3 | The fusion projection | Signals → who is here, per-`via` windows | No |
| 4 | A real BLE signal | `via: ble` from bonded phones | **Yes** |
| 5 | An ordinary evening | The roster checked against the couch | **Yes, and people** |

### Phase 1 — `via` on presence

`presence.arrived` and `presence.left` carry `via`. Additive: absent means
unknown, which is exactly what the existing rows are.

- Vocabulary: `ble`, `wifi`, `heartbeat`, `asserted`. Fixed set, validated on
  the way in, but an unknown value is **preserved and treated as unknown**
  rather than rejected — `POST /api/events` stays open per the invariant.
- `rssi` rides along when the signal has one. Coarse confidence is a projection
  concern, not a log concern; the log records the reading.
- Everything SESH emits today becomes `via: heartbeat`, which is honest about
  what it has always meant.

**Tests:** a `presence.arrived` posted through `POST /api/events` with no `via`
is valid and reads as unknown; one with an unrecognised `via` is preserved;
round-trip through the store.

### Phase 2 — attention and presence come apart

Today `presence.rs` answers one question and `veto.rs` uses it for another.

- **Attention** — is this person looking at SESH right now. The heartbeat
  measures it well. Correct input for *may this person act*.
- **Presence** — is this person in the room tonight. Correct input for the
  roster, the denominator, and every later arc.

Both are computed; callers name which they want. The veto denominator moves to
presence, and that is the one behaviour change in this phase — it is why the
phase exists, and it is why the arc does not close until Phase 5 checks the
denominator against a real room.

**Tests:** a phone that goes quiet loses attention but keeps presence until its
presence window expires; the denominator follows presence, not attention.

### Phase 3 — the fusion projection

A pure function over signal sequences, in the same category as the existing
projections. No I/O, no hardware, no clock beyond `Clock::mono_ms`.

Settled here, per the spec's *Behaviour to settle*:

- **Per-`via` windows.** A BLE gap of thirty seconds is someone walking to the
  kitchen; a heartbeat gap of thirty seconds is a locked screen. These are not
  the same timeout and pretending they are is the current defect in miniature.
- **Precedence when signals disagree.** Proposed: presence is the *union* of
  signals that are live, and a stronger `via` only overrides a weaker one when
  it is **positively absent**, never merely stale. BLE saying "gone" outranks
  the heartbeat saying "here"; BLE saying *nothing* does not.
- **`asserted` outranks everything and expires.** A room that can be told it is
  wrong is the spec's requirement; an assertion that never expires is a lie with
  a long half-life.
- **`person.joined` keeps its meaning** — becoming known to the house, once.
  Recognition is `presence.arrived` with a `via`. They were always different
  events and only looked like one because joining was the only way in.

**Tests:** synthetic evenings — seed BLE beats, heartbeats and assertions on a
timeline, assert the roster at each step. This is where the logic lives, and it
is all testable with no network and no radio.

### Phase 4 — a real BLE signal, from bonded phones

Emits `presence.arrived { via: "ble", rssi }` through the same `Room::record`
path as everything else. RSSI is what separates *in this room* from *in this
apartment*; its threshold is tuned in Phase 5 against a real room, not guessed
here.

**The signal source is a phone bonded once to the Pi.** A bond exchanges an
Identity Resolving Key, and the IRK is what makes a rotating address resolvable
back to a person — which is the entire difficulty the measurement below
describes. Everybody already walks in holding a phone, so nothing has to be
bought, carried, or remembered, and this phase is not blocked on a purchase.

**The cost, stated rather than waved away:** a one-time Bluetooth pairing per
person, at some point that is not necessarily their first thirty seconds in the
house. **One-time per person, not per visit.** That is the trade, and it is
accepted deliberately.

What the bonded-phone model requires, and must be built rather than assumed:

- **Enrolment is a bond, not a registry entry.** The pairing itself is the
  enrolment; what SESH stores is the mapping from resolved identity to person.
  That is identity data, so it belongs with `people` — the one table Arc 1
  permits `ALTER TABLE` on, and nowhere else.
- **Match, never enumerate.** Unchanged from the tag plan and **not
  negotiable.** The scanner resolves observed addresses against enrolled
  identities and ignores everything else. It must never build a list of what is
  in range: that is a surveillance device, and at ~340 addresses per 19 minutes
  in this flat it is also useless.
- **An unbonded phone is not a person.** No implicit enrolment from proximity,
  ever. Walking past the Pi is not consent and is not identification.
- **Tags remain a supported `via`.** Anyone who would rather carry a tag than
  bond a phone still can, and the resident case — keys already in a pocket — is
  where a tag is genuinely the nicer object. The fusion cannot tell them apart
  and does not need to: both are `via: ble`.

**Land it incrementally, and this is a requirement rather than a nicety: the
first bonded phone must work before the last one does.** Each person enrols
independently, everyone else stays on `heartbeat`, and the fusion already
handles a room where different people are known by different signals. There is
no flag day and there must not be one.

Two things already measured that this phase must not rediscover:

- A passive scan sees ~340 addresses in 19 minutes in this flat, 56% of them
  rotating and only 18 of 340 ever giving a name. Resolving against known
  identities is the only thing that works at that density.
- **Killing a scan client mid-discovery latches `Discovering: yes`**, after
  which the adapter silently finds nothing and `scan off` fails with
  `org.bluez.Error.Failed`. Cost two dead ends on 2026-08-20. Whatever holds
  discovery must stop it on every exit path — normal end, signal, and panic.

And one thing learned the hard way, which this phase depends on directly:
**`bluetoothctl pair` as a one-shot exits before the bond is written**, leaving
`Paired: yes` that evaporates and no `[LinkKey]` on disk. Fixed in #35; the
working recipe is in `deploy/README.md` and `deploy/pair-speaker.sh`. Enrolment
here is the same mechanism against a different class of device, so it starts
from the fixed version rather than rediscovering the defect.

### Phase 5 — an ordinary evening

Not a test suite. The spec is explicit and it is the same bar Arcs 1 and 2
closed against: **the roster at 11pm, compared against the people actually in
the room, counted by looking up from the couch.**

Until that comparison exists, no threshold derived from presence — the veto
denominator above all — is trustworthy, and no design may be tuned against the
numbers currently in the log.

## Definition of Done

1. Every presence row SESH writes carries a `via`; rows that predate the change
   read as unknown and nothing breaks.
2. A `presence.arrived` posted through `POST /api/events` by a producer that
   has never heard of `via` is still valid.
3. Attention and presence are separately answerable, and the veto denominator
   uses presence.
4. The fusion projection is proven over synthetic evenings with no hardware.
5. A real BLE signal produces `via: ble` rows in the live log on TatePi, from
   at least one bonded phone — and **the first bonded phone works while others
   are still on `heartbeat`**, because a flag day is a failure of this design.
6. **The 11pm roster matches the people in the room**, checked by looking, with
   real phones belonging to real people. A tag standing in for a phone does not
   close this item: the enrolment path is part of what is being tested.
7. Gate green at every phase boundary.

Items 5 and 6 are the ones a green suite cannot see. Arc 2 lost its time to
exactly four bugs of that kind, and every one of them lived outside a test
boundary.

## Not in scope

- **Backfilling the existing rows.** Append-only; absent `via` is unknown.
- **The roster surface, Attract Mode, profiles, tiers, party mode.** Unblocked
  by this arc, delivered by later ones.
- **Wifi as a signal.** It is in the `via` vocabulary from Phase 1 so the log
  and the fusion can accept it, but no wifi watcher is built here. BLE is the
  signal that distinguishes this room from this apartment, and that distinction
  is the point.
- **Replacing the heartbeat.** BLE outranks it. It does not remove it, and the
  degraded mode the vision describes stays exactly as it is.
