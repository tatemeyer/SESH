#!/bin/sh
# Pair the room's Bluetooth speaker and route music to it.
#
#     sh deploy/pair-speaker.sh            # pair a new speaker
#     sh deploy/pair-speaker.sh --show     # what is paired and where music goes
#
# Run as your normal user, not root: the sink lives in your session's PipeWire
# and the drop-in goes in your systemd user directory.
#
# On a Victrola Brighton, put the unit in pairing mode by holding the Bluetooth
# button until the indicator flashes. Note that the turntable and the speaker
# are the same device: switching to phono drops the A2DP link, which SESH
# records as `audio.sink_lost` rather than treating as a fault.
set -eu

DROPIN_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/sesh-librespot.service.d"
DROPIN="$DROPIN_DIR/sink.conf"

fail() { echo "error: $1" >&2; exit 1; }

[ "$(id -u)" -ne 0 ] || fail "run as your normal user, not root"
command -v bluetoothctl >/dev/null 2>&1 || fail "bluetoothctl not found"
command -v pactl >/dev/null 2>&1 || fail "pactl not found"

show() {
    echo "Paired devices:"
    bluetoothctl devices Paired | sed 's/^/  /' || echo "  (none)"
    echo
    echo "Sinks:"
    pactl list short sinks | awk -F'\t' '{print "  " $2}'
    echo
    echo "Music is routed to:"
    if [ -f "$DROPIN" ]; then
        sed -n 's/^Environment=PULSE_SINK=/  /p' "$DROPIN"
    else
        echo "  (no drop-in — falls through to the default sink, i.e. the TV)"
    fi
}

if [ "${1:-}" = "--show" ]; then
    show
    exit 0
fi

echo "==> Put the speaker in pairing mode now, then press enter."
read -r _

echo "==> Scanning for 20 seconds"
bluetoothctl --timeout 20 scan on >/dev/null 2>&1 || true
bluetoothctl devices | grep -v "$(bluetoothctl devices Paired | cut -d' ' -f2 | paste -sd'|' -)" || true
echo
bluetoothctl devices
echo
printf "MAC of the speaker: "
read -r MAC
[ -n "$MAC" ] || fail "no MAC given"

# trust before connect: Trusted=yes is what makes it come back by itself after
# the speaker is powered off and on, which is the normal way a room is used.
echo "==> Pairing, trusting, connecting"
bluetoothctl pair "$MAC" || fail "pairing failed"
bluetoothctl trust "$MAC" || fail "could not trust $MAC"
bluetoothctl connect "$MAC" || fail "could not connect $MAC"

echo "==> Waiting for the sink to appear"
SINK=""
i=0
while [ "$i" -lt 30 ]; do
    SINK="$(pactl list short sinks | awk -F'\t' '$2 ~ /^bluez_output\./ {print $2; exit}')"
    [ -n "$SINK" ] && break
    i=$((i + 1))
    sleep 1
done

[ -n "$SINK" ] || fail "connected, but no bluez_output sink appeared after 30s.
If this device is output-only rather than a true A2DP sink, it cannot be the
room's speaker. Everything else in SESH still works; music stays on the TV."

echo "==> Routing librespot to $SINK"
mkdir -p "$DROPIN_DIR"
cat > "$DROPIN" <<EOF
# Written by deploy/pair-speaker.sh. Safe to delete: without it music falls
# through to the default sink, which is the TV.
[Service]
Environment=PULSE_SINK=$SINK
EOF

systemctl --user daemon-reload
systemctl --user restart sesh-librespot.service 2>/dev/null || true

echo
show
echo
echo "Now confirm it survives a power cycle: turn the speaker off and on and"
echo "re-run with --show. Trusted devices reconnect by themselves."
