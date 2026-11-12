//! Chunk rendering: builds Bevy meshes from shared chunk data on the async task pool.
//!
//! Meshing runs on `AsyncComputeTaskPool` so the frame is never blocked. Each chunk becomes one
//! entity with a mesh, and the mesh material samples the procedural atlas.

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    log::debug,
    mesh::{Indices, PrimitiveTopology},
    platform::collections::HashMap,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future},
};
use deepcore_shared::{
    atlas::{AtlasLayout, atlas_uv, build_atlas_pixels},
    content::{BlockId, ContentRegistry},
    voxel::{
        CHUNK_SIZE, Chunk, ChunkPos,
        coords::BlockPos,
        mesher::{MeshData, greedy_mesh_with},
    },
};

/// Marks the plugin that renders voxel chunks.
pub struct ChunkRenderPlugin;

impl Plugin for ChunkRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClientChunks>()
            .init_resource::<ChunkVisuals>()
            .add_systems(Startup, setup_atlas)
            .add_systems(Update, (poll_mesh_tasks, apply_chunk_mesh_updates).chain());
    }
}

/// A chunk the client currently holds, and whether its mesh is up to date.
#[derive(Debug, Clone)]
pub struct ClientChunk {
    /// The chunk data.
    pub chunk: Chunk,
    /// The mesh entity, once meshed.
    pub entity: Option<Entity>,
    /// The mesh revision the entity currently shows.
    pub meshed_revision: u64,
    /// A monotonically increasing revision, bumped on every edit.
    pub revision: u64,
}

/// All chunks the client has loaded.
#[derive(Resource, Default)]
pub struct ClientChunks {
    /// Chunks by position.
    pub chunks: HashMap<ChunkPos, ClientChunk>,
}

impl ClientChunks {
    /// Inserts or replaces a chunk from server data.
    pub fn insert(&mut self, chunk: Chunk) {
        let pos = chunk.pos();
        let entry = self.chunks.entry(pos).or_insert_with(|| ClientChunk {
            chunk: chunk.clone(),
            entity: None,
            meshed_revision: 0,
            revision: 1,
        });
        entry.chunk = chunk;
        entry.revision += 1;
    }

    /// Applies a single block edit.
    pub fn set_block(&mut self, pos: BlockPos, block: BlockId) {
        let chunk_pos = pos.chunk();
        if let Some(entry) = self.chunks.get_mut(&chunk_pos) {
            entry.chunk.set_local(pos.chunk_local(), block);
            entry.revision += 1;
        }
    }

    /// Reads a block, returning `None` if the chunk is not loaded.
    #[must_use]
    pub fn get_block(&self, pos: BlockPos) -> Option<BlockId> {
        self.chunks
            .get(&pos.chunk())
            .map(|c| c.chunk.get_local(pos.chunk_local()))
    }

    /// Removes a chunk and its entity.
    pub fn remove(&mut self, pos: &ChunkPos) -> Option<Entity> {
        self.chunks.remove(pos).and_then(|c| c.entity)
    }

    /// Number of loaded chunks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.chunks.len()
    }
}

/// Shared rendering assets for chunks.
#[derive(Resource, Default)]
pub struct ChunkVisuals {
    /// The atlas material every chunk mesh uses.
    pub material: Handle<StandardMaterial>,
    /// Block ID to atlas tile lookup.
    pub layout: Option<AtlasLayout>,
    /// Which blocks occlude faces.
    solid: Vec<bool>,
}

/// A meshing job in flight.
#[derive(Component)]
pub struct MeshTask {
    /// Which chunk this job is for.
    pub pos: ChunkPos,
    /// The chunk revision being meshed.
    pub revision: u64,
    task: Task<MeshData>,
}

fn setup_atlas(
    registry: Res<ContentRegistry>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut visuals: ResMut<ChunkVisuals>,
) {
    let layout = AtlasLayout::new(registry.blocks.len());
    let pixels = build_atlas_pixels(&registry, &layout);
    let size = layout.size;

    let mut image = Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    // Voxel atlases must use nearest filtering with no mipmaps. With linear filtering and
    // mipmaps, a 16px tile averages down to a flat colour at any distance, which is exactly the
    // "no texture detail" artefact this avoids.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Nearest,
        min_filter: ImageFilterMode::Nearest,
        mipmap_filter: ImageFilterMode::Nearest,
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        ..default()
    });
    let handle = images.add(image);

    visuals.material = materials.add(StandardMaterial {
        base_color_texture: Some(handle),
        base_color: Color::WHITE,
        perceptual_roughness: 0.9,
        metallic: 0.0,
        // Lit: the mesher bakes per-face shading into vertex colours, which gives block
        // definition, and the sun/headlamp drive the darkness hazard.
        unlit: false,
        alpha_mode: AlphaMode::Mask(0.5),
        ..default()
    });
    visuals.solid = registry
        .blocks
        .iter()
        .map(|b| b.solid && b.hardness > 0.0)
        .collect();
    visuals.layout = Some(layout);
}

/// Starts meshing jobs for every chunk whose mesh is stale.
fn apply_chunk_mesh_updates(
    mut commands: Commands,
    chunks: ResMut<ClientChunks>,
    visuals: Res<ChunkVisuals>,
    existing_tasks: Query<&MeshTask>,
) {
    if visuals.solid.is_empty() {
        return;
    }
    let in_flight: Vec<(ChunkPos, u64)> =
        existing_tasks.iter().map(|t| (t.pos, t.revision)).collect();

    // Collect the work first so neighbour lookups can borrow the map immutably.
    let mut jobs: Vec<(ChunkPos, u64, Chunk, [Option<Chunk>; 6])> = Vec::new();
    for (pos, entry) in chunks.chunks.iter() {
        if entry.meshed_revision == entry.revision {
            continue;
        }
        if in_flight
            .iter()
            .any(|(p, r)| p == pos && *r == entry.revision)
        {
            continue;
        }
        let neighbours = neighbour_chunks(&chunks.chunks, *pos);
        jobs.push((*pos, entry.revision, entry.chunk.clone(), neighbours));
    }

    for (pos, revision, chunk, neighbours) in jobs {
        let solid = visuals.solid.clone();

        let pool = AsyncComputeTaskPool::get();
        let task = pool.spawn(async move {
            let get = |x: i32, y: i32, z: i32| -> BlockId {
                if (0..CHUNK_SIZE).contains(&x)
                    && (0..CHUNK_SIZE).contains(&y)
                    && (0..CHUNK_SIZE).contains(&z)
                {
                    chunk.get_local([x as usize, y as usize, z as usize])
                } else {
                    sample_neighbour(&neighbours, x, y, z)
                }
            };
            let solid_fn = |b: BlockId| solid.get(b.0 as usize).copied().unwrap_or(false);
            // Merge is off: the renderer samples a texture atlas, and an atlas cannot tile
            // inside a merged quad (see `greedy_mesh_with`).
            greedy_mesh_with(get, solid_fn, false)
        });

        commands.spawn(MeshTask {
            pos,
            revision,
            task,
        });
    }
}

/// Collects the six neighbour chunks for face culling.
fn neighbour_chunks(chunks: &HashMap<ChunkPos, ClientChunk>, pos: ChunkPos) -> [Option<Chunk>; 6] {
    let offsets = [
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ];
    offsets.map(|(dx, dy, dz)| {
        chunks
            .get(&ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz))
            .map(|c| c.chunk.clone())
    })
}

/// Samples a block just outside the chunk from the neighbouring chunk, if loaded.
fn sample_neighbour(neighbours: &[Option<Chunk>; 6], x: i32, y: i32, z: i32) -> BlockId {
    // Figure out which neighbour holds this coordinate.
    let (index, local) = if x >= CHUNK_SIZE {
        (0, [0, y, z])
    } else if x < 0 {
        (1, [CHUNK_SIZE - 1, y, z])
    } else if y >= CHUNK_SIZE {
        (2, [x, 0, z])
    } else if y < 0 {
        (3, [x, CHUNK_SIZE - 1, z])
    } else if z >= CHUNK_SIZE {
        (4, [x, y, 0])
    } else {
        (5, [x, y, CHUNK_SIZE - 1])
    };
    let Some(chunk) = &neighbours[index] else {
        return BlockId(0);
    };
    let local = local.map(|v| v.clamp(0, CHUNK_SIZE - 1) as usize);
    chunk.get_local(local)
}

/// Finishes meshing jobs and spawns or updates the chunk entities.
fn poll_mesh_tasks(
    mut commands: Commands,
    mut tasks: Query<(Entity, &mut MeshTask)>,
    mut chunks: ResMut<ClientChunks>,
    visuals: Res<ChunkVisuals>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    for (task_entity, mut task) in &mut tasks {
        let Some(mesh_data) = block_on(future::poll_once(&mut task.task)) else {
            continue;
        };
        commands.entity(task_entity).despawn();

        let Some(entry) = chunks.chunks.get_mut(&task.pos) else {
            continue;
        };
        if task.revision < entry.meshed_revision {
            continue;
        }
        let pos = task.pos;
        let origin = pos.origin();

        debug!(chunk = ?task.pos, quads = mesh_data.quads, "finished meshing");
        if mesh_data.is_empty() {
            // Nothing to draw: drop any existing entity and mark it meshed.
            if let Some(entity) = entry.entity.take() {
                commands.entity(entity).despawn();
            }
            entry.meshed_revision = task.revision;
            continue;
        }

        let mesh = build_bevy_mesh(&mesh_data, &visuals);
        let mesh_handle = meshes.add(mesh);
        let transform = Transform::from_xyz(origin.x as f32, origin.y as f32, origin.z as f32);

        match entry.entity {
            Some(entity) => {
                commands
                    .entity(entity)
                    .insert((Mesh3d(mesh_handle), transform));
            }
            None => {
                let entity = commands
                    .spawn((
                        Mesh3d(mesh_handle),
                        MeshMaterial3d(visuals.material.clone()),
                        transform,
                        ChunkVisual { pos },
                    ))
                    .id();
                entry.entity = Some(entity);
            }
        }
        entry.meshed_revision = task.revision;
    }
}

/// Marker for a rendered chunk entity.
#[derive(Component)]
pub struct ChunkVisual {
    /// Which chunk this entity draws.
    pub pos: ChunkPos,
}

/// Converts shared mesh data into a Bevy mesh with atlas UVs and baked shading.
fn build_bevy_mesh(data: &MeshData, visuals: &ChunkVisuals) -> Mesh {
    let layout = visuals.layout.as_ref();
    let mut positions = Vec::with_capacity(data.positions.len());
    let mut normals = Vec::with_capacity(data.normals.len());
    let mut uvs = Vec::with_capacity(data.uvs.len());
    let mut colors = Vec::with_capacity(data.colors.len());

    // The mesher emits quads with tile-space UVs; `atlas_uv` maps each into its block's
    // atlas cell. That mapping lives in `shared` so it is unit-tested there.
    let mut i = 0;
    while i < data.positions.len() {
        let block = quad_block(data, i);
        for k in 0..4 {
            positions.push(data.positions[i + k]);
            normals.push(data.normals[i + k]);
            uvs.push(match layout {
                Some(layout) => atlas_uv(layout, block, data.uvs[i + k]),
                None => data.uvs[i + k],
            });
            // The atlas texture carries the block's colour and pattern, so the mesh must not
            // also multiply by the mesher's shade factor (that would darken it to black).
            colors.push([1.0, 1.0, 1.0, 1.0]);
        }
        i += 4;
    }

    let indices: Vec<u32> = data.indices.clone();
    let mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(indices));
    // Bounding volumes are computed by the renderer from the mesh attributes.
    mesh
}

/// Recovers the block a quad belongs to by looking back at the mesher's ordering.
///
/// The mesher pushes quads in block order, so the block is stored in a side channel. To keep
/// the mesh buffers flat we re-derive it from a parallel list.
fn quad_block(data: &MeshData, _vertex: usize) -> BlockId {
    // The mesher appends four vertices per quad in the same order as `quads`, so the quad
    // index is `vertex / 4`. `MeshData` keeps blocks in a parallel vector.
    let quad_index = _vertex / 4;
    data.quad_blocks
        .get(quad_index)
        .copied()
        .unwrap_or(BlockId(0))
}

/// Reports how many chunks are loaded and how many meshes are in flight.
#[derive(Debug, Default, Resource)]
pub struct ChunkStats {
    /// Loaded chunks.
    pub loaded: usize,
    /// Chunks waiting to be meshed.
    pub pending: usize,
    /// Chunk meshes currently drawn.
    pub drawn: usize,
}

/// Keeps [`ChunkStats`] up to date for the debug overlay.
pub fn update_chunk_stats(
    chunks: Res<ClientChunks>,
    tasks: Query<&MeshTask>,
    visuals: Query<&ChunkVisual>,
    mut stats: ResMut<ChunkStats>,
) {
    stats.loaded = chunks.len();
    stats.pending = tasks.iter().count();
    stats.drawn = visuals
        .iter()
        .filter(|v| v.pos.y <= 0)
        .count()
        .max(visuals.iter().count());
}
