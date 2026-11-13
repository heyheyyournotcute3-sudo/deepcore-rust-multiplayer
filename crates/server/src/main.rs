//! Deepcore dedicated server: a headless Bevy app that owns the world and every game rule.

use std::time::Instant;

use anyhow::Result;
use bevy::{
    app::{AppExit, ScheduleRunnerPlugin},
    prelude::*,
    time::common_conditions::on_timer,
};
use clap::Parser;
use deepcore_shared::{SERVER_TICK_HZ, SharedPlugin, content::ContentRegistry};

mod bots;
mod loot;
mod machines;
mod net;
mod persist;
mod research;
mod sim;
mod world;
use tracing::info;

/// Command-line options for the dedicated server.
#[derive(Parser, Debug, Clone, Resource)]
#[command(name = "server", about = "Deepcore authoritative game server")]
pub struct ServerArgs {
    /// UDP port to listen on.
    #[arg(long, default_value_t = 7777)]
    pub port: u16,
    /// World seed. The same seed always produces the same mine.
    #[arg(long, default_value_t = 1337)]
    pub seed: u64,
    /// Artificial one-way latency to simulate, in milliseconds.
    #[arg(long, default_value_t = 0)]
    pub simulate_latency: u32,
    /// Artificial packet loss to simulate, in percent.
    #[arg(long, default_value_t = 0)]
    pub simulate_loss: u8,
    /// Number of headless bot clients to spawn for load and desync testing.
    #[arg(long, default_value_t = 0)]
    pub bots: u32,
    /// Exit after this many seconds (used by automated smoke tests).
    #[arg(long)]
    pub exit_after: Option<f32>,
    /// Directory for the save files.
    #[arg(long, default_value = "saves")]
    pub save_dir: String,
}

/// Tracks how long the last simulation tick took, so the server can report its health.
#[derive(Resource, Debug)]
struct TickStats {
    ticks: u64,
    last_report: Instant,
}

impl Default for TickStats {
    fn default() -> Self {
        Self {
            ticks: 0,
            last_report: Instant::now(),
        }
    }
}

/// Marker for the headless camera-free root entity; keeps the world non-empty and greppable.
#[derive(Component)]
struct ServerRoot;

fn main() -> Result<()> {
    let args = ServerArgs::parse();

    // Load and validate content before starting: a broken content file must fail loudly.
    let content_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
    let registry = ContentRegistry::load_from_dir(&content_dir)?;
    info!(
        blocks = registry.blocks.len(),
        recipes = registry.recipes.len(),
        hash = registry.content_hash(),
        "content loaded"
    );

    App::new()
        .insert_resource(args.clone())
        .insert_resource(registry)
        .add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_secs_f64(1.0 / SERVER_TICK_HZ as f64),
        )))
        .add_plugins(bevy::log::LogPlugin::default())
        .add_plugins(SharedPlugin)
        .add_plugins(bevy::state::app::StatesPlugin)
        .add_plugins((
            net::NetPlugin,
            world::WorldPlugin,
            sim::SimPlugin,
            research::ResearchPlugin,
            machines::MachinePlugin,
            persist::PersistencePlugin,
            bots::BotPlugin,
            loot::LootPlugin,
        ))
        .insert_resource(persist::SavePaths::new(&args.save_dir))
        .init_resource::<TickStats>()
        .add_systems(Startup, setup)
        .add_systems(FixedUpdate, count_tick)
        .add_systems(
            FixedUpdate,
            report_health.run_if(on_timer(std::time::Duration::from_secs(5))),
        )
        .add_systems(Update, exit_if_requested)
        .run();

    Ok(())
}

fn setup(mut commands: Commands, args: Res<ServerArgs>) {
    commands.spawn((ServerRoot, Name::new("DeepcoreServer")));
    info!(
        port = args.port,
        seed = args.seed,
        latency_ms = args.simulate_latency,
        loss_pct = args.simulate_loss,
        bots = args.bots,
        "Deepcore server starting at {} Hz",
        SERVER_TICK_HZ
    );
}

fn count_tick(mut stats: ResMut<TickStats>) {
    stats.ticks += 1;
}

fn report_health(mut stats: ResMut<TickStats>) {
    let elapsed = stats.last_report.elapsed().as_secs_f32();
    let rate = stats.ticks as f32 / elapsed.max(f32::EPSILON);
    info!(
        ticks = stats.ticks,
        measured_hz = rate,
        "server tick health"
    );
    stats.ticks = 0;
    stats.last_report = Instant::now();
}

fn exit_if_requested(
    args: Res<ServerArgs>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
    mut exit: MessageWriter<AppExit>,
    world: Option<Res<world::ServerWorld>>,
    paths: Option<Res<persist::SavePaths>>,
) {
    let Some(limit) = args.exit_after else {
        return;
    };
    *elapsed += time.delta_secs();
    if *elapsed >= limit {
        info!("reached --exit-after limit, shutting down");
        if let (Some(world), Some(paths)) = (world, paths) {
            persist::save_on_shutdown(&world, &paths);
        }
        exit.write(AppExit::Success);
    }
}
