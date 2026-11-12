//! Client networking: connection to a dedicated server or a locally hosted one.
//!
//! This module holds the connection configuration and the handshake values the client sends.
//! The transport wiring lives in the multiplayer milestone; the shared types it will use are
//! already defined in `deepcore_shared::protocol`.

use bevy::prelude::*;

/// Where the client should connect, if anywhere.
///
/// Several fields (address, port, name, latency/loss simulation, hosting flag) are read by the
/// transport layer when the multiplayer milestone wires it up; they are part of the public
/// configuration surface today.
#[allow(dead_code)]
#[derive(Debug, Clone, Resource)]
pub struct ServerConfigRes {
    /// Server address.
    pub address: String,
    /// Server port.
    pub port: u16,
    /// The world seed the server uses, so the client can predict generation.
    pub seed: u64,
    /// The player's display name.
    pub name: String,
    /// Simulated one-way latency in milliseconds.
    pub simulate_latency: u32,
    /// Simulated packet loss in percent.
    pub simulate_loss: u8,
    /// Whether the client is hosting the server in-process.
    pub hosting: bool,
}

impl Default for ServerConfigRes {
    fn default() -> Self {
        Self {
            address: "127.0.0.1".to_string(),
            port: 7777,
            seed: 1337,
            name: "Miner".to_string(),
            simulate_latency: 0,
            simulate_loss: 0,
            hosting: true,
        }
    }
}

/// Client networking plugin.
pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, _app: &mut App) {}
}
