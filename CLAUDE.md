# SESH

A living room that knows who's in it. A Raspberry Pi 5 wired to the TV runs
the room: it knows who's on the couch, turns phones into controllers, keeps a
permanent record of what the house has done, and reacts to it.

It is **not** a media box. Kodi, RetroArch, and Moonlight already do their jobs
and are launched as apps. SESH is the layer that ties the room, the people in
it, and its history together.

## Architectural invariants

These are load-bearing. Breaking one is a design change, not a bug fix — stop
and raise it rather than working around it.

- **The event log is append-only.** No `UPDATE` or `DELETE` may ever target the
  `events` table, in code or in a migration. `crates/seshd/src/store/` is the
  only module that writes SQL — the whole directory, so the guarantee is still
  auditable by reading one place.
- **`Room::record` is the only write path.** Nothing else may touch the store.
  The rule is "you cannot change state without leaving a record."
- **All derived state is rebuildable from the log alone.** Projections may
  cache; they are never the only copy of a fact. The `people` table is the one
  exception, and deliberately so — it is an identity registry, source data
  rather than a projection.
- **`POST /api/events` is an open ingest port.** How game results get captured
  is a deliberately deferred decision; that endpoint is where any strategy
  plugs in later. Do not narrow it to the kinds SESH currently emits.
- **Process control goes behind the `Platform` trait**, so the launcher stays
  testable off-Pi.

Architecture and rationale: `docs/superpowers/specs/2026-08-15-sesh-vision-design.md`
Arc 1 plan: `docs/superpowers/plans/2026-08-15-arc1-log-and-room.md`
Known follow-ups and Pi risks: `docs/arc1-followups.md`

## The gate

All five must be green before any commit:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cd surfaces && npm test
cd surfaces && npm run build
```

`#![warn(missing_docs)]` plus `-D warnings` means **every** `pub` item needs a
doc comment, including public struct fields and module `//!` headers.

**CI runs these same five on every pull request** — `.github/workflows/gate.yml`.
This list is the specification; that file follows it. If they disagree, the file
is the bug. CI is a floor and not a substitute: it runs on x86 and proves the
suite is green, never that the room works, so every arc still closes on the Pi.

## Conventions

- **TDD.** Write the failing test, run it and watch it fail, then implement.
  Never weaken or delete a test to make a suite green — if a test encodes the
  wrong expectation, say so and explain why before changing it.
- **A test double must be able to fail the way the real thing fails.** If the
  real dependency can return an error status, hand back malformed data, or fail
  the query outright, the double needs a mode for it and at least one test must
  use it. A double built from the documentation encodes what a service
  *promises*, not what it *does*, and this project's worst bugs have all lived
  in that gap — a stub answering `204` to everything hid the veto failure that
  sent a room to Spotify Jam. Evidence: `docs/test-double-audit.md`.
- **A test that passes against the broken code is not testing the fix.** Break
  the fix and watch the test fail, or test at a seam where the defect is
  actually reachable. Where a real failure was observed, reproduce its error
  verbatim.
- **Always open PRs against the default branch.** A stacked PR strands its upper
  half: it merges into its parent, displays **MERGED**, CI passes, and never
  reaches `master`. There is no GitHub signal for this — no notification, no
  failing check, no state change. It has happened four times across three repos,
  here as Arc 3 Phases 1–3, caught only by grepping `master` for a symbol they
  introduced. `.github/workflows/branch-hygiene.yml` now fails any PR that does
  not target `master`, and audits nightly for merges that never landed.

  **When a parent squash-merges, do not rebase the child.** The squash rewrote
  the parent's commits, so a child still carrying the originals collides
  `add/add` on every file the parent added. Re-create the branch from `master`
  and cherry-pick only the child's own commits.
- **Conventional Commits**: `type(scope): description`. A `feat`/`fix` whose
  motivation isn't obvious from the subject gets a 1–2 sentence body saying
  *why*. Agent commits carry the harness's `Co-Authored-By`/`Claude-Session`
  trailers.
- **Soft ceiling of 300 lines per file.** Split by responsibility when crossed.
- **No UI framework** in `surfaces/` — Arc 1's surface is a grid, and a Pi's
  browser is happier with less.
- **Dependencies are pinned.** Do not add, remove, or bump one without saying
  why it is necessary.
- Arc 1 is deliberately **unauthenticated and LAN-bound**. The per-person token
  model arrives in Arc 2. Absence of auth is a decided tradeoff, not an
  oversight — but nothing may make it worse than "unauthenticated on a home
  LAN" (no shell invocation, no escaping the app registry).

## Building and deploying

Everything is built **on the Pi**, not cross-compiled:

```bash
sh deploy/build.sh        # as your normal user, never root
sudo sh deploy/install.sh
```

Full runbook, verification checklist, and troubleshooting: `deploy/README.md`.
