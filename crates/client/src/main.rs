//! Deepcore client: windowed Bevy app that renders the mine and talks to the server.

use anyhow::Result;
use bevy::{
    core_pipeline::tonemapping::Tonemapping,
    prelude::*,
    window::{PresentMode, WindowResolution},
};
use clap::Parser;
use deepcore_shared::SharedPlugin;
use tracing::info;

/// Command-line options for the client.
#[derive(Parser, Debug, Clone, Resource)]
#[command(name = "client", about = "Deepcore game client")]
pub struct ClientArgs {
    /// Join a server at this address instead of showing the menu.
    #[arg(long)]
    pub connect: Option<String>,
    /// Server port used with `--connect`.
    #[arg(long, default_value_t = 7777)]
    pub port: u16,
    /// Display name shown to teammates.
    #[arg(long, default_value = "Miner")]
    pub name: String,
    /// Artificial one-way latency to simulate, in milliseconds.
    #[arg(long, default_value_t = 0)]
    pub simulate_latency: u32,
    /// Artificial packet loss to simulate, in percent.
    #[arg(long, default_value_t = 0)]
    pub simulate_loss: u8,
    /// Render this many frames and then exit (used by automated smoke tests).
    #[arg(long)]
    pub exit_after_frames: Option<u32>,
}

fn main() -> Result<()> {
    let args = ClientArgs::parse();

    App::new()
        .insert_resource(args.clone())
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
        .add_systems(Startup, setup_scene)
        .add_systems(Update, (spin_cube, exit_after_frames))
        .run();

    Ok(())
}

/// Marker for the placeholder test cube shown at startup.
#[derive(Component)]
struct SpinCube;

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Ground plane so the cube has something to sit on.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(16.0, 16.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.24, 0.20),
            perceptual_roughness: 1.0,
            ..default()
        })),
    ));

    commands.spawn((
        SpinCube,
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.55, 0.50, 0.48),
            perceptual_roughness: 0.8,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.5, 0.0),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: light_consts::lux::OVERCAST_DAY,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Camera3d::default(),
        Tonemapping::AcesFitted,
        Transform::from_xyz(-3.5, 2.5, 6.0).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));

    info!("client started; M0 skeleton scene loaded");
}

fn spin_cube(time: Res<Time>, mut cubes: Query<&mut Transform, With<SpinCube>>) {
    for mut transform in &mut cubes {
        transform.rotate_y(time.delta_secs() * 0.8);
    }
}

fn exit_after_frames(
    args: Res<ClientArgs>,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(limit) = args.exit_after_frames else {
        return;
    };
    *frames += 1;
    if *frames >= limit {
        info!(
            frames = *frames,
            "reached --exit-after-frames limit, shutting down"
        );
        exit.write(AppExit::Success);
    }
}
