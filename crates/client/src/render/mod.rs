//! Rendering: chunk meshes, the procedural atlas, lighting and the world stream.

pub mod chunks;
pub mod lighting;
pub mod world_stream;

use bevy::prelude::*;

/// Everything the client needs to draw the voxel world.
pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            chunks::ChunkRenderPlugin,
            world_stream::WorldStreamPlugin,
            lighting::LightingPlugin,
        ))
        .init_resource::<chunks::ChunkStats>()
        .add_systems(Update, chunks::update_chunk_stats);
    }
}
