//! Power and steam networks.
//!
//! Networks are sets of connected blocks of the same medium: copper pipes carry steam, cables
//! carry electricity. Topology is only recomputed when a node is placed or removed; each tick
//! just sums supply and demand, which keeps the cost flat as the base grows.

use bevy::platform::collections::HashMap;

/// The medium a network carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Medium {
    /// Steam in copper pipes.
    Steam,
    /// Electricity in cables.
    Electric,
}

/// A network node: something that produces, consumes or stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// One node's contribution to a network.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    /// Identifier, stable for the lifetime of the block entity.
    pub id: NodeId,
    /// How much this node can supply, in kW or steam units per second.
    pub supply: f32,
    /// How much this node wants, in kW or steam units per second.
    pub demand: f32,
    /// Stored energy (batteries only), in kJ.
    pub stored: f32,
    /// Maximum storage, in kJ. Zero for nodes that cannot store.
    pub capacity: f32,
}

impl Node {
    /// A consumer that draws `demand` and produces nothing.
    #[must_use]
    pub fn consumer(id: NodeId, demand: f32) -> Self {
        Self {
            id,
            supply: 0.0,
            demand,
            stored: 0.0,
            capacity: 0.0,
        }
    }

    /// A producer that supplies `supply`.
    #[must_use]
    pub fn producer(id: NodeId, supply: f32) -> Self {
        Self {
            id,
            supply,
            demand: 0.0,
            stored: 0.0,
            capacity: 0.0,
        }
    }

    /// A battery with `capacity` and an initial charge.
    #[must_use]
    pub fn battery(id: NodeId, capacity: f32, stored: f32) -> Self {
        Self {
            id,
            supply: 0.0,
            demand: 0.0,
            stored: stored.min(capacity),
            capacity,
        }
    }
}

/// The result of solving one network for one tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NetworkSolution {
    /// Total supply available this tick.
    pub supply: f32,
    /// Total demand requested this tick.
    pub demand: f32,
    /// Fraction of demand that can be met, 0..1. Consumers run at this speed.
    pub satisfaction: f32,
    /// Energy charged into batteries this tick.
    pub charged: f32,
    /// Energy drawn out of batteries this tick.
    pub discharged: f32,
}

/// Solves a network's supply against its demand.
///
/// Rules from the design:
/// - If demand exceeds supply, every consumer runs at `supply / demand` speed (brownout).
/// - Batteries absorb surplus and cover deficits.
/// - Batteries never discharge below empty and never charge past capacity.
pub fn solve(nodes: &[Node], dt: f32) -> NetworkSolution {
    let mut supply: f32 = 0.0;
    let mut demand: f32 = 0.0;
    let mut stored: f32 = 0.0;
    let mut capacity: f32 = 0.0;
    for node in nodes {
        supply += node.supply.max(0.0);
        demand += node.demand.max(0.0);
        stored += node.stored.max(0.0);
        capacity += node.capacity.max(0.0);
    }

    let mut charged = 0.0;
    let mut discharged = 0.0;

    if supply >= demand {
        // Surplus charges the batteries, if there are any with room.
        let surplus = supply - demand;
        let room = (capacity - stored).max(0.0);
        // Convert the per-second surplus into energy for this tick.
        charged = (surplus * dt).min(room);
    } else {
        // Deficit is covered by stored energy where possible.
        let deficit = demand - supply;
        let available = stored.max(0.0);
        discharged = (deficit * dt).min(available);
    }

    // What actually reaches consumers this tick.
    let delivered = supply + discharged / dt.max(1e-6);
    let satisfaction = if demand <= 0.0 {
        1.0
    } else {
        (delivered / demand).clamp(0.0, 1.0)
    };

    NetworkSolution {
        supply,
        demand,
        satisfaction,
        charged,
        discharged,
    }
}

/// A set of networks keyed by a component identifier.
#[derive(Debug, Default)]
pub struct NetworkGraph {
    /// Nodes belonging to each network.
    networks: HashMap<u32, Vec<Node>>,
    /// Which network each node belongs to.
    assignment: HashMap<NodeId, u32>,
}

impl NetworkGraph {
    /// Creates an empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a node to a network.
    pub fn insert(&mut self, network: u32, node: Node) {
        self.remove(node.id);
        self.assignment.insert(node.id, network);
        self.networks.entry(network).or_default().push(node);
    }

    /// Removes a node from whatever network it is in.
    pub fn remove(&mut self, id: NodeId) {
        if let Some(network) = self.assignment.remove(&id)
            && let Some(list) = self.networks.get_mut(&network)
        {
            list.retain(|n| n.id != id);
        }
    }

    /// Which network a node belongs to.
    #[must_use]
    pub fn network_of(&self, id: NodeId) -> Option<u32> {
        self.assignment.get(&id).copied()
    }

    /// Solves one network, returning `None` if it does not exist.
    #[must_use]
    pub fn solve_network(&self, network: u32, dt: f32) -> Option<NetworkSolution> {
        self.networks.get(&network).map(|nodes| solve(nodes, dt))
    }

    /// Number of networks.
    #[must_use]
    pub fn network_count(&self) -> usize {
        self.networks.len()
    }

    /// Number of nodes in a network.
    #[must_use]
    pub fn len_of(&self, network: u32) -> usize {
        self.networks.get(&network).map_or(0, Vec::len)
    }

    /// Merges two networks, used when a placed block joins them.
    pub fn merge(&mut self, a: u32, b: u32) {
        if a == b {
            return;
        }
        let moved: Vec<Node> = self.networks.remove(&b).unwrap_or_default();
        for node in &moved {
            self.assignment.insert(node.id, a);
        }
        self.networks.entry(a).or_default().extend(moved);
    }
}

/// Applies a solution's charge/discharge back onto the battery nodes.
///
/// Call after [`solve`] to persist the energy change.
pub fn apply_storage(nodes: &mut [Node], solution: &NetworkSolution) {
    // Charge: fill batteries proportionally to their free space.
    if solution.charged > 0.0 {
        let to_charge = solution.charged;
        let room: f32 = nodes.iter().map(|n| (n.capacity - n.stored).max(0.0)).sum();
        if room > 0.0 {
            for node in nodes.iter_mut() {
                let free = (node.capacity - node.stored).max(0.0);
                let share = to_charge * (free / room);
                node.stored = (node.stored + share).min(node.capacity);
            }
        }
    }

    // Discharge: drain batteries proportionally to their stored energy.
    if solution.discharged > 0.0 {
        let to_discharge = solution.discharged;
        let stored: f32 = nodes.iter().map(|n| n.stored.max(0.0)).sum();
        if stored > 0.0 {
            for node in nodes.iter_mut() {
                let share = to_discharge * (node.stored.max(0.0) / stored);
                node.stored = (node.stored - share).max(0.0);
            }
        }
    }
}

/// Converts a machine's power requirement into a per-tick demand.
#[must_use]
pub fn demand_per_tick(kw: f32, dt: f32) -> f32 {
    kw * dt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supply_meets_demand() {
        let nodes = vec![
            Node::producer(NodeId(1), 100.0),
            Node::consumer(NodeId(2), 60.0),
        ];
        let sol = solve(&nodes, 1.0);
        assert!((sol.satisfaction - 1.0).abs() < 1e-6);
        assert!(
            sol.charged.abs() < 1e-6,
            "with no batteries there is nowhere to put the surplus"
        );
    }

    #[test]
    fn surplus_charges_a_battery_when_present() {
        let nodes = vec![
            Node::producer(NodeId(1), 100.0),
            Node::consumer(NodeId(2), 60.0),
            Node::battery(NodeId(3), 500.0, 0.0),
        ];
        let sol = solve(&nodes, 1.0);
        assert!((sol.charged - 40.0).abs() < 1e-4, "charged {}", sol.charged);
    }

    #[test]
    fn brownout_scales_with_supply_ratio() {
        let nodes = vec![
            Node::producer(NodeId(1), 50.0),
            Node::consumer(NodeId(2), 100.0),
        ];
        let sol = solve(&nodes, 1.0);
        assert!(
            (sol.satisfaction - 0.5).abs() < 1e-6,
            "got {}",
            sol.satisfaction
        );
    }

    #[test]
    fn batteries_cover_a_deficit() {
        let nodes = vec![
            Node::consumer(NodeId(1), 100.0),
            Node::battery(NodeId(2), 1000.0, 500.0),
        ];
        let sol = solve(&nodes, 1.0);
        assert!(sol.discharged > 0.0, "battery should discharge");
        assert!(
            (sol.satisfaction - 1.0).abs() < 1e-6,
            "battery should fully cover the deficit"
        );
    }

    #[test]
    fn empty_batteries_cannot_cover_a_deficit() {
        let nodes = vec![
            Node::consumer(NodeId(1), 100.0),
            Node::battery(NodeId(2), 1000.0, 0.0),
        ];
        let sol = solve(&nodes, 1.0);
        assert!((sol.satisfaction).abs() < 1e-6);
    }

    #[test]
    fn storage_round_trips() {
        let mut nodes = vec![Node::battery(NodeId(1), 100.0, 0.0)];
        // Charge with 40 of surplus.
        let sol = solve(&nodes, 1.0);
        apply_storage(&mut nodes, &sol);
        assert!(nodes[0].stored.abs() < 1e-6, "no supply means no charge");

        nodes.push(Node::producer(NodeId(2), 40.0));
        let sol = solve(&nodes, 1.0);
        apply_storage(&mut nodes, &sol);
        assert!(
            (nodes[0].stored - 40.0).abs() < 1e-4,
            "stored {}",
            nodes[0].stored
        );

        // Now demand it back.
        nodes.retain(|n| n.id != NodeId(2));
        nodes.push(Node::consumer(NodeId(3), 40.0));
        let sol = solve(&nodes, 1.0);
        apply_storage(&mut nodes, &sol);
        assert!(nodes[0].stored.abs() < 1e-3, "battery should be drained");
    }

    #[test]
    fn battery_never_overcharges() {
        let mut nodes = vec![
            Node::battery(NodeId(1), 10.0, 9.0),
            Node::producer(NodeId(2), 1000.0),
        ];
        let sol = solve(&nodes, 1.0);
        apply_storage(&mut nodes, &sol);
        assert!(
            nodes[0].stored <= 10.0 + 1e-4,
            "overcharged to {}",
            nodes[0].stored
        );
    }

    #[test]
    fn network_graph_merges_when_connected() {
        let mut graph = NetworkGraph::new();
        graph.insert(1, Node::producer(NodeId(1), 50.0));
        graph.insert(2, Node::consumer(NodeId(2), 20.0));
        assert_eq!(graph.network_count(), 2);
        graph.merge(1, 2);
        assert_eq!(graph.network_count(), 1);
        assert_eq!(graph.len_of(1), 2);
        assert_eq!(graph.network_of(NodeId(2)), Some(1));
        let sol = graph.solve_network(1, 1.0).unwrap();
        assert!((sol.supply - 50.0).abs() < 1e-6);
        assert!((sol.demand - 20.0).abs() < 1e-6);
    }

    #[test]
    fn removing_a_node_detaches_it() {
        let mut graph = NetworkGraph::new();
        graph.insert(1, Node::producer(NodeId(7), 50.0));
        graph.remove(NodeId(7));
        assert_eq!(graph.len_of(1), 0);
        assert_eq!(graph.network_of(NodeId(7)), None);
    }

    #[test]
    fn zero_demand_is_fully_satisfied() {
        let nodes = vec![Node::producer(NodeId(1), 10.0)];
        let sol = solve(&nodes, 1.0);
        assert!((sol.satisfaction - 1.0).abs() < 1e-6);
    }
}
