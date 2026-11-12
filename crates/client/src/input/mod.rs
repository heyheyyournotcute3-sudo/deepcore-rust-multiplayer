//! Player input: first-person controls, block targeting and hotbar selection.

pub mod controller;
pub mod mining;

use bevy::prelude::*;

/// All client input systems.
pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((controller::PlayerControllerPlugin, mining::MiningPlugin));
    }
}
