//! Client world: streams chunks around the player and keeps them meshed.
//!
//! In single-process play the client generates chunks itself from the shared world seed using
//! the same deterministic generator the server uses. In multiplayer the server sends chunk data
//! over the wire and this plugin loads it instead; both paths end in [`ClientChunks`].

use bevy::log::debug;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use deepcore_shared::{
    content::ContentRegistry,
    voxel::{ChunkPos, coords::BlockPos},
    worldgen::WorldGen,
};

use super::chunks::{ChunkVisual, ClientChunks};

/// Plugin that owns the client's chunk streaming.
pub struct WorldStreamPlugin;

impl Plugin for WorldStreamPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldStreamer>()
            .add_systems(Startup, init_worldgen)
            .add_systems(Update, stream_chunks);
    }
}

/// The client's local world generator and streaming state.
#[derive(Resource, Default)]
pub struct WorldStreamer {
    /// The generator, built from the world seed and content.
    pub generator: Option<WorldGen>,
    /// Where the player is, in blocks, used to decide what to stream.
    pub focus: [f32; 3],
    /// Chunks we have already asked for.
    requested: HashSet<ChunkPos>,
    /// Set when running in single-process mode where the client generates its own chunks.
    pub local_generation: bool,
    /// Horizontal view radius in chunks.
    pub radius_xz: i32,
    /// Vertical view radius in chunks.
    pub radius_y: i32,
}

fn init_worldgen(
    mut streamer: ResMut<WorldStreamer>,
    registry: Res<ContentRegistry>,
    args: Res<crate::ClientArgs>,
    server: Option<Res<crate::net::ServerConfigRes>>,
) {
    streamer.radius_xz = args.view_radius.clamp(1, 12);
    streamer.radius_y = (args.view_radius / 2).clamp(1, 8);
    let seed = server.map_or(1337, |s| s.seed);
    match WorldGen::new(&registry, seed) {
        Ok(generator) => {
            info!(seed, "client world generator ready");
            streamer.generator = Some(generator);
            streamer.local_generation = true;
        }
        Err(e) => {
            error!("failed to build client world generator: {e}");
        }
    }
}

/// Streams chunks in a radius around the player, nearest first, and unloads far ones.
fn stream_chunks(
    mut chunks: ResMut<ClientChunks>,
    mut streamer: ResMut<WorldStreamer>,
    mut commands: Commands,
    visuals: Query<(Entity, &ChunkVisual)>,
    camera: Query<&Transform, With<Camera3d>>,
    mut budget: Local<u32>,
) {
    let Some(generator) = streamer.generator.clone() else {
        return;
    };
    let Some(cam) = camera.iter().next() else {
        return;
    };
    streamer.focus = cam.translation.to_array();
    debug!(camera = ?cam.translation, "streaming pass");
    let centre = BlockPos::new(
        cam.translation.x.floor() as i32,
        cam.translation.y.floor() as i32,
        cam.translation.z.floor() as i32,
    )
    .chunk();

    // Limit generation per frame so streaming never stalls a frame.
    *budget = budget.saturating_add(2).min(24);
    let mut generated = 0;

    // Build the list of chunks we want, sorted nearest first.
    let mut wanted: Vec<(i32, ChunkPos)> = Vec::new();
    let radius_xz = streamer.radius_xz;
    let radius_y = streamer.radius_y;
    for dy in -radius_y..=radius_y {
        for dz in -radius_xz..=radius_xz {
            for dx in -radius_xz..=radius_xz {
                // Skip the corners of the view box: a cube of chunks is mostly wasted work.
                if dx.abs() == radius_xz && dz.abs() == radius_xz {
                    continue;
                }
                let pos = ChunkPos::new(centre.x + dx, centre.y + dy, centre.z + dz);
                if chunks.chunks.contains_key(&pos) {
                    continue;
                }
                let dist = dx * dx + dy * dy * 2 + dz * dz;
                wanted.push((dist, pos));
            }
        }
    }
    wanted.sort_by_key(|(d, _)| *d);

    for (_, pos) in wanted {
        if generated >= *budget {
            break;
        }
        // Generate on the spot for now; the async mesher handles the expensive part.
        let chunk = generator.generate_chunk(pos);
        chunks.insert(chunk);
        streamer.requested.insert(pos);
        generated += 1;
    }
    if generated > 0 {
        *budget -= generated;
    }

    // Unload chunks that fell outside the radius.
    let far: Vec<ChunkPos> = chunks
        .chunks
        .keys()
        .copied()
        .filter(|p| p.chebyshev_distance(&centre) > radius_xz + 1)
        .collect();
    for pos in far {
        if let Some(entity) = chunks.remove(&pos) {
            commands.entity(entity).despawn();
        }
        streamer.requested.remove(&pos);
    }
    // The visuals query is kept so this system runs after meshing is set up.
    let _ = visuals;
}
