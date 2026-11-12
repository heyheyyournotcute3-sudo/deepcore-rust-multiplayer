//! Scene lighting: a sun for the surface camp and a headlamp for the mine.
//!
//! Below layer 1 the world is dark by design, so the player's headlamp is the main light
//! source. The sun fades out as the player descends.

use bevy::prelude::*;

use crate::input::controller::FirstPersonCamera;

/// Plugin that spawns and drives the scene lights.
pub struct LightingPlugin;

impl Plugin for LightingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_lights)
            .add_systems(Update, drive_headlamp);
    }
}

/// Marker for the headlamp's spotlight, parented to the camera.
#[derive(Component)]
pub struct Headlamp;

/// Marker for the sun.
#[derive(Component)]
pub struct Sun;

fn spawn_lights(mut commands: Commands) {
    commands.spawn((
        Sun,
        DirectionalLight {
            illuminance: 30_000.0,
            // Shadow maps over a 1024-block world need very large cascades; they produce
            // acne at this scale and cost a lot on integrated GPUs. The mine is lit by lamps,
            // so the sun does not cast shadows.
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(40.0, 90.0, 40.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // A little ambient so unlit surfaces are not pure black; the mine still needs a lamp.
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.68, 0.76, 0.92),
        brightness: 1600.0,
        ..default()
    });

    // The headlamp follows the camera; a spotlight is parented to it below.
    commands.spawn((
        Headlamp,
        SpotLight {
            intensity: 220_000.0,
            range: 34.0,
            outer_angle: 0.62,
            inner_angle: 0.28,
            color: Color::srgb(1.0, 0.96, 0.86),
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::default(),
    ));
}

/// Keeps the headlamp at the camera and fades the sun with depth.
fn drive_headlamp(
    camera: Query<(&Transform, &GlobalTransform), With<FirstPersonCamera>>,
    mut headlamp: Query<&mut Transform, (With<Headlamp>, Without<FirstPersonCamera>)>,
    mut sun: Query<&mut DirectionalLight, (With<Sun>, Without<Headlamp>)>,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    let Ok((transform, _global)) = camera.single() else {
        return;
    };
    if let Ok(mut lamp) = headlamp.single_mut() {
        lamp.translation = transform.translation;
        lamp.rotation = transform.rotation;
    }

    // Sunlight fades from full at the surface to nothing 20 blocks down.
    let depth = (-transform.translation.y).max(0.0);
    let daylight = (1.0 - depth / 24.0).clamp(0.0, 1.0);
    if let Ok(mut light) = sun.single_mut() {
        light.illuminance = 30_000.0 * daylight;
    }
    ambient.brightness = 70.0 + 1530.0 * daylight;
}
