//! Generates the block texture atlas PNG from `assets/data/blocks.ron`.
//!
//! Run with `cargo run -p atlas_gen`. The client also generates the atlas in memory at
//! startup, so this tool exists for inspection and for shipping a prebuilt PNG.

use std::path::PathBuf;

use anyhow::{Context, Result};
use deepcore_shared::atlas::{AtlasLayout, build_atlas_pixels};
use deepcore_shared::content::ContentRegistry;

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let data = root.join("assets/data");
    let registry = ContentRegistry::load_from_dir(&data).context("loading content")?;
    let layout = AtlasLayout::new(registry.blocks.len());
    let pixels = build_atlas_pixels(&registry, &layout);

    let out_dir = root.join("assets/textures");
    std::fs::create_dir_all(&out_dir)?;
    let out = out_dir.join("atlas.png");

    let file = std::fs::File::create(&out)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), layout.size, layout.size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&pixels)?;

    println!(
        "wrote {} ({}x{}, {} tiles, {} blocks)",
        out.display(),
        layout.size,
        layout.size,
        layout.tile_count(),
        registry.blocks.len()
    );
    Ok(())
}
