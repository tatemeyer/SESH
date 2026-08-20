#!/bin/sh
# Pair the room's Bluetooth speaker and route music to it.
#
#     sh deploy/pair-speaker.sh <MAC>      # pair that device
#     sh deploy/pair-speaker.sh --scan     # find the MAC first
#     sh deploy/pair-speaker.sh            # scan, then prompt (needs a terminal)
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
    echo "Speaker bond:"
    SPK="$(bluetoothctl devices Paired | awk '/Victrola|Speaker|Brighton/ {print $2; exit}')"
    if [ -z "$SPK" ]; then
        echo "  no speaker is paired. If one used to be, its bond was lost —"
        echo "  re-run this script with the speaker in pairing mode."
    else
        bluetoothctl info "$SPK" | grep -E "Paired|Trusted|Connected" | sed 's/^\s*/  /'
    fi
    echo
    echo "Sinks:"
    pactl list short sinks | awk -F'\t' '{print "  " $2}'
    echo
    echo "Music is routed to:"
    if [ -f "$DROPIN" ]; then
        WANT="$(sed -n 's/^Environment=PULSE_SINK=//p' "$DROPIN")"
        echo "  $WANT"
        if ! pactl list short sinks | awk -F'\t' '{print $2}' | grep -qxF "$WANT"; then
            echo "  ^ that sink does not exist right now, so PipeWire falls through"
            echo "    to the default and music comes out of the TV. Pair the speaker"
            echo "    again to get out of the fallback."
        fi
    else
        echo "  (no drop-in — falls through to the default sink, i.e. the TV)"
    fi
}

scan() {
    echo "==> Scanning for 20 seconds. The speaker must be in pairing mode."
    bluetoothctl --timeout 20 scan on >/dev/null 2>&1 || true
    echo
    echo "Devices seen (named ones first — a speaker in pairing mode shows its name):"
    bluetoothctl devices \
        | awk '$3 !~ /^([0-9A-F]{2}-){5}[0-9A-F]{2}$/ {print "  " $0}'
    echo
    echo "Everything else:"
    bluetoothctl devices \
        | awk '$3 ~ /^([0-9A-F]{2}-){5}[0-9A-F]{2}$/ {print "  " $0}' | head -10
}

case "${1:-}" in
    --show) show; exit 0 ;;
    --scan) scan; echo; echo "Then: $0 <MAC>"; exit 0 ;;
esac

MAC="${1:-}"

# Prompting needs a terminal, and this script is run over ssh, from an agent,
# and from install-time notes as often as by hand. Taking the MAC as an argument
# is the path that always works; the prompt is the convenience, not the
# contract.
if [ -z "$MAC" ]; then
    [ -t 0 ] || fail "no MAC given and no terminal to ask on.
Run '$0 --scan' to find it, then '$0 <MAC>'."

    echo "==> Put the speaker in pairing mode now, then press enter."
    read -r _
    scan
    echo
    printf "MAC of the speaker: "
    read -r MAC
fi

[ -n "$MAC" ] || fail "no MAC given"
echo "$MAC" | grep -qiE '^([0-9a-f]{2}:){5}[0-9a-f]{2}$' \
    || fail "'$MAC' is not a MAC address (expected AA:BB:CC:DD:EE:FF)"

# BlueZ only knows about a device while discovery is running — outside a scan,
# `pair` answers "Device not available" even for a speaker sitting in pairing
# mode two feet away. So hold a scan open across the pairing.
echo "==> Holding discovery open"
bluetoothctl --timeout 45 scan on >/dev/null 2>&1 &
SCAN_PID=$!
trap 'kill "$SCAN_PID" 2>/dev/null || true' EXIT INT TERM
sleep 8

bluetoothctl info "$MAC" >/dev/null 2>&1 \
    || fail "$MAC is not visible. Is it still in pairing mode? Most speakers
leave it after a minute or two. Press the button again and re-run."

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

# Connecting a Bluetooth sink makes WirePlumber adopt it as the *default*, which
# would send Kodi, RetroArch and Moonlight to the speaker too — the exact
# opposite of this phase's promise that game audio stays on the TV. Pin it back.
# An explicitly configured default is respected across disconnects, so this
# holds when the speaker is power-cycled.
HDMI="$(pactl list short sinks | awk -F'\t' '$2 ~ /^alsa_output/ {print $2; exit}')"
if [ -n "$HDMI" ]; then
    echo "==> Pinning the default sink back to $HDMI"
    pactl set-default-sink "$HDMI" || echo "warning: could not pin the default sink."
else
    echo "warning: no ALSA sink found to pin the default to; game audio may follow the speaker."
fi

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
echo "Now confirm the bond actually persisted, because on this hardware it may"
echo "not have. Turn the speaker off and on, wait a few seconds, then:"
echo
echo "    $0 --show"
echo
echo "Paired: yes  -> good, it will come back by itself from now on."
echo "Paired: no   -> the bond did not survive. \`Trusted\` alone cannot"
echo "                reconnect anything; it needs a stored link key. The"
echo "                Victrola Brighton was observed doing exactly this:"
echo "                /var/lib/bluetooth/<adapter>/<mac>/info held [General]"
echo "                and nothing else, where a device that reconnects has"
echo "                [LinkKey] or [LongTermKey] too. Re-run this script with"
echo "                the speaker in pairing mode to play again."
