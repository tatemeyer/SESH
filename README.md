# SESH

A living room that knows who's in it.

`SESH` runs on a Raspberry Pi 5 wired to the living room TV. It knows who's on
the couch, turns everyone's phone into a controller, keeps a permanent record
of everything the house has ever done, and reacts to it.

It is not a media box. Kodi, RetroArch, and Moonlight already do their jobs and
are launched as apps. `SESH` is the layer that ties the room, the people in it,
and its entire history into one thing.

- **Vision and architecture:** [`docs/superpowers/specs/2026-08-15-sesh-vision-design.md`](docs/superpowers/specs/2026-08-15-sesh-vision-design.md)
- **Arc 1 plan:** [`docs/superpowers/plans/2026-08-15-arc1-log-and-room.md`](docs/superpowers/plans/2026-08-15-arc1-log-and-room.md)

## Status

Design approved, Arc 1 planned. No implementation yet.

Arc 1 — *The Log & The Room* — is done when the Pi boots to SESH's own
screen and launches and quits Kodi, RetroArch, and Moonlight from a
controller, with every launch and exit recorded in the event log.

## Development

Follows the [superpowers](https://github.com/obra/superpowers) methodology: no
implementation without an approved design doc and plan first. Specs live in
`docs/superpowers/specs/`, plans in `docs/superpowers/plans/`.
