#!/bin/sh
# Provision a Raspberry Pi to boot into SESH.
#
# Run as root from the repo root, AFTER `sh deploy/build.sh`:
#     sudo sh deploy/install.sh
#
# Assumes Raspberry Pi OS (64-bit). Works on Lite; on the Desktop image it
# will install alongside the existing desktop, and the autologin change below
# will take over tty1 — see deploy/README.md before running it there.
set -eu

fail() {
    echo "error: $1" >&2
    exit 1
}

[ "$(id -u)" -eq 0 ] || fail "run this with sudo"
[ -f Cargo.toml ] && [ -d deploy ] || fail "run this from the repo root"

# Default to whoever invoked sudo. SESH runs as a normal login user, not a
# system account, because it needs a real session on tty1 to host the
# compositor that its launched apps draw into.
SESH_USER="${SESH_USER:-${SUDO_USER:-pi}}"
id -u "$SESH_USER" >/dev/null 2>&1 || fail "user '$SESH_USER' does not exist. Set SESH_USER=<name> and retry."

[ -x target/release/seshd ] || fail "target/release/seshd not found. Run 'sh deploy/build.sh' first."
[ -f surfaces/dist/index.html ] || fail "surfaces/dist not built. Run 'sh deploy/build.sh' first."

echo "==> Installing packages"
apt-get update
# moonlight-qt is NOT in the Raspberry Pi OS repositories. It is listed here
# so the install fails loudly rather than silently shipping a broken tile; if
# apt cannot find it, install it from the Moonlight project's own repository
# (https://github.com/moonlight-stream/moonlight-qt) and re-run this script.
# Verify with `which moonlight-qt` before rebooting.
# fonts-noto-color-emoji: the tile icons in surfaces/src/styles.css are emoji
#   codepoints (\1F3AC, \1F579, \1F5A5). Pi OS Lite ships no emoji font, so
#   without this every tile renders an empty tofu box.
# qt6-wayland: moonlight-qt is Qt6, but Pi OS ships only the Qt5 Wayland
#   plugin. Without it Moonlight aborts ~600ms after launch under labwc with
#   "no Qt platform plugin could be initialized" — which looks exactly like a
#   SESH launcher bug and sends you debugging the wrong thing.
apt-get install -y labwc seatd curl kodi retroarch \
    fonts-noto-color-emoji qt6-wayland || fail "package install failed"
if ! command -v chromium-browser >/dev/null 2>&1 && ! command -v chromium >/dev/null 2>&1; then
    apt-get install -y chromium-browser || apt-get install -y chromium || fail "could not install Chromium"
fi
if ! command -v moonlight-qt >/dev/null 2>&1; then
    echo "warning: moonlight-qt is not installed. The Moonlight tile will fail to launch."
    echo "         Install it from https://github.com/moonlight-stream/moonlight-qt and re-run."
fi

echo "==> Letting game controllers attach (BlueZ HID)"
# BlueZ's input profile defaults to ClassicBondedOnly=true, which refuses the
# HID profile to any device that is not bonded. A DualShock 4 pairs without
# bonding, so the controller connects at the Bluetooth layer and is then
# rejected at the input layer: `bluetoothd` logs
#     hidp_add_connection() Rejected connection from !bonded device
# and no /dev/input node ever appears. The controller shows as "Connected" in
# every UI while being invisible to the browser, which is a genuinely
# confusing place to land. SESH's TV UI is controller-driven, so this is a
# requirement, not a convenience.
#
# The tradeoff is deliberate and matches Arc 1's LAN trust model: an unbonded
# HID device in Bluetooth range can send input. In a living room that is the
# same threat as someone picking the controller up.
#
# Restarting bluetoothd drops every active connection *and* was observed to
# lose the bond for a paired A2DP speaker outright — `bluetoothctl info` came
# back `Paired: no` afterwards, and the room's music fell through to the TV.
# install.sh is meant to be re-runnable, so it must not bounce Bluetooth on a
# run that changes nothing. Restart only when the file actually changed.
INPUT_CONF=/etc/bluetooth/input.conf
if [ -f "$INPUT_CONF" ]; then
    INPUT_CONF_BEFORE="$(cat "$INPUT_CONF")"
    if grep -qE '^[[:space:]]*ClassicBondedOnly[[:space:]]*=' "$INPUT_CONF"; then
        sed -i 's/^[[:space:]]*ClassicBondedOnly[[:space:]]*=.*/ClassicBondedOnly=false/' "$INPUT_CONF"
    elif grep -qE '^[[:space:]]*#[[:space:]]*ClassicBondedOnly' "$INPUT_CONF"; then
        sed -i 's/^[[:space:]]*#[[:space:]]*ClassicBondedOnly.*/ClassicBondedOnly=false/' "$INPUT_CONF"
    elif grep -qE '^[[:space:]]*\[General\]' "$INPUT_CONF"; then
        # Appending a second [General] would make the key file fail to parse,
        # so extend the existing one instead.
        sed -i '0,/^[[:space:]]*\[General\]/s//[General]\nClassicBondedOnly=false/' "$INPUT_CONF"
    else
        printf '\n[General]\nClassicBondedOnly=false\n' >> "$INPUT_CONF"
    fi
    if [ "$INPUT_CONF_BEFORE" = "$(cat "$INPUT_CONF")" ]; then
        echo "    already set; leaving Bluetooth alone so a paired speaker stays paired"
    else
        systemctl restart bluetooth || echo "warning: could not restart bluetooth; reboot to apply."
    fi
else
    echo "warning: $INPUT_CONF not found. If a controller pairs but never appears"
    echo "         as an input device, set ClassicBondedOnly=false there by hand."
fi

echo "==> Installing librespot (Spotify Connect endpoint for the room)"
# Neither librespot nor raspotify is in the Raspberry Pi OS archive, so the
# package comes from the project's own Cloudsmith repo — the same shape of
# problem as moonlight-qt, but this one has an installable source.
#
# Not fatal if it fails. Without librespot the queue still fills, the veto
# still works, and the log still records the night; only the sound goes away.
# A room that boots without music beats an installer that refuses to finish.
if ! command -v librespot >/dev/null 2>&1; then
    if [ ! -f /etc/apt/sources.list.d/raspotify.list ]; then
        curl -sSL https://dtcooper.github.io/raspotify/key.asc \
            | gpg --dearmor -o /usr/share/keyrings/raspotify_key.gpg 2>/dev/null || true
        chmod 644 /usr/share/keyrings/raspotify_key.gpg 2>/dev/null || true
        echo 'deb [signed-by=/usr/share/keyrings/raspotify_key.gpg] https://dtcooper.github.io/raspotify raspotify main' \
            > /etc/apt/sources.list.d/raspotify.list
        apt-get update || true
    fi
    apt-get install -y raspotify || echo "warning: raspotify install failed; music will have no output device."
fi

# The packaged unit is a *system* service running as its own user, which cannot
# reach the login session's PipeWire and therefore cannot see the Bluetooth
# sink at all. Left enabled it competes with ours and plays to nothing, which
# looks exactly like a broken speaker. SESH ships a user unit instead.
if systemctl list-unit-files raspotify.service >/dev/null 2>&1; then
    systemctl disable --now raspotify.service 2>/dev/null || true
    systemctl mask raspotify.service 2>/dev/null || true
fi

echo "==> Adding $SESH_USER to the groups the compositor needs"
usermod -aG video,input,render,audio "$SESH_USER"
loginctl enable-linger "$SESH_USER"

echo "==> Installing binary, surface bundle, and configuration"
install -Dm755 target/release/seshd /usr/local/bin/seshd

# apps.toml is configuration, not an artifact. It is the one installed file a
# user is *told* to edit — it ships with a placeholder Sunshine host and the
# closing message asks them to replace it — so overwriting it on every run
# means the installer reverts that edit and silently breaks the Moonlight tile,
# presenting as Moonlight failing for no reason. Keep an existing copy and drop
# the shipped template beside it, so an upgrade can still see what changed.
if [ -e /etc/sesh/apps.toml ]; then
    APPS_TOML_KEPT=yes
    install -Dm644 deploy/apps.toml /etc/sesh/apps.toml.dist
else
    APPS_TOML_KEPT=no
    install -Dm644 deploy/apps.toml /etc/sesh/apps.toml
fi

# spotify.toml holds the house account's client secret, so it is 0640 and
# group-readable by the SESH user rather than world-readable like apps.toml.
# Same never-clobber rule: it is filled in by hand and an upgrade that wiped it
# would present as "the music stopped working" with no cause on screen.
if [ -e /etc/sesh/spotify.toml ]; then
    SPOTIFY_TOML_KEPT=yes
    install -Dm644 deploy/spotify.toml /etc/sesh/spotify.toml.dist
else
    SPOTIFY_TOML_KEPT=no
    install -Dm640 -g "$SESH_USER" deploy/spotify.toml /etc/sesh/spotify.toml
fi
rm -rf /usr/local/share/sesh/web
mkdir -p /usr/local/share/sesh/web
cp -r surfaces/dist/. /usr/local/share/sesh/web/

SESH_HOME="$(getent passwd "$SESH_USER" | cut -d: -f6)"
[ -n "$SESH_HOME" ] || fail "could not resolve home directory for $SESH_USER"

install -Dm644 deploy/seshd.service "${SESH_HOME}/.config/systemd/user/seshd.service"
install -Dm644 deploy/systemd/sesh-librespot.service \
    "${SESH_HOME}/.config/systemd/user/sesh-librespot.service"
install -Dm755 deploy/pair-speaker.sh /usr/local/bin/sesh-pair-speaker

# The speaker drop-in is configuration written by pair-speaker.sh and names a
# MAC this script cannot know. Never write it, never remove it — the same rule
# as apps.toml, and wiping it would present as "music moved back to the TV" on
# an upgrade with nothing on screen explaining why.
install -Dm644 deploy/labwc/rc.xml "${SESH_HOME}/.config/labwc/rc.xml"
install -Dm755 deploy/labwc/autostart "${SESH_HOME}/.config/labwc/autostart"
mkdir -p "${SESH_HOME}/.local/share/sesh"

echo "==> Checking the boot target"
if systemctl get-default 2>/dev/null | grep -q graphical; then
    cat <<'EOM'

  This Pi currently boots to the graphical desktop. SESH needs the display
  to itself — a running display manager fights labwc for the seat, which
  shows up as a black screen or a flickering handoff with no clear cause.

  Switching the boot target to Console Autologin. To put the desktop back:

      sudo raspi-config nonint do_boot_behaviour B4

EOM
    if command -v raspi-config >/dev/null 2>&1; then
        raspi-config nonint do_boot_behaviour B2
    else
        systemctl set-default multi-user.target
        echo "note: raspi-config not found; set the default target to multi-user instead."
    fi
fi

echo "==> Starting labwc on login to tty1"
PROFILE="${SESH_HOME}/.bash_profile"
# Bash reads ~/.bash_profile INSTEAD of ~/.profile for login shells. Creating
# this file where none existed therefore silently drops whatever ~/.profile
# did — on a stock Pi that is the line sourcing ~/.bashrc, so an SSH login
# lands without nvm, ~/.local/bin, or any other shell setup, with nothing to
# suggest the installer caused it. The kiosk on tty1 never notices because it
# execs labwc immediately; every other login shell does. Seed the fallback
# before appending the kiosk hook.
if [ ! -e "$PROFILE" ]; then
    cat > "$PROFILE" <<'EOF'
# Bash reads this file instead of ~/.profile for login shells, so source it
# explicitly to keep ~/.bashrc (nvm, PATH, completions) working.
[ -f "$HOME/.profile" ] && . "$HOME/.profile"
EOF
fi
if ! grep -q "exec labwc" "$PROFILE" 2>/dev/null; then
    cat >> "$PROFILE" <<'EOF'

# Start the SESH kiosk when logging in on the console.
if [ -z "${WAYLAND_DISPLAY:-}" ] && [ "$(tty)" = "/dev/tty1" ]; then
    exec labwc
fi
EOF
fi
chown -R "$SESH_USER:$SESH_USER" "${SESH_HOME}/.config" "${SESH_HOME}/.local" "$PROFILE"

echo "==> Enabling autologin on tty1"
mkdir -p /etc/systemd/system/getty@tty1.service.d
cat > /etc/systemd/system/getty@tty1.service.d/autologin.conf <<EOF
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin ${SESH_USER} --noclear %I \$TERM
EOF
systemctl daemon-reload

echo
echo "Installed for user: $SESH_USER"
echo
echo "Before rebooting:"
if [ "$APPS_TOML_KEPT" = yes ]; then
    echo "  1. Kept your existing /etc/sesh/apps.toml. This release's template is"
    echo "     at /etc/sesh/apps.toml.dist if you want to diff it."
else
    echo "  1. Edit /etc/sesh/apps.toml and replace GAMING-PC with your Sunshine host."
fi
echo "  2. Confirm each command resolves: which kodi retroarch moonlight-qt"
if [ "$SPOTIFY_TOML_KEPT" = yes ]; then
    echo "  3. Pair the room speaker, if you have not:  sesh-pair-speaker"
echo "     Without one, music comes out of the TV, which is the fallback."
echo "  4. Kept your existing /etc/sesh/spotify.toml. This release's template"
    echo "     is at /etc/sesh/spotify.toml.dist if you want to diff it."
else
    echo "  3. For music: put the house account's client id and secret in"
    echo "     /etc/sesh/spotify.toml, then run 'seshd auth-spotify' once as"
    echo "     $SESH_USER. Skip this if you are not using the queue yet — the"
    echo "     room launches apps without it."
fi
echo
echo "Then: sudo reboot"
