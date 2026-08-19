# Clock trust — verified on TatePi, 2026-08-19

Task 7 of `docs/superpowers/plans/2026-08-19-clock-trust.md`, run as a real cold
boot with Kodi left running. **PASS**, and it corrected two things this project
had written down as fact.

## The boot

```
boot:      2026-08-19 14:33:02
[ 133.592629] systemd: Started seshd.service
[ 134.191960] seshd: seshd listening addr=0.0.0.0:7373
[ 134.942851] systemd-timesyncd: Initial clock synchronization to 14:35:16.521684
```

| | |
|---|---|
| `seshd` start → bind | **0.60s** (budget is 10.48s) |
| `seshd` start → NTP | **1.35s** |
| Reconciliation ran | ~0.6s **before** NTP landed |

Boot chain PASS on every link, screenshot has real content, all 39 pre-reboot
events survived unchanged, and exactly one row was appended:

```json
{ "id": 40, "kind": "app.exited", "subject": "kodi",
  "ts_ms": 1787168115747,
  "payload": { "clock_synced": false,
               "exit_observed": false,
               "last_alive_ms": 1787167943552,
               "reason": "seshd restarted while this app was running" } }
```

Marked, as designed — the clock genuinely was not set when it was written.

## Correction 1: the inversion is not guaranteed by a clean reboot

The spec says of the reconciliation row:

> So it writes `ts_ms < last_alive_ms`: upper bound below lower bound, same
> payload. After a reboot with an app left running that is guaranteed rather
> than a race.

**That is wrong, and this boot shows it.** `ts_ms - last_alive_ms = +172,195 ms`
— the bound held. The row is marked *and* correct.

The mechanism is why. `systemd-timesyncd` restores the timestamp it saved to
`/var/lib/systemd/timesync/clock`, and it rewrites that file constantly —
observed mtime seven seconds old on an idle box. So after a **clean** shutdown
the restored clock is at or after the last event's timestamp, reconciliation
stamps later still, and the bound survives.

Inverting it needs the saved clock file to be *older than the last event*, which
takes an unclean stop between timesyncd's last save and the event — a power cut,
which `~/.config/labwc/autostart` already calls "normal use" for a room
appliance. The mechanism is proven by unit test
(`reconcile::tests::a_row_written_before_the_clock_is_set_says_so`, which steps
the clock back explicitly); what is now measured is that a clean reboot does not
reach it.

**This does not weaken the fix.** The mark records what the box *knew*, not
whether it turned out lucky, and being conservative on a boot that happened to
come out right is the correct behaviour rather than a false positive. But the
spec claimed a guarantee it does not have, and that claim travelled through two
PR descriptions before anything checked it.

## Correction 2: removing the wait was right, but it is a trade, not a free win

`seshd` start → NTP has now been measured three times: **9.2s**, **20.8s**, and
**1.35s**. The variance is the finding; there is no typical value to design
against.

On *this* boot the reverted ten-second wait would have cost 1.35s, stayed inside
the kiosk's 10.48s budget, and produced a row that was unmarked and correct —
strictly better data. On the 20.8s boot it would have blown the budget and put a
connection-refused page on the TV.

So removing it buys immunity from the black screen at the price of a marked row
on boots where NTP is quick. That is the right trade for a room appliance, and
it is the spec's own principle — but PR #18 argued it as though the wait had no
upside, and it has one.

**If better data is ever worth wanting back**, the lever is
`deploy/labwc/autostart`, not `seshd`: raise the kiosk's patience above the
ceiling and a bounded wait becomes safe again. Not recommended today — the mark
works, and one moving part beats two — but it is the option, and it is written
down so it does not have to be rediscovered.

## Harness

`check_clock.py` judges every reconciliation row written since the pre-reboot
snapshot: pass if unmarked with `ts_ms >= last_alive_ms`, pass if marked, fail if
unmarked with `ts_ms` below `last_alive_ms`, and INCONCLUSIVE if the reboot left
nothing to reconcile — which is what the first attempt at this hit, because
merging a PR does not install a binary.

`verify.sh` now also records `seshd`'s start-to-bind against the kiosk's budget,
so the black-screen risk shows up in `VERDICT.txt` rather than needing to be
found by reading `autostart` again.
