use deepcore_shared::content::ContentRegistry;
use deepcore_shared::voxel::ChunkPos;
use deepcore_shared::worldgen::WorldGen;
use std::collections::BTreeMap;

fn main() {
    let dir = std::path::Path::new("assets/data");
    let reg = ContentRegistry::load_from_dir(dir).unwrap();
    let g = WorldGen::new(&reg, 4242).unwrap();
    for pos in [ChunkPos::new(3, -6, -3), ChunkPos::new(3, -3, -3)] {
        let c = g.generate_chunk(pos);
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for b in c.to_vec() {
            *counts.entry(reg.block_name(b).to_string()).or_default() += 1;
        }
        println!(
            "chunk {pos:?} origin {:?} depth {}",
            pos.origin(),
            pos.origin().depth()
        );
        for (k, v) in counts {
            println!("  {k}: {v}");
        }
    }
}
