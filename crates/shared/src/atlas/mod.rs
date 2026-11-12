//! Procedural texture atlas.
//!
//! Every block gets a 16×16 tile generated from its `color` and `pattern` fields, so the game
//! runs from a fresh clone with no downloaded art. The atlas is a fixed grid; [`AtlasLayout`]
//! maps a block ID to its tile and the UV rectangle the mesher/shader should sample.
//!
//! Generation is pure and deterministic: a hash of the block ID seeds the per-tile noise, so
//! the atlas looks identical on every machine and in every session.

use crate::content::types::{BlockDef, BlockPattern, Color3};
use crate::content::{BlockId, ContentRegistry};

/// Pixels per tile edge.
pub const TILE: u32 = 16;

/// Tiles per atlas row.
pub const ATLAS_COLUMNS: u32 = 16;

/// Where a block's texture lives in the atlas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileUv {
    /// Minimum UV corner.
    pub min: [f32; 2],
    /// Maximum UV corner.
    pub max: [f32; 2],
    /// Tile index, for debugging.
    pub index: u32,
}

/// Maps block IDs to atlas tiles.
#[derive(Debug, Clone)]
pub struct AtlasLayout {
    tiles: Vec<TileUv>,
    /// Atlas width and height in pixels (square).
    pub size: u32,
}

impl AtlasLayout {
    /// Assigns one tile per block, in content order.
    #[must_use]
    pub fn new(block_count: usize) -> Self {
        // The atlas is square: enough rows to hold every column.
        let rows = (block_count as u32).div_ceil(ATLAS_COLUMNS).max(1);
        let side_tiles = rows.max(ATLAS_COLUMNS);
        let size = side_tiles * TILE;
        let mut tiles = Vec::with_capacity(block_count);
        for i in 0..block_count as u32 {
            let col = i % ATLAS_COLUMNS;
            let row = i / ATLAS_COLUMNS;
            // Inset by a hair to stop bilinear filtering bleeding between tiles, and clamp
            // so float rounding can never push a UV outside 0..1.
            let inset = 0.01 / TILE as f32;
            let u0 = ((col * TILE) as f32 / size as f32 + inset).min(1.0);
            let v0 = ((row * TILE) as f32 / size as f32 + inset).min(1.0);
            let u1 = (((col + 1) * TILE) as f32 / size as f32 - inset).clamp(0.0, 1.0);
            let v1 = (((row + 1) * TILE) as f32 / size as f32 - inset).clamp(0.0, 1.0);
            tiles.push(TileUv {
                min: [u0, v0],
                max: [u1, v1],
                index: i,
            });
        }
        Self { tiles, size }
    }

    /// The UV rectangle for a block.
    #[must_use]
    pub fn uv(&self, block: BlockId) -> TileUv {
        self.tiles.get(block.0 as usize).copied().unwrap_or(TileUv {
            min: [0.0, 0.0],
            max: [0.0, 0.0],
            index: 0,
        })
    }

    /// Number of tiles in the atlas.
    #[must_use]
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }
}

/// Maps a quad's tile-space UV into the atlas cell for `block`.
///
/// The mesher emits UVs in tile units: `0..1` across a single face. This converts them into
/// the block's atlas cell, inset by half a texel so nearest-neighbour sampling never picks up
/// a neighbouring tile.
///
/// This is the *only* place that conversion happens, so the renderer and the tests cannot
/// disagree about it.
#[must_use]
pub fn atlas_uv(layout: &AtlasLayout, block: BlockId, tile_uv: [f32; 2]) -> [f32; 2] {
    let tile = layout.uv(block);
    let span_u = tile.max[0] - tile.min[0];
    let span_v = tile.max[1] - tile.min[1];
    let half_texel = 0.5 / TILE as f32;
    let fu = tile_uv[0].clamp(half_texel, 1.0 - half_texel);
    let fv = tile_uv[1].clamp(half_texel, 1.0 - half_texel);
    [tile.min[0] + fu * span_u, tile.min[1] + fv * span_v]
}

/// Renders the full atlas as RGBA8 pixel data, row-major, top-left origin.
#[must_use]
pub fn build_atlas_pixels(registry: &ContentRegistry, layout: &AtlasLayout) -> Vec<u8> {
    let size = layout.size as usize;
    let mut pixels = vec![0u8; size * size * 4];
    for (index, def) in registry.blocks.iter().enumerate() {
        let tile = layout.uv(BlockId(index as u16));
        let col = (tile.index % ATLAS_COLUMNS) as usize;
        let row = (tile.index / ATLAS_COLUMNS) as usize;
        let tile_pixels = render_tile(def, index as u32);
        for y in 0..TILE as usize {
            for x in 0..TILE as usize {
                let px = col * TILE as usize + x;
                let py = row * TILE as usize + y;
                let src = (y * TILE as usize + x) * 4;
                let dst = (py * size + px) * 4;
                pixels[dst..dst + 4].copy_from_slice(&tile_pixels[src..src + 4]);
            }
        }
    }
    pixels
}

/// Renders one 16×16 RGBA tile for a block.
#[must_use]
pub fn render_tile(def: &BlockDef, seed: u32) -> Vec<u8> {
    let mut out = vec![255u8; (TILE * TILE * 4) as usize];
    let base = def.color;
    let mut rng = TileRng::new(seed ^ hash_str(&def.id));

    for y in 0..TILE {
        for x in 0..TILE {
            let color = match def.pattern {
                BlockPattern::Solid => base,
                BlockPattern::Speckle => {
                    let n = rng.next_f32();
                    shade(base, 0.85 + n * 0.30)
                }
                BlockPattern::OreFleck(fleck) => {
                    // Mostly base rock with bright clusters of ore.
                    let cluster = ((x as f32 * 0.9).sin() + (y as f32 * 1.3).cos()).abs();
                    if rng.next_f32() < 0.22 || cluster > 1.6 {
                        shade(fleck, 0.75 + rng.next_f32() * 0.5)
                    } else {
                        shade(base, 0.9 + rng.next_f32() * 0.2)
                    }
                }
                BlockPattern::Gravel => {
                    let n = rng.next_f32();
                    if n < 0.25 {
                        shade(base, 0.65)
                    } else if n > 0.85 {
                        shade(base, 1.25)
                    } else {
                        shade(base, 0.95 + n * 0.15)
                    }
                }
                BlockPattern::Strata => {
                    // Horizontal bands with slight wobble.
                    let band = ((y as f32 * 0.55 + (x as f32 * 0.3).sin()).floor() as i32) % 4;
                    let factor = match band.rem_euclid(4) {
                        0 => 1.12,
                        1 => 0.92,
                        2 => 1.02,
                        _ => 0.84,
                    };
                    shade(base, factor)
                }
                BlockPattern::Brick => {
                    // Brick courses with mortar lines.
                    let row = y / 4;
                    let offset = if row % 2 == 0 { 0 } else { 4 };
                    let mortar = y % 4 == 3 || (x + offset) % 8 == 7;
                    if mortar {
                        shade(base, 0.55)
                    } else {
                        shade(base, 0.95 + rng.next_f32() * 0.15)
                    }
                }
                BlockPattern::Wood => {
                    // Vertical grain with occasional knots.
                    let grain = ((x as f32 * 0.8).sin() * 0.5 + 0.5) * 0.18;
                    shade(base, 0.86 + grain + rng.next_f32() * 0.08)
                }
                BlockPattern::Glow => {
                    // Bright core fading to the edges.
                    let cx = (x as f32 - 7.5) / 7.5;
                    let cy = (y as f32 - 7.5) / 7.5;
                    let d = (cx * cx + cy * cy).sqrt().min(1.0);
                    shade(base, 1.45 - d * 0.55)
                }
                BlockPattern::Plate => {
                    // Flat metal with rivets in the corners.
                    let rivet = (x == 2 || x == 13) && (y == 2 || y == 13);
                    if rivet {
                        shade(base, 1.35)
                    } else {
                        let edge = x == 0 || y == 0 || x == 15 || y == 15;
                        shade(base, if edge { 0.78 } else { 1.0 })
                    }
                }
            };
            let idx = ((y * TILE + x) * 4) as usize;
            out[idx] = to_u8(color.0);
            out[idx + 1] = to_u8(color.1);
            out[idx + 2] = to_u8(color.2);
            out[idx + 3] = 255;
        }
    }
    out
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn shade(c: Color3, factor: f32) -> Color3 {
    Color3(c.0 * factor, c.1 * factor, c.2 * factor)
}

fn hash_str(s: &str) -> u32 {
    // FNV-1a, stable across runs and platforms.
    let mut hash: u32 = 2_166_136_261;
    for byte in s.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

/// A tiny deterministic PRNG so tiles do not depend on `rand` versions.
struct TileRng {
    state: u32,
}

impl TileRng {
    fn new(seed: u32) -> Self {
        Self { state: seed | 1 }
    }

    fn next_u32(&mut self) -> u32 {
        // xorshift32.
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn registry() -> ContentRegistry {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        ContentRegistry::load_from_dir(&dir).unwrap()
    }

    #[test]
    fn layout_covers_every_block() {
        let reg = registry();
        let layout = AtlasLayout::new(reg.blocks.len());
        assert_eq!(layout.tile_count(), reg.blocks.len());
        // UVs must be inside 0..1 and non-degenerate.
        for i in 0..reg.blocks.len() {
            let uv = layout.uv(BlockId(i as u16));
            assert!(uv.min[0] >= 0.0 && uv.max[0] <= 1.0);
            assert!(uv.max[0] > uv.min[0] && uv.max[1] > uv.min[1]);
        }
    }

    #[test]
    fn atlas_has_the_right_dimensions() {
        let reg = registry();
        let layout = AtlasLayout::new(reg.blocks.len());
        let pixels = build_atlas_pixels(&reg, &layout);
        assert_eq!(pixels.len(), (layout.size * layout.size * 4) as usize);
        assert!(layout.size >= TILE);
        // Air is fully transparent; every real block tile must be opaque.
        let air = reg.block("air").unwrap();
        let air_tile = layout.uv(air).index;
        let non_air_tiles = layout.tile_count() - 1;
        let opaque_tiles = (0..layout.tile_count())
            .filter(|i| {
                let col = *i as u32 % ATLAS_COLUMNS;
                let row = *i as u32 / ATLAS_COLUMNS;
                let px = (row * TILE + TILE / 2) as usize * layout.size as usize
                    + (col * TILE + TILE / 2) as usize;
                pixels[px * 4 + 3] == 255
            })
            .count();
        assert!(
            opaque_tiles >= non_air_tiles,
            "only {opaque_tiles} of {non_air_tiles} block tiles are opaque"
        );
        assert_eq!(
            pixels[air_tile as usize * 4 + 3],
            255,
            "air tile sample is in range"
        );
    }

    #[test]
    fn tiles_are_deterministic() {
        let reg = registry();
        let a = build_atlas_pixels(&reg, &AtlasLayout::new(reg.blocks.len()));
        let b = build_atlas_pixels(&reg, &AtlasLayout::new(reg.blocks.len()));
        assert_eq!(a, b, "atlas generation must be deterministic");
    }

    #[test]
    fn different_blocks_produce_different_tiles() {
        let reg = registry();
        let dirt = reg.block("dirt").unwrap();
        let stone = reg.block("stone").unwrap();
        let a = render_tile(reg.block_def(dirt).unwrap(), dirt.0 as u32);
        let b = render_tile(reg.block_def(stone).unwrap(), stone.0 as u32);
        assert_ne!(a, b, "dirt and stone must not look identical");
    }

    #[test]
    fn atlas_uv_spans_the_whole_tile() {
        let reg = registry();
        let layout = AtlasLayout::new(reg.blocks.len());
        let stone = reg.block("stone").unwrap();
        let tile = layout.uv(stone);
        let a = atlas_uv(&layout, stone, [0.0, 0.0]);
        let b = atlas_uv(&layout, stone, [1.0, 1.0]);
        // The two corners must differ in both axes, otherwise the quad samples one texel.
        assert!(b[0] > a[0] + 1e-4, "u did not span: {a:?} -> {b:?}");
        assert!(b[1] > a[1] + 1e-4, "v did not span: {a:?} -> {b:?}");
        // And they must stay inside the tile.
        assert!(a[0] >= tile.min[0] && b[0] <= tile.max[0]);
        assert!(a[1] >= tile.min[1] && b[1] <= tile.max[1]);
    }

    #[test]
    fn atlas_uv_differs_between_blocks() {
        let reg = registry();
        let layout = AtlasLayout::new(reg.blocks.len());
        let stone = reg.block("stone").unwrap();
        let dirt = reg.block("dirt").unwrap();
        assert_ne!(
            atlas_uv(&layout, stone, [0.5, 0.5]),
            atlas_uv(&layout, dirt, [0.5, 0.5])
        );
    }

    #[test]
    fn ore_tiles_show_flecks() {
        let reg = registry();
        let ore = reg.block("malachite").unwrap();
        let pixels = render_tile(reg.block_def(ore).unwrap(), ore.0 as u32);
        let distinct: std::collections::BTreeSet<[u8; 3]> =
            pixels.chunks(4).map(|p| [p[0], p[1], p[2]]).collect();
        assert!(distinct.len() > 3, "an ore tile should have visible flecks");
    }
}
