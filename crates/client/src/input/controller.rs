//! First-person controller: mouse look, WASD movement through the shared movement code, and
//! the camera rig.

use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use deepcore_shared::{
    content::ContentRegistry,
    movement::{CollisionWorld, MoveInput, PlayerBody, base_spawn, step_body},
    voxel::{BlockPos, coords::SURFACE_Y},
};

use crate::{prediction::PredictionState, render::chunks::ClientChunks};

/// The client's local player state, predicted every frame.
#[derive(Resource)]
pub struct LocalPlayer {
    /// The predicted body.
    pub body: PlayerBody,
    /// Input accumulated since the last fixed tick.
    pub input: MoveInput,
    /// Whether the mouse is captured for looking around.
    pub captured: bool,
    /// Whether the player is sprinting (for the HUD).
    pub sprinting: bool,
    /// Whether the player is crouching.
    pub crouching: bool,
}

impl Default for LocalPlayer {
    fn default() -> Self {
        Self {
            body: {
                let mut body = PlayerBody::new(base_spawn(&Default::default()));
                // Start looking slightly down, as a miner entering the camp would.
                body.pitch = -0.28;
                body
            },
            input: MoveInput::default(),
            captured: false,
            sprinting: false,
            crouching: false,
        }
    }
}

/// Marker for the first-person camera entity.
#[derive(Component)]
pub struct FirstPersonCamera;

/// Plugin providing movement input and the camera.
pub struct PlayerControllerPlugin;

impl Plugin for PlayerControllerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalPlayer>()
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, (grab_cursor, read_look_input, move_player).chain())
            .add_systems(Update, follow_camera.after(move_player));
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        FirstPersonCamera,
        // Mark this camera as the owner of the primary egui context, so the UI pass always
        // has a target even though the camera is spawned during Startup.
        bevy_egui::PrimaryEguiContext,
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 75f32.to_radians(),
            near: 0.05,
            far: 600.0,
            ..default()
        }),
        Transform::from_xyz(0.5, 2.0, 0.5).looking_at(Vec3::new(0.5, 2.0, -1.0), Vec3::Y),
    ));
}

/// Captures the cursor when the window is clicked and releases it on escape.
fn grab_cursor(
    mut windows: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut player: ResMut<LocalPlayer>,
) {
    let Ok(mut cursor) = windows.single_mut() else {
        return;
    };
    if mouse.just_pressed(MouseButton::Left) && !player.captured {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
        player.captured = true;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
        player.captured = false;
    }
}

/// Reads mouse motion into yaw and pitch.
fn read_look_input(motion: Res<AccumulatedMouseMotion>, mut player: ResMut<LocalPlayer>) {
    if !player.captured {
        return;
    }
    const SENSITIVITY: f32 = 0.0022;
    player.body.yaw -= motion.delta.x * SENSITIVITY;
    player.body.pitch -= motion.delta.y * SENSITIVITY;
    player.body.pitch = player.body.pitch.clamp(-1.5533, 1.5533);
    player.input.yaw = player.body.yaw;
    player.input.pitch = player.body.pitch;
}

/// Advances the predicted body with the shared movement code.
fn move_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    registry: Res<ContentRegistry>,
    chunks: Res<ClientChunks>,
    mut player: ResMut<LocalPlayer>,
    mut prediction: ResMut<PredictionState>,
) {
    let mut forward = 0.0;
    let mut right = 0.0;
    if keys.pressed(KeyCode::KeyW) {
        forward += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        forward -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        right += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        right -= 1.0;
    }
    let sprint = keys.pressed(KeyCode::ShiftLeft);
    let crouch = keys.pressed(KeyCode::ControlLeft);
    let jump = keys.pressed(KeyCode::Space);

    player.sprinting = sprint && (forward != 0.0 || right != 0.0);
    player.crouching = crouch;
    player.input = MoveInput {
        forward,
        right,
        jump,
        sprint,
        crouch,
        yaw: player.body.yaw,
        pitch: player.body.pitch,
    };

    // Step with a fixed substep so behaviour matches the server regardless of frame rate.
    let dt = time.delta_secs().min(0.1);
    let world = ChunkCollision {
        chunks: &chunks,
        registry: &registry,
    };
    let input = player.input;
    step_body(&mut player.body, &input, &registry.balance, &world, dt);

    // Record the input so it can be sent to the server and replayed after a correction.
    // In single-process play nothing acknowledges it, so the buffer stays bounded internally.
    prediction.record(player.body);
}

/// Places the camera at the player's eyes, using the predicted body.
fn follow_camera(
    player: Res<LocalPlayer>,
    registry: Res<ContentRegistry>,
    time: Res<Time>,
    mut prediction: ResMut<PredictionState>,
    mut camera: Query<&mut Transform, With<FirstPersonCamera>>,
) {
    let Ok(mut transform) = camera.single_mut() else {
        return;
    };
    // A pending server correction is removed smoothly over a short window rather than
    // snapping the camera, which keeps 150 ms connections feeling responsive.
    let smoothing = prediction.tick_smoothing(time.delta_secs());
    let eye = player.body.eye_position(&registry.balance);
    transform.translation = Vec3::from_array(eye) + smoothing;
    let dir = player.body.look_direction();
    transform.look_to(Vec3::from_array(dir), Vec3::Y);
}

/// Collision view over the client's loaded chunks.
pub struct ChunkCollision<'a> {
    /// The loaded chunks.
    pub chunks: &'a ClientChunks,
    /// Content, for the solid flags.
    pub registry: &'a ContentRegistry,
}

impl CollisionWorld for ChunkCollision<'_> {
    fn is_solid(&self, pos: BlockPos) -> bool {
        // The world floor and walls are solid even before their chunks stream in.
        if pos.y <= SURFACE_Y - deepcore_shared::worldgen::WORLD_DEPTH {
            return true;
        }
        match self.chunks.get_block(pos) {
            Some(block) => self
                .registry
                .block_def(block)
                .is_some_and(|d| d.solid && d.hardness > 0.0),
            // Unloaded space is treated as empty so the player is never trapped while streaming.
            None => false,
        }
    }
}
