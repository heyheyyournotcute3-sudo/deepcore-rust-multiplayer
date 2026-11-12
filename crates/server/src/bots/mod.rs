//! Bot players for load and desync testing.
//!
//! `server --bots N` spawns N simulated players that walk, dig and idle. They drive the *same*
//! server systems a real client drives (movement validation, mining timers, inventory grants)
//! by writing the same intents a network client would send, without needing a socket per bot.
//!
//! This is deliberate: a socket-level bot would mostly test renet, while the failure modes we
//! care about — desync, panics under load, machines starving, chunk cache growth — live in the
//! simulation. The network path itself is covered by the two-client test.

use bevy::prelude::*;
use deepcore_shared::{
    content::ContentRegistry,
    inventory::Inventory,
    movement::{MoveInput, PlayerBody, step_body},
    protocol::{NetPlayerInfo, NetTransform, NetVitals},
    voxel::{AIR, BlockPos, coords::SURFACE_Y},
};
use rand_chacha::ChaCha8Rng;
use rand_core::{Rng, SeedableRng};
use tracing::info;

use crate::{sim::ServerPlayer, world::ServerWorld};

/// What a bot is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Goal {
    /// Walk in the current facing direction.
    Walk,
    /// Dig the block under its feet.
    Dig,
    /// Stand still and look around.
    Idle,
}

/// A simulated player.
#[derive(Component, Debug)]
pub struct Bot {
    /// Index, used for the name.
    pub index: u32,
    /// Its own RNG, so bots do not move in lockstep.
    pub rng: ChaCha8Rng,
    /// Current goal.
    goal: Goal,
    /// Ticks left on the current goal.
    goal_ticks: u32,
    /// Yaw it is facing.
    yaw: f32,
    /// How many blocks it has mined, reported at the end of the run.
    pub mined: u32,
}

/// All bots, plus aggregate counters for the load report.
#[derive(Resource, Default)]
pub struct BotPool {
    /// Number of bots spawned.
    pub count: u32,
    /// Total blocks mined by all bots.
    pub total_mined: u32,
}

/// The bot plugin.
pub struct BotPlugin;

impl Plugin for BotPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BotPool>()
            // Bots spawn players into the world, so the world must exist first.
            .add_systems(Startup, spawn_bots.after(crate::world::WorldInit))
            .add_systems(FixedUpdate, drive_bots)
            .add_systems(
                Update,
                report_bots.run_if(bevy::time::common_conditions::on_timer(
                    std::time::Duration::from_secs(10),
                )),
            );
    }
}

/// Spawns the requested number of bots as real `ServerPlayer` entities.
fn spawn_bots(
    mut commands: Commands,
    args: Res<crate::ServerArgs>,
    mut world: ResMut<ServerWorld>,
    registry: Res<ContentRegistry>,
    mut pool: ResMut<BotPool>,
) {
    if args.bots == 0 {
        return;
    }
    let balance = &registry.balance;
    for index in 0..args.bots {
        // Spread bots around the camp so they do not all stand in one block.
        let angle = index as f32 * 0.7;
        let radius = 3.0 + (index % 5) as f32;
        let spawn = [
            angle.cos() * radius,
            SURFACE_Y as f32 + 1.05,
            angle.sin() * radius,
        ];
        // Nudge upward until the spawn point is clear.
        let mut body = PlayerBody::new(spawn);
        let mut attempts = 0;
        while crate::sim::spawn_blocked(&world, &registry, body.position) && attempts < 40 {
            body.position[1] += 1.0;
            attempts += 1;
        }

        let mut inventory = Inventory::player();
        if let Some(pickaxe) = registry.item("steel_pickaxe") {
            inventory.set_slot(0, deepcore_shared::inventory::ItemStack::new(pickaxe, 1));
        }

        commands.spawn((
            Bot {
                index,
                rng: ChaCha8Rng::seed_from_u64(u64::from(index) * 7919 + 13),
                goal: Goal::Walk,
                goal_ticks: 0,
                yaw: angle,
                mined: 0,
            },
            ServerPlayer {
                client: Entity::PLACEHOLDER,
                name: format!("Bot {index}"),
                body,
                last_seq: 0,
                inventory,
                selected: 0,
                vitals: NetVitals {
                    health: balance.max_health,
                    stamina: balance.max_stamina,
                    oxygen: balance.max_oxygen,
                    heat: 0.0,
                    dose: 0.0,
                    depth: 0,
                },
                hazards: deepcore_shared::hazards::HazardState {
                    dose: 0.0,
                    heat: 0.0,
                    oxygen: balance.max_oxygen,
                },
                protection: deepcore_shared::hazards::Protection::default(),
                mining: None,
            },
            bevy_replicon::prelude::Replicated,
            deepcore_shared::protocol::net_register::ReplicatedTransform(NetTransform {
                position: body.position,
                velocity: body.velocity,
                yaw: body.yaw,
                pitch: -0.3,
                acked_seq: 0,
                server_tick: 0,
            }),
            deepcore_shared::protocol::net_register::ReplicatedPlayerInfo(NetPlayerInfo {
                name: format!("Bot {index}"),
                health: balance.max_health,
                sprinting: false,
                crouching: false,
                held_item: None,
            }),
            Transform::from_xyz(spawn[0], spawn[1], spawn[2]),
        ));
        let _ = &mut world;
        pool.count += 1;
    }
    info!(bots = pool.count, "spawned bot players");
}

/// Moves and mines for every bot, one fixed step at a time.
fn drive_bots(
    mut bots: Query<(&mut Bot, &mut ServerPlayer)>,
    mut world: ResMut<ServerWorld>,
    registry: Res<ContentRegistry>,
    mut pool: ResMut<BotPool>,
) {
    if bots.is_empty() {
        return;
    }
    let dt = deepcore_shared::TICK_SECONDS;

    for (mut bot, mut player) in &mut bots {
        // Choose a new goal when the current one expires.
        if bot.goal_ticks == 0 {
            bot.goal = match bot.rng.next_u32() % 3 {
                0 => Goal::Walk,
                1 => Goal::Dig,
                _ => Goal::Idle,
            };
            bot.goal_ticks = 30 + bot.rng.next_u32() % 120;
            if bot.goal == Goal::Walk {
                bot.yaw = (bot.rng.next_u32() % 628) as f32 / 100.0;
            }
        }
        bot.goal_ticks -= 1;

        let input = match bot.goal {
            Goal::Walk => MoveInput {
                forward: 1.0,
                right: 0.0,
                jump: bot.rng.next_u32() % 90 == 0,
                sprint: bot.rng.next_u32() % 4 == 0,
                crouch: false,
                yaw: bot.yaw,
                pitch: -0.3,
            },
            Goal::Dig | Goal::Idle => MoveInput {
                yaw: bot.yaw,
                pitch: -0.5,
                ..Default::default()
            },
        };

        // Use the same collision view the real server uses.
        let collision = crate::sim::world_collision(&world, &registry);
        step_body(&mut player.body, &input, &registry.balance, &collision, dt);

        // Digging: mine the block directly beneath the bot.
        if bot.goal == Goal::Dig && player.mining.is_none() {
            let feet = BlockPos::new(
                player.body.position[0].floor() as i32,
                (player.body.position[1] - 0.5).floor() as i32,
                player.body.position[2].floor() as i32,
            );
            // `block()` generates the chunk on demand; `peek()` would report air for a chunk
            // that has not been generated yet, which is why bots saw an empty world at first.
            let here = world.block(feet);
            if here != AIR
                && registry
                    .block_def(here)
                    .is_some_and(|d| d.min_tier > 0 && d.min_tier <= 2)
            {
                player.mining = Some((feet, 0.2));
            }
        }

        // Complete mining the same way a player's timer does.
        if let Some((target, remaining)) = player.mining {
            let remaining = remaining - dt;
            if remaining > 0.0 {
                player.mining = Some((target, remaining));
            } else {
                player.mining = None;
                let block = world.block(target);
                if block != AIR
                    && let Some(def) = registry.block_def(block)
                {
                    let drops = def.drops.clone();
                    crate::world::set_block_direct(&mut world, target, AIR);
                    for (item_name, count) in drops {
                        if let Some(item) = registry.item(&item_name) {
                            let max = registry.item_def(item).map_or(1, |d| d.stack);
                            player.inventory.add(item, count, max);
                        }
                    }
                    bot.mined += 1;
                    pool.total_mined += 1;
                }
            }
        }
    }
}

/// Logs what the bots have been doing, so a load run leaves a readable trail.
fn report_bots(pool: Res<BotPool>, world: Res<ServerWorld>, bots: Query<&Bot>) {
    if pool.count == 0 {
        return;
    }
    // Report the per-bot mining spread: an even spread means the sim is not starving bots.
    let mut per_bot: Vec<(u32, u32)> = bots.iter().map(|b| (b.index, b.mined)).collect();
    per_bot.sort_unstable();
    let busiest = per_bot.iter().map(|(_, m)| *m).max().unwrap_or(0);
    let quietest = per_bot.iter().map(|(_, m)| *m).min().unwrap_or(0);
    info!(
        bots = pool.count,
        blocks_mined = pool.total_mined,
        busiest,
        quietest,
        loaded_chunks = world.loaded_chunks_for_report(),
        "bot load report"
    );
}
