//! Deterministic world generation.
//!
//! The same seed always produces exactly the same world on every machine: all randomness comes
//! from a per-chunk `ChaCha8Rng` seeded with `(world_seed, chunk_pos)`, and all terrain shaping
//! comes from integer lattice value noise. Nothing depends on iteration order, hash ordering or
//! wall-clock time.

use noise::{NoiseFn, Value};
use rand_chacha::ChaCha8Rng;
use rand_core::{Rng, SeedableRng};

use crate::content::{BlockId, ContentRegistry, HazardRule};
use crate::voxel::coords::{BlockPos, CHUNK_SIZE, ChunkPos};
use crate::voxel::{AIR, Chunk};

/// The width and depth of the world, in blocks.
pub const WORLD_SIZE_XZ: i32 = 1024;

/// The total depth of the mine below the surface.
pub const WORLD_DEPTH: i32 = 512;

/// Surface features generated on top of the flat camp area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceFeature {
    /// Nothing.
    None,
    /// A tree trunk, which can be chopped for timber.
    Tree,
}

/// The generator for one world seed.
pub struct WorldGen {
    seed: u64,
    cave_noise: Value,
    surface_noise: Value,
    base_floor: BlockId,
    bedrock: BlockId,
    water: BlockId,
    methane: BlockId,
    leaves: BlockId,
    trunk: BlockId,
    grass: BlockId,
    /// Base block per depth, precomputed from the layer definitions.
    depth_blocks: Vec<LayerSample>,
    /// Ore rules per depth, precomputed from the layer definitions.
    depth_ores: Vec<Vec<OreSample>>,
    /// Hazard rules per depth.
    depth_hazards: Vec<Vec<HazardRule>>,
}

/// The filler blocks valid at one depth, with their weights.
#[derive(Debug, Clone)]
struct LayerSample {
    base: BlockId,
    fillers: Vec<(BlockId, f32)>,
    cave_density: f32,
}

/// One ore rule resolved for one depth.
#[derive(Debug, Clone)]
struct OreSample {
    block: BlockId,
    frequency: f32,
    vein_size: (u32, u32),
}

impl WorldGen {
    /// Builds a generator from the content registry and a world seed.
    ///
    /// Fails if the content lacks the mandatory blocks (`air`, `bedrock`, `water`, ...).
    pub fn new(registry: &ContentRegistry, seed: u64) -> anyhow::Result<Self> {
        let need = |id: &str| -> anyhow::Result<BlockId> {
            registry
                .block(id)
                .ok_or_else(|| anyhow::anyhow!("world generation needs a block called \"{id}\""))
        };
        let bedrock = need("bedrock")?;
        let base_floor = need("stone")?;
        let water = need("water")?;
        let methane = need("methane")?;
        let leaves = need("tree_leaves")?;
        let trunk = need("tree_trunk")?;
        let grass = need("grass")?;

        let total = (WORLD_DEPTH + 2) as usize;
        let mut depth_blocks = Vec::with_capacity(total);
        let mut depth_ores = Vec::with_capacity(total);
        let mut depth_hazards = Vec::with_capacity(total);
        for depth in 0..=WORLD_DEPTH + 1 {
            let layer = registry.layer_at_depth(depth);
            let (base, fillers, cave_density) = match layer {
                Some(l) => {
                    let base = registry
                        .block(&l.base_block)
                        .ok_or_else(|| anyhow::anyhow!("layer {} base block missing", l.id))?;
                    let mut fillers = Vec::new();
                    for (name, weight) in &l.filler_blocks {
                        if let Some(id) = registry.block(name) {
                            fillers.push((id, *weight));
                        }
                    }
                    (base, fillers, l.cave_density)
                }
                None => (base_floor, Vec::new(), 0.4),
            };
            depth_blocks.push(LayerSample {
                base,
                fillers,
                cave_density,
            });

            let mut ores = Vec::new();
            if let Some(l) = layer {
                for ore in &l.ores {
                    let in_range = ore
                        .depth_within
                        .is_none_or(|(lo, hi)| depth >= l.depth.0 + lo && depth <= l.depth.0 + hi);
                    if !in_range {
                        continue;
                    }
                    if let Some(block) = registry.block(&ore.block) {
                        ores.push(OreSample {
                            block,
                            frequency: ore.frequency,
                            vein_size: ore.vein_size,
                        });
                    }
                }
            }
            depth_ores.push(ores);
            depth_hazards.push(layer.map(|l| l.hazards.clone()).unwrap_or_default());
        }

        Ok(Self {
            seed,
            cave_noise: Value::new((seed & 0xFFFF_FFFF) as u32),
            surface_noise: Value::new((seed.rotate_left(17) & 0xFFFF_FFFF) as u32),
            base_floor,
            bedrock,
            water,
            methane,
            leaves,
            trunk,
            grass,
            depth_blocks,
            depth_ores,
            depth_hazards,
        })
    }

    /// The world seed.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Generates a whole chunk.
    #[must_use]
    pub fn generate_chunk(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::filled(pos, self.base_floor);
        let origin = pos.origin();
        let mut rng = self.chunk_rng(pos);

        // Pass 1: strata, caves, bedrock bounds and the camp surface.
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let world = BlockPos::new(origin.x + x, origin.y + y, origin.z + z);
                    let block = self.block_at(&mut rng, world);
                    chunk.set_local([x as usize, y as usize, z as usize], block);
                }
            }
        }

        // Pass 2: ore veins, so they can span the whole chunk.
        self.place_ore_veins(&mut rng, pos, &mut chunk);

        // Pass 3: fluid pockets (water and methane).
        self.place_pockets(&mut rng, pos, &mut chunk);

        // Pass 4: surface decoration (trees) for the top layer chunks.
        if pos.y >= 0 {
            self.place_trees(&mut rng, pos, &mut chunk);
        }

        chunk
    }

    /// Returns the generated block at a world position without generating a whole chunk.
    ///
    /// Used for spawn placement and for spot checks; it must agree with [`Self::generate_chunk`].
    #[must_use]
    pub fn block_at_world(&self, world: BlockPos) -> BlockId {
        let chunk = world.chunk();
        let mut rng = self.chunk_rng(chunk);
        self.block_at(&mut rng, world)
    }

    fn chunk_rng(&self, pos: ChunkPos) -> ChaCha8Rng {
        let mut seed = [0u8; 32];
        seed[0..8].copy_from_slice(&self.seed.to_le_bytes());
        seed[8..12].copy_from_slice(&pos.x.to_le_bytes());
        seed[12..16].copy_from_slice(&pos.y.to_le_bytes());
        seed[16..20].copy_from_slice(&pos.z.to_le_bytes());
        ChaCha8Rng::from_seed(seed)
    }

    fn block_at(&self, rng: &mut ChaCha8Rng, world: BlockPos) -> BlockId {
        // Unbreakable bounds: the floor of the world and the outer walls.
        if world.y <= -WORLD_DEPTH || !self.in_horizontal_bounds(world.x, world.z) {
            return self.bedrock;
        }

        let depth = world.depth();

        // Above the surface is open air, except the thin grass skin at depth 0.
        if depth < 0 {
            return AIR;
        }
        if depth == 0 {
            // Flat-ish camp: gentle noise decides grass versus bare dirt.
            let n = self
                .surface_noise
                .get([world.x as f64 * 0.02, world.z as f64 * 0.02]);
            return if n > -0.15 {
                self.grass
            } else {
                self.base_floor
            };
        }

        let sample = &self.depth_blocks[depth as usize];

        // Carve caves with 3D noise, modulated by the layer's cave density.
        let cave = self.cave_noise.get([
            world.x as f64 * 0.045,
            world.y as f64 * 0.06,
            world.z as f64 * 0.045,
        ]);
        let threshold = 0.72 - sample.cave_density as f64 * 0.30;
        if cave.abs() > threshold {
            // Caves are open, but deep ones can be flooded later by pockets.
            return AIR;
        }

        // Pick the filler block for this voxel.
        let mut block = sample.base;
        if !sample.fillers.is_empty() {
            let roll = (rng.next_u32() % 10_000) as f32 / 10_000.0;
            let mut acc = 0.0;
            for (filler, weight) in &sample.fillers {
                acc += weight;
                if roll < acc {
                    block = *filler;
                    break;
                }
            }
        }
        block
    }

    fn in_horizontal_bounds(&self, x: i32, z: i32) -> bool {
        let half = WORLD_SIZE_XZ / 2;
        (-half..half).contains(&x) && (-half..half).contains(&z)
    }

    fn place_ore_veins(&self, rng: &mut ChaCha8Rng, pos: ChunkPos, chunk: &mut Chunk) {
        let origin = pos.origin();
        for y in 0..CHUNK_SIZE {
            let depth = (origin.y + y).neg_abs_depth();
            let Some(ores) = self.depth_ores.get(depth as usize) else {
                continue;
            };
            for ore in ores {
                // `frequency` is the chance that a given voxel seeds a vein. A 32x32 slice
                // has 1024 voxels, so frequency 0.01 gives roughly ten veins per slice.
                let per_slice = ore.frequency * 1024.0;
                let mut seeds = per_slice.floor() as u32;
                let frac = per_slice - seeds as f32;
                if (rng.next_u32() % 1000) as f32 / 1000.0 < frac {
                    seeds += 1;
                }
                for _ in 0..seeds {
                    let start = [
                        (rng.next_u32() % CHUNK_SIZE as u32) as i32,
                        y,
                        (rng.next_u32() % CHUNK_SIZE as u32) as i32,
                    ];
                    let size = ore.vein_size.0
                        + (rng.next_u32() % (ore.vein_size.1 - ore.vein_size.0 + 1).max(1));
                    self.grow_vein(rng, chunk, start, size, ore.block);
                }
            }
        }
    }

    /// Grows a blob of ore from a seed voxel, staying inside the chunk.
    fn grow_vein(
        &self,
        rng: &mut ChaCha8Rng,
        chunk: &mut Chunk,
        start: [i32; 3],
        size: u32,
        ore: BlockId,
    ) {
        let mut cells = vec![start];
        let mut placed = 0u32;
        while let Some(cell) = cells.pop() {
            if placed >= size {
                break;
            }
            if cell.iter().any(|&c| !(0..CHUNK_SIZE).contains(&c)) {
                continue;
            }
            let local = [cell[0] as usize, cell[1] as usize, cell[2] as usize];
            let existing = chunk.get_local(local);
            // Only replace natural rock, never air (so veins do not float in caves).
            if existing == AIR || existing == self.bedrock || existing == self.water {
                continue;
            }
            chunk.set_local(local, ore);
            placed += 1;
            for _ in 0..3 {
                let dir = rng.next_u32() % 6;
                let mut next = cell;
                match dir {
                    0 => next[0] += 1,
                    1 => next[0] -= 1,
                    2 => next[1] += 1,
                    3 => next[1] -= 1,
                    4 => next[2] += 1,
                    _ => next[2] -= 1,
                }
                cells.push(next);
            }
        }
    }

    fn place_pockets(&self, rng: &mut ChaCha8Rng, pos: ChunkPos, chunk: &mut Chunk) {
        let origin = pos.origin();
        for y in 0..CHUNK_SIZE {
            let depth = (origin.y + y).neg_abs_depth();
            let Some(hazards) = self.depth_hazards.get(depth as usize) else {
                continue;
            };
            for hazard in hazards {
                let (chance, volume, fluid) = match hazard {
                    HazardRule::Flooding {
                        pocket_chance,
                        max_volume,
                    } => (*pocket_chance, *max_volume, self.water),
                    HazardRule::Gas {
                        pocket_chance,
                        max_volume,
                    } => (*pocket_chance, *max_volume, self.methane),
                    _ => continue,
                };
                let roll = (rng.next_u32() % 100_000) as f32 / 100_000.0;
                if roll > chance {
                    continue;
                }
                let start = [
                    (rng.next_u32() % CHUNK_SIZE as u32) as i32,
                    y,
                    (rng.next_u32() % CHUNK_SIZE as u32) as i32,
                ];
                let size = (volume / 40).clamp(4, 64);
                self.grow_vein(rng, chunk, start, size, fluid);
            }
        }
    }

    fn place_trees(&self, rng: &mut ChaCha8Rng, pos: ChunkPos, chunk: &mut Chunk) {
        // Trees only on the surface chunk layer, and never inside the central camp area.
        let origin = pos.origin();
        let trees = rng.next_u32() % 4;
        for _ in 0..trees {
            let x = (rng.next_u32() % CHUNK_SIZE as u32) as i32;
            let z = (rng.next_u32() % CHUNK_SIZE as u32) as i32;
            let world_x = origin.x + x;
            let world_z = origin.z + z;
            // Keep the middle 64x64 blocks of the world clear for the Base.
            if world_x.abs() < 32 && world_z.abs() < 32 {
                continue;
            }
            let height = 4 + (rng.next_u32() % 3) as i32;
            // Find the surface: the first non-air voxel from the top of this chunk.
            let mut surface = None;
            for y in (0..CHUNK_SIZE).rev() {
                let local = [x as usize, y as usize, z as usize];
                if chunk.get_local(local) != AIR {
                    surface = Some(y);
                    break;
                }
            }
            let Some(ground) = surface else { continue };
            for h in 1..=height {
                let local_y = ground + h;
                if local_y < CHUNK_SIZE {
                    chunk.set_local([x as usize, local_y as usize, z as usize], self.trunk);
                }
            }
            // A simple canopy.
            let top = ground + height;
            for dy in -1..=1 {
                for dz in -2i32..=2 {
                    for dx in -2i32..=2 {
                        if dx.abs() == 2 && dz.abs() == 2 {
                            continue;
                        }
                        let ly = top + dy;
                        let lx = x + dx;
                        let lz = z + dz;
                        if (0..CHUNK_SIZE).contains(&lx)
                            && (0..CHUNK_SIZE).contains(&lz)
                            && (0..CHUNK_SIZE).contains(&ly)
                            && chunk.get_local([lx as usize, ly as usize, lz as usize]) == AIR
                        {
                            chunk.set_local([lx as usize, ly as usize, lz as usize], self.leaves);
                        }
                    }
                }
            }
        }
    }

    /// Finds a safe spawn position at the Base, on top of the surface.
    #[must_use]
    pub fn spawn_position(&self) -> [f32; 3] {
        // The Base is the flat camp at the origin; the surface block there is solid.
        [0.5, 1.0, 0.5]
    }
}

/// Helper: converts a world Y coordinate into a non-negative depth.
trait DepthExt {
    fn neg_abs_depth(self) -> i32;
}

impl DepthExt for i32 {
    fn neg_abs_depth(self) -> i32 {
        (-self).max(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voxel::coords::local_to_index;

    fn registry() -> ContentRegistry {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        ContentRegistry::load_from_dir(&dir).expect("content must load for worldgen tests")
    }

    #[test]
    fn generation_is_deterministic() {
        let reg = registry();
        let gen_a = WorldGen::new(&reg, 1337).unwrap();
        let gen_b = WorldGen::new(&reg, 1337).unwrap();
        for pos in [
            ChunkPos::new(0, 0, 0),
            ChunkPos::new(1, -1, 0),
            ChunkPos::new(-3, -8, 2),
            ChunkPos::new(0, -16, 0),
        ] {
            let a = gen_a.generate_chunk(pos);
            let b = gen_b.generate_chunk(pos);
            assert_eq!(a.to_vec(), b.to_vec(), "chunk {pos:?} differs between runs");
        }
    }

    #[test]
    fn different_seeds_differ() {
        let reg = registry();
        let gen_a = WorldGen::new(&reg, 1).unwrap();
        let gen_b = WorldGen::new(&reg, 2).unwrap();
        let pos = ChunkPos::new(2, -3, 1);
        assert_ne!(
            gen_a.generate_chunk(pos).to_vec(),
            gen_b.generate_chunk(pos).to_vec(),
            "two seeds produced an identical chunk"
        );
    }

    #[test]
    fn layers_appear_at_the_right_depths() {
        let reg = registry();
        let generator = WorldGen::new(&reg, 99).unwrap();
        // Topsoil chunk (depth 0..16) must contain dirt/grass, not basalt.
        let top = generator.generate_chunk(ChunkPos::new(1, -1, 1));
        let top_blocks = top.to_vec();
        let dirt = reg.block("dirt").unwrap();
        let grass = reg.block("grass").unwrap();
        assert!(
            top_blocks.iter().any(|b| *b == dirt || *b == grass),
            "topsoil chunk has no dirt or grass"
        );

        // Deep chunk (depth ~320..352) must contain gneiss or obsidian, never dirt.
        let deep = generator.generate_chunk(ChunkPos::new(1, -11, 1));
        let deep_blocks = deep.to_vec();
        let gneiss = reg.block("gneiss").unwrap();
        let obsidian = reg.block("obsidian").unwrap();
        assert!(
            deep_blocks.iter().any(|b| *b == gneiss || *b == obsidian),
            "deep chunk has no gneiss or obsidian"
        );
        assert!(
            !deep_blocks.contains(&dirt),
            "dirt found at 320 blocks down"
        );
    }

    #[test]
    fn world_edges_are_bedrock() {
        let reg = registry();
        let generator = WorldGen::new(&reg, 7).unwrap();
        let bedrock = reg.block("bedrock").unwrap();
        let half = WORLD_SIZE_XZ / 2;
        // A chunk just outside the world's horizontal bounds.
        let outside = ChunkPos::new(half / CHUNK_SIZE, -2, 0);
        let chunk = generator.generate_chunk(outside);
        assert!(
            chunk.to_vec().iter().all(|b| *b == bedrock),
            "outside the world bounds everything must be bedrock"
        );
        // The floor of the world.
        let floor = generator.generate_chunk(ChunkPos::new(0, -WORLD_DEPTH / CHUNK_SIZE, 0));
        let lowest_row: Vec<_> = (0..CHUNK_SIZE)
            .map(|z| floor.get_local([0, 0, z as usize]))
            .collect();
        assert!(lowest_row.iter().all(|b| *b == bedrock || *b == AIR));
    }

    #[test]
    fn chunks_have_caves_or_ore() {
        let reg = registry();
        let generator = WorldGen::new(&reg, 4242).unwrap();
        let chunk = generator.generate_chunk(ChunkPos::new(3, -6, -3));
        let blocks = chunk.to_vec();
        let air = blocks.iter().filter(|b| **b == AIR).count();
        assert!(air > 0, "a deep chunk should have carved caves");
        // Ore of some kind must be present. This chunk sits in the Basalt Furnace layer,
        // so check that layer's ores; ore rules are per-layer by design.
        let ore_blocks: Vec<BlockId> = [
            "malachite",
            "hematite",
            "cassiterite",
            "galena",
            "pentlandite",
            "chromite",
            "graphite_vein",
            "fluorite",
            "gold_vein",
            "sulfur_deposit",
        ]
        .iter()
        .filter_map(|n| reg.block(n))
        .collect();
        assert!(
            blocks.iter().any(|b| ore_blocks.contains(b)),
            "no ore generated in a layer-2/3 chunk"
        );
    }

    #[test]
    fn index_helpers_agree_with_chunk() {
        let reg = registry();
        let generator = WorldGen::new(&reg, 5).unwrap();
        let pos = ChunkPos::new(0, -2, 0);
        let chunk = generator.generate_chunk(pos);
        for i in 0..crate::voxel::CHUNK_VOLUME {
            let local = crate::voxel::coords::index_to_local(i);
            assert_eq!(
                chunk.get_local(local),
                chunk.to_vec()[local_to_index(local)]
            );
        }
    }
}
