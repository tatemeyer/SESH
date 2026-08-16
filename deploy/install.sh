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

echo "==> Adding $SESH_USER to the groups the compositor needs"
usermod -aG video,input,render,audio "$SESH_USER"
loginctl enable-linger "$SESH_USER"

echo "==> Installing binary, surface bundle, and configuration"
install -Dm755 target/release/seshd /usr/local/bin/seshd
install -Dm644 deploy/apps.toml /etc/sesh/apps.toml
rm -rf /usr/local/share/sesh/web
mkdir -p /usr/local/share/sesh/web
cp -r surfaces/dist/. /usr/local/share/sesh/web/

SESH_HOME="$(getent passwd "$SESH_USER" | cut -d: -f6)"
[ -n "$SESH_HOME" ] || fail "could not resolve home directory for $SESH_USER"

install -Dm644 deploy/seshd.service "${SESH_HOME}/.config/systemd/user/seshd.service"
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
echo "  1. Edit /etc/sesh/apps.toml and replace GAMING-PC with your Sunshine host."
echo "  2. Confirm each command resolves: which kodi retroarch moonlight-qt"
echo
echo "Then: sudo reboot"
