# Deepcore — Progress

Continuation notes for future sessions. Read this file, then `docs/PLAN.md`.

## Current state

**Milestone in progress: M0 (workspace and skeleton).**

## Environment facts (verified, not assumed)

- Rust: `rustc 1.99.1` / `cargo 1.99.1`, edition 2024.
- crates.io needs a `User-Agent` header through this sandbox's proxy; plain `cargo` works.
- Bevy's latest stable is **0.20.1** (not 0.19 as the prompt assumed). Verified compatibility:
  `bevy_replicon 0.45.5` (needs bevy ^0.20), `bevy_replicon_renet 0.21.0`,
  `bevy_egui 0.43.1` (needs bevy ^0.20.0, egui ^0.36).
- **Deviation:** `rusqlite` and `sqlx` cannot coexist in one Cargo graph (both declare
  `links = "sqlite3"`). Using `sqlx` for server *and* lobby.
- Sandbox has no GPU/display; the client is verified by compiling plus a headless smoke test.

## Build / run commands

```bash
cargo build --workspace
cargo run -p server                       # headless authoritative server
cargo run -p client                       # windowed client
./scripts/check.sh                        # fmt + clippy + tests + content validation
```

## Known issues

- None yet.
