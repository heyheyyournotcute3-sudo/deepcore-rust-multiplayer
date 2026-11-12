//! Integer coordinates and small vector helpers for the voxel world.
//!
//! The world is a right-handed, Y-up grid of unit cubes. Block `(x, y, z)` occupies the space
//! from `(x, y, z)` to `(x+1, y+1, z+1)`. Depth below the surface is measured as `SURFACE_Y - y`.

use serde::{Deserialize, Serialize};

/// Y coordinate of the flat camp surface. Everything below is mine.
pub const SURFACE_Y: i32 = 0;

/// Width of a chunk in blocks.
pub const CHUNK_SIZE: i32 = 32;

/// A block position in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BlockPos {
    /// X coordinate.
    pub x: i32,
    /// Y coordinate (up is positive).
    pub y: i32,
    /// Z coordinate.
    pub z: i32,
}

impl BlockPos {
    /// Creates a block position.
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Depth below the surface, in blocks.
    #[must_use]
    pub const fn depth(&self) -> i32 {
        SURFACE_Y - self.y
    }

    /// The chunk that contains this block.
    #[must_use]
    pub const fn chunk(&self) -> ChunkPos {
        ChunkPos::new(
            self.x.div_euclid(CHUNK_SIZE),
            self.y.div_euclid(CHUNK_SIZE),
            self.z.div_euclid(CHUNK_SIZE),
        )
    }

    /// Offset of this block inside its chunk, each component in `0..CHUNK_SIZE`.
    #[must_use]
    pub const fn chunk_local(&self) -> [usize; 3] {
        [
            self.x.rem_euclid(CHUNK_SIZE) as usize,
            self.y.rem_euclid(CHUNK_SIZE) as usize,
            self.z.rem_euclid(CHUNK_SIZE) as usize,
        ]
    }

    /// The centre of the block in world space, for rendering and raycasts.
    #[must_use]
    pub fn center(&self) -> [f32; 3] {
        [
            self.x as f32 + 0.5,
            self.y as f32 + 0.5,
            self.z as f32 + 0.5,
        ]
    }

    /// This position offset by `(dx, dy, dz)`.
    #[must_use]
    pub const fn offset(&self, dx: i32, dy: i32, dz: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            z: self.z + dz,
        }
    }

    /// The six face neighbours, in the fixed order used by the mesher.
    #[must_use]
    pub fn neighbours(&self) -> [Self; 6] {
        [
            self.offset(1, 0, 0),
            self.offset(-1, 0, 0),
            self.offset(0, 1, 0),
            self.offset(0, -1, 0),
            self.offset(0, 0, 1),
            self.offset(0, 0, -1),
        ]
    }

    /// Squared distance to another block position.
    #[must_use]
    pub fn distance_squared(&self, other: &Self) -> i32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        dx * dx + dy * dy + dz * dz
    }
}

/// A chunk position, in chunk units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChunkPos {
    /// Chunk X.
    pub x: i32,
    /// Chunk Y.
    pub y: i32,
    /// Chunk Z.
    pub z: i32,
}

impl ChunkPos {
    /// Creates a chunk position.
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// The block position of this chunk's minimum corner.
    #[must_use]
    pub const fn origin(&self) -> BlockPos {
        BlockPos::new(
            self.x * CHUNK_SIZE,
            self.y * CHUNK_SIZE,
            self.z * CHUNK_SIZE,
        )
    }

    /// World-space centre of the chunk, used for sorting by distance.
    #[must_use]
    pub fn center(&self) -> [f32; 3] {
        let half = CHUNK_SIZE as f32 / 2.0;
        [
            self.x as f32 * CHUNK_SIZE as f32 + half,
            self.y as f32 * CHUNK_SIZE as f32 + half,
            self.z as f32 * CHUNK_SIZE as f32 + half,
        ]
    }

    /// Chebyshev distance in chunks, used for interest management.
    #[must_use]
    pub fn chebyshev_distance(&self, other: &Self) -> i32 {
        (self.x - other.x)
            .abs()
            .max((self.y - other.y).abs())
            .max((self.z - other.z).abs())
    }
}

/// Converts a linear index in `0..CHUNK_VOLUME` back into local `[x, y, z]`.
#[must_use]
pub fn index_to_local(index: usize) -> [usize; 3] {
    let size = CHUNK_SIZE as usize;
    let x = index % size;
    let z = (index / size) % size;
    let y = index / (size * size);
    [x, y, z]
}

/// Converts local `[x, y, z]` into a linear index in `0..CHUNK_VOLUME`.
#[must_use]
pub fn local_to_index(local: [usize; 3]) -> usize {
    let size = CHUNK_SIZE as usize;
    local[0] + local[2] * size + local[1] * size * size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_coordinates_round_trip() {
        for x in [-100, -33, -32, -1, 0, 1, 31, 32, 100] {
            let pos = BlockPos::new(x, -x, x * 2);
            let chunk = pos.chunk();
            let local = pos.chunk_local();
            let back = BlockPos::new(
                chunk.x * CHUNK_SIZE + local[0] as i32,
                chunk.y * CHUNK_SIZE + local[1] as i32,
                chunk.z * CHUNK_SIZE + local[2] as i32,
            );
            assert_eq!(pos, back, "round trip failed for {pos:?}");
            assert!(local.iter().all(|&l| l < CHUNK_SIZE as usize));
        }
    }

    #[test]
    fn index_round_trip() {
        let volume = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;
        for index in [0, 1, 31, 32, 1024, volume / 2, volume - 1] {
            let local = index_to_local(index);
            assert_eq!(local_to_index(local), index);
        }
    }

    #[test]
    fn depth_is_positive_underground() {
        assert_eq!(BlockPos::new(0, 0, 0).depth(), 0);
        assert_eq!(BlockPos::new(0, -64, 0).depth(), 64);
        assert_eq!(BlockPos::new(0, 12, 0).depth(), -12);
    }

    #[test]
    fn chebyshev_uses_largest_axis() {
        let a = ChunkPos::new(0, 0, 0);
        let b = ChunkPos::new(3, -5, 2);
        assert_eq!(a.chebyshev_distance(&b), 5);
    }
}
