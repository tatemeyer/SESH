# Arc 1 — Known follow-ups and Pi risks

Recorded at the close of Arc 1 ("The Log & The Room"). None of these blocked
the merge gate; all came out of the whole-branch review and its fix wave.

## Follow-ups, in descending order of value

1. **Move `launch_app`/`quit_app` off the runtime workers.**
   `crates/seshd/src/api/apps.rs` calls the synchronous `Launcher::launch`/
   `quit` directly from `async` axum handlers, and `reap_loop` blocks on a
   `std::sync::Mutex` inside a spawned task. The SIGTERM grace period widened
   that block from roughly zero to ~2s. With four workers on a Pi 5 and one
   viewer this is invisible, but `spawn_blocking` (or a dedicated thread) is
   the correct shape.

2. ~~**Cap and jitter the WebSocket reconnect.**~~ **Resolved 2026-08-20.**
   `surfaces/src/api.ts` retried on a flat 1000ms with no backoff. Against a
   `seshd` that stays down — a failed upgrade, say — the kiosk Chromium opened
   one socket and logged one `console.error` per second indefinitely. It is one
   client, so not a server-side storm, but console growth was unbounded in a
   browser that never restarts.

   The delay now doubles per consecutive failure, is jittered to 50–100% of
   that, and caps at 30s, so the TV still recovers within half a minute of
   `seshd` returning. A successful open resets the backoff. The
   `console.error` is now emitted once per *outage* rather than once per
   attempt, which is what actually bounds the console — the backoff alone
   would only have slowed the growth.

3. ~~**Record that the SIGTERM test has never executed.**~~ **Resolved
   2026-08-15 on the Pi.** `kill_lets_a_unix_child_run_its_shutdown_path` in
   `crates/seshd/src/launcher/platform.rs` is `#[cfg(unix)]` and had only ever
   been syntax-checked on a Windows dev machine. It has now run on the target
   hardware and passed, as part of a full green suite (62 Rust tests, 26
   vitest).

4. **Redraw after clearing a launch error.**
   `surfaces/src/main.ts`'s success path clears `state.notice` without calling
   `draw()`, so a stale error line survives until the next render. Masked in
   practice: a successful launch stacks the app window over SESH, and the
   following `app.launched` event triggers `refresh()` → `draw()`.

## Risks that were settled on the Pi

Bring-up ran on the target hardware on 2026-08-15/16 (Pi 5, 8GB, Bookworm,
labwc). The outcomes below are measured, not predicted — do not re-litigate
them from the original risk framing, which was written off-hardware.

1. ~~**The reaper may false-positive on Kodi.**~~ **Did not materialise.**
   This was ranked the likeliest failure and it is simply not one. Debian's
   `/usr/bin/kodi` wrapper does not fork in a way that fools the reaper: the
   tracked pid is the real one, the exit was detected cleanly, `current`
   returned to `null`, and no orphan process survived. Verified across all
   three apps — Kodi (38s), RetroArch (21s), Moonlight (0.6s).
   **The `setsid`/process-group change this entry prescribes is therefore not
   required.** It remains defensible as hardening (see risk 3), but anyone
   reading this doc should know the motivating failure was disproved.

2. ~~**Moonlight's package is not in the Raspberry Pi OS repositories.**~~
   **Confirmed and resolved.** `moonlight-qt` 6.1.0-4 installs from the
   Cloudsmith repository (`distro=debian codename=bookworm`), not from Pi OS.
   A second, harder problem hid behind it: `moonlight-qt` is Qt6 and Pi OS
   ships only the Qt5 Wayland plugin, so under labwc it aborted ~600ms after
   launch with *"no Qt platform plugin could be initialized"* — which presents
   exactly like a SESH launcher bug. `qt6-wayland` is now an `install.sh`
   dependency. Moonlight reaches `HostNotFoundError` against the placeholder
   host, which is the correct failure for an unconfigured Sunshine target.

3. **`kill -TERM` signals the direct child, not its process group.** *(Open.)*
   An app launched through a wrapper script would swallow the signal and leave
   a grandchild behind after the SIGKILL fallback. With risk 1 disproved this
   is no longer urgent, but it is still the correct shape. An unmerged branch,
   `origin/claude/sesh-pi-bringup-t213m7` (`1ba1662`), already implements it;
   it needs review and a decision rather than a rewrite.

## Found during the on-hardware install

Recorded 2026-08-16, from the first real `sudo sh deploy/install.sh` run.

1. ~~**`install.sh` broke non-kiosk login shells.**~~ **Fixed in this change.**
   The script appends the `exec labwc` hook to `~/.bash_profile`. Bash reads
   that file *instead of* `~/.profile` for login shells, so creating it where
   none existed silently dropped the stock Pi `~/.profile` line that sources
   `~/.bashrc` — costing an SSH login nvm, `~/.local/bin`, and completions,
   with nothing pointing at the installer. The tty1 kiosk never noticed
   because it `exec`s labwc before any of that matters. `install.sh` now seeds
   a `. "$HOME/.profile"` fallback when it creates the file, and leaves a
   pre-existing `~/.bash_profile` alone.

2. ~~**The kiosk shared the user's default Chromium profile.**~~ **Fixed.**
   The first real boot came up black with a live mouse cursor: labwc and
   `seshd` were both healthy, but Chromium exited 21 before painting. A
   `SingletonLock` left in `~/.config/chromium` back in January pointed at
   `raspberrypi-2225` — this Pi's hostname *before* it was renamed `TatePi`.
   Chromium refuses to open a profile locked by "another computer", and since
   the lock records a hostname that no longer exists, it can never self-heal.
   It then tried to report this through a modal dialog, which had no display
   to appear on. No log anywhere mentioned Chromium; the only symptom was a
   black screen.
   **Any Pi renamed after its first desktop Chromium run hits this**, which is
   the common case — the stock image ships as `raspberrypi`. The kiosk now
   uses a dedicated profile under `~/.local/share/sesh/chromium` and clears a
   stale lock when no kiosk is running. Verified by planting the exact
   `raspberrypi-2225` lock and confirming the kiosk still starts.
   *Bearing on the entry below:* a restart supervisor would **not** have saved
   this. Chromium failed instantly and deterministically, so a respawn loop
   would have spun forever against the same black screen.

3. **Nothing supervises the kiosk browser.** *(Open.)*
   `deploy/labwc/autostart` launches Chromium once with `&`. `seshd` has
   `Restart=always`, but the browser has no equivalent: if it OOMs or crashes,
   the TV shows an empty labwc desktop indefinitely and the only recovery is a
   power cycle. For an always-on room device that is the wrong failure mode.
   The fix is a supervised restart — a user unit with `Restart=always`, or a
   respawn loop in `autostart`. Note the limit established above: supervision
   only helps a browser that dies *intermittently*. It is no defence against a
   deterministic startup failure, which is what actually took the room down.

4. ~~**A `seshd` restart orphans the running app's exit event.**~~ **Fixed
   2026-08-17.** `Launcher` held the launch/exit pairing only in
   memory, so restarting the daemon while an app runs leaves an
   `app.launched` that no `app.exited` will ever close. The log is
   append-only, so the gap is permanent. Observed on the Pi: RetroArch
   launched 23:24:22, `seshd` restarted 23:51:44 to reload `apps.toml`, and
   the log still claims RetroArch is running.
   This breaks the architecture's load-bearing claim that derived state is
   rebuildable from the log — rebuild "what is running" from that log and you
   get RetroArch, forever. It is also routine rather than exceptional: every
   config reload, every crash under `Restart=always`, and every power cut on
   an appliance that lives in a living room produces one.
   Established while diagnosing it, and worth not re-deriving: `seshd.service`
   uses the default `KillMode=control-group` and launched apps are children
   inside the unit's cgroup, so **restarting `seshd` kills every app it
   launched.** A dangling launch therefore always means the app is dead; only
   the exit *time* is unknown.
   `seshd` now closes such launches at startup, before it binds, appending an
   `app.exited` marked `exit_observed: false` and carrying both provable time
   bounds rather than inventing an observation. Design in
   `docs/superpowers/specs/2026-08-16-launch-reconciliation.md`, implementation
   in `docs/superpowers/plans/2026-08-17-launch-reconciliation.md` and
   `crates/seshd/src/reconcile.rs`. The RetroArch row above is closed by
   event 6 on the live Pi; the log has no unclosed launches.

   **Do not generalise this to music.** Arc 2's conductor
   (`crates/seshd/src/conductor.rs`) reconciles the *opposite* way, and the
   difference is the cgroup argument above rather than a matter of taste.
   librespot is its own service outside `seshd`'s cgroup, so restarting
   `seshd` does not stop the music — after a restart a track really is still
   playing, and closing the row would write a lie into an append-only log.
   The conductor therefore polls the source on startup and corrects the log to
   match reality, recording `music.started` or `music.skipped` as needed.
   Same word, opposite direction, because the premise differs: here the log's
   claim is provably stale, there the log's claim is probably still true.

   **The `ts_ms` bound is conditional, and was not when this was written.**
   `reconcile.rs` says its timestamp is "the upper bound on when the app died".
   On a Pi with no RTC that is only true once the clock has been set: at a cold
   boot `seshd` starts before NTP answers — measured at 9.2s, wall clock 13m28s
   slow — and reconciliation runs inside that window, producing a `ts_ms` below
   the `last_alive_ms` in its own payload. Startup now waits a bounded ten
   seconds for the clock, and a row written anyway carries
   `clock_synced: false`. Check that key before relying on the bound. See
   `docs/superpowers/specs/2026-08-19-clock-trust.md`.

5. ~~**`install.sh` clobbers a configured `apps.toml`.**~~ **Fixed.**
   The installer used to run `install -Dm644 deploy/apps.toml
   /etc/sesh/apps.toml` unconditionally, so re-running it silently reverted
   the Sunshine host to the `GAMING-PC` placeholder — having just told the
   user to edit that very file. It presented as Moonlight breaking for no
   reason. `apps.toml` is configuration rather than a build artifact, so the
   installer now keeps an existing copy and writes the shipped template to
   `/etc/sesh/apps.toml.dist` beside it, which also gives an upgrade something
   to diff against. The closing message adapts to say which happened.

6. ~~**`/etc/sesh/apps.toml` still ships the placeholder Sunshine host.**~~
   **Configured on TatePi 2026-08-16.** The repo template still ships
   `GAMING-PC`/`Desktop`, which is correct for a template, so the Moonlight
   tile is still installed non-working by default on a fresh Pi and
   `install.sh` only prints a reminder. Follow-up 5 above is the sharper edge
   of the same problem.

## Verified on hardware, so treat as settled

- `seshd` runs correctly from its **installed** location as a systemd user
  unit: `/usr/local/bin/seshd` against `/etc/sesh/apps.toml`,
  `/usr/local/share/sesh/web`, and `~/.local/share/sesh/sesh.db`. `/api/apps`
  lists all three apps, the surface bundle serves 200.
- **The event log survives an uncleanly killed daemon.** A `SIGKILL` with an
  uncheckpointed 57KB WAL lost nothing — the event read back identically
  after restart. That was the durability half of the Definition of Done's
  "survives a reboot"; the reboot itself is now done too, below.
- **The Pi boots unattended into the SESH home screen, on the chain that is
  actually in `master`.** *(Closed 2026-08-17.)* This needs stating carefully,
  because an earlier version of this bullet claimed the same thing and was
  measuring a different system. That bring-up boot predated PR #4, so the
  kiosk-profile fix — the one that stops the TV coming up black — had never
  been exercised from a cold start. Before rebooting, all three deployed
  artifacts were diffed against `master` (`autostart` identical, the installed
  binary carrying PR #7's reconciliation marker, the web bundle byte-identical
  to a fresh build), so the boot tested the repo rather than a stale install.

  Timeline from a cold boot, captured unattended:

  | t+ | link |
  |---|---|
  | 17s | tty1 autologin → labwc, wayland socket up |
  | 18s | `autostart` → `seshd` answering `/api/apps` |
  | 18s | kiosk Chromium process alive |
  | 22s | surface fetched, parsed and **executed** — its WebSocket established |

  Confirmed three independent ways: the owner watched the TV come up into SESH;
  a `grim` capture at 3840x2160 shows the wordmark, all three tiles with real
  emoji icons, and the focus ring on Kodi; and all 8 pre-reboot events read
  back byte-identical afterwards, with reconciliation correctly finding nothing
  to close.

  **Arc 1's Definition of Done is therefore met in full.**
- **Moonlight streams from the gaming PC and returns.** The Pi is paired with
  Sunshine; `moonlight-qt list` reports `Desktop` and `Steam Big Picture`.
  Launching the tile put the PC's desktop on the TV at 1080p60 and quitting
  returned to the SESH home screen with `current` back to `null` and both
  events recorded. Two things learned: `moonlight-qt` needs a Qt platform even
  for CLI work, so pairing over SSH requires `QT_QPA_PLATFORM=offscreen` or it
  fails with a misleading *"no Qt platform plugin"*; and `stream` accepts
  `--1080`/`--4K`/`--fps`/`--bitrate`/`--quit-after` directly, so the kiosk
  does not depend on saved GUI settings. `--quit-after` matters for a room
  device — without it the host keeps the session open after everyone leaves.
- **Controller navigation works** once BlueZ's `ClassicBondedOnly` is relaxed
  (a DualShock 4 pairs without bonding, so the HID profile is refused and no
  `/dev/input` node appears while every UI still reports "Connected").
  Selection moves between tiles and clamps at the grid edge.

## Operating this box: two traps worth not rediscovering

Neither is a SESH bug. Both cost real time during Arc 1, and the second cost it
twice.

1. **`grim` blocks forever on a long-idle session.** After ~17 hours sitting on
   a static home screen, `wlr-screencopy` stopped returning frames entirely:
   `grim` hung past 300s and through three 45s retries, while labwc still
   answered new clients, `wlr-randr` returned the full mode list, Chromium held
   its socket open, and the TV reported `pwr-state: on` over HDMI-CEC. Forcing
   an output commit did not help; nor did CEC `image-view-on` plus an
   active-source claim. Immediately after a reboot the same command succeeded
   on the first attempt, so it is a property of the idle session, not of the
   hardware or the compositor build.

   Consequences: **always bound `grim` with `timeout`** in anything unattended,
   and do not treat a missing screenshot as evidence of a black screen. The
   reliable, display-independent proof that the kiosk really loaded is an
   ESTABLISHED connection to `:7373` from a `chromium` process —
   `ss -tnp | grep '127.0.0.1:7373.*chromium'`. The surface opens that socket as
   the last thing `main.ts` does, so it proves the page was fetched, parsed and
   executed. Chromium merely being alive proves nothing: during the black-screen
   failure it was alive too, for the second before it exited 21.

2. **`pgrep -f` / `pkill -f` match your own command line.** Recorded once
   already after `pgrep -f kodi` produced a false "still running" that appeared
   to falsify a spec's premise. It then happened again in the `pkill` form,
   where the pattern matched the invoking shell and killed it mid-task. The
   pattern is in your own `argv`, so the tool sees it.

   Use an exact-name match (`pgrep -x kodi.bin`), `ps -eo pid,comm`, or resolve
   pids first and `kill` those. Reserve `-f` for patterns that genuinely cannot
   appear in the command you are typing — `autostart`'s
   `pgrep -f "user-data-dir=$KIOSK_PROFILE"` is legitimate, and was checked by
   confirming every returned pid was really `chromium`.

   The verification harness in `~/sesh-boot-verify/` applies both lessons; its
   `README.md` explains how to re-arm it for a future reboot.

## Deliberate Arc 1 scope decisions, for the record

- **No authentication.** `seshd` binds `0.0.0.0:7373` unauthenticated. The
  per-person token model arrives with phones in Arc 2. There is no injection
  path and no way to escape the app registry — commands and arguments come
  only from a root-installed `apps.toml`, and `ProcessPlatform::spawn` never
  invokes a shell. The residual is that any LAN host can append unbounded
  events through the ingest port and eventually fill the SD card.
- **`POST /api/events` is deliberately open.** It is the documented ingest
  port for the deferred game-capture decision (manual phone reporting, screen
  watching, or emulator RAM reading). Narrowing it to the kinds SESH currently
  emits would defeat its purpose.
- **The `people` table is created but unused.** It is source data — an
  identity registry — not a projection, so it does not fall under the
  "rebuildable from the log" invariant. Arc 2 will need an `ALTER TABLE` to
  add BLE identifiers and a phone token; the append-only, no-migrations
  discipline covers the `events` table only.
- **Phones will load `http://<pi>:7373`, which is not a secure context.**
  The TV kiosk uses `127.0.0.1`, which is, so the Gamepad API works there. No
  service workers or `getUserMedia` on phones without TLS — worth folding into
  Arc 2's spec.
