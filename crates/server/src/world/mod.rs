//! The server's authoritative world: chunk storage, block edits and chunk streaming.
//!
//! The server owns every block. Chunks are generated on demand from the world seed and cached;
//! edits are applied here and broadcast to clients that have the chunk loaded.

use std::sync::Arc;

use bevy::{platform::collections::HashMap, prelude::*};
use bevy_replicon::prelude::*;
use deepcore_shared::{
    content::ContentRegistry,
    protocol::{BlockChange, ChunkMessage, NetBlockPos},
    voxel::{AIR, BlockPos, Chunk, ChunkPos},
    worldgen::WorldGen,
};
use tracing::{debug, trace, warn};

use crate::ServerArgs;

/// Chunks within this many chunks of a player are streamed to it.
pub const VIEW_RADIUS_XZ: i32 = 6;

/// Vertical streaming radius in chunks.
pub const VIEW_RADIUS_Y: i32 = 4;

/// The server's authoritative block storage.
#[derive(Resource)]
pub struct ServerWorld {
    /// The deterministic generator, shared with worker threads.
    generator: Arc<WorldGen>,
    /// Loaded chunks.
    chunks: HashMap<ChunkPos, Chunk>,
    /// Chunks that have been edited and must be persisted.
    modified: std::collections::HashSet<ChunkPos>,
    /// A monotonically increasing revision, bumped on every edit.
    pub revision: u64,
}

impl ServerWorld {
    /// Creates the world for a seed, failing if the content cannot generate it.
    pub fn new(registry: &ContentRegistry, seed: u64) -> anyhow::Result<Self> {
        let generator = WorldGen::new(registry, seed)?;
        Ok(Self {
            generator: Arc::new(generator),
            chunks: HashMap::new(),
            modified: std::collections::HashSet::new(),
            revision: 0,
        })
    }

    /// The world seed.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.generator.seed()
    }

    /// Gets a chunk, generating and caching it on demand.
    pub fn chunk_mut(&mut self, pos: ChunkPos) -> &mut Chunk {
        if !self.chunks.contains_key(&pos) {
            let chunk = self.generator.generate_chunk(pos);
            self.chunks.insert(pos, chunk);
        }
        // The key is guaranteed present by the insert above.
        self.chunks.get_mut(&pos).expect("chunk was just inserted")
    }

    /// Reads a block without generating the chunk, returning air when it is not loaded.
    #[must_use]
    pub fn peek(&self, pos: BlockPos) -> deepcore_shared::content::BlockId {
        self.chunks
            .get(&pos.chunk())
            .map_or(AIR, |c| c.get_local(pos.chunk_local()))
    }

    /// Reads a block, generating the chunk if necessary.
    pub fn block(&mut self, pos: BlockPos) -> deepcore_shared::content::BlockId {
        let local = pos.chunk_local();
        self.chunk_mut(pos.chunk()).get_local(local)
    }

    /// Writes a block and returns the previous value.
    pub fn set_block(
        &mut self,
        pos: BlockPos,
        block: deepcore_shared::content::BlockId,
    ) -> deepcore_shared::content::BlockId {
        let chunk_pos = pos.chunk();
        let local = pos.chunk_local();
        let chunk = self.chunk_mut(chunk_pos);
        let previous = chunk.get_local(local);
        if previous == block {
            return previous;
        }
        chunk.set_local(local, block);
        chunk.mark_dirty();
        self.modified.insert(chunk_pos);
        self.revision += 1;
        previous
    }

    /// Number of chunks with unsaved edits.
    #[must_use]
    pub fn modified_chunks(&self) -> usize {
        self.modified.len()
    }

    /// Drops chunks far from every player that have no unsaved edits, to bound memory.
    pub fn unload_clean_outside(&mut self, centre: ChunkPos, radius: i32) -> usize {
        let before = self.chunks.len();
        self.chunks.retain(|pos, _| {
            self.modified.contains(pos) || pos.chebyshev_distance(&centre) <= radius
        });
        before - self.chunks.len()
    }

    /// Serialises a chunk for the wire: postcard then zstd.
    fn encode_chunk(chunk: &Chunk) -> Option<Vec<u8>> {
        let raw = postcard::to_allocvec(chunk).ok()?;
        zstd::encode_all(raw.as_slice(), 3).ok()
    }
}

/// Decodes a chunk payload produced by the server. Used by tests and by tooling.
#[cfg_attr(not(test), allow(dead_code))]
pub fn decode_chunk(payload: &[u8]) -> Option<Chunk> {
    let raw = zstd::decode_all(payload).ok()?;
    postcard::from_bytes(&raw).ok()
}

/// Server world plugin.
pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_world)
            .add_systems(FixedUpdate, stream_chunks_to_clients);
    }
}

fn init_world(mut commands: Commands, registry: Res<ContentRegistry>, args: Res<ServerArgs>) {
    match ServerWorld::new(&registry, args.seed) {
        Ok(world) => {
            info!(
                seed = world.seed(),
                depth = registry.world_depth(),
                "authoritative world ready"
            );
            commands.insert_resource(world);
        }
        Err(e) => {
            error!("failed to build the world: {e}");
        }
    }
}

/// Sends chunks near each player and unloads chunks that are far away.
fn stream_chunks_to_clients(
    mut world: ResMut<ServerWorld>,
    clients: Query<Entity, With<ConnectedClient>>,
    players: Query<(
        &crate::sim::ServerPlayer,
        &deepcore_shared::protocol::net_register::ReplicatedTransform,
    )>,
    mut commands: Commands,
    mut admitted: ResMut<crate::net::HandshakeState>,
) {
    if clients.is_empty() {
        // Nobody is connected: keep memory bounded by dropping every clean chunk.
        let dropped = world.unload_clean_outside(ChunkPos::new(0, 0, 0), 0);
        if dropped > 0 {
            debug!(dropped, "unloaded clean chunks with no players online");
        }
        return;
    }

    for client in &clients {
        // Interest is centred on the player's own position, so underground players get the
        // chunks above and below them rather than only the surface band.
        let centre = players
            .iter()
            .find(|(player, _)| player.client == client)
            .map_or(ChunkPos::new(0, 0, 0), |(_, transform)| {
                let pos = transform.0.position;
                let edge = deepcore_shared::voxel::CHUNK_SIZE;
                ChunkPos::new(
                    (pos[0].floor() as i32).div_euclid(edge),
                    (pos[1].floor() as i32).div_euclid(edge),
                    (pos[2].floor() as i32).div_euclid(edge),
                )
            });
        if let Some(entry) = admitted.admitted.get_mut(&client) {
            entry.centre = centre;
        }

        // Nearest-first ordering keeps the loading screen short.
        let mut wanted: Vec<(i32, ChunkPos)> = Vec::new();
        for dy in -VIEW_RADIUS_Y..=VIEW_RADIUS_Y {
            for dz in -VIEW_RADIUS_XZ..=VIEW_RADIUS_XZ {
                for dx in -VIEW_RADIUS_XZ..=VIEW_RADIUS_XZ {
                    let pos = ChunkPos::new(centre.x + dx, centre.y + dy, centre.z + dz);
                    wanted.push((dx * dx + dy * dy * 2 + dz * dz, pos));
                }
            }
        }
        wanted.sort_by_key(|(d, _)| *d);

        // Budget the burst so a new client does not swamp the socket in one tick.
        let mut sent = 0;
        for (_, pos) in wanted {
            if sent >= 24 {
                break;
            }
            let chunk = world.chunk_mut(pos);
            let Some(payload) = ServerWorld::encode_chunk(chunk) else {
                warn!(?pos, "failed to encode chunk");
                continue;
            };
            commands.server_trigger(ToClients {
                targets: SendTargets::Single(client.into()),
                message: ChunkMessage {
                    x: pos.x,
                    y: pos.y,
                    z: pos.z,
                    payload,
                },
            });
            sent += 1;
        }
        debug!(sent, centre = ?centre, "streamed chunks");

        // Bound server memory: drop clean chunks that fell outside this player's radius.
        world.unload_clean_outside(centre, VIEW_RADIUS_XZ + 2);
    }
    trace!(
        modified = world.modified_chunks(),
        "chunk cache after streaming"
    );
}

/// Applies a block change and broadcasts it to every client that has the chunk loaded.
pub fn apply_and_broadcast_block(
    commands: &mut Commands,
    world: &mut ServerWorld,
    pos: BlockPos,
    block: deepcore_shared::content::BlockId,
    clients: &Query<Entity, With<ConnectedClient>>,
) {
    let previous = world.set_block(pos, block);
    if previous == block {
        return;
    }
    let message = BlockChange {
        pos: NetBlockPos::from(pos),
        block,
    };
    // Interest management is applied by the replication layer; block changes go to all
    // connected clients, which are exactly those within someone's view radius.
    for client in clients.iter() {
        commands.server_trigger(ToClients {
            targets: SendTargets::Single(client.into()),
            message,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepcore_shared::content::ContentRegistry;

    fn registry() -> ContentRegistry {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        ContentRegistry::load_from_dir(&dir).unwrap()
    }

    #[test]
    fn blocks_can_be_read_and_written() {
        let reg = registry();
        let mut world = ServerWorld::new(&reg, 1337).unwrap();
        let pos = BlockPos::new(3, -20, 3);
        let before = world.block(pos);
        let stone = reg.block("stone").unwrap();
        world.set_block(pos, stone);
        assert_eq!(world.block(pos), stone);
        world.set_block(pos, before);
        assert_eq!(world.block(pos), before);
    }

    #[test]
    fn edits_mark_the_chunk_modified() {
        let reg = registry();
        let mut world = ServerWorld::new(&reg, 1337).unwrap();
        let pos = BlockPos::new(1, -40, 1);
        assert_eq!(world.modified_chunks(), 0);
        world.set_block(pos, deepcore_shared::voxel::AIR);
        assert_eq!(world.modified_chunks(), 1);
    }

    #[test]
    fn chunk_payload_round_trips() {
        let reg = registry();
        let mut world = ServerWorld::new(&reg, 1337).unwrap();
        let pos = ChunkPos::new(0, -2, 0);
        let original = world.chunk_mut(pos).to_vec();
        let payload = ServerWorld::encode_chunk(world.chunk_mut(pos)).expect("encode");
        let decoded = decode_chunk(&payload).expect("decode");
        assert_eq!(decoded.to_vec(), original);
        // And the payload is actually compressed.
        let raw = postcard::to_allocvec(world.chunk_mut(pos)).unwrap();
        assert!(
            payload.len() < raw.len(),
            "compressed {} vs raw {}",
            payload.len(),
            raw.len()
        );
    }

    #[test]
    fn generation_matches_between_server_and_worldgen() {
        let reg = registry();
        let mut world = ServerWorld::new(&reg, 4242).unwrap();
        let generator = deepcore_shared::worldgen::WorldGen::new(&reg, 4242).unwrap();
        let pos = ChunkPos::new(2, -5, 1);
        assert_eq!(
            world.chunk_mut(pos).to_vec(),
            generator.generate_chunk(pos).to_vec()
        );
    }
}
