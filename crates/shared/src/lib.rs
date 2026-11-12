//! Deepcore shared game logic.
//!
//! This crate holds every rule of the game: content registries, voxel storage, world
//! generation, movement and collision, inventory, crafting, power, hazards and the network
//! protocol types. It contains no rendering and no socket code, so the same code runs on the
//! authoritative server and inside the client.

use bevy::prelude::*;

/// Tick rate of the authoritative simulation, in Hz.
pub const SERVER_TICK_HZ: u32 = 30;

/// Length of one simulation tick, in seconds.
pub const TICK_SECONDS: f32 = 1.0 / SERVER_TICK_HZ as f32;

/// Version of the wire protocol. Bump whenever replicated components or messages change shape.
pub const PROTOCOL_VERSION: u32 = 1;

/// Registers the shared resources and systems that both the server and the client need.
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(SERVER_TICK_HZ as f64));
    }
}

/// Returns the fixed simulation timestep in seconds.
#[must_use]
pub fn tick_seconds() -> f32 {
    TICK_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_matches_rate() {
        assert!((tick_seconds() * SERVER_TICK_HZ as f32 - 1.0).abs() < 1e-6);
    }
}
