//! Block targeting, mining and placing from the client.
//!
//! The client raycasts from the camera to find the targeted block, draws an outline around it,
//! predicts the crack animation, and sends intents to the server. In single-process play the
//! same plugin applies the edit locally.

use bevy::prelude::*;
use deepcore_shared::{
    content::{ContentRegistry, types::ItemCategory},
    mining::{MiningCheck, area_blocks, check_mining, raycast},
    voxel::{AIR, BlockPos, coords::SURFACE_Y},
};

use crate::{
    input::controller::{ChunkCollision, LocalPlayer},
    render::chunks::ClientChunks,
};

/// The block the player is currently looking at.
#[derive(Resource, Default)]
pub struct Targeting {
    /// The hit block, if any.
    pub hit: Option<deepcore_shared::mining::RayHit>,
    /// Whether the current tool can break it.
    pub can_mine: bool,
    /// The tier the block needs, when it cannot be mined.
    pub required_tier: u8,
    /// Predicted seconds to break the target.
    pub break_seconds: f32,
    /// Seconds of mining progress on the current target.
    pub progress: f32,
}

/// Plugin for targeting, mining and placing.
pub struct MiningPlugin;

impl Plugin for MiningPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Targeting>()
            .add_systems(Startup, spawn_outline)
            .add_systems(
                Update,
                (update_targeting, update_outline, mine_and_place).chain(),
            );
    }
}

/// The wireframe outline drawn around the targeted block.
#[derive(Component)]
pub struct BlockOutline;

fn spawn_outline(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // A slightly oversized cube, drawn with an unlit material and no depth write so it reads
    // clearly against any block.
    let mesh = meshes.add(Cuboid::new(1.002, 1.002, 1.002));
    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 1.0, 1.0, 0.9),
        emissive: LinearRgba::new(2.0, 2.0, 2.0, 1.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    commands.spawn((
        BlockOutline,
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::default(),
        Visibility::Hidden,
    ));
}

/// Raycasts from the camera and works out whether the target can be mined.
fn update_targeting(
    mut targeting: ResMut<Targeting>,
    chunks: Res<ClientChunks>,
    registry: Res<ContentRegistry>,
    player: Res<LocalPlayer>,
) {
    let world = ChunkCollision {
        chunks: &chunks,
        registry: &registry,
    };
    let eye = player.body.eye_position(&registry.balance);
    let dir = player.body.look_direction();
    let reach = current_tool(&registry, &player).map_or(registry.balance.reach, |t| t.reach);

    let hit = raycast(
        eye,
        dir,
        reach,
        |pos| {
            chunks.get_block(pos).is_some_and(|b| {
                registry
                    .block_def(b)
                    .is_some_and(|d| d.solid && d.hardness > 0.0)
            })
        },
        |pos| pos.y > SURFACE_Y - deepcore_shared::worldgen::WORLD_DEPTH,
    );
    let _ = world;

    targeting.hit = hit;
    targeting.can_mine = false;
    targeting.required_tier = 0;
    targeting.break_seconds = 0.0;

    if let Some(hit) = hit
        && let Some(block) = chunks.get_block(hit.block)
        && let Some(def) = registry.block_def(block)
    {
        let tool = current_tool(&registry, &player).unwrap_or_else(|| {
            // Fall back to the starting pickaxe so the HUD always has something to show.
            default_tool(&registry)
        });
        match check_mining(
            &tool,
            def.hardness,
            def.min_tier,
            1.0,
            deepcore_shared::TICK_SECONDS,
        ) {
            MiningCheck::Allowed { seconds } => {
                targeting.can_mine = true;
                targeting.break_seconds = seconds;
            }
            MiningCheck::TooHard { required_tier } => {
                targeting.required_tier = required_tier;
            }
            MiningCheck::Impossible => {}
        }
    }
}

/// Moves the outline onto the targeted block.
fn update_outline(
    targeting: Res<Targeting>,
    mut outlines: Query<(&mut Transform, &mut Visibility), With<BlockOutline>>,
) {
    let Ok((mut transform, mut visibility)) = outlines.single_mut() else {
        return;
    };
    match targeting.hit {
        Some(hit) => {
            let centre = hit.block.center();
            transform.translation = Vec3::new(centre[0], centre[1], centre[2]);
            *visibility = Visibility::Visible;
        }
        None => *visibility = Visibility::Hidden,
    }
}

/// Applies mining and placing input.
fn mine_and_place(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut targeting: ResMut<Targeting>,
    mut chunks: ResMut<ClientChunks>,
    registry: Res<ContentRegistry>,
    player: Res<LocalPlayer>,
) {
    let Some(hit) = targeting.hit else {
        targeting.progress = 0.0;
        return;
    };
    if !mouse.pressed(MouseButton::Left) || !targeting.can_mine {
        targeting.progress = 0.0;
        return;
    }

    targeting.progress += time.delta_secs();
    if targeting.progress < targeting.break_seconds {
        return;
    }
    targeting.progress = 0.0;

    // Break the target (and the area around it for wide tools).
    let tool = current_tool(&registry, &player).unwrap_or_else(|| default_tool(&registry));
    for pos in area_blocks(hit.block, hit.face, tool.area) {
        if chunks.get_block(pos).is_some_and(|b| b != AIR) {
            chunks.set_block(pos, AIR);
        }
    }
}

/// The tool the player currently has selected, if it is a tool.
#[must_use]
pub fn current_tool(
    registry: &ContentRegistry,
    _player: &LocalPlayer,
) -> Option<deepcore_shared::content::types::ToolSpec> {
    // Until the hotbar UI lands, the player always holds the starting pickaxe.
    registry
        .item("steel_pickaxe")
        .and_then(|id| registry.item_def(id))
        .and_then(|def| match &def.category {
            ItemCategory::Tool(spec) => Some(*spec),
            _ => None,
        })
}

fn default_tool(registry: &ContentRegistry) -> deepcore_shared::content::types::ToolSpec {
    current_tool(registry, &LocalPlayer::default()).unwrap_or(
        deepcore_shared::content::types::ToolSpec {
            tier: 1,
            speed: 1.0,
            area: deepcore_shared::content::types::Area(1, 1),
            energy: deepcore_shared::content::types::ToolEnergy::Manual,
            reach: 5.0,
        },
    )
}

/// Places a block from the hotbar into the targeted position.
#[allow(dead_code)]
pub fn try_place(
    chunks: &mut ClientChunks,
    pos: BlockPos,
    block: deepcore_shared::content::BlockId,
) -> bool {
    if chunks.get_block(pos).is_some_and(|b| b == AIR) {
        chunks.set_block(pos, block);
        true
    } else {
        false
    }
}
