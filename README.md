# Frontline Foundry

A team-based military tycoon in the spirit of Roblox *War Tycoon*: build a base of ore droppers, turn income into walls, turrets, weapons and vehicles, then fight other players. Mechanics follow the genre; names, art and code are original. There are **no purchases of any kind**.

- **Client** — Rust, Bevy 0.20, Rapier physics, egui (`client/`). Clients simulate everything they own.
- **Server** — Go (`server/`): auth, rooms, persistence, object-ownership registry, UDP relay. Runs in Docker with Postgres (`deploy/`).
- **Protocol** — protobuf (`proto/frontline.proto`). TCP 4002 for reliable traffic, UDP 4003 for realtime state.
- **Shared data** — `shared/map.json`, `shared/vehicles.json` (used by both sides); tycoon items and weapons in `client/assets/tycoon/*.ron`.

## How it works

- **Rooms are save files.** Creating one makes you host (kick/ban, grant cash, edit rules, switch teams, wipe a base, transfer host, archive). Rooms are archived, never deleted, and can be restored and continued. Cash, XP, rank, rebirths and bases are saved per player per room.
- **Client-trusted, server-relayed.** Each object has one owner. The server drops updates from non-owners and updates with a stale epoch, relays the rest, and simulates unowned objects (gravity, ground, static collision, despawn timers). Cheating by an owner is possible by design; host admin tools are the mitigation.
- **Tycoon.** Buy pads → droppers → conveyor → upgraders → collector bank → claim pad → cash. Walls, towers, turrets, weapon racks, barracks, med station, armor depot, vehicle pads, HQ (unlocks rebirth for a permanent income multiplier).

## Run it

```bash
make server-run            # local in-memory server
make client-run SERVER=127.0.0.1
make server-test client-test
make deploy                # rsync + docker compose on host "hermes"
```

The client defaults to `76.179.76.99` (override with `FF_SERVER`). The deployed server needs TCP 4002 and UDP 4003 reachable.

## Controls

WASD move · Shift sprint · Space jump · LMB fire · RMB aim down sights · F inspect weapon · G gunsmith · 1–9 / scroll weapons · R reload · E interact/enter/exit · Enter chat · Tab scores · Esc menu · F1 admin (host).

The gunsmith (G) customizes every weapon: optics (iron, red dot, holographic, 3x, 8x), muzzles, underbarrel grips/laser, magazines, stocks, 17 finishes and reticle colours. Everything is free and unlocks with rank (two finishes with rebirths); choices are saved to `<config>/frontline-foundry/armory.json` and other players see your build.
Vehicles: WASD drive, Space brake (helicopter: Space up, Ctrl down), R flips cars and tanks, mouse aims turret/heading.

## Layout

```
client/{proto,sim,game}   Rust workspace (sim = pure logic with tests)
server/                   Go module `frontline`
proto/ shared/ deploy/ docs/
```

`docs/art-prompts.md` holds the image prompts used for the reference art.

:)
