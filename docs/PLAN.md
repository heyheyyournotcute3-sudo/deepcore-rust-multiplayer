# Deepcore — Plan

> Written after reading the full build prompt, before writing game code.
> This file states what the game is, how the code is laid out, and the order of work.

## 1. What we are building

**Deepcore** is a co-op, first-person, voxel mining game in Rust. Up to 16 players share one
persistent 1024 × 1024 × 512 mine. Everyone starts with a steel pickaxe and nothing else. The
loop is: *dig down → haul ore home → build a machine → the machine unlocks a better tool or a
new material → dig deeper*. Six eras later the team builds a nuclear reactor and a Nuclear
Drill that bores a 3×3 tunnel at 12× speed.

The design's spine is that **every unlock is a physical structure in the world**, not a menu
entry. Mining depth is gated by tool tier; tool tier is gated by machines; machines are gated
by materials; materials are gated by depth. Nothing can be skipped.

## 2. Architectural rules (the ones that keep it sane)

1. **`shared` holds every rule.** Blocks, items, recipes, machines, tech tree, worldgen,
   movement, collision, inventory maths, power/steam solving, hazards, and the protocol types
   all live in `deepcore-shared`. It has no rendering and no sockets.
2. **`server` decides, `client` shows.** The server owns mining timers, inventory, machine
   progress, power, hazards, research and persistence. The client predicts *only its own
   movement* and sends intents for everything else.
3. **Data over code.** Content lives in `assets/data/*.ron`. Adding an ore, item, recipe,
   machine or tech node must never require a Rust edit. The loaders resolve string IDs to
   compact `u16` IDs and validate the whole graph, failing fast with a readable error.
4. **One ECS, both sides.** Bevy runs headless on the server (`MinimalPlugins` + the game's
   server plugins) and windowed on the client. The same components and the same shared systems
   exist on both; only the plugins differ.
5. **Small files, one plugin per feature.** Target < 400 lines per file.

## 3. Crate layout

```
deepcore/
  Cargo.toml            # workspace, pinned engine versions, profiles
  crates/shared/        # pure game logic (no render, no net I/O)
    content/            # RON types, registries, validator, content hash
    voxel/              # BlockPos/ChunkPos maths, Chunk, palette compression
    worldgen/           # layers, caves, ore veins — deterministic from seed
    movement/           # AABB-vs-voxel collision + player controller (both sides)
    inventory/          # stacks, slots, transfer rules, stacking
    crafting/           # recipe matching, machine state machine
    power/              # steam + electric network graphs and solvers
    hazards/            # heat, gas, water, radiation, cave-in maths
    protocol/           # replicated components, intents, events, channels
    lib.rs              # SharedPlugin
  crates/server/        # headless authoritative binary `server`
  crates/client/        # windowed binary `client`
  crates/lobby/         # axum + sqlx account/server-browser service
  tools/atlas_gen/      # builds the procedural texture atlas PNG from blocks.ron
```

## 4. Verified dependency set

Checked live against crates.io on 2026-11-12 (not from memory). Bevy moved to **0.20.1** since
the prompt was written, and the whole ecosystem supports it:

| Crate | Version | Notes |
|---|---|---|
| bevy | 0.20.1 | default features off; per-crate feature lists |
| bevy_replicon | 0.45.5 | requires bevy ^0.20 |
| bevy_replicon_renet | 0.21.0 | requires bevy ^0.20, bevy_renet ^6 |
| bevy_egui | 0.43.1 | requires bevy ^0.20.0, egui ^0.36 |
| noise | 0.9 | |
| sqlx | 0.9 | **replaces rusqlite** — see deviation below |

**Deviation from the prompt:** the prompt asks for `rusqlite` on the server and `sqlx` in the
lobby. Cargo forbids this: both crates link the native `sqlite3` library and only one crate in
a graph may declare `links = "sqlite3"`. The build fails with a hard resolver error. We use
`sqlx` everywhere instead — one SQLite stack, async, and Postgres-ready as the prompt wanted
for the lobby. Recorded in `PROGRESS.md`.

Bevy ecosystem crates are pinned with `=` (exact) because they break on every release; utility
crates use caret ranges and the committed `Cargo.lock` freezes the rest.

## 5. Milestones

Each milestone must end with: compiling tree, green `cargo test --workspace`, clean
`cargo clippy --workspace -- -D warnings`, and an updated `PROGRESS.md`.

- **M0 — Workspace and skeleton.** Crates, pinned versions, profiles, `scripts/check.sh`.
  Client opens a window with a lit cube; server ticks headless at 30 Hz.
- **M1 — Voxel world (single process).** Palette chunks, six-layer worldgen with caves and
  veins, procedural atlas, greedy meshing on the async pool, AABB movement, raycast targeting,
  mining with hardness/tier, placing, hotbar, depth meter.
- **M2 — Multiplayer foundation.** Handshake (protocol + content hash), chunk streaming with
  interest management, replicated players with interpolation, movement prediction and
  reconciliation, intents with full server validation, menu with Host/Join, latency+loss
  simulation, bot clients.
- **M3 — Content, inventory, crafting (Era 0).** Validator + `balance_report`, egui inventory
  with drag/split/quick-move, pickups, death crate, respawn, workbench, trees, ladders,
  supports, oil lamps, darkness.
- **M4 — Machines, research, Era 1.** Block entities, multi-block ghost placement, machine UI
  with idle reasons, fuel burning, Kiln/Furnace/Anvil/Blast Furnace, Research Desk, tech graph.
- **M5 — Era 2, steam and logistics.** Pipe networks + steam solver, pump/boiler/engine,
  crusher and washer yield bonuses, conveyors, rails/minecarts/lift, Steam Drill, flooding.
- **M6 — Era 3, electricity.** Cable networks + electric solver, brownouts, batteries,
  turbine/generator/arc furnace/electrolysis/circuit assembler, upgrade modules, ore scanner,
  vent fan, electric pump, gas hazard.
- **M7 — Era 4, chemistry.** Chemical plant, alloy furnace, machine shop, rare-earth separator,
  decon shower, heat + cave-ins, suits and Geiger counter, Tungsten-Carbide Drill.
- **M8 — Era 5, nuclear.** Leach → precipitate → convert → centrifuge cascade → fabricate,
  control rods, reactor multi-block with scram/overheat, cooling pool, waste vault, Nuclear
  Drill with 3×3 mining, radiation everywhere.
- **M9 — Persistence and polish.** Region files + SQLite, autosave, crash-safe writes, audio,
  particles, menus and settings, objective list, balance pass.
- **M10 — Accounts and server browser.** Lobby service, argon2 accounts, tokens, heartbeats,
  Dockerfiles.

## 6. Testing strategy

- Pure unit tests for every rule in `shared` (palette round-trips, worldgen determinism,
  break-time maths, inventory moves incl. `proptest`, recipe matching, power/steam solving,
  radiation maths).
- Headless `App` tests that build the server plugins without a window, feed scripted intents,
  step the schedule and assert on outcomes.
- One progression integration test per era; the nuclear chain test is the most important.
- Network convergence tests with simulated latency in one process.

## 7. Risk log

| Risk | Mitigation |
|---|---|
| Bevy 0.20 API differs from memory | Read docs.rs/examples for 0.20.1 before each new subsystem; compile constantly |
| Sandbox has no GPU | Headless server is the testable path; client verified by compiling + `--headless` smoke test |
| Replicon 0.45 API changed | Read its examples early in M2, before building on it |
| Content volume (100+ recipes) | Generate the RON from a single validated source and check with the validator |
