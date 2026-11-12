//! Chunk storage with per-chunk palette compression.
//!
//! A chunk is 32×32×32 blocks. Each chunk stores its own palette (the distinct block IDs it
//! contains) plus a packed index array. A chunk full of one block costs a few bytes, which is
//! what makes streaming a 1024×1024×512 world practical over the network.
//!
//! Index width adapts to palette size: 1, 2, 4, 8 or 16 bits. At 16 bits the chunk is simply
//! an uncompressed `u16` array.

use serde::{Deserialize, Serialize};

use super::coords::{CHUNK_SIZE, ChunkPos, index_to_local, local_to_index};
use crate::content::BlockId;

/// Number of blocks in a chunk.
pub const CHUNK_VOLUME: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;

/// The block ID used for empty space.
pub const AIR: BlockId = BlockId(0);

/// A dense chunk of blocks, compressed through a per-chunk palette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    /// Distinct block IDs in this chunk, in first-seen order.
    palette: Vec<BlockId>,
    /// Packed palette indices, `bits_per_index` bits each.
    data: Vec<u64>,
    /// Bits used per palette index.
    bits: u8,
    /// Position of this chunk in the world.
    pos: ChunkPos,
    /// Whether this chunk has been modified since generation (and therefore must be saved).
    dirty: bool,
}

impl Chunk {
    /// Creates an empty chunk filled with air.
    #[must_use]
    pub fn empty(pos: ChunkPos) -> Self {
        Self::filled(pos, AIR)
    }

    /// Creates a chunk filled entirely with one block.
    #[must_use]
    pub fn filled(pos: ChunkPos, block: BlockId) -> Self {
        let mut chunk = Self {
            palette: vec![block],
            data: Vec::new(),
            bits: 0,
            pos,
            dirty: false,
        };
        chunk.bits = 0;
        chunk.data.clear();
        chunk
    }

    /// The position of this chunk.
    #[must_use]
    pub fn pos(&self) -> ChunkPos {
        self.pos
    }

    /// Whether this chunk has unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Clears the dirty flag after a successful save.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Marks the chunk as edited.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Number of distinct blocks in this chunk's palette.
    #[must_use]
    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    /// The palette of this chunk.
    #[must_use]
    pub fn palette(&self) -> &[BlockId] {
        &self.palette
    }

    /// Bits per index in the packed representation.
    #[must_use]
    pub fn bits_per_index(&self) -> u8 {
        self.bits
    }

    /// Approximate in-memory size of this chunk's storage, in bytes.
    #[must_use]
    pub fn storage_bytes(&self) -> usize {
        self.data.len() * std::mem::size_of::<u64>()
    }

    /// Gets the block at local coordinates. Out-of-range coordinates return air.
    #[must_use]
    pub fn get_local(&self, local: [usize; 3]) -> BlockId {
        if local.iter().any(|&v| v >= CHUNK_SIZE as usize) {
            return AIR;
        }
        if self.bits == 0 {
            return self.palette.first().copied().unwrap_or(AIR);
        }
        let index = local_to_index(local);
        let per_word = 64 / self.bits as usize;
        let word = index / per_word;
        let shift = (index % per_word) * self.bits as usize;
        let mask = (1u64 << self.bits) - 1;
        let packed = self.data.get(word).copied().unwrap_or(0);
        let palette_index = ((packed >> shift) & mask) as usize;
        self.palette.get(palette_index).copied().unwrap_or(AIR)
    }

    /// Sets the block at local coordinates. Out-of-range coordinates are ignored.
    pub fn set_local(&mut self, local: [usize; 3], block: BlockId) {
        if local.iter().any(|&v| v >= CHUNK_SIZE as usize) {
            return;
        }
        if self.bits == 0 {
            if self.palette.first().copied().unwrap_or(AIR) == block {
                return;
            }
            // Palette must grow: upgrade from the single-value form, then write below.
            self.grow_to_two(block);
        }
        let palette_index = match self.palette.iter().position(|&b| b == block) {
            Some(i) => i,
            None => {
                let i = self.palette.len();
                self.palette.push(block);
                let needed = bits_for(self.palette.len());
                if needed > self.bits {
                    self.repack(needed);
                }
                i
            }
        };
        let index = local_to_index(local);
        let per_word = 64 / self.bits as usize;
        let word = index / per_word;
        let shift = (index % per_word) * self.bits as usize;
        let mask = (1u64 << self.bits) - 1;
        if let Some(slot) = self.data.get_mut(word) {
            *slot = (*slot & !(mask << shift)) | ((palette_index as u64 & mask) << shift);
            self.dirty = true;
        }
    }

    /// Fills the whole chunk with one block, resetting the palette.
    pub fn fill(&mut self, block: BlockId) {
        self.palette = vec![block];
        self.data.clear();
        self.bits = 0;
        self.dirty = true;
    }

    /// Returns the blocks as a flat vector in local-index order. Used by the mesher and tests.
    #[must_use]
    pub fn to_vec(&self) -> Vec<BlockId> {
        (0..CHUNK_VOLUME)
            .map(|i| self.get_local(index_to_local(i)))
            .collect()
    }

    fn grow_to_two(&mut self, block: BlockId) {
        let existing = self.palette.first().copied().unwrap_or(AIR);
        self.palette = vec![existing, block];
        self.bits = 1;
        // Fill everything with index 0 (the original block); the set_local caller overwrites one.
        self.data = vec![0u64; CHUNK_VOLUME.div_ceil(64)];
    }

    fn repack(&mut self, new_bits: u8) {
        let old_bits = self.bits;
        let old_data = std::mem::take(&mut self.data);
        let per_word_old = 64 / old_bits.max(1) as usize;
        let per_word_new = 64 / new_bits as usize;
        let mut data = vec![0u64; CHUNK_VOLUME.div_ceil(per_word_new)];
        let old_mask = if old_bits == 0 {
            0
        } else {
            (1u64 << old_bits) - 1
        };
        for index in 0..CHUNK_VOLUME {
            let value = if old_bits == 0 {
                0u64
            } else {
                let word = index / per_word_old;
                let shift = (index % per_word_old) * old_bits as usize;
                (old_data.get(word).copied().unwrap_or(0) >> shift) & old_mask
            };
            let word = index / per_word_new;
            let shift = (index % per_word_new) * new_bits as usize;
            data[word] |= value << shift;
        }
        self.bits = new_bits;
        self.data = data;
    }
}

/// Bits needed to index a palette of `len` entries.
#[must_use]
pub fn bits_for(len: usize) -> u8 {
    match len {
        0 | 1 => 0,
        2 => 1,
        _ => {
            let needed = (usize::BITS - (len - 1).leading_zeros()) as u8;
            match needed {
                0..=1 => 1,
                2 => 2,
                3..=4 => 4,
                5..=8 => 8,
                _ => 16,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos() -> ChunkPos {
        ChunkPos::new(0, 0, 0)
    }

    #[test]
    fn empty_chunk_is_air() {
        let chunk = Chunk::empty(pos());
        assert_eq!(chunk.get_local([0, 0, 0]), AIR);
        assert_eq!(chunk.get_local([31, 31, 31]), AIR);
        assert_eq!(chunk.bits_per_index(), 0);
        assert_eq!(chunk.storage_bytes(), 0);
    }

    #[test]
    fn single_block_set_upgrades_palette() {
        let mut chunk = Chunk::empty(pos());
        chunk.set_local([1, 2, 3], BlockId(7));
        assert_eq!(chunk.get_local([1, 2, 3]), BlockId(7));
        assert_eq!(chunk.get_local([1, 2, 4]), AIR);
        assert_eq!(chunk.palette_len(), 2);
        assert!(chunk.is_dirty());
    }

    #[test]
    fn many_blocks_round_trip() {
        let mut chunk = Chunk::empty(pos());
        let mut expected = vec![AIR; CHUNK_VOLUME];
        for (i, slot) in expected.iter_mut().enumerate() {
            let block = BlockId(((i * 7) % 250) as u16);
            chunk.set_local(index_to_local(i), block);
            *slot = block;
        }
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(chunk.get_local(index_to_local(i)), *want, "index {i}");
        }
        assert_eq!(chunk.to_vec(), expected);
    }

    #[test]
    fn palette_widths_are_correct() {
        assert_eq!(bits_for(0), 0);
        assert_eq!(bits_for(1), 0);
        assert_eq!(bits_for(2), 1);
        assert_eq!(bits_for(3), 2);
        assert_eq!(bits_for(4), 2);
        assert_eq!(bits_for(5), 4);
        assert_eq!(bits_for(16), 4);
        assert_eq!(bits_for(17), 8);
        assert_eq!(bits_for(256), 8);
        assert_eq!(bits_for(257), 16);
    }

    #[test]
    fn uniform_chunk_compresses_to_nothing() {
        let mut chunk = Chunk::filled(pos(), BlockId(3));
        for y in 0..CHUNK_SIZE as usize {
            for z in 0..CHUNK_SIZE as usize {
                for x in 0..CHUNK_SIZE as usize {
                    assert_eq!(chunk.get_local([x, y, z]), BlockId(3));
                }
            }
        }
        assert_eq!(chunk.storage_bytes(), 0);
        // A single edit must still be correct after the palette upgrade.
        chunk.set_local([5, 5, 5], BlockId(9));
        assert_eq!(chunk.get_local([5, 5, 5]), BlockId(9));
        assert_eq!(chunk.get_local([5, 5, 6]), BlockId(3));
    }

    #[test]
    fn out_of_range_access_is_air() {
        let chunk = Chunk::empty(pos());
        assert_eq!(chunk.get_local([32, 0, 0]), AIR);
    }
}
