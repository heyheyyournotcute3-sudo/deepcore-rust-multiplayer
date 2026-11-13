# Deepcore

A co-op, first-person, voxel mining game in Rust, built on Bevy. Dig down through six layers of
rock, haul ore home, build machines, and climb a six-era tech ladder until you can run a nuclear
reactor and bore 3×3 tunnels with a Nuclear Drill.

Everything is procedural: the world is generated from a seed and every block texture and sound is
generated in code. A fresh clone builds and runs with **no downloaded assets**.

## Quick start

```bash
cargo run -p server                    # headless authoritative server, 30 Hz, UDP :7777
cargo run -p client                    # windowed client, plays the local world
cargo run -p client -- --connect 127.0.0.1 --name Alice   # join a server
cargo run -p server -- --bots 8        # server with 8 simulated players
cargo run -p lobby                     # accounts + server browser, HTTP :8080
./scripts/check.sh                     # format, lint, tests, content validation
```

## Controls

| Input | Action |
|---|---|
| `WASD` | Move |
| `Shift` | Sprint |
| `Ctrl` | Crouch |
| `Space` | Jump |
| Left click | Mine (hold) |
| Right click | Place the selected block or machine |
| `1`–`9` | Select hotbar slot |
| `E` / `Tab` | Inventory |
| `C` | Workbench crafting |
| `T` | Tech tree |
| `F3` | Debug stats |
| `Esc` | Release mouse / close screens |

## What is implemented

### World and simulation

- **1024 × 1024 × 512 mine** with six layers, an unbreakable bedrock floor and walls, generated
  deterministically from a seed: 3D-noise caves, ore veins clustered in blobs, water and methane
  pockets, trees on the surface.
- **Palette-compressed chunks** (adaptive 1/2/4/8/16-bit indices) and a **greedy mesher** that
  merges a solid chunk to six quads (the renderer uses a per-face mode so an atlas can tile
  correctly).
- **Deterministic AABB-vs-voxel movement** shared verbatim by client and server, so prediction
  matches authority.
- **Mining** with per-block hardness and tool-tier gating, break times rounded to server ticks,
  3×3 area mining for the Nuclear Drill, and Amanatides-Woo voxel raycasting for targeting and
  line-of-sight checks.
- **Procedural texture atlas**: 16×16 tiles per block generated from `color` + `pattern`
  (speckle, ore flecks, strata, brick courses, wood grain, glow, riveted plate).

### Multiplayer

- Server-authoritative **bevy_replicon + renet** transport. The handshake checks the protocol
  version and a SHA-256 **content hash**, then authorizes the client; a client with stale content
  is refused with "Content mismatch".
- **Chunk streaming** with interest management centred on each player, nearest-first with a
  per-tick budget, and clean-chunk unloading to bound server memory.
- **Client intents** validated server-side: reach, line of sight, tool tier, item presence,
  machine ownership. Invalid intents are dropped and logged, never applied.
- Private **inventory sync** to the owner only; coarse machine state replicated, full slots sent
  only to the client with that machine open.

### Content and progression

- **98 blocks, 180 items, 37 machines, 153 recipes, 32 tech nodes, 6 layers**, all in RON and
  validated at load: dangling references, duplicate IDs, tech cycles, missing parents, recipes
  naming machines that cannot run them, and dead-end ores all fail fast with a readable message.
- The full **nuclear chain** works: crush → leach → precipitate → convert → 4-to-16-centrifuge
  cascade → fabricate fuel rods and core cells → control rods → reactor → waste casks.
- **Team research**: deposit items at a node; parents gate children; the tree replicates to all.
- Machines run the shared state machine: fuel burning, catalysts (flux), output-full and no-power
  stalls, upgrade modules, pinned recipes, centrifuge cascades, and reactor heat/coolant/scram/
  meltdown.

### Persistence

- **Region files** (8×8×8 chunks) with a versioned header and zstd-compressed payloads.
- **SQLite** for players, machines and research.
- Crash-safe **atomic writes** (temp file → fsync → rename), autosave every five minutes and on
  clean shutdown, and a restart restores chunks, machines, research and each player's position,
  health and inventory.

### Lobby

- axum + sqlx service: register, log in, validate tokens, list servers, register servers,
  heartbeats.
- **argon2** password hashing with per-hash salts; session tokens are 32 random bytes stored only
  as SHA-256 hashes; login is rate limited.
- Dockerfiles for the dedicated server and the lobby, plus a compose file.

## Verification

`./scripts/check.sh` runs formatting, `clippy -D warnings`, and **149 tests**:

- **Unit**: palette round-trips, mesher merging and cross-chunk culling, worldgen determinism,
  movement and collision, break-time maths, raycasting, inventory transfers (with `proptest`
  invariants), recipe matching, machine state machines, power/steam solving with brownouts,
  hazard and reactor formulas, atlas UV mapping, WAV generation, lobby password/token handling.
- **Progression integration** (`crates/server/tests/progression.rs`), one per era, all driving the
  real state machines. The nuclear one runs raw uranium through every structure to a working
  Nuclear Drill and a stable reactor.
- **Content invariants**: every tool and machine is craftable, every machine is tech-gated,
  recipes fit their machines' slots, every era's cost is reachable.

Verified by running it, not just testing it:

- server + two real clients: both admitted, players spawned, chunks decoded, no errors;
- `--bots 8` for 45 s: no panics, blocks mined, chunks loaded;
- save → restart → load across two real server processes;
- lobby exercised end to end with `curl`;
- headless screenshot (`xvfb-run`) confirming terrain, textures, HUD and hotbar render.

## Layout

```
crates/
  shared/    every rule: content, voxels, worldgen, movement, mining, inventory,
             crafting, power, hazards, protocol
  server/    headless authority: net, world, sim, machines, research, loot, persist, bots
  client/    windowed client: render, input, hud, ui, audio, prediction, net
  lobby/     axum + sqlx accounts and server browser
tools/atlas_gen/   builds the atlas PNG from blocks.ron
assets/data/       the RON content files — the single source of truth
```

`shared` holds every rule, `server` decides, `client` shows.

## Adding content

1. Add a block to `blocks.ron` (its `color` and `pattern` are its texture).
2. Add the item it drops to `items.ron`.
3. Add a recipe to `recipes.ron`.
4. Reference the block from a layer's `ores` in `layers.ron`.
5. Validate:

```bash
cargo run -p deepcore-shared --bin balance_report
```

It fails with a readable message on any broken reference and prints the raw material demand per
era so pacing can be checked on paper. Changing content changes the hash, so clients with old
content are refused — that is intentional.

## Requirements

- Rust 1.88+ (edition 2024; developed on 1.99)
- Linux: `libasound2-dev libudev-dev libx11-dev libxkbcommon-dev libwayland-dev`
- The server and the lobby need no GPU and no display.

## Docker

```bash
docker compose -f docker/docker-compose.yml up --build
```

## Known limitations

See [`PROGRESS.md`](PROGRESS.md) for the current state and the honest list of what is not done
yet (latency/loss injection, machine models on the client, pickups drawn in the world, a server
browser UI inside the client).
