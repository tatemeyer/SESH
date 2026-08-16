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

2. **Cap and jitter the WebSocket reconnect.**
   `surfaces/src/api.ts` retries on a flat 1000ms with no backoff. Against a
   `seshd` that stays down — a failed upgrade, say — the kiosk Chromium opens
   one socket and logs one `console.error` per second indefinitely. It is one
   client, so not a server-side storm, but console growth is unbounded in a
   browser that never restarts.

3. **Record that the SIGTERM test has never executed.**
   `kill_lets_a_unix_child_run_its_shutdown_path` in
   `crates/seshd/src/launcher/platform.rs` is `#[cfg(unix)]` and was written
   on a Windows dev machine with no CI in this repo. It is syntax-checked
   only; its first real run is on the Pi. The neighbouring comment claims the
   graceful path "is covered by the test below," which is true on Linux and
   misleading on the dev loop.

4. **Redraw after clearing a launch error.**
   `surfaces/src/main.ts`'s success path clears `state.notice` without calling
   `draw()`, so a stale error line survives until the next render. Masked in
   practice: a successful launch stacks the app window over SESH, and the
   following `app.launched` event triggers `refresh()` → `draw()`.

## Risks that can only be settled on the Pi

These are ordered by how likely they are to bite during Task 13.

1. **The reaper may false-positive on Kodi.**
   `ProcessPlatform` tracks only the direct child. Debian's `/usr/bin/kodi` is
   a shell wrapper; if it forks rather than `exec`s — or if `--standalone`'s
   restart loop respawns the real binary — the tracked pid exits immediately
   while Kodi stays on screen. `is_running` then returns false, `reap` records
   a false `app.exited`, `current` clears, and SESH believes nothing is
   running while an unkillable Kodi covers the TV.
   *Diagnose:* compare `pgrep -a kodi` against the pid SESH is tracking on
   first launch. *If it forks:* spawn via `setsid` and signal the process
   group rather than the child.

2. **Moonlight's package is not in the Raspberry Pi OS repositories.**
   `deploy/apps.toml` now specifies `moonlight-qt`, which is the binary name
   the Debian/Flatpak package installs. If `apt` cannot find it, install it
   from the Moonlight project's own repository and verify with
   `which moonlight-qt` before rebooting. Arc 1's Definition of Done requires
   all three apps to launch and return.

3. **`kill -TERM` signals the direct child, not its process group.**
   An app launched through a wrapper script would swallow the signal and leave
   a grandchild behind after the SIGKILL fallback. This is not a regression —
   `child.kill()` had the identical limitation — but it compounds risk 1.

## Deliberate Arc 1 scope decisions, for the record

- **No authentication.** `seshd` binds `0.0.0.0:7373` unauthenticated. The
  per-person token model arrives with phones in Arc 3. There is no injection
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
  "rebuildable from the log" invariant. Arc 3 will need an `ALTER TABLE` to
  add BLE identifiers and a phone token; the append-only, no-migrations
  discipline covers the `events` table only.
- **Phones will load `http://<pi>:7373`, which is not a secure context.**
  The TV kiosk uses `127.0.0.1`, which is, so the Gamepad API works there. No
  service workers or `getUserMedia` on phones without TLS — worth folding into
  Arc 3's spec.
