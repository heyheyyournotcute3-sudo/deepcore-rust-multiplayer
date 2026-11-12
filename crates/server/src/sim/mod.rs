//! The authoritative simulation: players, mining, placing, movement validation and vitals.
//!
//! Every gameplay rule runs here. The client only sends intents; anything that fails validation
//! is dropped and logged, never applied.

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use deepcore_shared::{
    content::{ContentRegistry, types::ItemCategory},
    hazards::{HazardExposure, HazardState, Protection, apply_exposure, can_regenerate},
    inventory::Inventory,
    mining::{MiningCheck, area_blocks, check_mining, raycast},
    movement::{MoveInput, PlayerBody, step_body},
    protocol::{
        InventoryUpdate, MiningFeedback, MoveInputMsg, NetBlockPos, NetPlayerInfo, NetTransform,
        NetVitals, PlaceBlockMsg, StartMiningMsg,
        net_register::{ReplicatedPlayerInfo, ReplicatedTransform},
    },
    voxel::{AIR, BlockPos, coords::SURFACE_Y},
};
use tracing::{debug, warn};

use crate::{
    net::SpawnPlayerFor,
    world::{ServerWorld, apply_and_broadcast_block},
};

/// A server-side player entity.
#[derive(Component, Debug)]
pub struct ServerPlayer {
    /// The client this player belongs to.
    pub client: Entity,
    /// Display name.
    pub name: String,
    /// The authoritative body.
    pub body: PlayerBody,
    /// The last movement sequence number processed.
    pub last_seq: u32,
    /// Inventory.
    pub inventory: Inventory,
    /// Which hotbar slot is selected.
    pub selected: u8,
    /// Vitals.
    pub vitals: NetVitals,
    /// Hazard state.
    pub hazards: HazardState,
    /// Protection from worn gear.
    pub protection: Protection,
    /// A mining action in progress: the target block and remaining seconds.
    pub mining: Option<(BlockPos, f32)>,
}

/// The simulation plugin.
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(spawn_player_on_request).add_systems(
            FixedUpdate,
            (
                apply_move_inputs,
                handle_place,
                tick_mining,
                tick_vitals,
                replicate_players,
                push_inventory_updates,
            )
                .chain(),
        );
    }
}

/// Spawns a player entity for a client that finished the handshake.
fn spawn_player_on_request(
    request: On<SpawnPlayerFor>,
    mut commands: Commands,
    mut world: ResMut<ServerWorld>,
    registry: Res<ContentRegistry>,
    admitted: Res<crate::net::HandshakeState>,
) {
    let client = request.client;
    let name = admitted
        .admitted
        .get(&client)
        .map_or_else(|| "Miner".to_string(), |a| a.name.clone());

    // Find a safe spawn on the surface near the camp centre.
    let spawn = find_spawn(&mut world, &registry);
    let mut body = PlayerBody::new(spawn);
    body.pitch = -0.2;

    let mut inventory = Inventory::player();
    // Everyone starts with a steel pickaxe in the first hotbar slot.
    if let Some(pickaxe) = registry.item("steel_pickaxe") {
        inventory.set_slot(0, deepcore_shared::inventory::ItemStack::new(pickaxe, 1));
    }

    let balance = &registry.balance;
    let vitals = NetVitals {
        health: balance.max_health,
        stamina: balance.max_stamina,
        oxygen: balance.max_oxygen,
        heat: 0.0,
        dose: 0.0,
        depth: 0,
    };

    let entity = commands
        .spawn((
            ServerPlayer {
                client,
                name: name.clone(),
                body,
                last_seq: 0,
                inventory,
                selected: 0,
                vitals,
                hazards: HazardState {
                    dose: 0.0,
                    heat: 0.0,
                    oxygen: balance.max_oxygen,
                },
                protection: Protection::default(),
                mining: None,
            },
            Replicated,
            ReplicatedTransform(NetTransform {
                position: body.position,
                velocity: body.velocity,
                yaw: body.yaw,
                pitch: body.pitch,
                acked_seq: 0,
                server_tick: 0,
            }),
            ReplicatedPlayerInfo(NetPlayerInfo {
                name,
                health: balance.max_health,
                sprinting: false,
                crouching: false,
                held_item: None,
            }),
            Transform::from_xyz(spawn[0], spawn[1], spawn[2]),
        ))
        .id();

    info!(?entity, ?client, "spawned player");
}

/// Finds a spawn position above solid ground near the camp.
fn find_spawn(world: &mut ServerWorld, registry: &ContentRegistry) -> [f32; 3] {
    // Search downward from just above the surface for the first solid block.
    for x in [0, 4, -4, 8, -8, 12, -12] {
        for z in [0, 4, -4, 8, -8] {
            let column = BlockPos::new(x, SURFACE_Y, z);
            if registry
                .block_def(world.block(column))
                .is_some_and(|d| d.solid)
            {
                return [
                    f32::from(x as i16) + 0.5,
                    SURFACE_Y as f32 + 1.05,
                    f32::from(z as i16) + 0.5,
                ];
            }
        }
    }
    [0.5, SURFACE_Y as f32 + 1.05, 0.5]
}

/// Applies validated movement intents from clients.
fn apply_move_inputs(
    mut players: Query<&mut ServerPlayer>,
    mut intents: Option<MessageReader<FromClient<MoveInputMsg>>>,
    world: Res<ServerWorld>,
    registry: Res<ContentRegistry>,
) {
    let Some(intents) = intents.as_mut() else {
        return;
    };
    for FromClient {
        client_id,
        message: event,
    } in intents.read()
    {
        let Some(entity) = client_id.entity() else {
            continue;
        };
        let Ok(mut player) = players.get_mut(entity) else {
            continue;
        };
        // Drop out-of-order or replayed inputs.
        if event.seq <= player.last_seq {
            continue;
        }
        player.last_seq = event.seq;

        // Clamp the input so a malicious client cannot move faster than allowed.
        let input = MoveInput {
            forward: event.forward.clamp(-1.0, 1.0),
            right: event.right.clamp(-1.0, 1.0),
            jump: event.jump,
            sprint: event.sprint,
            crouch: event.crouch,
            yaw: event.yaw,
            pitch: event.pitch.clamp(-1.5533, 1.5533),
        };

        let collision = ServerCollision {
            world: &world,
            registry: &registry,
        };
        step_body(
            &mut player.body,
            &input,
            &registry.balance,
            &collision,
            deepcore_shared::TICK_SECONDS,
        );
    }
}

/// Collision view over the server's authoritative world.
struct ServerCollision<'a> {
    world: &'a ServerWorld,
    registry: &'a ContentRegistry,
}

impl deepcore_shared::movement::CollisionWorld for ServerCollision<'_> {
    fn is_solid(&self, pos: BlockPos) -> bool {
        // Out-of-bounds and unbreakable blocks are solid.
        if pos.y <= SURFACE_Y - deepcore_shared::worldgen::WORLD_DEPTH {
            return true;
        }
        self.registry
            .block_def(self.world.peek(pos))
            .is_some_and(|d| d.solid && d.hardness > 0.0)
    }
}

/// Runs mining timers and applies completed breaks.
fn tick_mining(
    mut players: Query<&mut ServerPlayer>,
    mut starts: Option<MessageReader<FromClient<StartMiningMsg>>>,
    mut world: ResMut<ServerWorld>,
    registry: Res<ContentRegistry>,
    mut commands: Commands,
    clients: Query<Entity, With<ConnectedClient>>,
) {
    // Start or restart mining.
    let Some(starts) = starts.as_mut() else {
        return;
    };
    for FromClient {
        client_id,
        message: event,
    } in starts.read()
    {
        let Some(entity) = client_id.entity() else {
            continue;
        };
        let Ok(mut player) = players.get_mut(entity) else {
            continue;
        };
        let target: BlockPos = event.block.into();
        let eye = player.body.eye_position(&registry.balance);
        let reach = tool_of(&registry, &player.inventory, player.selected)
            .map_or(registry.balance.reach, |t| t.reach);

        // Validate reach and line of sight on the server, never trusting the client.
        let Some(block) = registry.block_def(world.block(target)) else {
            continue;
        };
        let distance = squared_distance(eye, target.center()).sqrt();
        if distance > reach + 1.0 {
            warn!(?target, distance, "mining intent out of reach");
            continue;
        }
        if !line_of_sight(&mut world, &registry, eye, target) {
            warn!(?target, "mining intent without line of sight");
            continue;
        }

        let tool = tool_of(&registry, &player.inventory, player.selected).unwrap_or(
            deepcore_shared::content::types::ToolSpec {
                tier: 1,
                speed: 1.0,
                area: deepcore_shared::content::types::Area(1, 1),
                energy: deepcore_shared::content::types::ToolEnergy::Manual,
                reach: 5.0,
            },
        );
        match check_mining(
            &tool,
            block.hardness,
            block.min_tier,
            1.0,
            deepcore_shared::TICK_SECONDS,
        ) {
            MiningCheck::Allowed { seconds } => {
                player.mining = Some((target, seconds));
            }
            MiningCheck::TooHard { required_tier } => {
                send_feedback(
                    &mut commands,
                    entity,
                    MiningFeedback {
                        block: NetBlockPos::from(target),
                        kind: 2,
                        required_tier,
                    },
                );
                player.mining = None;
            }
            MiningCheck::Impossible => {
                player.mining = None;
            }
        }
    }

    // Advance timers.
    for mut player in &mut players {
        let Some((target, remaining)) = player.mining else {
            continue;
        };
        let remaining = remaining - deepcore_shared::TICK_SECONDS;
        if remaining > 0.0 {
            player.mining = Some((target, remaining));
            continue;
        }
        player.mining = None;

        // Break the block and grant the drops.
        let Some(def) = registry.block_def(world.block(target)) else {
            continue;
        };
        let drops: Vec<_> = def.drops.clone();
        let tool = tool_of(&registry, &player.inventory, player.selected);
        let area = tool.map_or(deepcore_shared::content::types::Area(1, 1), |t| t.area);
        // The face is only used for area mining; a 1x1 tool ignores it.
        let blocks = area_blocks(target, deepcore_shared::mining::Face::Up, area);

        for pos in blocks {
            let Some(block_def) = registry.block_def(world.block(pos)) else {
                continue;
            };
            if block_def.hardness <= 0.0 || block_def.min_tier == 0 {
                continue;
            }
            let block_drops = block_def.drops.clone();
            apply_and_broadcast_block(&mut commands, &mut world, pos, AIR, &clients);
            for (item_name, count) in block_drops {
                if let Some(item) = registry.item(&item_name) {
                    let max = registry.item_def(item).map_or(1, |d| d.stack);
                    let leftover = player.inventory.add(item, count, max);
                    if leftover > 0 {
                        // No room: the item is dropped in the world. Pickups land with the
                        // inventory milestone; for now it is reported so it is not silent.
                        debug!(item = %item_name, leftover, "inventory full, drops lost");
                    }
                }
            }
        }
        // Keep the outer binding's drops meaningful for readability of the intent above.
        let _ = drops;

        send_feedback(
            &mut commands,
            player.client,
            MiningFeedback {
                block: NetBlockPos::from(target),
                kind: 1,
                required_tier: 0,
            },
        );
    }
}

/// Advances vitals and hazards once per tick.
fn tick_vitals(mut players: Query<&mut ServerPlayer>, registry: Res<ContentRegistry>) {
    let dt = deepcore_shared::TICK_SECONDS;
    for mut player in &mut players {
        let depth = SURFACE_Y - player.body.position[1].floor() as i32;
        player.vitals.depth = depth.max(0);

        let layer = registry.layer_at_depth(depth.max(0));
        let exposure = HazardExposure {
            radiation: layer.map_or(0.0, |l| {
                l.hazards
                    .iter()
                    .map(|h| match h {
                        deepcore_shared::content::types::HazardRule::Radiation { dose_per_sec } => {
                            *dose_per_sec
                        }
                        _ => 0.0,
                    })
                    .sum()
            }),
            heat: layer.map_or(0.0, |l| {
                l.hazards
                    .iter()
                    .map(|h| match h {
                        deepcore_shared::content::types::HazardRule::Heat { heat_per_sec } => {
                            *heat_per_sec
                        }
                        _ => 0.0,
                    })
                    .sum()
            }),
            oxygen_drain: 0.0,
            damage: 0.0,
        };

        let protection = player.protection;
        let damage = apply_exposure(
            &mut player.hazards,
            &exposure,
            protection,
            &registry.balance,
            dt,
        );
        player.vitals.health = (player.vitals.health - damage).max(0.0);
        if can_regenerate(&player.hazards, &registry.balance) {
            player.vitals.health =
                (player.vitals.health + 0.4 * dt).min(registry.balance.max_health);
        }
        player.vitals.dose = player.hazards.dose;
        player.vitals.heat = player.hazards.heat;
        player.vitals.oxygen = player.hazards.oxygen;
    }
}

/// Pushes authoritative transforms to the replication layer.
fn replicate_players(
    mut players: Query<(
        &ServerPlayer,
        &mut ReplicatedTransform,
        &mut ReplicatedPlayerInfo,
    )>,
    time: Res<Time>,
) {
    let tick = time.elapsed_secs() as u32;
    for (player, mut transform, mut info) in &mut players {
        transform.0 = NetTransform {
            position: player.body.position,
            velocity: player.body.velocity,
            yaw: player.body.yaw,
            pitch: player.body.pitch,
            acked_seq: player.last_seq,
            server_tick: tick,
        };
        info.0.health = player.vitals.health;
        info.0.sprinting = false;
        info.0.crouching = false;
        info.0.held_item = player
            .inventory
            .slot(usize::from(player.selected))
            .and_then(|s| s.item);
        // The name is stable for the session but is re-sent with the info component so a
        // client that joins late still learns it.
        if info.0.name.is_empty() {
            info.0.name = player.name.clone();
        }
    }
}

/// Sends each player their own inventory whenever it changes.
///
/// Inventories are private: they go to the owner as a message, never as a replicated component.
fn push_inventory_updates(
    players: Query<(Entity, &ServerPlayer), Changed<ServerPlayer>>,
    mut commands: Commands,
) {
    for (entity, player) in &players {
        commands.server_trigger(ToClients {
            targets: SendTargets::Single(player.client.into()),
            message: InventoryUpdate {
                slots: player.inventory.slots().to_vec(),
                selected: player.selected,
            },
        });
        let _ = entity;
    }
}

/// Places a block from the player's inventory, after validation.
pub fn handle_place(
    mut events: Option<MessageReader<FromClient<PlaceBlockMsg>>>,
    mut players: Query<&mut ServerPlayer>,
    mut world: ResMut<ServerWorld>,
    registry: Res<ContentRegistry>,
    mut commands: Commands,
    clients: Query<Entity, With<ConnectedClient>>,
) {
    let Some(events) = events.as_mut() else {
        return;
    };
    for FromClient {
        client_id,
        message: event,
    } in events.read()
    {
        let Some(entity) = client_id.entity() else {
            continue;
        };
        let Ok(mut player) = players.get_mut(entity) else {
            continue;
        };
        let pos: BlockPos = event.pos.into();
        let eye = player.body.eye_position(&registry.balance);
        if squared_distance(eye, pos.center()).sqrt() > registry.balance.reach + 1.0 {
            warn!(?pos, "place intent out of reach");
            continue;
        }
        // The slot must actually hold a placeable block.
        let Some(stack) = player.inventory.slot(usize::from(event.item_slot)).copied() else {
            continue;
        };
        let Some(item) = stack.item else { continue };
        let Some(item_def) = registry.item_def(item) else {
            continue;
        };
        let ItemCategory::Block(block_name) = &item_def.category else {
            continue;
        };
        let Some(block) = registry.block(block_name) else {
            continue;
        };
        // The target must be empty.
        if world.block(pos) != AIR {
            continue;
        }
        apply_and_broadcast_block(&mut commands, &mut world, pos, block, &clients);
        player.inventory.remove(item, 1);
    }
}

/// The tool in a hotbar slot, if it is a tool.
fn tool_of(
    registry: &ContentRegistry,
    inventory: &Inventory,
    slot: u8,
) -> Option<deepcore_shared::content::types::ToolSpec> {
    let stack = inventory.slot(usize::from(slot))?;
    let item = stack.item?;
    match &registry.item_def(item)?.category {
        ItemCategory::Tool(spec) => Some(*spec),
        _ => None,
    }
}

fn squared_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

/// Voxel raycast from the eye to the target, so players cannot mine through walls.
fn line_of_sight(
    world: &mut ServerWorld,
    registry: &ContentRegistry,
    eye: [f32; 3],
    target: BlockPos,
) -> bool {
    let centre = target.center();
    let dir = [centre[0] - eye[0], centre[1] - eye[1], centre[2] - eye[2]];
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len < 1e-4 {
        return true;
    }
    let dir = [dir[0] / len, dir[1] / len, dir[2] / len];
    // Sample the world once up front: the raycast closure cannot hold a mutable borrow.
    let mut solid_at = |pos: BlockPos| {
        registry
            .block_def(world.block(pos))
            .is_some_and(|d| d.solid && d.hardness > 0.0)
    };
    let mut solidity: Vec<(BlockPos, bool)> = Vec::new();
    // Collect the blocks the ray could touch, then answer the raycast from that snapshot.
    let steps = (len + 2.0).ceil() as i32;
    for i in 0..=steps {
        let t = f32::from(i as i16) / f32::from(steps.max(1) as i16);
        let p = [
            eye[0] + dir[0] * len * t,
            eye[1] + dir[1] * len * t,
            eye[2] + dir[2] * len * t,
        ];
        let bp = BlockPos::new(
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        );
        if !solidity.iter().any(|(existing, _)| *existing == bp) {
            solidity.push((bp, solid_at(bp)));
        }
    }
    let hit = raycast(
        eye,
        dir,
        len + 1.0,
        |pos| {
            solidity
                .iter()
                .find(|(existing, _)| *existing == pos)
                .is_some_and(|(_, solid)| *solid)
        },
        |_| true,
    );
    hit.is_none_or(|h| h.block == target)
}

fn send_feedback(commands: &mut Commands, client: Entity, message: MiningFeedback) {
    commands.server_trigger(ToClients {
        targets: SendTargets::Single(client.into()),
        message,
    });
}
