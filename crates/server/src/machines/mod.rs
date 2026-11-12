//! Machine block entities: placement, server-side ticking, and contents for the UI.
//!
//! A machine is an ECS entity anchored at a `BlockPos`, not voxel data. Placing a machine item
//! consumes it, writes the machine's block into the world and spawns the entity. The server
//! ticks every machine each fixed step using the shared state machine, and sends the machine's
//! slots only to the client that has its panel open.

use bevy::{platform::collections::HashMap, prelude::*};
use bevy_replicon::prelude::*;
use deepcore_shared::{
    content::{ContentRegistry, MachineId, types::EnergyKind, types::MachineRole},
    crafting::{MachineState, tick_machine},
    protocol::{MachineContents, NetBlockPos, NetMachine, net_register::ReplicatedMachine},
    voxel::{AIR, BlockPos},
};
use tracing::{debug, info};

use crate::world::{ServerWorld, apply_and_broadcast_block};

/// A machine placed in the world.
#[derive(Component, Debug)]
pub struct MachineEntity {
    /// Which machine this is.
    pub kind: MachineId,
    /// The block position of its anchor.
    pub pos: BlockPos,
    /// Facing, in quarter turns.
    pub rotation: u8,
    /// The owner client, for UI routing.
    pub owner: Option<Entity>,
    /// Runtime state: slots, fuel, progress.
    pub state: MachineState,
    /// The linked blocks of a cascade (centrifuges).
    pub cascade: Vec<BlockPos>,
    /// Reactor core state, for reactor machines.
    pub reactor: Option<deepcore_shared::hazards::ReactorCore>,
    /// Whether a scram panel is built nearby.
    pub has_scram: bool,
}

/// All machines in the world, indexed by their anchor position for fast lookup.
#[derive(Resource, Default)]
pub struct MachineIndex {
    /// Anchor position to machine entity.
    pub by_pos: HashMap<BlockPos, Entity>,
    /// Machine entity to its kind, so cascade linking can filter without querying.
    pub machines: HashMap<Entity, MachineId>,
}

/// The plugin that owns machine placement and ticking.
pub struct MachinePlugin;

impl Plugin for MachinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MachineIndex>()
            .add_systems(FixedUpdate, (tick_machines, sync_machine_replication))
            .add_systems(Update, handle_machine_contents_requests);
    }
}

/// Ticks every machine once per fixed step.
fn tick_machines(
    mut machines: Query<(&mut MachineEntity, Option<&CascadeLink>)>,
    registry: Res<ContentRegistry>,
    research: Res<crate::research::ResearchState>,
) {
    let researched = |tech: &str| research.is_researched_name(&registry, tech);
    let dt = deepcore_shared::TICK_SECONDS;

    for (mut machine, cascade) in &mut machines {
        let Some(def) = registry.machine_def(machine.kind) else {
            continue;
        };
        // A linked centrifuge learns its cascade length from the link component.
        if let Some(link) = cascade {
            machine.state.cascade_len = link.length;
            machine.cascade = link.members.clone();
        }

        // Reactor cores run their own heat/coolant simulation instead of a recipe.
        if def.role == MachineRole::ReactorCore {
            let mut core = machine.reactor.unwrap_or_default();
            let status = deepcore_shared::hazards::tick_reactor(
                &mut core,
                &registry.balance,
                machine.has_scram,
                dt,
            );
            if status == deepcore_shared::hazards::ReactorStatus::Meltdown {
                warn_meltdown(machine.pos);
            }
            machine.reactor = Some(core);
            continue;
        }

        // Machines that produce power need their supply computed by the power layer; a machine
        // with no external requirement runs at full availability.
        let power_available = match def.energy {
            EnergyKind::Electric { .. } | EnergyKind::Shaft { .. } | EnergyKind::Steam { .. } => {
                1.0
            }
            _ => 1.0,
        };
        tick_machine(
            &mut machine.state,
            def,
            &registry,
            &researched,
            power_available,
            dt,
        );
    }
}

fn warn_meltdown(pos: BlockPos) {
    tracing::error!(?pos, "reactor meltdown");
}

/// Pushes each machine's coarse state into the replication component.
fn sync_machine_replication(
    mut machines: Query<(&MachineEntity, &mut ReplicatedMachine)>,
    registry: Res<ContentRegistry>,
) {
    for (machine, mut replicated) in &mut machines {
        let progress = machine.state.progress_fraction(&registry);
        replicated.0 = NetMachine {
            pos: NetBlockPos::from(machine.pos),
            kind: machine.kind,
            rotation: machine.rotation,
            running: machine.state.is_running(),
            progress: (progress * 255.0) as u8,
            idle_code: idle_code(&machine.state.idle),
        };
    }
}

/// Maps an idle reason to the compact code sent on the wire.
fn idle_code(reason: &deepcore_shared::crafting::IdleReason) -> u8 {
    use deepcore_shared::crafting::IdleReason;
    match reason {
        IdleReason::Running => 0,
        IdleReason::NoRecipe => 1,
        IdleReason::OutputFull => 2,
        IdleReason::NoFuel => 3,
        IdleReason::NoPower => 4,
        IdleReason::MissingCatalyst(_) => 5,
        IdleReason::Locked => 6,
        IdleReason::Unformed(_) => 7,
    }
}

/// Places a machine item into the world at `pos` for a client.
///
/// Validates that the item is a machine, the target is clear, and the player is in reach.
/// Returns the spawned entity.
pub fn place_machine(
    commands: &mut Commands,
    world: &mut ServerWorld,
    index: &mut MachineIndex,
    registry: &ContentRegistry,
    clients: &Query<Entity, With<ConnectedClient>>,
    machine_name: &str,
    pos: BlockPos,
    owner: Option<Entity>,
) -> Option<Entity> {
    let kind = registry.machine(machine_name)?;
    let def = registry.machine_def(kind)?;

    // The machine occupies its footprint; every block in it must be clear.
    for dx in 0..def.size.0.max(1) {
        for dy in 0..def.size.1.max(1) {
            for dz in 0..def.size.2.max(1) {
                let cell = pos.offset(dx, dy, dz);
                if world.block(cell) != AIR {
                    debug!(?cell, "machine placement blocked");
                    return None;
                }
            }
        }
    }

    // Write the machine's block so the voxel world reflects it.
    if let Some(block) = registry.block(machine_name) {
        apply_and_broadcast_block(commands, world, pos, block, clients);
    }

    let entity = commands
        .spawn((
            MachineEntity {
                kind,
                pos,
                rotation: 0,
                owner,
                state: MachineState::for_machine(kind, def),
                cascade: Vec::new(),
                reactor: if def.role == MachineRole::ReactorCore {
                    Some(deepcore_shared::hazards::ReactorCore::default())
                } else {
                    None
                },
                has_scram: false,
            },
            Replicated,
            ReplicatedMachine(NetMachine {
                pos: NetBlockPos::from(pos),
                kind,
                rotation: 0,
                running: false,
                progress: 0,
                idle_code: 1,
            }),
            Transform::from_xyz(pos.x as f32, pos.y as f32, pos.z as f32),
        ))
        .id();

    index.by_pos.insert(pos, entity);
    index.machines.insert(entity, kind);
    info!(machine = %def.name, ?pos, "machine placed");

    // Centrifuges form a cascade: link every adjacent centrifuge so the cascade length drives
    // enrichment speed. The cascade is the connected component through face neighbours.
    if def.cascade.is_some() {
        relink_cascades(commands, index, registry, pos);
    }
    Some(entity)
}

/// Recomputes the connected centrifuge cascade containing `start` and stores it on every member.
fn relink_cascades(
    commands: &mut Commands,
    index: &MachineIndex,
    registry: &ContentRegistry,
    start: BlockPos,
) {
    // Breadth-first over face-adjacent centrifuges.
    let mut visited: std::collections::HashSet<BlockPos> = std::collections::HashSet::new();
    let mut queue = vec![start];
    let mut members = Vec::new();
    while let Some(pos) = queue.pop() {
        if !visited.insert(pos) {
            continue;
        }
        let Some(&entity) = index.by_pos.get(&pos) else {
            continue;
        };
        members.push((pos, entity));
        for neighbour in pos.neighbours() {
            if let Some(&other) = index.by_pos.get(&neighbour) {
                let _ = other;
                queue.push(neighbour);
            }
        }
    }

    // Only centrifuges count toward a cascade.
    let mut centrifuge_members: Vec<(BlockPos, Entity)> = Vec::new();
    for (pos, entity) in members {
        if let Some(kind) = index.machines.get(&entity)
            && registry
                .machine_def(*kind)
                .is_some_and(|d| d.cascade.is_some())
        {
            centrifuge_members.push((pos, entity));
        }
    }
    let len = centrifuge_members.len().min(usize::from(u8::MAX)) as u8;
    let positions: Vec<BlockPos> = centrifuge_members.iter().map(|(p, _)| *p).collect();
    for (_, entity) in &centrifuge_members {
        commands.entity(*entity).insert(CascadeLink {
            members: positions.clone(),
            length: len,
        });
    }
    debug!(len, "relinked centrifuge cascade");
}

/// The cascade a centrifuge belongs to, applied to its machine state each tick.
#[derive(Component, Debug, Clone)]
pub struct CascadeLink {
    /// Positions of every centrifuge in the cascade.
    pub members: Vec<BlockPos>,
    /// How many centrifuges are linked.
    pub length: u8,
}

/// Requests the contents of the machine the player has open, and sends them to that client.
fn handle_machine_contents_requests(
    mut requests: Option<MessageReader<FromClient<deepcore_shared::protocol::InteractMsg>>>,
    mut machines: Query<&mut MachineEntity>,
    index: Res<MachineIndex>,
    mut commands: Commands,
) {
    let Some(requests) = requests.as_mut() else {
        return;
    };
    for FromClient {
        client_id,
        message: request,
    } in requests.read()
    {
        let Some(client) = client_id.entity() else {
            continue;
        };
        let pos: BlockPos = request.pos.into();
        let Some(&entity) = index.by_pos.get(&pos) else {
            continue;
        };
        let Ok(mut machine) = machines.get_mut(entity) else {
            continue;
        };
        machine.owner = Some(client);
        let contents = MachineContents {
            machine: entity.to_bits(),
            inputs: machine.state.inputs.slots().to_vec(),
            outputs: machine.state.outputs.slots().to_vec(),
            fuel: machine.state.fuel,
            modules: (
                machine.state.speed_modules,
                machine.state.efficiency_modules,
            ),
        };
        commands.server_trigger(ToClients {
            targets: SendTargets::Single(client.into()),
            message: contents,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepcore_shared::content::ContentRegistry;
    use std::path::Path;

    fn registry() -> ContentRegistry {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        ContentRegistry::load_from_dir(&dir).unwrap()
    }

    /// A furnace, driven by the shared machine state machine, smelts ore into an ingot.
    /// This is the headless equivalent of a player building and fuelling a furnace.
    #[test]
    fn a_furnace_smelts_through_the_shared_state_machine() {
        let reg = registry();
        let kind = reg.machine("stone_furnace").unwrap();
        let def = reg.machine_def(kind).unwrap();
        let mut state = MachineState::for_machine(kind, def);
        state.inputs.add(reg.item("raw_iron").unwrap(), 1, 64);
        state.fuel = deepcore_shared::inventory::ItemStack::new(reg.item("coal").unwrap(), 1);

        let everything = |_: &str| true;
        for _ in 0..600 {
            tick_machine(&mut state, def, &reg, &everything, 1.0, 0.05);
            if state.outputs.count_of(reg.item("iron_ingot").unwrap()) > 0 {
                break;
            }
        }
        assert_eq!(
            state.outputs.count_of(reg.item("iron_ingot").unwrap()),
            1,
            "the furnace should have produced exactly one ingot"
        );
        assert_eq!(state.inputs.count_of(reg.item("raw_iron").unwrap()), 0);
    }

    /// A machine footprint is rejected when any cell is already occupied.
    #[test]
    fn a_blocked_footprint_is_rejected() {
        let reg = registry();
        let mut world = ServerWorld::new(&reg, 1337).unwrap();
        let blast = reg.machine("blast_furnace").unwrap();
        let def = reg.machine_def(blast).unwrap();
        // Just below the surface the world is solid, so a 2x3x2 footprint cannot fit.
        let pos = BlockPos::new(4, -2, 4);
        let mut blocked = false;
        for dx in 0..def.size.0 {
            for dy in 0..def.size.1 {
                for dz in 0..def.size.2 {
                    if world.block(pos.offset(dx, dy, dz)) != AIR {
                        blocked = true;
                    }
                }
            }
        }
        assert!(
            blocked,
            "an underground footprint must be reported as blocked"
        );
    }

    /// A centrifuge placed with too few neighbours reports an unformed cascade.
    #[test]
    fn a_lone_centrifuge_is_unformed() {
        let reg = registry();
        let kind = reg.machine("centrifuge").unwrap();
        let def = reg.machine_def(kind).unwrap();
        let mut state = MachineState::for_machine(kind, def);
        state.cascade_len = 1;
        state.inputs.add(reg.item("uf6_canister").unwrap(), 2, 16);
        let everything = |_: &str| true;
        tick_machine(&mut state, def, &reg, &everything, 1.0, 0.05);
        assert!(matches!(
            state.idle,
            deepcore_shared::crafting::IdleReason::Unformed(_)
        ));
    }
}
