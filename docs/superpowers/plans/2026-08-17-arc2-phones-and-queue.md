# Arc 2 — Phones & the Shared Queue — Implementation Plan

**Goal:** Scan a QR on the TV and the room's music queue is in your hand. Anyone
on the couch can add a track, everyone can see who added what, and a majority
can veto. Every bit of it is in the log.

**Spec:** `docs/superpowers/specs/2026-08-17-arc2-phones-and-queue.md`
(merged as PR #8; the three open questions are answered below).

**Status:** Proposed. Needs approval before implementation.

---

## The three open questions, answered 2026-08-17

1. **Bluetooth speaker:** a **Victrola Brighton** — a record player that is also
   a Bluetooth speaker. Not yet paired: `bluetoothctl devices Paired` shows only
   the Basilisk X mouse, and the only PipeWire sink is HDMI.
2. **House Spotify account:** exists, and is **Premium**. Both librespot and
   Development Mode require it, so this unblocks the whole playback half.
3. **Now-playing card on the TV:** yes. Phase 5.

### What answer 1 changes

The spec deferred the vinyl handoff partly because there was no turntable to
test against. There now is, and it is *the same device as the speaker*. That is
a stronger position than the spec assumed, and it is worth writing down before
it gets rediscovered:

> On a Victrola that is playing a record, Bluetooth is not the active input.
> Switching the unit to phono drops the A2DP link. **The speaker disconnecting
> is the vinyl handoff signal** — the exact mechanism the vision describes,
> available for free as a side effect of Phase 6's connection monitoring.

This plan still does **not** implement the handoff. It implements the signal:
Phase 6 records `audio.sink_lost` / `audio.sink_found` and pauses the queue on
loss. Making the room *react* to it (dimming lights, showing "now spinning") is
a later arc reading events that will, by then, already be in the log.

## Shape of the work

Six phases, each landing as its own PR behind a green gate. Ordered so that
**every phase is independently useful and independently revertible**, and so the
two phases with real-world failure modes (Spotify auth, Bluetooth audio) come
after the parts that can be proven without them.

| # | Phase | Lands | Needs network/hardware? |
|---|---|---|---|
| 1 | Identity: people, tokens, QR join | A phone that knows who it is | No |
| 2 | The queue, as pure logic | Queue + veto, fully tested | No |
| 3 | The `Player` seam and Spotify | Search and control, no queue wiring | Spotify |
| 4 | The conductor | Queue actually plays | Spotify |
| 5 | Surfaces | Phone UI + TV now-playing card | No |
| 6 | Audio out | Victrola paired, music off HDMI | Bluetooth |

Phases 1, 2 and 5's logic are all testable on any machine. Phases 3, 4 and 6 are
the ones that can only be finished on the Pi, which is where this session runs.

## Decisions made while planning

These are the calls that would otherwise get made silently mid-implementation.
Several correct or sharpen the spec.

### D1 — A queue entry is an event id, not a track URI *(corrects the spec)*

The spec's event table keys `music.vetoed` and `music.skipped` on the track URI
in `subject`. **That breaks the moment two people queue the same song**, which
on a night with a shared queue is not an edge case — it is a running joke.

So: `subject` stays the track URI, because "what did we play the night Marcus
came over" should read naturally off the log. Identity moves to the payload:

```json
{ "entry": 42 }   // the id of the music.queued event this refers to
```

The log assigns ids, so the queue gets stable entry identity for free and
without a new column.

### D2 — Presence is the existing `Roster`, driven by phone heartbeats

Veto needs a denominator. Rather than invent a second notion of who is here,
phones heartbeat, and seshd records `presence.arrived` / `presence.left` on
**transitions only** — so the existing Arc 1 `Roster` projection *is* the
roster, exactly as the vision anticipated for the degraded no-BLE mode.

Window: a phone is present if seen within 10 minutes. Transitions only, so the
log gains roughly two rows per person per night rather than one per heartbeat.
When BLE lands in a later arc it becomes a second producer of the same two
event kinds, and nothing downstream changes.

### D3 — Tokens: `Authorization: Bearer`, never in a URL, never in the log

The QR encodes `http://<pi>:7373/join?c=<code>`. The page POSTs the code to
`/api/join`, gets a token, stores it in `localStorage`, and replaces the URL.
The token never appears in a URL, a cookie, or the event log.

Join codes live **in memory only**. They are secrets, and the log is
append-only and served unauthenticated — nothing that must ever stop being true
belongs in it.

**What the one-time code actually protects, stated precisely,** because the
spec's "a camera through a window" framing does not survive contact with the
threat model: anyone already on the LAN can simply fetch the QR endpoint, so
rotation buys nothing against them — and the LAN is the declared trust boundary
anyway. What rotation *does* defeat is a **photograph of the TV used later**:
the guest who left, the picture in someone's camera roll, the frame in a
streamed clip. That is a real and likely exposure, and burning the code on
exchange plus rotating every 60s bounds it to the minute it was displayed.

### D4 — Person ids are readable slugs

`people.id` is `TEXT PRIMARY KEY`. Ids are slugified display names (`sam`,
`marcus`), deduped with a numeric suffix (`sam-2`). The alternative — random
ids — makes `GET /api/events` unreadable by a human, and a log a person can read
is a stated value of this project.

### D5 — Tokens are stored unhashed, deliberately

Hashing would need a new dependency whose entire benefit, under this threat
model, is that someone who has already stolen the SQLite file cannot *queue
music*. That is theater. Stored raw, documented here, revisited if a token ever
authorises something that matters.

### D6 — The `Player` trait is async, via `async-trait`

`Platform` is synchronous because `std::process` is. An HTTP client is not.
`reqwest::blocking` inside `spawn_blocking` is a trap — the blocking client
panics when it finds a Tokio runtime handle, and `spawn_blocking` threads have
one. Stable Rust still has no object-safe `async fn` in traits, and `Arc<dyn
Player>` is required to swap in `MockPlayer`, so `async-trait` it is.

### D7 — The pre-push window, and what it costs

The conductor hands Spotify the next track a few seconds before the current one
ends, so there is no silence at the seam. Spotify's queue cannot be reordered or
emptied, so **for those few seconds that track is committed**.

Rather than pretend otherwise: the window is 5 seconds, and a veto that lands
inside it is honoured *late* — the track starts, and the conductor immediately
calls `next()` and records `music.skipped` with `why: vetoed`. Two seconds of a
song nobody wanted, instead of a veto that silently does nothing. The
alternative — no pre-push — trades that for a gap of dead air between every
single track, which is worse in a room.

### D8 — On restart, reconcile against Spotify, do not close the row

Arc 1's launch reconciliation closes dangling `app.launched` rows because
restarting seshd *kills* the app — the cgroup argument. **That argument does not
hold here.** librespot is its own service outside seshd's cgroup, so after a
seshd restart the music is very much still playing.

So the music path reconciles the other way: on startup the conductor polls
`GET /me/player` and believes *reality*, recording `music.started` or
`music.skipped` as needed to make the log agree with what the speaker is doing.

A pleasing consequence of the queue being a projection: **the pending queue
survives a seshd restart for free**, which Spotify's own queue does not.

### D9 — One bundle, one entry point, a switch at boot

`surfaces/` gains no router and no framework. `main.ts` branches on
`location.pathname`: `/` mounts the TV home screen, `/join` and `/phone` mount
the phone. seshd already serves `index.html` for unknown paths precisely so the
surface owns its routing. The phone downloads a couple of KB of TV code it never
runs; a second Vite entry point would cost a second HTML route and a build
change to save that, which is not a trade worth making.

### D10 — An invariant needs its wording widened, and that is a design change

`CLAUDE.md` says *"`crates/seshd/src/store.rs` is the only module that writes
SQL."* Phase 1 adds `people` reads and writes, and store.rs is already 215 lines
against a 300-line ceiling.

Raising it rather than working around it, as the invariant demands: the rule's
*purpose* is that every SQL statement in SESH is auditable in one place, so the
append-only guarantee can be checked by reading one thing. Splitting into a
`store/` module — `store/mod.rs`, `store/events.rs`, `store/people.rs` — keeps
that purpose exactly and satisfies the ceiling. **Task 1.1 updates the
invariant's wording to say "the `store` module" and nothing else about it
changes.** If that is not acceptable, the alternative is a 350-line store.rs,
and I would rather ask than pick.

## New dependencies, with justification

Four, all in `crates/seshd`. Per `CLAUDE.md` none goes in without a reason.

| Crate | Why | Why not something else |
|---|---|---|
| `reqwest` (`rustls-tls`, `json`) | Spotify Web API calls | Hand-rolling on the `hyper` we already have transitively costs ~200 lines of connection and TLS setup. `rustls` over `native-tls` avoids an OpenSSL build on the Pi. |
| `async-trait` | Object-safe `Arc<dyn Player>` | See D6. Proc macro only, no runtime weight. |
| `qrcode` | Render the join QR as SVG server-side | A JS QR library in `surfaces/` would be the first runtime dependency in a deliberately framework-free surface, and CSP/offline rules out a CDN. |
| `getrandom` | 16 random bytes for tokens and join codes | `rand` pulls a whole distributions/PRNG stack to do one thing; `getrandom` is already in the tree transitively. |

No new dependencies in `surfaces/`.

---

## Phase 1 — Identity: people, tokens, QR join

*No network, no hardware. Ends with: a phone that knows who it is.*

- [x] **1.1 Split the store, widen the invariant**

  `store.rs` → `store/mod.rs` (open, pragmas, schema), `store/events.rs`
  (`append`, `read_since`, `last_id`), `store/people.rs`. Pure move, no
  behaviour change — the existing store tests must pass untouched, which is what
  makes it a safe first task. Update the `CLAUDE.md` invariant wording per D10.

- [x] **1.2 `people` gains `token` and `joined_ms`**

  `ALTER TABLE people ADD COLUMN ...`, guarded so it is idempotent on an
  existing DB (Bookworm's SQLite predates `ADD COLUMN IF NOT EXISTS`; check
  `PRAGMA table_info`). Permitted by the invariant — `people` is an identity
  registry, not a projection.

  While here, `CLAUDE.md` and `docs/arc1-followups.md` still say the token model
  arrives in "Arc 3"; the reorder recorded in the vision doc made it Arc 2.
  Fix the references so the invariants file does not read as stale.

  `Store::upsert_person`, `Store::person_by_token`, `Store::people`.

  Tests: a fresh DB gets the columns; a DB created by the *old* schema gets them
  added without data loss (build one with the Arc 1 `CREATE TABLE` and migrate
  it — this is the case that runs on the live Pi); lookup by an unknown token is
  `None`.

- [x] **1.3 Join codes**

  `crates/seshd/src/join.rs`. In-memory, `Mutex<VecDeque<(code, issued_ms)>>`,
  holding the current code and the previous one so a scan that lands across a
  rotation still works. `issue()`, `redeem(code) -> bool` (burns it),
  `rotate_if_stale(now)`.

  Tests, all pure with an injected clock: redeem succeeds once and only once; a
  code older than the window is refused; the previous code stays valid until the
  next rotation; an unknown code is refused.

- [x] **1.4 The join endpoints**

  - `GET /api/join/qr.svg` — current code as a QR, `Cache-Control: no-store`
  - `POST /api/join {code, name}` → `201 {id, name, token}`; records
    `person.joined` with the new id as actor; burns the code
  - `GET /api/me` — resolve a bearer token to `{id, name}`; `401` if unknown

  A wrong or reused code is `403`, not `404` — it is a refusal, not a missing
  thing.

- [x] **1.5 The token extractor**

  An axum extractor resolving `Authorization: Bearer` → `Person`, rejecting with
  `401`. Applied to the music write endpoints in Phase 2 and **nowhere else**:
  `/api/events` stays open, per the invariant, and the TV surface has no token
  and needs none.

- [x] **1.6 Heartbeat and presence**

  `POST /api/heartbeat` (bearer). A `Presence` tracker holding last-seen per
  person; a tokio task sweeping every 60s. Records `presence.arrived` on a
  first-in-window beat and `presence.left` when a phone goes 10 minutes quiet —
  transitions only (D2).

  Tests are pure over an injected clock: no event on a repeat beat inside the
  window; exactly one `presence.left` after the window; re-arrival after a
  timeout emits `presence.arrived` again.

**Phase 1 done when:** scanning the QR on a phone yields a token, a name, a
`people` row, a `person.joined` event, and the phone appearing in
`GET /api/roster`. Verified on the Pi with a real phone camera.

*Amended on delivery (2026-08-17).* That last sentence quietly depended on
Phase 5, which is where the join screen and the on-TV QR live — there is no
page for a camera to land on yet. Phase 1 was instead verified on the Pi
against the real binary with the QR **decoded from the served SVG by
`zbarimg`**, which is the same bytes a camera would read, and the whole flow
driven over HTTP: join, refuse the reused code, `/api/me`, heartbeat, and a
restart. The camera itself moves to Phase 5's verification, where it is a test
of the screens rather than of this. Recorded rather than quietly reinterpreted.

Verification record: `docs/arc2-phase1-verification.md`.

## Phase 2 — The queue, as pure logic

*No network, no hardware, no Spotify. This is the phase that has to be right.*

- [x] **2.1 Event kinds**

  `MUSIC_QUEUED`, `MUSIC_VETOED`, `MUSIC_SKIPPED`, `MUSIC_STARTED` in
  `event::kind`.

- [x] **2.2 The `Queue` projection**

  `crates/seshd/src/projections/queue.rs`, implementing `Projection` like
  `Roster`. Holds pending entries in queue order and the now-playing entry, each
  entry carrying its id (D1), URI, title, artist, duration, and who added it,
  plus the set of actors who have vetoed it.

  Folding rules:
  - `music.queued` → append a pending entry keyed by the event id
  - `music.started` → move that entry from pending to now-playing
  - `music.skipped` → clear it from wherever it is (pending *or* now-playing;
    one rule covers both drop-before-play and skip-while-playing)
  - `music.vetoed` → record the actor's vote against `payload.entry`, deduped
    by actor

  Tests, pure: queue order is insertion order; the same URI queued twice yields
  two independently vetoable entries *(the D1 regression test)*; a veto from the
  same person twice counts once; skipping a pending entry removes it without
  touching now-playing; a fold matches a rebuild over the same events; unknown
  kinds are ignored; a veto naming an entry that no longer exists is dropped
  rather than panicking.

- [x] **2.3 The veto threshold**

  A pure function `should_skip(votes: &BTreeSet<String>, present: &[String])`.
  Strict majority of those present, minimum two — so one person in an empty room
  cannot "veto", which is just skipping.

  Tests: 1 of 1 does not skip; 2 of 2 does; 2 of 3 does; 1 of 3 does not; 3 of 5
  does; a vote from someone no longer present does not count toward the majority
  but is not an error.

- [x] **2.4 Hang it on `Room`**

  `queue: Mutex<Queue>` beside `roster`, rebuilt in `Room::new`, folded in
  `record`, exposed via `Room::queue()`. Same lock order as documented in
  `room.rs`; no guard held across an `.await`.

- [x] **2.5 The queue endpoints**

  - `GET /api/music` — now playing, pending, and each entry's veto tally. Open,
    like the rest of the read surface; the TV needs it.
  - `POST /api/music/queue` (bearer) — records `music.queued`
  - `POST /api/music/veto` (bearer) — records `music.vetoed`

  No player calls yet. This phase ends with a queue that is completely correct
  and completely silent.

Verification record: `docs/arc2-phase2-verification.md`.

**Phase 2 done when:** two bearer tokens can build a queue, vetoes tally, the
threshold fires, and the whole thing rebuilds identically from the log. Zero
network in the test suite.

## Phase 3 — The `Player` seam and Spotify

*First phase needing the outside world.*

- [x] **3.1 The trait**

  `crates/seshd/src/player/mod.rs`:

  ```rust
  #[async_trait]
  pub trait Player: Send + Sync + 'static {
      async fn playback(&self) -> Result<Option<Playback>>;
      async fn search(&self, query: &str) -> Result<Vec<Track>>;
      async fn enqueue(&self, uri: &str) -> Result<()>;
      async fn skip(&self) -> Result<()>;
      async fn transfer(&self) -> Result<()>;
  }
  ```

  Plus `MockPlayer` recording calls and returning scripted playback, which is
  what every later test uses.

- [x] **3.2 Credentials and OAuth**

  `/etc/sesh/spotify.toml` (`0640 root:tate`) holds `client_id`,
  `client_secret`, `device_name`. The refresh token goes to
  `~/.local/share/sesh/spotify-token.json` at `0600` — never `/etc`, never the
  log.

  A `seshd auth-spotify` subcommand runs the Authorization Code flow: print the
  URL, bind `127.0.0.1:7374` for the callback, exchange, write the token file.
  Loopback is what Spotify permits without HTTPS. Two ways to complete it, both
  documented: the Pi's own Chromium, or `ssh -L 7374:127.0.0.1:7374` from the
  laptop.

  Scopes: `user-read-playback-state`, `user-modify-playback-state`.

- [x] **3.3 `SpotifyPlayer`**

  `player/spotify.rs`. Access-token refresh on 401 with a single retry; respect
  `Retry-After` on 429; `search` caps at the API's current maximum of 10 (spec).
  Map the JSON into the local `Track`/`Playback` types at the boundary so no
  Spotify shape leaks past this module.

  Tests: the HTTP-free half — JSON→`Track` mapping including a null album image
  and a missing artist; `Playback` mapping for playing, paused, and nothing-
  active; the refresh-once-then-fail path against a stub. Live calls are checked
  by hand in 3.4 rather than mocked into a false sense of security.

- [x] **3.4 Prove it on hardware** *(read paths verified against real Spotify on
  2026-08-19; `enqueue`/`skip` still unrun — see below)*

  Authorize the house account, `transfer()` to the Pi's device, search a track,
  enqueue it, `skip()`. Record what the API actually returned, especially
  anything the February 2026 changes moved. This is the task most likely to
  surface a surprise, which is why it comes before the conductor depends on it.

  Kept as `crates/seshd/tests/spotify_live.rs`, `#[ignore]`d so the gate stays
  offline and deterministic. Re-run it after any change to `player/spotify.rs`
  and after Spotify announces an API change:

  ```text
  cargo test --test spotify_live -- --ignored --nocapture
  ```

  **What the real API returned.** No surprises, which is itself the finding —
  the February 2026 changes had already been absorbed correctly:

  - `search` returned **exactly 10** results for a broad query, confirming
    `SEARCH_LIMIT` against the live cap rather than against the changelog.
  - Every result mapped cleanly: `spotify:track:` URI, non-empty title and
    artist, positive duration. The multi-artist join and the `- Remastered
    2011` title suffixes came through as Spotify sends them, unmangled.
  - `playback()` with nothing playing returned **204**, mapping to `None`
    rather than erroring. This is the ordinary state of a quiet room and the
    conductor will see it constantly.
  - `transfer()` failed with the by-name error it was written to give: *no
    Spotify Connect device named "SESH"*. Correct until librespot arrives in
    Phase 6 — the point of the assertion is that it never degrades into a bare
    404 that reads like a bug.
  - Refresh worked from a cold start with no cached access token, so the stored
    refresh token and both scopes are good.

  **Still unrun: `enqueue` and `skip`.** They are behind `SESH_LIVE_MUTATE=1`
  because they change what the house account is playing, and a routine test run
  must not interrupt music in the room. Both need an **active Connect device**,
  which does not exist until Phase 6, so today they can only return Spotify's
  no-active-device error. Run them once librespot is up:

  ```text
  SESH_LIVE_MUTATE=1 cargo test --test spotify_live -- --ignored --nocapture
  ```

  Phase 4 must therefore treat "no active device" as an expected, recoverable
  state rather than a fault — the exact shape of that error is unconfirmed, so
  the conductor should degrade on any `enqueue`/`skip` failure rather than
  matching on its text.

## Phase 4 — The conductor

- [x] **4.1 The loop**

  `crates/seshd/src/conductor.rs`, a tokio task like `reap_loop`. Polls
  `Player::playback()` every 3s while playing, 15s while idle.

  - Track ends / changes → `music.started` for whatever is now playing
  - Within 5s of the end and a successor exists and has not been pushed →
    `enqueue()` it (D7)
  - Veto crosses the threshold → pending entry dropped, or `skip()` if playing
  - Track was pre-pushed and then vetoed → let it start, `skip()` immediately,
    record `why: vetoed` (D7)
  - Empty queue → nothing; do not fill with recommendations. A quiet room is a
    correct outcome.

- [x] **4.2 Startup reconciliation (D8)**

  On startup, poll actual playback and record what is needed for the log to
  agree with the speaker. Explicitly *not* Arc 1's close-the-dangling-row
  approach, and `docs/arc1-followups.md` gets a line saying why, so the next
  person does not "fix" this into the wrong shape.

- [x] **4.3 Degrade honestly**

  Spotify unreachable → queue keeps accepting adds, `GET /api/music` reports
  `player: "offline"`, the loop backs off to 30s and retries. Launching Kodi is
  unaffected, per the vision's rule that every subsystem degrades to *the room
  still plays media*.

  Tests: all of 4.1–4.3 against `MockPlayer` with a driven clock. No network in
  the suite.

  **Built as `crates/seshd/src/conductor.rs`, 22 tests in
  `crates/seshd/tests/conductor.rs`.** No clock at all in the end, driven or
  otherwise: `tick()` does one pass and *returns* the next interval, so the
  waiting lives in `run_loop` and nothing else. A test that advanced a fake
  clock would have been testing tokio's timer rather than the conductor's
  decisions.

  Two deviations from this plan, both discovered while building it:

  - **`Player` gained `play(uri)`.** `enqueue` appends to Spotify's queue but
    never *begins* — with nothing on the speaker, enqueueing is silent and
    stays silent. A room whose queue fills up before anyone has started
    anything is the ordinary way an evening begins, and the plan as written
    left it silent forever. `play` uses `uris:` rather than `context_uri:` so
    Spotify cannot wander into an album nobody chose once the track ends.
  - **`GET /api/music/search?q=` was missing.** Phase 5.3 needs it and no
    phase specified it. Added here, since it is backend work: proxied through
    `seshd` so the house account's access token never reaches a browser. It
    503s on a box with no credentials and 502s on a source that is down —
    neither is an empty list, because a phone showing "no results" for an
    outage sends someone to reboot the router.

  **Mutation tested: 40 mutants, 39 caught.** The first run caught 34 and the
  six survivors were all genuine gaps, not noise:

  - `Status::set`'s return value only drives a log line, so nothing pinned it.
    Inverted, the Pi logs "the music source is answering again" on every poll
    of a healthy source and never logs the transition that matters.
  - The `REWIND_MS` threshold, both the term and its boundary. Spotify's
    reported progress jitters between polls; without the threshold any
    backwards wobble during the pre-push window reads as "the same song started
    again" and writes a spurious `music.started` into an append-only log every
    few seconds for the length of the track.
  - Both identity checks in `claim`. One lets a committed track wrongly claim a
    *different* song that starts; the other never clears the committed marker,
    so the conductor believes something is pending forever and every later seek
    becomes a replay.

  The one survivor left is `replace run_loop with ()`. `run_loop` is the
  infinite sleep-and-tick wrapper — it never returns, so no test can observe
  the difference. Contorting the suite to kill it would test tokio's timer,
  which is precisely what the `tick()` design exists to avoid.

  Also worth recording, because the test for it is easy to write wrongly: a
  **pause is not an ending**. Spotify reports it as `Some` with
  `is_playing: false`, and treating that as the track finishing would clear
  the TV card the moment somebody answered the door.

## Phase 5 — Surfaces

- [x] **5.1 The boot switch** — `main.ts` branches on `location.pathname` (D9).

- [x] **5.2 The join screen** — `/join?c=`: name field, POST, store token, go to
  `/phone`. An already-joined phone skips straight through.

- [x] **5.3 The phone queue** — search box (debounced, 10 results), tap to
  queue, the pending list with who added each track, a veto button per entry
  showing the tally (`2/3`), and now-playing at the top. Thumb-sized targets;
  this is used standing up in a dark room. Polls `GET /api/music`, and
  heartbeats while the page is visible.

- [x] **5.4 The TV now-playing card** — a strip under the tile grid: track,
  artist, who queued it, and how many are waiting. Hidden when nothing is
  playing, so a room with no music looks exactly as it does today. Fed by the
  existing WS feed, adding `music.*` to the kinds that trigger `refresh()`.

- [x] **5.5 The QR** — on the TV home screen, small, in a corner, next to
  "scan to join". `<img src="/api/join/qr.svg">` refreshed on the rotation
  interval.

  Tests: `renderHome` with and without now-playing; the phone view's pure
  render given a queue; the veto button's disabled state once you have voted.

  **Built. 57 vitest tests, up from 26.** Files: `route.ts` (the switch),
  `tv.ts` (the TV controller, moved out of `main.ts` so `main.ts` is only the
  switch), `phone.ts` (both phone controllers), `views/join.ts`,
  `views/phone.ts`, and the strip plus QR added to `views/home.ts`.

  Notes worth keeping:

  - **`surfaceFor` lives in its own module.** `main.ts` starts a surface as a
    side effect of being imported, so testing the routing by importing
    `main.ts` would boot the TV inside the test runner.
  - **The phone polls; it does not use the WebSocket.** The feed exists and the
    TV uses it, but a phone in a pocket with a silently dropped socket showing
    a stale queue is worse than three seconds of lag. `document.hidden` gates
    both the poll and the heartbeat, so a pocketed phone stops counting toward
    the veto denominator — which is the correct answer to "who is in the room".
  - **`body { overflow: hidden }` had to become `body:has(.home)`.** It was a
    TV rule written when the TV was the only surface; left alone it made the
    phone queue unscrollable, which reads as a broken app rather than a long
    page.
  - **One existing test changed, deliberately.** `renderHome`'s XSS test
    asserted `querySelector("img")` was null, which was a valid proxy only
    while the home screen had no images of its own. The join QR is a
    legitimate `<img>`. The assertion now names the *injected* element and
    also checks the escaped name survives as text — stricter than what it
    replaced, not weaker.
  - **A real bug, found by running the binary rather than the suite.** D9 says
    "seshd already serves `index.html` for unknown paths precisely so the
    surface owns its routing." It did not. `ServeDir::not_found_service`
    serves the fallback body and then **forces the status to 404**, so `/join`
    and `/phone` returned a byte-perfect page under a `404 Not Found`. Arc 1's
    surface only ever lived at `/`, which `ServeDir` resolves directly, so the
    fallback had never once been exercised. A browser renders a 404 body
    without complaint — the phone would have worked in the room while `curl
    -f`, the boot verification harness, and anything caching by status all
    disagreed. `ServeDir::fallback` passes the status through;
    `crates/seshd/tests/surface_routes.rs` pins it, including the case that
    must still 404, since a missing bundle is a broken install rather than a
    page. Worth generalising: the Rust suite was green, clippy was clean, and
    every DOM test passed. Neither layer speaks HTTP, so neither could see it.

  - **Still unverified: how any of it looks.** Every test here is a DOM
    assertion. Nothing has confirmed that the strip fits under the grid on a
    real 16:9 TV, that the QR scans at 8vw from couch distance, or that the
    veto buttons are actually thumb-sized in a dark room. That is the Phase 1
    real-phone check deferred to here, and it needs the hardware.

## Phase 6 — Audio out

- [ ] **6.1 Pair the Victrola** — *needs the device in pairing mode; `sesh-pair-speaker` does the rest*

  Put it in Bluetooth pairing mode, `bluetoothctl` pair/trust/connect, confirm
  an A2DP sink appears in `pactl list short sinks`, confirm it survives a
  reconnect after being powered off and on. `Trusted: yes` is what makes it
  auto-reconnect. BlueZ 5.66 and PipeWire's full bluez5 codec set (SBC, AAC,
  aptX, LDAC) are already present on this Pi — checked.

- [x] **6.2 librespot as a user unit**

  Neither `librespot` nor `raspotify` is in the Pi OS archive — checked — so
  raspotify comes from its Cloudsmith repo, the same shape of problem as
  `moonlight-qt`.

  **The stock raspotify unit is wrong for us** and this is the trap in this
  phase: it runs as a system service under its own user, which cannot reach the
  user session's PipeWire and so cannot see the Bluetooth sink at all. Ship
  `deploy/systemd/sesh-librespot.service` as a **user** unit alongside `seshd`,
  and mask the packaged system unit.

- [x] **6.3 Route music to the speaker, game audio to the TV**

  The default sink stays HDMI, so Kodi, RetroArch and Moonlight are untouched.
  Only librespot is redirected, via `PULSE_SINK` in the user unit. The sink name
  contains the speaker's MAC, so it cannot be known before pairing:
  `deploy/pair-speaker.sh` pairs the device and writes the systemd drop-in.

  If the speaker is absent, `PULSE_SINK` falls through to HDMI — music plays on
  the TV instead of nowhere, which is the right degradation.

- [x] **6.4 Watch the connection**

  A monitor recording `audio.sink_lost` / `audio.sink_found`, pausing the queue
  on loss and resuming on return. This is the vinyl signal (see above); this
  arc only records it.

- [x] **6.5 `install.sh`**

  raspotify repo and package, the user unit, the packaged system unit masked,
  the `spotify.toml` template — and it must be **re-runnable**, honouring the
  same lesson as PR #6: never clobber a configured file. `deploy/README.md`
  gains the pairing and authorization runbook.

---

## Definition of Done

The spec's list, with the ones that were unverifiable now resolved:

- Scanning the QR puts a working queue in your hand — no app install, no typing
  beyond a display name.
- Two phones both add tracks; they play in the order added.
- A majority veto skips the current track and the log says who voted.
- **Music comes out of the Victrola while a game's audio still goes to the TV.**
- `GET /api/events` shows the whole night with actors attached, and survives a
  reboot.
- The five-command gate is green at every phase boundary, and the entire queue,
  veto, and conductor logic is tested with no network.

Plus two this plan adds:

- **The pending queue survives a `seshd` restart** — it is a projection, so this
  is free, and it is worth asserting on hardware because it is the clearest
  demonstration of why the log is the design.
- **Unplugging the speaker mid-song does not lose the queue**, and the log says
  when the sink went away.

Phase 5's surfaces were checked in the room on 2026-08-19; the record, including
one prediction the room falsified, is `docs/arc2-phase5-verification.md`.

## Risks, ranked

1. **Spotify Development Mode friction (Phase 3).** Five authorized users is
   plenty for one house account, but the dashboard, redirect-URI rules, and
   scope grants are where an afternoon goes. Mitigated by 3.4 landing before
   anything depends on it.
2. **Bluetooth audio on a Pi is historically the least reliable thing here
   (Phase 6).** Stutter under Wi-Fi load, A2DP dropouts, sinks not returning
   after power-cycling. Mitigated by the HDMI fallback and by keeping the
   default sink untouched, so a bad speaker night degrades to "music on the TV"
   rather than a broken room. If the Victrola's Bluetooth turns out to be
   output-only rather than a true A2DP sink, this phase becomes "buy a speaker"
   and everything else in the arc still ships — which is why it is last.
3. **The pre-push window (D7)** is a real, if small, correctness compromise. It
   is documented rather than hidden, and the mitigation (skip immediately after
   a late veto) is tested.
4. **Log growth from presence heartbeats.** Transitions-only keeps it to a
   couple of rows per person per night. Worth measuring after a real night
   rather than assuming.

## Not in this arc

Unchanged from the spec, and restated so they do not creep in: BLE presence;
the vinyl *handoff* (the signal only); voting on anything but music; any
Spotify feature beyond search, queue, and skip. Playlists, recommendations, and
"what should we play next" are a different product.

## Depends on

~~**Arc 1 is not closed until the Pi reboots.**~~ **Done 2026-08-17, before any
of this started.** The Pi was rebooted and came up into the SESH home screen
unattended in 22 seconds, on the boot chain that is actually in `master` — the
deployed artifacts were diffed against it first. All 8 events survived. Arc 1's
Definition of Done is met in full; see `docs/arc1-followups.md`.

Nothing else blocks Phase 1.
