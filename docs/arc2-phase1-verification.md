# Arc 2 Phase 1 — verification record

Run on TatePi, 2026-08-17, against the real `seshd` binary built from this
branch, on a spare port with a throwaway database so the live log was never
touched.

## How the QR was read

The QR was decoded from the served SVG rather than photographed:
`rsvg-convert` rasterises it and `zbarimg` reads it — the same bytes a phone's
camera would. `zbar-tools` was installed on the Pi for this and is a dev tool,
not a runtime dependency.

This matters because the join URL is the one thing in Phase 1 that cannot be
checked by unit tests: the TV fetches the QR over `127.0.0.1`, so a QR built
from the request's `Host` header would encode loopback and be useless to every
phone that scanned it. The decode proves it does not.

```
decoded: http://192.168.40.195:7474/join?c=6cebc398a7c348979c7c479d50efcca8
```

`192.168.40.195` is the Pi's Ethernet address, chosen by the routing-table
probe in `config::detect_lan_ip`, with the port taken from `--bind`.

## Results

| Check | Result |
|---|---|
| QR decodes to a LAN join URL, not loopback | PASS |
| Join with the scanned code | `201`, token returned |
| **The same code a second time** | `403` |
| `GET /api/me` with the token | `200` |
| `GET /api/me` with a bad token / no token | `401` / `401` |
| Heartbeat puts them on `/api/roster` | `["marcus"]` |
| A repeat heartbeat appends nothing | 2 events → 2 events |
| The token appears nowhere in the log | PASS |
| Restart: roster rebuilt from the log | `["marcus"]` |
| Restart: the token still works | `200` |
| **Restart: no spurious `presence.arrived`** | 2 events → 2 events |

The log after the run — the first events SESH has ever held with an actor
attached:

```
 1  person.joined      actors=["marcus"]  {"name":"Marcus"}
 2  presence.arrived   actors=["marcus"]  {}
```

and the registry row, with a 64-hex-character (32-byte) token:

```
('marcus', 'Marcus', 1787006625105, 64)
```

## The two rows that are the point

Two checks in that table are worth more than the rest, because both guard
against a bug that would only ever show up as slow log rot:

- **A code cannot be spent twice.** This is what makes a photograph of the TV
  worthless a minute later, which is the only thing rotation actually defends
  against — anyone already on the LAN can simply fetch the QR endpoint, and the
  LAN is SESH's declared trust boundary.
- **A restart appends no spurious arrival.** `Presence` is seeded from the
  rebuilt roster at startup. Without that, every daemon restart — every config
  reload, every crash under `Restart=always`, every power cut — would append a
  `presence.arrived` for someone who never left, and the log is append-only, so
  each one would be permanent. Exactly the shape of the Arc 1 dangling-launch
  bug, caught before it could happen rather than after.

## Not verified here

A real phone camera, and the QR actually on the TV. Both need the Phase 5
surfaces, which do not exist yet — there is no `/join` page for a camera to
land on. See the amendment in the plan.
