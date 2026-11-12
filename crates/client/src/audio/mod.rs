//! Audio: dig, place, machine hum and alarms.
//!
//! All sounds are synthesised at startup, so the game needs no audio assets. This plugin sets up
//! the sound bank; the systems that trigger sounds land with the polish milestone.

use bevy::prelude::*;

/// Client audio plugin.
pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, _app: &mut App) {}
}
