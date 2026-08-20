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
        # Bonded is the one that matters: Paired can read yes for a pairing
        # that stored no link key, and such a pairing does not survive.
        bluetoothctl info "$SPK" | grep -E "Paired|Bonded|Trusted|Connected" | sed 's/^\s*/  /'
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

# One bluetoothctl session, held open across the whole handshake.
#
# This is load-bearing, and the reason is worth keeping. `bluetoothctl pair`
# run as a one-shot exits the instant it prints "Pairing successful", taking
# its pairing agent with it. On the Victrola the bond then never persists:
# BlueZ reports `Paired: yes` for exactly as long as the connection lasts,
# writes `[General]` and no `[LinkKey]` to
# /var/lib/bluetooth/<adapter>/<mac>/info, and the device is back to
# `Paired: no` afterwards. That was mistaken for the speaker's firmware.
#
# Holding one session open — with an explicit agent, and `pairable on` — gets
# `Bonded: yes` and a real `[LinkKey]` on disk. Verified on TatePi 2026-08-20:
# the same speaker that would not hold a bond across a disconnect now
# reconnects without re-pairing.
echo "==> Pairing, trusting, connecting"
{
    echo "agent NoInputNoOutput"
    echo "default-agent"
    echo "pairable on"
    sleep 3
    echo "pair $MAC"
    # Do not shorten: the bond is written after "Pairing successful" prints,
    # and quitting early is precisely the bug this replaced.
    sleep 25
    # trust before connect: Trusted=yes is what lets it come back by itself,
    # but trust without a key cannot reconnect anything — the bond above is
    # what makes this meaningful.
    echo "trust $MAC"
    sleep 2
    echo "connect $MAC"
    sleep 8
    echo "quit"
} | bluetoothctl >/dev/null 2>&1 || true

# The only question that matters is whether a bond survived. `Paired: yes`
# alone is not enough — that is true of a pairing that never stored a key.
bluetoothctl info "$MAC" | grep -qE '^\s*Bonded: yes' \
    || fail "pairing did not produce a bond.
Put the speaker back in pairing mode and re-run. If it still fails, check
'sudo grep -c LinkKey /var/lib/bluetooth/*/$MAC/info' — no key there means
the bond is not being stored, not that the speaker refused."

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
echo "The bond is real: this script checked for \`Bonded: yes\`, which only holds"
echo "when BlueZ has written a link key to disk. The speaker reconnects by"
echo "itself from here; you should not need to pair it again."
echo
echo "To confirm after a power cycle, turn the speaker off and on, then:"
echo
echo "    $0 --show"
echo
echo "Paired: yes  -> as expected."
echo "Paired: no   -> the bond was lost, which should no longer happen. Check"
echo "                /var/lib/bluetooth/<adapter>/<mac>/info: a working bond"
echo "                has a [LinkKey] section, and [General] alone means no key"
echo "                was stored. Re-run this script with the speaker in"
echo "                pairing mode, and say so — it would be a new failure."
