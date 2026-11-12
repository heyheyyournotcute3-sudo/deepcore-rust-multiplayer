# Deepcore — Progress

Continuation notes. Read this file, then `docs/PLAN.md`.

## Current state

**M0–M2 complete. M3 (inventory/crafting UI) and the M4 UI surfaces are in. Machines as
placeable block entities, and the deeper era content, are next.**

### Working and verified

- **Workspace**: 4 crates + `atlas_gen` tool. `./scripts/check.sh` passes (fmt, clippy
  `-D warnings`, 104 tests, content validation).
- **Server**: headless 30 Hz fixed-timestep app, tick-health logging, CLI flags
  (`--port --seed --simulate-latency --simulate-loss --bots --exit-after`).
- **Client**: windowed, renders the generated world with procedural textures, streams chunks
  around the player, first-person controller with collision, block targeting with outline,
  mining with tier gating and 3×3 area mining, HUD (crosshair, depth meter + layer name,
  "Too hard — needs Tier N", F3 stats), egui help overlay, headlamp + sun lighting that fades
  with depth.
- **Content**: 98 blocks, 180 items, 37 machines, 153 recipes, 32 tech nodes, 6 layers, all
  validated. `balance_report` prints per-era raw material demand.
- **Shared rules** (all unit-tested): palette chunks, greedy mesher, deterministic worldgen,
  AABB movement, break-time/tier/raycast, inventory (+proptest), machine state machine,
  power/steam solving with brownouts and batteries, hazards (radiation/heat/oxygen),
  reactor core with scram and meltdown, protocol types.
- **Verified visually** via headless screenshot (xvfb + software Vulkan): terrain, trees,
  speckled block textures, HUD all render correctly.
- **Multiplayer (M2) verified with two real clients against a real server**:
  handshake (protocol + content hash) accepted, players spawned server-side, chunks
  streamed and decoded with zero errors, replication of transforms registered.
- **UI (M3/M4 surfaces)**: inventory screen with click-to-pick-up / drop / merge / swap,
  always-on hotbar with number-key selection, workbench hand-crafting panel, tech tree
  screen, machine panel (slots, idle reason, progress, reactor readout). Mouse capture is
  released while a screen is open. Verified by screenshot.
- Inventory is pushed from the server to its owner as a private message on change; the
  hotbar's selected item is what the client uses as its tool.

### Next up

1. **M4 machines in the world**: block entities as ECS entities at a `BlockPos`, multi-block
   ghost placement with rotation, server machine ticking (`shared::crafting::tick_machine` is
   ready and tested), machine contents sent to the client with the UI open.
2. **Research**: Research Desk deposits feeding `shared` tech nodes; replicate tech state.
3. Item pickups in the world + death crate + respawn.
4. Bot clients (`--bots`) and latency/loss simulation (flags parsed, injection not wired).

## Environment facts (verified, not assumed)

- Rust `1.99.1`, edition 2024.
- crates.io needs a `User-Agent` through this sandbox's proxy; plain `cargo` works.
- Bevy latest stable is **0.20.1** (prompt assumed 0.19). Verified compatible set:
  `bevy 0.20.1`, `bevy_replicon 0.45.5`, `bevy_replicon_renet 0.21.0`, `bevy_egui 0.43.1`
  (egui 0.36).
- **Deviation:** `rusqlite` + `sqlx` cannot coexist (both declare `links = "sqlite3"`).
  Using `sqlx` for server and lobby.
- No GPU, but `xvfb-run` + Mesa llvmpipe works, so the client can be run and screenshotted.

### Bevy 0.20 API notes (differ from older versions)

- Buffered events are `Message` + `MessageWriter`/`MessageReader` (not `Event`).
- Observers take `On<E>`, not `Trigger<E>`.
- `Mesh::new(topology, usages).with_inserted_attribute(..)`; `Indices::U32`.
- `TextFont { font_size: FontSize::Px(..) }`.
- `DirectionalLight { shadow_maps_enabled }`, `PointLight`, `GlobalAmbientLight { brightness }`.
- Screenshots: `commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path))`.
- bevy_egui: UI systems must run in `EguiPrimaryContextPass`, not `Update`.

## Build / run / verify

```bash
cargo build --workspace
cargo run -p server
cargo run -p client
cargo run -p deepcore-shared --bin balance_report
cargo run -p atlas_gen
./scripts/check.sh

# headless visual check (no display needed)
xvfb-run -a ./target/debug/client --exit-after-frames 25 --view-radius 2 --screenshot shot.png
```

Note: software rendering runs at ~3 FPS, so keep `--view-radius` small for smoke tests.

## Known issues / limitations

- Mining is validated server-side, but the client still applies the break locally first, so a
  rejected break briefly shows then reverts when the block-change arrives. Needs prediction
  polish.
- The workbench crafting panel runs the recipe locally and does not yet send a `Craft`
  intent, so in multiplayer the server does not re-validate hand crafting.
- The tech tree screen shows the full tree from content, but research deposits are not yet
  wired to the server, so nothing unlocks in play yet.
- The machine panel exists but nothing spawns machines in the world yet.
- `--simulate-latency` / `--simulate-loss` / `--bots` are parsed but not yet wired to renet.
- Block textures use a per-face (non-merged) mesh because an atlas cannot tile inside a merged
  quad. `greedy_mesh_with(.., merge = true)` is available for per-block materials later.
- `client::net` and parts of `client::prediction` are scaffolding with `#[allow(dead_code)]`
  until the transport is wired; both are covered by unit tests.
- Inventory/hotbar UI is not drawn yet (rules are complete and tested).
