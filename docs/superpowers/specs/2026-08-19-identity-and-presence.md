# SESH — Identity & Presence

**Date:** 2026-08-19
**Status:** Proposed. Needs approval before a plan is written.
**Target:** v1 goal unset. This spec defines the shape; the arc it lands in is
a separate decision.
**Blocks:** the roster, Attract Mode's honest first draft, per-person profiles,
and any veto threshold anyone should believe.

---

## What the log can and cannot tell us

The live log on TatePi, three days in:

```
total events: 211
span: 2026-08-16 21:23 -> 2026-08-19 20:15

    93  music.started
    92  music.skipped
     5  app.launched
     5  app.exited
     4  audio.sink_found
     3  presence.arrived
     3  presence.left
     3  audio.sink_lost
     2  music.queued
     1  person.joined

events with an actor: 9 / 211
people who have ever existed: 1
```

Every presence row in the log is Tate, and they read like this:

```
12:53:05  presence.arrived  tate
13:04:56  presence.left     tate     (11m 51s)
17:11:37  presence.arrived  tate
17:54:19  presence.left     tate     (42m 42s)
19:54:01  presence.arrived  tate
20:15:08  presence.left     tate     (21m 07s)
```

**None of this is behaviour, and this spec does not argue from it.** Every row
was written during active development, by the one person building the thing
while it ran. The 93 `music.started` rows are a Spotify session playing next to
a compiler rather than a house listening to music, and those three arrival /
departure pairs are genuine comings and goings — Tate did leave, several times.
The log is an accurate record of a period that was not an ordinary evening.

That is worth stating as a finding rather than a footnote: **SESH has never
observed a normal night.** Every behavioural claim currently available about
this product — including "nobody queues" — is unfounded until one honest evening
exists in the log. Designing against these numbers would be designing against
the development process.

What the log does establish is smaller, and survives the caveat:

- `person.joined` is **1**. One human being has ever existed in this system.
- Nine of 211 events carry an actor, and only two kinds of row are social at all
  — `person.joined` and `music.queued`.

The defect this spec addresses is visible in the **code**, not in the data,
which is where the next section starts. It would be just as true against an
empty log.

> **Note for `2026-08-19-what-the-room-chose.md`.** That spec opens on the same
> log and reads "61 of 63 tracks were chosen by Spotify rather than by anyone in
> the room" as a fact about the house. It is a fact about a development period.
> Its recommendation still stands on its own merits — the queue and autoplay
> produce genuinely indistinguishable rows, and that is a structural defect
> rather than a statistical one — but the ratio should not be quoted as
> behaviour, and its open question about whether the room should go silent when
> the queue runs dry cannot be answered from this data.

## What is actually wrong

Two things, and they are separable.

### 1. "Present" means "has the SESH tab open and foregrounded"

```
phone.ts:198      if (!document.hidden) void heartbeat();
presence.rs:102   state.present && now_ms - state.last_ms >= WINDOW_MS  ->  left
```

This is a defensible implementation and its comment says so plainly: *"a phone
that is asleep in someone's pocket is not in the room."* Read as an experience
it inverts the product. The person four beers into a Smash set with their phone
face-down on the couch is not in the room. The person who went home an hour ago
with the tab still open is.

Because presence is also the veto denominator, the majority threshold silently
tracks **who is currently looking at their phone** rather than who is here.

No row in the log demonstrates this yet, because nobody has spent an ordinary
evening in the room with SESH running. The claim rests on the two lines above,
which are enough on their own.

There are two different questions wearing one answer:

- **Attention** — who is looking at SESH right now. The heartbeat measures this
  well, and it is the right input for *may this person act*.
- **Presence** — who is in this room tonight. The roster, the trophy case,
  resume-night, and *"what was playing when Marcus walked in"* all need this,
  and the heartbeat is a bad proxy for it.

### 2. Identity is a credential, not an experience

`Identity` is `{ id, name }`. You scan a QR, type a name, and receive a token in
`localStorage`. From then on your entire existence in this product is a grey
byline under a track: `added by tate`.

There is **no roster anywhere** — not on the TV, not on the phone. The veto
button reads `1/2`, where the `2` is a count of human beings that nobody in the
room can see.

For a product whose one line is *"a living room that knows who's in it,"* the
room currently offers no evidence that it knows. You join SESH the way you join
a wifi network.

## Constraints

- **The log is append-only.** The 211 rows stand. Everything here applies
  forward, and every reader must cope with a prefix that predates it.
- **Additive.** A reader that does not understand the new fields must still be
  right about everything it already knew.
- **`POST /api/events` stays open.** A future producer must be able to post a
  `presence.arrived` without knowing about any of this.
- **`people` is an identity registry, not a projection.** `ALTER TABLE` is
  permitted here and nowhere else, per Arc 1.
- **No UI framework in `surfaces/`.** This constrains how deep profile
  customization can go before it becomes a separate decision. See *Open
  questions*.
- **`http://<pi>:7373` is not a secure context.** No service workers, no
  `getUserMedia`. Camera work happens in the phone's own camera app.

## Presence records how it knows

`presence.arrived` and `presence.left` carry the signal that produced them:

```json
{ "kind": "presence.arrived", "actor": "tate",   "via": "ble",  "rssi": -58 }
{ "kind": "presence.arrived", "actor": "sam",    "via": "wifi"              }
{ "kind": "presence.arrived", "actor": "jess",   "via": "heartbeat"         }
{ "kind": "presence.arrived", "actor": "marcus", "via": "asserted"          }
```

Absent means unknown, which is exactly what the existing rows are.

This is the fourth time this project has answered a question of this shape, and
the idiom is now settled house doctrine: `exit_observed: false` for an exit SESH
did not see, `clock_synced: false` for a timestamp it could not trust,
`source: autoplay` for a choice it did not make, and now a `via` for a person it
inferred rather than observed. **Record what you know about how you know it,
rather than flattening it into the thing you wish you knew.**

Two things fall out of this immediately:

- The heartbeat stops *being* the definition of presence and becomes one `via`
  among several — the weakest one. Attention and presence come apart without
  anything being deleted.
- BLE can land **incrementally**. The first person with a bonded phone gets
  `via: ble` while everyone else stays on `heartbeat`, and every reader still
  works. There is no flag day.

`asserted` is the escape hatch and it is not optional: the TV shows who the room
*believes* is here, and any human can correct it. **A room that can be told it
is wrong never becomes infuriating.**

## Getting a real presence signal

The decision is BLE proximity. The spec has to be honest about what that costs,
because there is a constraint here that nothing in the project has hit yet.

**Modern phones cannot be passively identified over BLE.** iOS and Android both
rotate their BLE advertising address roughly every fifteen minutes, specifically
to defeat this. A `btleplug` scan sees a fresh stranger four times an hour,
forever. The one exception is that such an address is a *Resolvable* Private
Address, and a device holding the Identity Resolving Key — exchanged only during
a real BLE **bond** — can resolve it back to a person.

*(Stated as a design constraint, not as verified fact. Per this project's own
rule about off-hardware claims, none of this is settled until it has been run
against real phones on TatePi.)*

So BLE has three honest shapes:

| Shape | Costs the person | Gives |
|---|---|---|
| **Bonded phone** | A one-time real Bluetooth pairing with the Pi, beyond the QR scan | True per-person BLE with no extra hardware. Heavier onboarding, and BlueZ bonding on this Pi has already bitten this project once (`ClassicBondedOnly`) |
| **A tag** — beacon on a keychain | Carrying a thing | Stable address, fully passive, no phone cooperation. The vision cut "guest NFC cartridges" as unnecessary; for a **resident**, whose keys are already in their pocket, this is the cheapest true signal available |
| **Watch / band** | Owning one | Often advertises more stably. Coverage across a friend group is uneven |

And one signal that is not BLE and belongs in the spec anyway:

**The house wifi already knows.** iOS and Android randomize wifi MACs *per
network but hold them stable for that network* — a phone presents the same
address to this apartment every time it walks in. No pairing, no tag, no app, no
user action at all. "You are on the house wifi" is also a trust boundary this
project has already declared load-bearing.

What wifi cannot do is the thing that matters most here: **wifi says you are in
the apartment; BLE RSSI says you are in *this room*.** A roommate asleep in their
bedroom at 2am is on the wifi and is not in the room, and under today's design
they would be silently inflating the veto denominator.

**Recommendation: presence is fused, not chosen.** BLE is the strongest `via`
and the reason the roster becomes trustworthy; wifi is a cheap, zero-friction
second opinion; heartbeat remains the floor and the degraded mode the vision
already described. The projection decides who is present from whatever signals
exist, and the log says which ones they were.

## The room's voice

**Housemate.** Lowercase, warm, first-person-room. The room talks like someone
who lives here.

```
first join        hey tate — first night here
regular arrives   tate's here
long absence      sam's back — first time since march
departure         sam headed out
veto in progress  1 of 2 want this gone
```

This is a decision about safety as much as charm. *"The room knows who's in
it"* is delightful or surveillant depending almost entirely on wording, and the
rejected alternative — an archival voice reading `SAM · departed 23:14 · gap: 5
months` — is the same log rendered as an attendance record. **No surface may
display a departure timestamp or a gap computed as arithmetic.** "first time
since march" is warm; "absent 147 days" is a personnel file.

## The reaction is proportional to how notable the event is

If the TV announces `tate's here` every morning, the announcement is wallpaper
inside a week and it takes the whole feature down with it. Tate lives here.

> **Law: the room's reaction scales with how rare the thing is.**

| Event | Reaction |
|---|---|
| Resident comes home on a Tuesday | Nothing, or a name quietly joining the roster |
| Regular arrives on a normal night | A line, briefly |
| First-time visitor | The TV stops what it is doing |
| Back after months | The TV stops what it is doing, and says how long |

Same event kind, same row, wildly different volume. This law is also what will
keep Attract Mode from becoming noise when its turn comes.

## The six beats

Identity is not a screen. It is the sequence of moments where the room does or
does not respond to a person.

**1 — Arrival.** Today nothing happens; the room is unchanged by a person
walking into it. The room should acknowledge arrival *on the TV*, which converts
joining from a private phone action into a public event in the room. This is the
single highest-leverage change on this axis and the event already exists.
Volume per the law above.

**2 — Becoming someone.** Today: scan, type a name, join. Two changes. The join
screen should offer **everyone who has ever been in this house** as tap targets
plus a "someone new" option, so returning is one tap and typing happens once
ever — the house's own history becomes the onboarding UI. And each person picks
**one visual token**, because a colour or a mascot reads at ten feet on a TV
where a 12px name never will.

**3 — Being here.** Build the roster. It does not exist in any surface today,
and building it also produces **the honest first draft of Attract Mode** — not
the broadcast ticker blocked on Arc 4's trophy case, but the quieter thing: the
TV, idle, showing who is in this room right now. True on day one, needing no
projection that does not already exist, and literally the product's tagline
rendered as a screen.

**4 — Doing things that make you visible.** A person's only trace today is grey
12px text. The room can reflect people back at themselves — *"tate's queued 5 of
the last 6"* is funny, and it is social pressure, and both are things the
product currently cannot exert at all: adding a track changes nothing anyone in
the room can see. Whether that is *why* the queue goes unused is not yet
knowable — see the caveat on the log above — but it is a cost worth removing
either way. The veto, the
one genuinely multiplayer mechanic in the product, currently happens silently in
three separate pockets and should happen on the TV.

**5 — Leaving.** `presence.left` fires and nothing observes it. Departure is the
natural hook for an end-of-night beat and is where the tone risk is highest; see
the voice section.

**6 — Returning.** *Nothing in the product currently touches this beat, and it
is the one only SESH can do.* The vision has resume-night, scoped to session
state. The human version is stronger and far cheaper:

```
sam's back — first time since march
first time all five of you have been here since the semester started
tate — 40 nights
```

Every one of those is a query over a log that already exists, and none needs the
trophy case, the bit button, or BLE. **This is the payoff of the append-only bet
delivered to a person's face instead of to `GET /api/events`.**

## Residents, and auto-connect

A resident is not *arriving*; they are *home*. That distinction has to exist in
the model or beat 1 destroys itself on the most frequent case in the house.

Once presence carries a real identity signal, **the QR becomes the first-time-only
path.** Your phone opens the page and the room already knows who you are, because
your keychain tag or your bonded phone walked in three minutes ago. This is a
better "quick connect" than remembering a token: `localStorage` is per-browser
and evaporates, whereas the room recognising a body survives a cleared browser,
a new phone, and Safari private mode.

## Guest tiers

| Tier | Who | Becomes one by |
|---|---|---|
| **Resident** | Lives here | Declared, once |
| **Regular** | Here most weeks | **Earned** — N nights present |
| **Guest** | Invited, been here before | Second visit |
| **Stranger** | Someone's plus-one at a party | Walking in |

Regular being *earned rather than assigned* matters: *"sam's a regular now"* is a
good moment on the TV and it is a pure projection over the log.

**The question is not the names, it is what they gate**, and there is a failure
mode here that nobody has hit yet: **a party of thirty makes majority veto
mathematically impossible.** Five friends need three votes; thirty people need
sixteen, and sixteen people will never tap a button. Tiering is one fix. The
other is noticing that **a hangout and a party are different products** — five
people want a shared queue, thirty want a request line — and that *party mode*
may be the more useful axis than tier. Unresolved; see *Open questions*.

## Profiles

Steam-shaped, with one unlock worth naming:

> **A profile is the trophy case keyed by person instead of by game.** Same
> projections, same engine, a different view.

That makes profiles not a side feature competing with Arc 4 for attention — it
makes them Arc 4's second surface, nearly free once the first exists.

- **A mascot or pet as the avatar**, because it reads at ten feet. It lives on
  the TV roster, on a bracket seed, in a lobby. It can react — celebrating a
  queued track, sulking at a vetoed one.
- **Cosmetics unlocked from the log.** 100 tracks queued. 50 nights present.
  First one here 20 times. Won a bracket. Survived a veto. Every one is a query,
  and every one applies **retroactively to a log that already exists** — the
  vision's promise that "a stat invented two years from now applies to every
  night ever hosted," finally pointed at a person rather than a game.
- **A showcase.** The person picks what is displayed:
  `tate — 412 queued · 3-night smash streak · here since february`.

**Cosmetics only matter if they are seen.** A profile nobody looks at is dead
weight, so profiles depend on the roster, the bracket, and whatever the TV
becomes. They cannot ship first.

## The Stage

Named here because it is where this axis leads, and scoped out of this spec so
it does not swallow it.

Today the TV is a **launcher** and phones are **remote controls**. The Jackbox
reframe is that the TV is a **stage** and phones are **seats** — the same
hardware, a different product. It is the fix for the deepest interaction flaw in
the system: *one person holds the controller.*

The room can ask the room a question — *"what are we doing?"* — and four tiles
appear and everyone taps, which is app launching as a group decision. Reactions
from a phone pop on the TV, at near-zero cost and high frequency, and every tap
is a logged event **with an actor**, which is precisely the social density that
Attract Mode and the trophy case are starving for.

One law comes with it, and it is the thing most second-screen products get
wrong:

> **Nothing may ever require everyone.** Anything that blocks on full
> participation dies the first time somebody is in the bathroom.

## Behaviour to settle in the plan

- The `via` vocabulary. `ble`, `wifi`, `heartbeat`, `asserted` are the four that
  have a meaning today. Whether `rssi` or a coarser confidence rides along.
- How signals fuse, and specifically what beats what when BLE says gone and the
  heartbeat says here.
- Whether the presence *window* differs per `via`. A BLE gap of thirty seconds
  is someone walking to the kitchen; a heartbeat gap of thirty seconds is a
  locked screen.
- Whether `person.joined` is still the right event once joining and being
  recognised come apart.
- What the roster looks like at ten feet, judged on the real Pi per the vision's
  testing rule, not in a desktop browser.

## Testing

- **Fusion is a pure function over signal sequences** and belongs in the same
  category as the projections: seed a synthetic evening of BLE beats, wifi
  leases and heartbeats, assert the roster. This is where the logic lives.
- One test must pin the **absent** case: a `presence.arrived` posted through
  `POST /api/events` by something that knows nothing about `via` is still valid,
  and readers must treat it as unknown rather than as any particular signal.
- **BLE itself is hardware-bound and is verified in the room**, with real phones
  belonging to real people, per this project's rule that a green suite cannot
  see an interaction with something outside its boundary. Arc 2 lost its time to
  exactly four such bugs.
- The real check is **an evening nobody is building during**: the roster at 11pm
  compared against the people actually in the room, counted by looking up from
  the couch. That comparison has never been made. Until it has, no threshold
  derived from presence — the veto denominator above all — should be trusted,
  and no design should be tuned against the numbers now in the log.

## Not in scope

- **Backfilling the 211.** Append-only. Absent `via` is unknown, which it is.
- **The Stage itself**, beyond naming it.
- **Attract Mode.** The roster is its honest first draft; the broadcast version
  still waits on the trophy case.
- **Whether BLE presence replaces the heartbeat.** It does not. It outranks it.

## Open questions for approval

**1. What do the tiers gate?** The recommendation is that they gate the **veto
denominator** and **admin powers** and nothing else — announcement volume comes
from the rarity law instead, and everyone lands in the trophy case, because
excluding people from the log fights the entire design. The alternative worth
weighing is that *party mode* replaces tiering for the denominator problem.

**2. How deep does profile customization go?** A preset grid of mascots and
colours ships inside the no-UI-framework rule as it stands. A Steam-depth editor
reopens that decision deliberately. The recommendation is to start with presets
and let the **earned** layer carry the depth, because unlocks are more
interesting than sliders.

**3. Which BLE shape does the house actually adopt?** Bonded phones are the
purest and cost every guest a pairing flow. Tags are trivially reliable and cost
a physical object the vision once cut. These are not exclusive, and the
resident case — keys already in a pocket — is the one where a tag clearly wins.
