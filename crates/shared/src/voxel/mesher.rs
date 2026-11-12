//! Greedy voxel meshing.
//!
//! The mesher walks the three axes of a chunk, builds a mask of visible faces per slice and
//! merges adjacent identical faces into large quads. A solid 32³ chunk becomes six quads
//! instead of 12,288 triangles, which is what keeps the frame rate up.
//!
//! Output quads carry their block ID (so the client can pick the atlas cell), tile-space UVs
//! (so the shader can tile textures across a merged quad) and a baked face shade factor.

use super::coords::CHUNK_SIZE;
use crate::content::BlockId;

/// One merged quad produced by the mesher.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    /// Block type this face belongs to, used to select the atlas cell.
    pub block: BlockId,
    /// The four corners in chunk-local block space, counter-clockwise when viewed from outside.
    pub corners: [[f32; 3]; 4],
    /// Outward face normal.
    pub normal: [f32; 3],
    /// UVs in tile units: `(0,0)`, `(w,0)`, `(w,h)`, `(0,h)`.
    pub uvs: [[f32; 2]; 4],
    /// Baked directional shade in `0.35..1.0`, applied as vertex colour.
    pub shade: f32,
}

impl Quad {
    /// Appends this quad's vertices and indices to the given buffers.
    pub fn push_into(
        &self,
        positions: &mut Vec<[f32; 3]>,
        normals: &mut Vec<[f32; 3]>,
        uvs: &mut Vec<[f32; 2]>,
        colors: &mut Vec<[f32; 4]>,
        indices: &mut Vec<u32>,
    ) {
        let base = positions.len() as u32;
        for i in 0..4 {
            positions.push(self.corners[i]);
            normals.push(self.normal);
            uvs.push(self.uvs[i]);
            colors.push([self.shade, self.shade, self.shade, 1.0]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Accumulated mesh data for one chunk, ready to be handed to Bevy as a [`Mesh`].
#[derive(Debug, Clone, Default)]
pub struct MeshData {
    /// Vertex positions in chunk-local space.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals.
    pub normals: Vec<[f32; 3]>,
    /// Vertex UVs in tile units.
    pub uvs: Vec<[f32; 2]>,
    /// Vertex colours carrying baked face shading.
    pub colors: Vec<[f32; 4]>,
    /// Triangle indices.
    pub indices: Vec<u32>,
    /// Number of quads merged into this mesh, for diagnostics.
    pub quads: usize,
}

impl MeshData {
    /// Appends a quad to this mesh.
    pub fn push_quad(&mut self, quad: &Quad) {
        quad.push_into(
            &mut self.positions,
            &mut self.normals,
            &mut self.uvs,
            &mut self.colors,
            &mut self.indices,
        );
        self.quads += 1;
    }

    /// True when the mesh has no geometry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Directional shade per face, indexed like the mesher's axis order (+X, -X, +Y, -Y, +Z, -Z).
const SHADE: [f32; 6] = [0.78, 0.72, 1.0, 0.45, 0.62, 0.58];

/// Meshes a chunk using greedy face merging.
///
/// `get` must return the block at a chunk-local coordinate, and may be called with any
/// coordinate in `-1..=CHUNK_SIZE` so the mesher can cull faces against neighbouring chunks.
/// `solid` decides which blocks occlude faces; non-solid blocks (air, water, gas) are not meshed.
pub fn greedy_mesh<F, S>(get: F, solid: S) -> MeshData
where
    F: Fn(i32, i32, i32) -> BlockId,
    S: Fn(BlockId) -> bool,
{
    let mut mesh = MeshData::default();
    let size = CHUNK_SIZE;
    let mut mask: Vec<Option<(BlockId, bool)>> = vec![None; (size * size) as usize];

    for d in 0..3usize {
        let u = (d + 1) % 3;
        let v = (d + 2) % 3;

        // Slice from -1 so faces on the chunk's negative boundary are considered too.
        for slice in -1..size {
            // Build the face mask for the plane between `slice` and `slice + 1`.
            for j in 0..size {
                for i in 0..size {
                    let mut a_pos = [0i32; 3];
                    a_pos[d] = slice;
                    a_pos[u] = i;
                    a_pos[v] = j;
                    let mut b_pos = a_pos;
                    b_pos[d] = slice + 1;

                    let a = get(a_pos[0], a_pos[1], a_pos[2]);
                    let b = get(b_pos[0], b_pos[1], b_pos[2]);
                    let a_solid = solid(a);
                    let b_solid = solid(b);

                    let cell = if a_solid && !b_solid {
                        Some((a, false))
                    } else if b_solid && !a_solid {
                        Some((b, true))
                    } else {
                        None
                    };
                    mask[(i + j * size) as usize] = cell;
                }
            }

            // Greedy merge the mask into rectangles.
            let mut n = 0usize;
            for j in 0..size {
                let mut i = 0i32;
                while i < size {
                    let current = mask[n];
                    let Some((block, back)) = current else {
                        i += 1;
                        n += 1;
                        continue;
                    };

                    // Grow width along u.
                    let mut w = 1i32;
                    while i + w < size && mask[n + w as usize] == current {
                        w += 1;
                    }

                    // Grow height along v.
                    let mut h = 1i32;
                    'grow: while j + h < size {
                        for k in 0..w {
                            let idx = n + k as usize + (h as usize) * size as usize;
                            if mask[idx] != current {
                                break 'grow;
                            }
                        }
                        h += 1;
                    }

                    // Emit the merged quad.
                    let mut base = [0i32; 3];
                    base[d] = slice + i32::from(back);
                    base[u] = i;
                    base[v] = j;

                    let mut du = [0i32; 3];
                    du[u] = w;
                    let mut dv = [0i32; 3];
                    dv[v] = h;

                    let p0 = [base[0] as f32, base[1] as f32, base[2] as f32];
                    let p1 = [
                        (base[0] + du[0]) as f32,
                        (base[1] + du[1]) as f32,
                        (base[2] + du[2]) as f32,
                    ];
                    let p2 = [
                        (base[0] + du[0] + dv[0]) as f32,
                        (base[1] + du[1] + dv[1]) as f32,
                        (base[2] + du[2] + dv[2]) as f32,
                    ];
                    let p3 = [
                        (base[0] + dv[0]) as f32,
                        (base[1] + dv[1]) as f32,
                        (base[2] + dv[2]) as f32,
                    ];

                    let mut normal = [0f32; 3];
                    normal[d] = if back { 1.0 } else { -1.0 };
                    let shade_index = d * 2 + usize::from(back);
                    let shade = SHADE[shade_index];

                    let wf = w as f32;
                    let hf = h as f32;
                    let corners = if back {
                        [p3, p2, p1, p0]
                    } else {
                        [p0, p1, p2, p3]
                    };
                    let uvs = if back {
                        [[0.0, 0.0], [0.0, hf], [wf, hf], [wf, 0.0]]
                    } else {
                        [[0.0, 0.0], [wf, 0.0], [wf, hf], [0.0, hf]]
                    };

                    mesh.push_quad(&Quad {
                        block,
                        corners,
                        normal,
                        uvs,
                        shade,
                    });

                    // Clear the consumed rectangle.
                    for l in 0..h {
                        for k in 0..w {
                            mask[n + k as usize + l as usize * size as usize] = None;
                        }
                    }

                    i += w;
                    n += w as usize;
                }
            }
        }
    }

    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn air_only(_: i32, _: i32, _: i32) -> BlockId {
        BlockId(0)
    }

    /// Everything that is not air occludes faces.
    fn solid_all(b: BlockId) -> bool {
        b != BlockId(0)
    }

    #[test]
    fn empty_chunk_produces_no_geometry() {
        let mesh = greedy_mesh(air_only, solid_all);
        assert!(mesh.is_empty());
        assert_eq!(mesh.quads, 0);
    }

    #[test]
    fn single_block_makes_six_quads() {
        let stone = BlockId(1);
        let get = |x: i32, y: i32, z: i32| {
            if (x, y, z) == (0, 0, 0) {
                stone
            } else {
                BlockId(0)
            }
        };
        let mesh = greedy_mesh(get, solid_all);
        assert_eq!(mesh.quads, 6, "a lone cube has six visible faces");
        assert_eq!(mesh.positions.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
    }

    #[test]
    fn solid_chunk_makes_six_big_quads() {
        let stone = BlockId(1);
        // Solid inside the chunk, air outside it.
        let get = |x: i32, y: i32, z: i32| {
            if (0..CHUNK_SIZE).contains(&x)
                && (0..CHUNK_SIZE).contains(&y)
                && (0..CHUNK_SIZE).contains(&z)
            {
                stone
            } else {
                BlockId(0)
            }
        };
        let mesh = greedy_mesh(get, solid_all);
        assert_eq!(mesh.quads, 6, "a full chunk merges to one quad per side");
    }

    #[test]
    fn identical_neighbours_merge() {
        let stone = BlockId(1);
        // A 4x1x1 bar inside the chunk should produce 6 quads, not 24.
        let get = |x: i32, y: i32, z: i32| {
            if (0..4).contains(&x) && y == 0 && z == 0 {
                stone
            } else {
                BlockId(0)
            }
        };
        let mesh = greedy_mesh(get, solid_all);
        assert_eq!(mesh.quads, 6);
    }

    #[test]
    fn different_blocks_do_not_merge() {
        let a = BlockId(1);
        let b = BlockId(2);
        let get = |x: i32, y: i32, z: i32| {
            if y == 0 && z == 0 && x == 0 {
                a
            } else if y == 0 && z == 0 && x == 1 {
                b
            } else {
                BlockId(0)
            }
        };
        let mesh = greedy_mesh(get, solid_all);
        // Each block still has 6 faces, but the shared faces between them are culled.
        assert_eq!(mesh.quads, 10);
    }

    #[test]
    fn non_solid_blocks_are_skipped() {
        let water = BlockId(5);
        let get = |x: i32, y: i32, z: i32| {
            if (x, y, z) == (0, 0, 0) {
                water
            } else {
                BlockId(0)
            }
        };
        let solid = |b: BlockId| b != water && b != BlockId(0);
        let mesh = greedy_mesh(get, solid);
        assert!(mesh.is_empty(), "water is not meshed as solid geometry");
    }

    #[test]
    fn neighbour_faces_are_culled_across_chunk_borders() {
        let stone = BlockId(1);
        // Block at the +X edge of the chunk, with a neighbour just outside it.
        let get = |x: i32, y: i32, z: i32| {
            if (x == 31 && y == 0 && z == 0) || (x == 32 && y == 0 && z == 0) {
                stone
            } else {
                BlockId(0)
            }
        };
        let mesh = greedy_mesh(get, solid_all);
        // The +X face is culled, so only 5 quads remain.
        assert_eq!(mesh.quads, 5);
    }
}
