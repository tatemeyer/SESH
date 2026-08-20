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

**Four non-obvious dependencies.** All four were found on real hardware; each
fails in a way that does not point at its own cause. `install.sh` and
`deploy/labwc/autostart` now handle all four, so this is background — read it
if you are installing by hand or debugging a partial install.

| Missing | Symptom | Handled by |
|---|---|---|
| `fonts-noto-color-emoji` | Every tile shows an empty tofu box instead of an icon | `install.sh` apt line |
| `qt6-wayland` | Moonlight dies with SIGABRT ~600ms after launch; looks like a SESH launcher bug | `install.sh` apt line |
| `--password-store=basic` | A modal *"Unlock Keyring — Authentication required"* dialog covers the kiosk, undismissable from a TV | `labwc/autostart` |
| `ClassicBondedOnly=false` | The controller pairs and reports **Connected**, but never becomes an input device and the browser never sees it | `install.sh` BlueZ step |

The tile icons are emoji codepoints in `surfaces/src/styles.css`; the font is
what makes them render. Moonlight is Qt6 while Pi OS ships only the Qt5 Wayland
plugin, so without `qt6-wayland` it cannot initialize a platform plugin at all.
The keyring dialog was confirmed on the Desktop image — it may not occur on
Lite, where gnome-keyring is usually absent, but the flag is harmless there.

The controller one is the nastiest to diagnose, because every user-facing
indicator says success. BlueZ's input profile defaults to
`ClassicBondedOnly=true` and refuses HID to any device that is not bonded; a
DualShock 4 pairs *without* bonding, so it connects, shows as **Connected** in
`bluetoothctl` and blueman alike, and is then silently rejected at the input
layer. The only place the truth appears is the daemon log:

```bash
journalctl -u bluetooth | grep -i "not bonded\|!bonded"
# profiles/input/device.c:hidp_add_connection()
#     Rejected connection from !bonded device AA:BB:CC:DD:EE:FF
```

Confirm the fix worked by checking that the pad became a real input device —
`bluetoothctl info` still reports `Bonded: no`, and that is expected:

```bash
grep -A5 'Name="Wireless Controller"' /proc/bus/input/devices   # want a Handlers= line
```

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

## Audio out: the room speaker

The default sink stays HDMI, so Kodi, RetroArch and Moonlight are untouched and
game audio always goes to the TV. **Only librespot is redirected**, through
`PULSE_SINK` on its user unit.

### Pairing

```sh
sesh-pair-speaker            # pair, trust, connect, and write the drop-in
sesh-pair-speaker --show     # what is paired and where music goes
```

On a Victrola Brighton, hold the Bluetooth button until the indicator flashes,
then run the script. It trusts the device before connecting, which is what makes
it come back by itself after the speaker is powered off and on — the normal way
a room gets used.

Pass the MAC directly when there is no terminal to prompt on (over ssh from a
script, from an agent, from these notes):

```sh
sesh-pair-speaker --scan               # find it
sesh-pair-speaker 2D:D4:65:45:03:4D    # pair it
```

**Two things the script handles that are easy to get wrong by hand:**

*BlueZ only knows a device while discovery is running.* Outside a scan, `pair`
answers `Device not available` for a speaker sitting in pairing mode two feet
away. The script holds a scan open across the pairing.

*Connecting a Bluetooth sink makes WirePlumber adopt it as the **default**.*
Left alone that sends Kodi, RetroArch and Moonlight to the speaker too — the
exact opposite of what this phase promises. The script pins the default back to
HDMI afterwards, and an explicitly configured default survives the speaker being
power-cycled. If game audio ever starts coming out of the speaker, this is what
came undone:

```sh
pactl get-default-sink      # should be the alsa_output HDMI one
```

The drop-in it writes lives at
`~/.config/systemd/user/sesh-librespot.service.d/sink.conf` and names the
speaker's sink, which contains its MAC and so cannot be known before pairing.
`install.sh` never writes or removes it, for the same reason it never clobbers
`apps.toml`.

**No speaker is a supported state.** With no drop-in, `PULSE_SINK` is unset,
PipeWire falls through to the default sink, and music plays on the TV. That is
the intended degradation — better than a room where the speaker is off and
nothing plays anywhere.

### Why librespot is a user unit

Raspotify ships a **system** unit running as its own user. That user cannot
reach the login session's PipeWire, so it cannot see the Bluetooth sink at all
and plays to nothing — which looks exactly like a broken speaker and sends you
debugging the wrong layer. `install.sh` masks the packaged unit and installs
`sesh-librespot.service` as a user unit alongside `seshd`.

`--name SESH` must match `device_name` in `/etc/sesh/spotify.toml`. If they
disagree, `transfer` correctly reports that it cannot find the room's device.

### Claim the room device once, by hand

**This step is not optional and nothing on the Pi can do it for you.**

librespot advertises itself over zeroconf, which makes it visible to Spotify
clients on the LAN — but *not* to the Web API. Until a Spotify client selects it
once, `GET /me/player/devices` does not list it at all, and `seshd` has no room
device to play to. Measured on this box: `avahi-browse -rt _spotify-connect._tcp`
showed `SESH` while the Web API listed only a phone and a desktop.

So, once, on any device signed in to the **house account**:

1. Open Spotify.
2. Play anything.
3. Devices → pick **SESH**.

That hands librespot the credentials, which `--system-cache` then keeps at
`~/.local/share/sesh/librespot`, so the claim survives reboots. Check it took:

```sh
curl -s http://127.0.0.1:7373/api/music     # the room device should be in play
journalctl --user -u sesh-librespot | grep -i credential
```

If music plays on someone's phone rather than in the room, this is why: `play`
and `enqueue` name the room's device when they can find it and fall back to
whatever is active when they cannot, warning as they go. A silent speaker is
better than a broken queue — but a queue playing into a pocket two streets away
is neither.

### The Victrola keeps its bond — once the agent outlives the pairing

> **Corrected 2026-08-20.** This section previously concluded that the speaker
> needs re-pairing every time it is powered off, and left open whether that was
> its firmware. It is not the firmware. It was ours, and it is fixed — the text
> below records the symptom, then the cause and the fix.

Observed on 2026-08-19, twice, and again on 2026-08-20 in seconds:
**pairing succeeds and then evaporates.** `bluetoothctl pair` reports success,
`Paired: yes` is true for as long as the connection lasts, and then the device
comes back `Paired: no` with no bluetoothd restart and nothing in the journal.

The cause is visible on disk. BlueZ stores each device under
`/var/lib/bluetooth/<adapter>/<mac>/info`, and the Victrola's holds:

```
[General]
Name=Victrola Brighton
Class=0x240408
SupportedTechnologies=BR/EDR;
Trusted=true
```

`[General]` and nothing else. The Basilisk mouse, which reconnects reliably, has
`[IdentityResolvingKey]`, `[LongTermKey]` and three more. **No link key was ever
stored**, so there is no bond to reconnect with — and `Trusted=true` persisting
is a red herring, because trust without a key cannot reconnect anything.

**The cause: the pairing agent died before the bond was written.**
`pair-speaker.sh` ran `bluetoothctl pair` as a one-shot, and that process exits
the instant it prints `Pairing successful`, unregistering its agent with it.
BlueZ had authorised the pairing but never persisted a key, so `Paired: yes`
was true only for the life of the connection.

Holding **one** `bluetoothctl` session open across the whole handshake — with
an explicit `agent NoInputNoOutput` and `pairable on`, and not quitting for
some seconds after `Pairing successful` — produces `Bonded: yes` and a real
`[LinkKey]` on disk. `pair-speaker.sh` now does exactly that, and refuses to
report success unless `Bonded: yes` holds, because `Paired: yes` alone is
precisely what a pairing that stored no key also reports.

Verified end to end on TatePi, 2026-08-20: bond removed, speaker re-paired by
the script, `[LinkKey]` written, then disconnected and reconnected **without
re-pairing**, and the sink came back. `seshd` recorded the matching
`audio.sink_lost` / `audio.sink_found` pair throughout.

Two traps found while establishing this, both worth not rediscovering:

- **Killing a client mid-discovery latches `Discovering: yes`.** The adapter
  then rejects `scan off` with `org.bluez.Error.Failed` and silently discovers
  nothing new, so a speaker sitting in pairing mode two feet away stays
  invisible. Powering the adapter off and on clears the flag.
- **`Pairable` does not persist.** It reads `no` after a `bluetoothd` restart
  or an adapter power cycle, which is why the script sets it every run rather
  than assuming it.

What works regardless: the sink appears on connect, `seshd` records
`audio.sink_found`, music routes to it, and losing it degrades to the TV.

### The vinyl handoff

The Victrola is a record player *and* the speaker, and those are the same input.
Switching it to phono drops the A2DP link and the sink disappears.

SESH treats that as a signal, not a fault: it records `audio.sink_lost` naming
the sink and pauses the source, then `audio.sink_found` and resumes when
Bluetooth comes back. Making the room *react* to that — dimming lights, showing
"now spinning" — is a later arc reading events that are already in the log.

### Re-running install.sh does not unpair the speaker

It used to. `install.sh` sets `ClassicBondedOnly=false` in
`/etc/bluetooth/input.conf` so game controllers can attach, and it restarted
`bluetoothd` unconditionally afterwards — including on runs where the file
already said that and nothing changed. Restarting bluetoothd drops live
connections, and on this Pi it lost the A2DP bond outright: `bluetoothctl info`
reported `Paired: no` and the music fell through to the TV mid-song.

It now restarts only when the file actually changed. If you ever do need to
bounce Bluetooth by hand, expect to re-run `sesh-pair-speaker` afterwards.

### If the speaker will not work

Bluetooth audio on a Pi is the least reliable thing in this system, which is why
this is the last phase and why the fallback is a supported state. If the device
turns out to be output-only rather than a true A2DP sink, `pair-speaker.sh` says
so by name and stops. Everything else still ships; music stays on the TV.

Check what happened:

```sh
systemctl --user status sesh-librespot
journalctl --user -u sesh-librespot -n 50
pactl list short sinks
bluetoothctl devices Paired
curl -s http://127.0.0.1:7373/api/events | grep audio.sink
```
