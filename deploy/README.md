# Bringing SESH up on the Pi

Everything is built and installed **on the Pi itself**. Cross-compiling from
Windows means installing an aarch64 linker and fighting it; a Pi 5 compiles
this project in a few minutes and the iteration loop stays one command.

Work over **SSH**, not a remote desktop. Once SESH is running, the Pi boots
into a fullscreen labwc kiosk with no desktop session, so screen-sharing tools
have nothing to share — but `ssh` keeps working no matter what the TV shows.

## 0. Before you start

- Raspberry Pi OS (64-bit). **Lite** is the target. The Desktop image works,
  but `install.sh` takes over autologin on tty1, so the desktop will stop
  appearing there — read step 4 before running it on a Desktop install.
- SSH enabled, and the Pi's address. `ssh <user>@<pi>` should work.
- The Pi and your gaming PC on the same LAN.

## 1. Get the code onto the Pi

```bash
ssh <user>@<pi>
git clone <your-repo-url> ~/sesh
cd ~/sesh
git checkout arc1-log-and-room   # omit once the branch is merged
```

## 2. Install the toolchains

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# re-open the shell, or: . "$HOME/.cargo/env"

node --version   # must be 20 or newer
```

Raspberry Pi OS Bookworm's `apt` Node is 18, which is too old for the surface
build. If `node --version` is below 20 (or absent), install a current Node via
[nodesource](https://github.com/nodesource/distributions) or `nvm` first.

## 3. Build

```bash
cd ~/sesh
sh deploy/build.sh
```

Builds `target/release/seshd` and `surfaces/dist/`. The first Rust build takes
several minutes; later ones are incremental. Run this as your normal user —
the script refuses to run as root, because root-owned artifacts in `target/`
break the next user build.

## 4. Install

```bash
sudo sh deploy/install.sh
```

Installs the binary, the surface bundle, `apps.toml`, the systemd **user**
unit, and the labwc config; adds you to `video`/`input`/`render`/`audio`;
enables lingering; and sets tty1 to autologin and `exec labwc`.

It installs for whoever ran `sudo` (override with `SESH_USER=<name>`).

**On a Desktop image:** the Pi boots into a graphical session managed by a
display manager, which would fight labwc for the seat — a black screen or a
flickering handoff with no obvious cause. `install.sh` detects this and
switches the boot target to Console Autologin
(`raspi-config nonint do_boot_behaviour B2`), announcing it as it goes. Your
desktop is not removed, only skipped at boot.

To put the desktop back:

```bash
sudo raspi-config nonint do_boot_behaviour B4
```

and, if you also want the SESH kiosk gone, delete
`/etc/systemd/system/getty@tty1.service.d/autologin.conf` and the `exec labwc`
block from `~/.bash_profile`.

You can still reach the desktop temporarily without undoing anything —
switch to another VT with `Ctrl+Alt+F2` and log in there.

**Moonlight is not in the Pi OS repositories.** `install.sh` warns rather than
failing. Install `moonlight-qt` from
[the Moonlight project](https://github.com/moonlight-stream/moonlight-qt) and
confirm with `which moonlight-qt`, or the Moonlight tile will fail to launch.

## 5. Configure

```bash
sudo nano /etc/sesh/apps.toml
```

Replace `GAMING-PC` with the hostname or LAN IP of the machine running
Sunshine, and `Desktop` with the app name Sunshine exposes. Then confirm every
command resolves:

```bash
which kodi retroarch moonlight-qt
```

## 6. Reboot

```bash
sudo reboot
```

The Pi boots to tty1, autologs in, starts labwc, which starts `seshd` and then
Chromium in kiosk mode pointed at `http://127.0.0.1:7373`.

## 7. Verify

Work down this list; it is Arc 1's Definition of Done.

1. The TV shows the SESH home screen — no desktop, no cursor, no browser
   chrome.
2. `systemctl --user status seshd` reports `active (running)`.
3. Three tiles: Kodi, RetroArch, Moonlight.
4. A controller's d-pad moves the selection border, and it **stops** at the
   grid edges rather than wrapping.
5. For each of the three apps:
   - Selecting the tile and pressing A starts it fullscreen over SESH.
   - `curl -s http://127.0.0.1:7373/api/apps` reports it as `current`.
   - **Quitting from inside the app's own menu** returns to SESH within about
     a second, and `current` becomes `null`. *(This is the reaper. If `current`
     stays stuck, see Troubleshooting.)*
   - Relaunching and pressing B on the controller also returns and clears
     `current`.
6. The log recorded the session:
   ```bash
   curl -s http://127.0.0.1:7373/api/events | python3 -m json.tool
   ```
   Expect alternating `app.launched` / `app.exited`, in the order you
   performed them, each with the app id as `subject`.
7. It survives a reboot — `sudo reboot`, then re-run the command above and
   confirm every earlier event is still there.

## Troubleshooting

```bash
systemctl --user status seshd
journalctl --user -u seshd -f
curl -s http://<pi>:7373/api/apps      # reachable from anywhere on the LAN
```

**`current` stays stuck after you quit an app from its own menu.**
The most likely cause, and the most likely failure in this whole runbook:
`seshd` tracks the process it spawned, but Debian's `/usr/bin/kodi` is a shell
wrapper. If it forks instead of `exec`ing, the tracked pid exits immediately
while Kodi stays on screen — so SESH records a false `app.exited` and then
believes nothing is running while an unkillable Kodi covers the TV.

Diagnose by comparing what SESH tracks against what is actually running:

```bash
pgrep -a kodi
curl -s http://127.0.0.1:7373/api/apps
```

If the pids differ, the fix is to spawn via `setsid` and signal the process
group rather than the child. See `docs/arc1-followups.md`.

**Nothing on the TV after reboot.** Check the compositor started at all:
`ssh` in and run `pgrep -a labwc`. If it is not running, log in on tty1 and
look for errors from `exec labwc` in `~/.bash_profile`.

**The kiosk shows a connection error.** `seshd` did not come up before
Chromium did. `journalctl --user -u seshd -n 50` will say why; the autostart
waits up to 10 seconds.

## Updating

```bash
cd ~/sesh
git pull
sh deploy/build.sh
sudo sh deploy/install.sh
systemctl --user restart seshd
```

A `seshd` restart drops the kiosk's WebSocket; the surface reconnects on its
own and re-fetches state, so the TV recovers without a reboot.
