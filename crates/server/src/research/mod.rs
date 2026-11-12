//! Team-shared research: the tech tree, deposits at the Research Desk, and unlocking.
//!
//! Research is the team's shared progression. A player delivers items to a tech node at the
//! Research Desk; when the node's cost is fully delivered it becomes researched, which unlocks
//! its recipes and machines for everyone. The server owns this state and replicates it.

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use deepcore_shared::{
    content::{ContentRegistry, ItemId, TechId},
    protocol::{NetTechState, Notification},
};

use crate::sim::ServerPlayer;

/// The team's research progress.
#[derive(Resource, Default, Debug)]
pub struct ResearchState {
    /// Nodes that are fully researched.
    pub researched: std::collections::BTreeSet<TechId>,
    /// Items delivered so far toward each node, keyed by node then item.
    pub deposits: std::collections::BTreeMap<TechId, std::collections::BTreeMap<ItemId, u32>>,
}

impl ResearchState {
    /// Whether a node is researched.
    #[must_use]
    pub fn is_researched(&self, node: TechId) -> bool {
        self.researched.contains(&node)
    }

    /// Whether a node is researched, looked up by its content ID string.
    #[must_use]
    pub fn is_researched_name(&self, registry: &ContentRegistry, name: &str) -> bool {
        registry
            .tech(name)
            .is_some_and(|node| self.researched.contains(&node))
    }

    /// Whether a node's parents are all researched, so it can be worked on.
    #[must_use]
    pub fn is_available(&self, registry: &ContentRegistry, node: TechId) -> bool {
        let Some(def) = registry.tech_def(node) else {
            return false;
        };
        def.parents.iter().all(|parent| {
            registry
                .tech(parent)
                .is_some_and(|p| self.researched.contains(&p))
        })
    }

    /// Delivers items toward a node. Returns `Ok(())` if the delivery was accepted, and
    /// reports whether the node completed.
    pub fn deliver(
        &mut self,
        registry: &ContentRegistry,
        node: TechId,
        items: &[(ItemId, u32)],
    ) -> Result<bool, &'static str> {
        if self.is_researched(node) {
            return Err("already researched");
        }
        if !self.is_available(registry, node) {
            return Err("prerequisites not researched");
        }
        let Some(def) = registry.tech_def(node) else {
            return Err("unknown node");
        };
        let required: Vec<(ItemId, u32)> = def
            .cost
            .iter()
            .filter_map(|(name, count)| registry.item(name).map(|id| (id, *count)))
            .collect();

        let entry = self.deposits.entry(node).or_default();
        for (item, count) in items {
            // Only accept items the node actually needs.
            if !required.iter().any(|(needed, _)| needed == item) {
                return Err("item not needed by this node");
            }
            let already = entry.get(item).copied().unwrap_or(0);
            let needed = required
                .iter()
                .find(|(needed, _)| needed == item)
                .map_or(0, |(_, c)| *c);
            let room = needed.saturating_sub(already);
            if room == 0 {
                return Err("already fully delivered");
            }
            entry.insert(*item, already + (*count).min(room));
        }

        let complete = required
            .iter()
            .all(|(item, needed)| entry.get(item).copied().unwrap_or(0) >= *needed);
        if complete {
            self.researched.insert(node);
            self.deposits.remove(&node);
        }
        Ok(complete)
    }

    /// The replicated snapshot of this state.
    #[must_use]
    pub fn snapshot(&self) -> NetTechState {
        NetTechState {
            researched: self.researched.iter().copied().collect(),
            deposits: self
                .deposits
                .iter()
                .map(|(node, items)| {
                    (
                        *node,
                        items.iter().map(|(item, count)| (*item, *count)).collect(),
                    )
                })
                .collect(),
        }
    }
}

/// The research plugin.
pub struct ResearchPlugin;

impl Plugin for ResearchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ResearchState>()
            .add_systems(Update, handle_research_deposits);
    }
}

/// Handles a player delivering items to a tech node.
fn handle_research_deposits(
    mut requests: Option<MessageReader<FromClient<deepcore_shared::protocol::ResearchDepositMsg>>>,
    mut players: Query<&mut ServerPlayer>,
    mut research: ResMut<ResearchState>,
    registry: Res<ContentRegistry>,
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
        let Ok(mut player) = players.get_mut(client) else {
            continue;
        };

        // The player must actually hold the items being offered.
        if !player.inventory.has_all(&request.items) {
            continue;
        }
        // Deliver first, then remove: a rejected delivery must not consume anything.
        match research.deliver(&registry, request.node, &request.items) {
            Ok(completed) => {
                for (item, count) in &request.items {
                    player.inventory.remove(*item, *count);
                }
                let name = registry
                    .tech_def(request.node)
                    .map_or("node".to_string(), |d| d.name.clone());
                let text = if completed {
                    format!("Research complete: {name}")
                } else {
                    format!("Delivered to {name}")
                };
                commands.server_trigger(ToClients {
                    targets: SendTargets::All,
                    message: Notification { text, level: 0 },
                });
                let snapshot = research.snapshot();
                commands.server_trigger(ToClients {
                    targets: SendTargets::All,
                    message: snapshot,
                });
            }
            Err(reason) => {
                commands.server_trigger(ToClients {
                    targets: SendTargets::Single(client.into()),
                    message: Notification {
                        text: format!("Cannot research: {reason}"),
                        level: 1,
                    },
                });
            }
        }
    }
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
    fn a_node_needs_its_parents_first() {
        let reg = registry();
        let mut state = ResearchState::default();
        let steel = reg.tech("steelmaking").unwrap();
        assert!(!state.is_available(&reg, steel));
        let items: Vec<(ItemId, u32)> = reg
            .tech_def(steel)
            .unwrap()
            .cost
            .iter()
            .filter_map(|(n, c)| reg.item(n).map(|i| (i, *c)))
            .collect();
        assert_eq!(
            state.deliver(&reg, steel, &items),
            Err("prerequisites not researched")
        );
    }

    #[test]
    fn delivering_the_full_cost_completes_a_node() {
        let reg = registry();
        let mut state = ResearchState::default();
        let basic = reg.tech("basic_tools").unwrap();
        assert!(state.is_available(&reg, basic));
        let items: Vec<(ItemId, u32)> = reg
            .tech_def(basic)
            .unwrap()
            .cost
            .iter()
            .filter_map(|(n, c)| reg.item(n).map(|i| (i, *c)))
            .collect();
        assert_eq!(state.deliver(&reg, basic, &items), Ok(true));
        assert!(state.is_researched(basic));
    }

    #[test]
    fn partial_delivery_is_remembered() {
        let reg = registry();
        let mut state = ResearchState::default();
        let basic = reg.tech("basic_tools").unwrap();
        let timber = reg.item("timber").unwrap();
        let needed = reg
            .tech_def(basic)
            .unwrap()
            .cost
            .iter()
            .find(|(n, _)| n == "timber")
            .map(|(_, c)| *c)
            .unwrap();
        assert_eq!(
            state.deliver(&reg, basic, &[(timber, needed / 2)]),
            Ok(false)
        );
        assert!(!state.is_researched(basic));
        assert_eq!(state.deliver(&reg, basic, &[(timber, needed)]), Ok(true));
    }

    #[test]
    fn unrelated_items_are_rejected() {
        let reg = registry();
        let mut state = ResearchState::default();
        let basic = reg.tech("basic_tools").unwrap();
        let gold = reg.item("gold_ingot").unwrap();
        assert_eq!(
            state.deliver(&reg, basic, &[(gold, 1)]),
            Err("item not needed by this node")
        );
    }

    /// Researches a node and everything it depends on, in dependency order.
    fn research_with_parents(state: &mut ResearchState, reg: &ContentRegistry, name: &str) {
        let node = reg.tech(name).expect("node exists");
        let parents: Vec<String> = reg
            .tech_def(node)
            .map(|d| d.parents.clone())
            .unwrap_or_default();
        for parent in parents {
            research_with_parents(state, reg, &parent);
        }
        let items: Vec<(ItemId, u32)> = reg
            .tech_def(node)
            .unwrap()
            .cost
            .iter()
            .filter_map(|(n, c)| reg.item(n).map(|i| (i, *c)))
            .collect();
        let _ = state.deliver(reg, node, &items);
    }

    #[test]
    fn completing_a_parent_unlocks_its_children() {
        let reg = registry();
        let mut state = ResearchState::default();
        research_with_parents(&mut state, &reg, "basic_tools");
        let field = reg.tech("field_research").unwrap();
        let items: Vec<(ItemId, u32)> = reg
            .tech_def(field)
            .unwrap()
            .cost
            .iter()
            .filter_map(|(n, c)| reg.item(n).map(|i| (i, *c)))
            .collect();
        assert_eq!(state.deliver(&reg, field, &items), Ok(true));
        // `smelting` requires `field_research`, so it is now available.
        let smelting = reg.tech("smelting").unwrap();
        assert!(state.is_available(&reg, smelting));
    }
}
