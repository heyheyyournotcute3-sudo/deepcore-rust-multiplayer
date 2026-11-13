//! Item pickups in the world, the death crate, and respawn.
//!
//! When a player's inventory is full, mined items drop into the world as pickups that anyone
//! can collect. When a player dies their backpack drops as a crate at the death spot (the
//! hotbar is kept), and they respawn at the Base.

use bevy::prelude::*;
use deepcore_shared::{content::ContentRegistry, inventory::ItemStack, voxel::BlockPos};
use tracing::{debug, info};

use crate::{sim::ServerPlayer, world::ServerWorld};

/// A pile of items lying on the ground.
#[derive(Component, Debug, Clone)]
pub struct Pickup {
    /// What is in the pile.
    pub items: Vec<ItemStack>,
    /// Where it sits.
    pub pos: [f32; 3],
    /// Seconds since it was dropped, so it can be cleaned up eventually.
    pub age: f32,
}

/// A death crate: a player's backpack, recoverable by anyone.
#[derive(Component, Debug, Clone)]
pub struct DeathCrate {
    /// The items inside.
    pub items: Vec<ItemStack>,
    /// The owner's name, for the HUD message.
    pub owner: String,
    /// Where it sits.
    pub pos: [f32; 3],
}

/// How close a player must be to pick something up, in blocks.
pub const PICKUP_RANGE: f32 = 2.0;

/// How long a pickup lives before it is removed, in seconds.
pub const PICKUP_LIFETIME: f32 = 600.0;

/// The pickup and death-crate plugin.
pub struct LootPlugin;

impl Plugin for LootPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, (tick_pickups, collect_pickups));
    }
}

/// Ages pickups and removes the ones that have lain around too long.
fn tick_pickups(
    mut commands: Commands,
    time: Res<Time>,
    mut pickups: Query<(Entity, &mut Pickup)>,
) {
    for (entity, mut pickup) in &mut pickups {
        pickup.age += time.delta_secs();
        if pickup.age > PICKUP_LIFETIME {
            debug!(pos = ?pickup.pos, "pickup expired");
            commands.entity(entity).despawn();
        }
    }
}

/// Lets players collect nearby pickups and death crates.
fn collect_pickups(
    mut commands: Commands,
    mut players: Query<&mut ServerPlayer>,
    mut pickups: Query<(Entity, &mut Pickup)>,
    mut crates: Query<(Entity, &mut DeathCrate)>,
    registry: Res<ContentRegistry>,
) {
    for mut player in &mut players {
        let feet = player.body.position;

        for (entity, mut pickup) in &mut pickups {
            if distance(feet, pickup.pos) > PICKUP_RANGE {
                continue;
            }
            // Move whatever fits; leave the rest on the ground.
            let mut remaining = Vec::new();
            for stack in pickup.items.clone() {
                let Some(item) = stack.item else { continue };
                let max = registry.item_def(item).map_or(1, |d| d.stack);
                let left = player.inventory.add(item, stack.count, max);
                if left > 0 {
                    remaining.push(ItemStack::new(item, left));
                }
            }
            if remaining.is_empty() {
                commands.entity(entity).despawn();
            } else {
                pickup.items = remaining;
            }
        }

        for (entity, mut crate_items) in &mut crates {
            if distance(feet, crate_items.pos) > PICKUP_RANGE {
                continue;
            }
            let leftover = player.inventory.restore(&crate_items.items, &registry);
            if leftover.is_empty() {
                info!(owner = %crate_items.owner, "death crate recovered");
                commands.entity(entity).despawn();
            } else {
                crate_items.items = leftover;
            }
        }
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Drops a pile of items at a position, spawning one pickup entity.
pub fn drop_items(commands: &mut Commands, items: Vec<ItemStack>, pos: [f32; 3]) -> Option<Entity> {
    let items: Vec<ItemStack> = items.into_iter().filter(|s| !s.is_empty()).collect();
    if items.is_empty() {
        return None;
    }
    Some(
        commands
            .spawn((
                Pickup {
                    items,
                    pos,
                    age: 0.0,
                },
                Transform::from_xyz(pos[0], pos[1], pos[2]),
            ))
            .id(),
    )
}

/// Handles a player's death: drop the backpack as a crate and respawn at the Base.
pub fn handle_death(
    commands: &mut Commands,
    player: &mut ServerPlayer,
    world: &mut ServerWorld,
    registry: &ContentRegistry,
) {
    // The hotbar survives; the backpack drops.
    let dropped = player.inventory.take_backpack();
    if !dropped.is_empty() {
        let pos = player.body.position;
        commands.spawn((
            DeathCrate {
                items: dropped,
                owner: player.name.clone(),
                pos,
            },
            Transform::from_xyz(pos[0], pos[1], pos[2]),
        ));
        info!(owner = %player.name, ?pos, "player died; backpack dropped as a crate");
    }

    // Respawn at the Base, on top of the surface.
    let spawn = crate::sim::find_spawn(world, registry);
    player.body = deepcore_shared::movement::PlayerBody::new(spawn);
    player.body.pitch = -0.2;
    player.mining = None;
    player.vitals.health = registry.balance.max_health;
    player.hazards = deepcore_shared::hazards::HazardState {
        dose: player.hazards.dose,
        heat: 0.0,
        oxygen: registry.balance.max_oxygen,
    };
}

/// Spawns a pickup for items a mining action could not fit in the inventory.
pub fn drop_overflow(
    commands: &mut Commands,
    registry: &ContentRegistry,
    item_name: &str,
    count: u32,
    at: BlockPos,
) {
    let Some(item) = registry.item(item_name) else {
        return;
    };
    let centre = at.center();
    let pos = [centre[0], centre[1] - 0.3, centre[2]];
    drop_items(commands, vec![ItemStack::new(item, count)], pos);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn registry() -> ContentRegistry {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        ContentRegistry::load_from_dir(&dir).unwrap()
    }

    #[test]
    fn a_pickup_keeps_its_items_and_position() {
        let pickup = Pickup {
            items: vec![ItemStack::new(deepcore_shared::content::ItemId(1), 3)],
            pos: [1.4, 2.6, -3.2],
            age: 0.0,
        };
        assert_eq!(pickup.items.len(), 1);
        assert_eq!(pickup.pos[1], 2.6);
    }

    #[test]
    fn empty_pickups_are_not_spawned() {
        let mut app = App::new();
        let items = vec![ItemStack::empty()];
        let entity = drop_items(&mut app.world_mut().commands(), items, [0.0; 3]);
        assert!(entity.is_none(), "an empty drop must not spawn an entity");
    }

    #[test]
    fn a_death_drops_the_backpack_and_keeps_the_hotbar() {
        let reg = registry();
        let mut world = crate::world::ServerWorld::new(&reg, 1337).unwrap();
        let mut player = crate::sim::test_player(&reg);
        let coal = reg.item("coal").unwrap();
        player.inventory.set_slot(0, ItemStack::new(coal, 5));
        player.inventory.set_slot(20, ItemStack::new(coal, 30));

        let mut app = App::new();
        handle_death(
            &mut app.world_mut().commands(),
            &mut player,
            &mut world,
            &reg,
        );

        // The hotbar item survives the death.
        assert_eq!(player.inventory.count_of(coal), 5);
        // And the player is back at full health.
        assert_eq!(player.vitals.health, reg.balance.max_health);
    }
}
