# Deepcore

A co-op, first-person, voxel mining game in Rust, built on Bevy. Dig down through six layers of
rock, haul ore home, build machines, and climb a six-era tech ladder until you can run a nuclear
reactor and bore 3×3 tunnels with a Nuclear Drill.

Everything is procedural: the world is generated from a seed, and every block texture is drawn
in code. A fresh clone builds and runs with **no downloaded assets**.

## Quick start

```bash
# headless authoritative server (30 Hz simulation)
cargo run -p server

# windowed client, playing the local world
cargo run -p client

# the full quality gate: format, lint, tests, content validation
./scripts/check.sh
```

## What works today

| Area | Status |
|---|---|
| Workspace, pinned Bevy 0.20.1 stack | done |
| Headless server at 30 Hz | done |
| Content pipeline: 98 blocks, 180 items, 37 machines, 153 recipes, 32 tech nodes, 6 layers | done, validated |
| Palette-compressed chunks, greedy mesher | done |
| Deterministic world generation (layers, caves, ore veins, fluid pockets, trees) | done |
| AABB-vs-voxel movement shared by client and server | done |
| Procedural texture atlas, chunk streaming and rendering | done |
| Mining with tier gating, 3×3 area mining, block targeting | done |
| HUD: crosshair, depth meter, "too hard — needs Tier N" warning, F3 stats | done |
| Inventory, crafting, power, hazards, reactor rules | logic done and unit-tested |
| Multiplayer transport, machine UIs, tech tree UI, persistence, lobby | not yet |

See [`PROGRESS.md`](PROGRESS.md) for the milestone-by-milestone state and known issues, and
[`docs/PLAN.md`](docs/PLAN.md) for the architecture and the design in full.

## Controls

| Input | Action |
|---|---|
| `WASD` | Move |
| `Shift` | Sprint |
| `Ctrl` | Crouch |
| `Space` | Jump |
| Left click | Mine the targeted block |
| `Esc` | Release the mouse |
| `F3` | Debug stats (chunks, position, content hash) |

Click the window to capture the mouse.

## How the code is laid out

```
crates/
  shared/    every rule of the game: content, voxels, worldgen, movement,
             mining, inventory, crafting, power, hazards, protocol
  server/    headless authoritative server
  client/    windowed game client (render, input, hud, ui, prediction, net)
  lobby/     axum + sqlx accounts and server browser service
tools/
  atlas_gen/ builds the procedural texture atlas PNG from blocks.ron
assets/data/ the RON content files — the single source of truth for content
```

The rule that keeps it sane: **`shared` holds every rule, `server` decides, `client` shows.**

## Adding content

All content lives in `assets/data/*.ron` and is validated at startup. Adding an ore, an item, a
recipe, a machine or a tech node never requires touching Rust:

1. Add the block to `blocks.ron` (give it a `color` and a `pattern`; that is its texture).
2. Add the item it drops to `items.ron`.
3. Add a recipe to `recipes.ron` that consumes it.
4. Reference the block from a layer's `ores` list in `layers.ron`.
5. Run the validator:

```bash
cargo run -p deepcore-shared --bin balance_report
```

The validator fails with a readable message on: unknown item/block/machine/tech references,
duplicate IDs, recipes naming a machine that does not exist, tech nodes with missing parents or
cycles, and ore blocks whose drops nothing consumes (dead-end ores). It also prints the raw
material demand per era so you can check pacing on paper.

If you change any content file, the content hash changes and clients with old content are
rejected at the handshake — that is intentional.

## Regenerating the texture atlas

The client builds the atlas in memory at startup. To inspect it as a PNG:

```bash
cargo run -p atlas_gen     # writes assets/textures/atlas.png
```

## Requirements

- Rust 1.88+ (edition 2024; developed on 1.99)
- Linux: `libasound2-dev libudev-dev libx11-dev libxkbcommon-dev libwayland-dev`
- The server needs no GPU and no display.

## Testing

```bash
cargo test --workspace          # unit + property tests
cargo clippy --workspace --all-targets -- -D warnings
```

The shared crate carries the bulk of the tests: palette round-trips, mesher merging, worldgen
determinism, movement and collision, break-time maths, raycasting, inventory transfers (including
`proptest` invariants), machine state machines, power/steam solving with brownouts, and the
hazard and reactor formulas.

## Headless smoke test

The client can be run without a display (useful in CI) and will render, screenshot and exit:

```bash
xvfb-run -a ./target/debug/client --exit-after-frames 25 --view-radius 2 --screenshot shot.png
```
