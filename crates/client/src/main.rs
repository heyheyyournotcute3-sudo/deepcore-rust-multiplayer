//! Deepcore client: windowed Bevy app that renders the mine and talks to the server.

use std::path::PathBuf;

use anyhow::Result;
// Bevy 0.20 also provides a `Context` extension trait on Result, so call the anyhow one
// explicitly to avoid an ambiguous method call.
use anyhow::Context as AnyhowContext;
use bevy::{
    app::AppExit,
    prelude::*,
    render::view::screenshot::{Screenshot, save_to_disk},
    window::{PresentMode, WindowResolution},
};
use clap::Parser;
use deepcore_shared::{SharedPlugin, content::ContentRegistry, worldgen::WorldGen};
use tracing::info;

mod audio;
mod hud;
mod input;
mod net;
mod prediction;
mod render;
mod ui;

/// Command-line options for the client.
#[derive(Parser, Debug, Clone, Resource)]
#[command(name = "client", about = "Deepcore game client")]
pub struct ClientArgs {
    /// Join a server at this address instead of playing the local world.
    #[arg(long)]
    pub connect: Option<String>,
    /// Server port used with `--connect`.
    #[arg(long, default_value_t = 7777)]
    pub port: u16,
    /// Display name shown to teammates.
    #[arg(long, default_value = "Miner")]
    pub name: String,
    /// World seed for the local world.
    #[arg(long, default_value_t = 1337)]
    pub seed: u64,
    /// Artificial one-way latency to simulate, in milliseconds.
    #[arg(long, default_value_t = 0)]
    pub simulate_latency: u32,
    /// Artificial packet loss to simulate, in percent.
    #[arg(long, default_value_t = 0)]
    pub simulate_loss: u8,
    /// Render this many frames and then exit (used by automated smoke tests).
    #[arg(long)]
    pub exit_after_frames: Option<u32>,
    /// Horizontal chunk view radius, in chunks.
    #[arg(long, default_value_t = 6)]
    pub view_radius: i32,
    /// Save a screenshot to this path just before exiting.
    #[arg(long)]
    pub screenshot: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = ClientArgs::parse();

    // Load and validate content before the app starts, so a broken content file fails loudly
    // instead of producing an empty world.
    let content_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
    let registry =
        AnyhowContext::with_context(ContentRegistry::load_from_dir(&content_dir), || {
            format!("loading content from {}", content_dir.display())
        })?;
    info!(
        blocks = registry.blocks.len(),
        items = registry.items.len(),
        machines = registry.machines.len(),
        recipes = registry.recipes.len(),
        hash = registry.content_hash(),
        "content loaded"
    );

    // Sanity-check that the world can be generated with this content and seed.
    AnyhowContext::context(
        WorldGen::new(&registry, args.seed),
        "building the world generator",
    )?;

    App::new()
        .insert_resource(args.clone())
        .insert_resource(registry)
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Deepcore".into(),
                resolution: WindowResolution::new(1280, 720),
                present_mode: PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(SharedPlugin)
        .add_plugins((
            render::RenderPlugin,
            input::InputPlugin,
            hud::HudPlugin,
            ui::UiPlugin,
            prediction::PredictionPlugin,
            audio::AudioPlugin,
            net::NetPlugin,
        ))
        .add_systems(Update, drive_lifecycle)
        .run();

    Ok(())
}

/// Handles `--exit-after-frames` and `--screenshot` for automated smoke tests.
fn drive_lifecycle(
    args: Res<ClientArgs>,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
    mut commands: Commands,
    mut captured: Local<bool>,
    stats: Res<crate::render::chunks::ChunkStats>,
) {
    *frames += 1;
    let Some(limit) = args.exit_after_frames else {
        return;
    };
    // Wait for the async mesher to settle before capturing, otherwise the screenshot shows an
    // empty world even though the game itself is fine a moment later.
    let meshed = stats.loaded > 0 && stats.pending == 0;
    if !*captured
        && meshed
        && *frames > 5
        && let Some(path) = &args.screenshot
    {
        *captured = true;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()));
        info!(
            frame = *frames,
            chunks = stats.loaded,
            "captured screenshot to {}",
            path.display()
        );
    }
    // Never exit while a screenshot is still pending: the capture needs a couple more frames
    // to read back from the GPU and write the file.
    let capture_settled = args.screenshot.is_none() || (*captured && *frames > limit + 20);
    if *frames >= limit && capture_settled {
        info!(
            frames = *frames,
            "reached --exit-after-frames limit, shutting down"
        );
        exit.write(AppExit::Success);
    }
}
