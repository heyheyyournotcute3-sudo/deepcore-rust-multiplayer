# Deepcore — Progress

Continuation notes. Read this file, then `docs/PLAN.md`.

## Current state

**M0 complete. M1 complete (single-process voxel world). Shared rule layer for M3–M8 is
written and unit-tested; transport/UI/persistence are next.**

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

### Next up

1. **M2 multiplayer**: wire `bevy_replicon` + `bevy_replicon_renet`. Protocol types are ready
   in `shared::protocol`; `client::prediction` holds the buffer half of reconciliation.
2. **M3 inventory/crafting UI** in egui (rules exist in `shared::inventory`/`crafting`).
3. **M4 machines**: block entities + machine UI (rules exist in `shared::crafting`).
4. Server-side chunk store, mining validation, block-change broadcast.

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

- Client generates its own world from the seed; it does not yet receive chunks from a server
  (that is the M2 transport work).
- Mining and placing are client-local only; server validation lands with M2.
- Block textures use a per-face (non-merged) mesh because an atlas cannot tile inside a merged
  quad. `greedy_mesh_with(.., merge = true)` is available for per-block materials later.
- `client::net` and parts of `client::prediction` are scaffolding with `#[allow(dead_code)]`
  until the transport is wired; both are covered by unit tests.
- Inventory/hotbar UI is not drawn yet (rules are complete and tested).
