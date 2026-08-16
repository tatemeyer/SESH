# Handoff: Arc 1 bring-up on the Pi

You are picking up SESH at the exact point where it first meets real hardware.
Everything before this was written and reviewed on a Windows dev machine. None
of it has ever run on Linux, on ARM, or against a real Kodi, RetroArch, or
Moonlight. Expect to find things.

Read `CLAUDE.md` first if you haven't — it carries the architectural
invariants and the commit/test conventions, and they still apply here.

## Your environment

- A **Raspberry Pi 5** running **Raspberry Pi OS Desktop (64-bit)**, wired to
  the living room TV.
- This repository, cloned locally. Remote is the private
  `github.com/tatemeyer/SESH`. Branch `master`.
- **The human is in the room with you.** They can look at the TV, press
  buttons on a game controller, and tell you what they see. Use that — several
  verification steps are impossible for you alone and trivial for them.
- A gaming PC on the same LAN running Sunshine (for Moonlight).

## What is already true

Arc 1 — "The Log & The Room" — is code-complete and has been through a
per-task review plus a whole-branch review. On the dev machine: 61 Rust tests
and 26 surface tests pass, clippy `-D warnings` and `cargo fmt --check` are
clean, and `npm run build` succeeds.

What that does **not** mean: the `deploy/` scripts have never been executed,
`seshd` has never run on Linux, and no app has ever actually been launched on
a TV. `deploy/build.sh` and `deploy/install.sh` are careful but unproven.

## Your job

Execute Task 13 of `docs/superpowers/plans/2026-08-15-arc1-log-and-room.md`:
build, install, and verify on the Pi. `deploy/README.md` is the runbook — it
has prerequisites, the install sequence, the seven-step verification
checklist, and troubleshooting. Follow it; don't reinvent it.

**Arc 1 is done when** the Pi boots straight to the SESH home screen, and Kodi,
RetroArch, and Moonlight each launch and return — whether quit from the
controller *or* from inside the app's own menu — with every launch and exit
recorded in the log, surviving a reboot. That is section 7 of
`deploy/README.md`, and it is the real bar.

## Known risks, in the order I expect them to bite

Read `docs/arc1-followups.md` for the full list. The ones that matter today:

**1. The reaper may false-positive on Kodi.** This is the likeliest failure in
the whole runbook. `seshd` tracks the process it spawned, but Debian's
`/usr/bin/kodi` is a shell wrapper. If it forks instead of `exec`ing, the
tracked pid exits immediately while Kodi stays on screen — SESH records a false
`app.exited`, clears `current`, and then believes nothing is running while an
unkillable Kodi covers the TV. Diagnose:

```bash
pgrep -a kodi
curl -s http://127.0.0.1:7373/api/apps
```

If the pids disagree, the fix shape is spawning via `setsid` and signalling the
process group instead of the child. That touches
`crates/seshd/src/launcher/platform.rs` and needs a test.

**2. Moonlight is not in the Raspberry Pi OS repositories.** `apps.toml`
specifies `moonlight-qt`, which is the binary the Debian/Flatpak package
installs. `install.sh` warns rather than failing. Install it from the Moonlight
project and confirm with `which moonlight-qt` — the DoD needs all three apps.

**3. Node may be too old.** The surface build needs Node 20+. Bookworm's apt
Node is 18. `deploy/build.sh` checks and fails with instructions.

**4. This is a Desktop image, so the boot gets taken over.** `install.sh`
detects the graphical target and switches to console autologin, because a
display manager fights labwc for the seat — which presents as a black screen
with no obvious cause. It announces this and prints the revert command
(`sudo raspi-config nonint do_boot_behaviour B4`). `Ctrl+Alt+F2` still reaches
a normal session.

## Guardrails

- **The architecture is settled.** The spec and plan in `docs/` were designed
  and approved deliberately. Fix bring-up bugs; do not redesign, refactor
  adjacent code, or "improve" a module you happen to be in. If bring-up reveals
  a genuine architectural problem, say so and stop rather than unilaterally
  reworking it.
- **Never weaken, skip, or delete a test to get a green run.** If a test
  encodes a wrong expectation, explain why before touching it. This branch
  already shipped one tautological test that review had to catch — don't add
  another.
- **Report honestly what you verified and what you didn't.** Anything requiring
  eyes on the TV or hands on a controller, you cannot do — **ask the human and
  wait for their answer.** Never write down a visual confirmation you did not
  receive. An honest gap is worth far more than a confident guess.
- Keep the five-command gate green before every commit.
- Don't commit a real Sunshine hostname into `deploy/apps.toml` — the repo copy
  keeps the `GAMING-PC` placeholder. The live config is `/etc/sesh/apps.toml`,
  which is outside the repo.
- Work on `master` and push, so the Windows side stays in sync. One commit per
  logical fix, Conventional Commits, with a body saying *why*.

## Useful commands

```bash
systemctl --user status seshd
journalctl --user -u seshd -f
curl -s http://127.0.0.1:7373/api/apps
curl -s http://127.0.0.1:7373/api/events | python3 -m json.tool
pgrep -a labwc; pgrep -a chromium
```

## When you're done

Report: which of the seven DoD steps passed, which failed and why, what you
fixed, and what still needs a human. If Arc 1's DoD is fully met, say so
plainly — it means the room works, and Arc 2 (attract mode) is next.
