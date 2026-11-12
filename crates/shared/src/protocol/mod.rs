//! Network protocol: replicated components, client intents and server events.
//!
//! Everything here is versioned by [`crate::PROTOCOL_VERSION`]. Bump that constant whenever a
//! type in this module changes shape, so mismatched clients are rejected at the handshake
//! instead of desyncing.

use serde::{Deserialize, Serialize};

use crate::content::{BlockId, ItemId, MachineId, RecipeId, TechId};
use crate::inventory::ItemStack;
use crate::mining::Face;
use crate::voxel::BlockPos;

/// A block position on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NetBlockPos {
    /// X.
    pub x: i32,
    /// Y.
    pub y: i32,
    /// Z.
    pub z: i32,
}

impl From<BlockPos> for NetBlockPos {
    fn from(p: BlockPos) -> Self {
        Self {
            x: p.x,
            y: p.y,
            z: p.z,
        }
    }
}

impl From<NetBlockPos> for BlockPos {
    fn from(p: NetBlockPos) -> Self {
        BlockPos::new(p.x, p.y, p.z)
    }
}

/// A face on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetFace {
    /// +X
    East,
    /// -X
    West,
    /// +Y
    Up,
    /// -Y
    Down,
    /// +Z
    South,
    /// -Z
    North,
}

impl From<Face> for NetFace {
    fn from(f: Face) -> Self {
        match f {
            Face::East => Self::East,
            Face::West => Self::West,
            Face::Up => Self::Up,
            Face::Down => Self::Down,
            Face::South => Self::South,
            Face::North => Self::North,
        }
    }
}

impl From<NetFace> for Face {
    fn from(f: NetFace) -> Self {
        match f {
            NetFace::East => Face::East,
            NetFace::West => Face::West,
            NetFace::Up => Face::Up,
            NetFace::Down => Face::Down,
            NetFace::South => Face::South,
            NetFace::North => Face::North,
        }
    }
}

/// A replicated player transform, sent every tick on the unreliable channel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NetTransform {
    /// Feet position.
    pub position: [f32; 3],
    /// Velocity, used for interpolation.
    pub velocity: [f32; 3],
    /// Yaw in radians.
    pub yaw: f32,
    /// Pitch in radians.
    pub pitch: f32,
    /// The last movement sequence number the server processed.
    pub acked_seq: u32,
    /// The server tick this state was produced on.
    pub server_tick: u32,
}

/// A replicated player's appearance and vitals, sent on change only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetPlayerInfo {
    /// Display name.
    pub name: String,
    /// Health, 0..max.
    pub health: f32,
    /// Whether the player is sprinting, for animation.
    pub sprinting: bool,
    /// Whether the player is crouching.
    pub crouching: bool,
    /// The item in the player's hand.
    pub held_item: Option<ItemId>,
}

/// Vitals visible only to their owner.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NetVitals {
    /// Health.
    pub health: f32,
    /// Stamina.
    pub stamina: f32,
    /// Oxygen in seconds.
    pub oxygen: f32,
    /// Body heat.
    pub heat: f32,
    /// Radiation dose.
    pub dose: f32,
    /// Depth below the surface.
    pub depth: i32,
}

/// A single block change, batched per tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockChange {
    /// Where the change happened.
    pub pos: NetBlockPos,
    /// The new block.
    pub block: BlockId,
}

/// A chunk sent from the server to a client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChunkMessage {
    /// Chunk coordinates.
    pub x: i32,
    /// Chunk Y.
    pub y: i32,
    /// Chunk Z.
    pub z: i32,
    /// zstd-compressed postcard payload of the chunk's palette and data.
    pub payload: Vec<u8>,
}

/// A chunk the client should drop because it left the view radius.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkUnload {
    /// Chunk coordinates.
    pub x: i32,
    /// Chunk Y.
    pub y: i32,
    /// Chunk Z.
    pub z: i32,
}

/// A replicated machine's visible state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetMachine {
    /// Position of the machine's anchor block.
    pub pos: NetBlockPos,
    /// Which machine this is.
    pub kind: MachineId,
    /// Yaw the machine faces, in quarter turns.
    pub rotation: u8,
    /// Whether it is currently producing.
    pub running: bool,
    /// Coarse progress, quantised to 0..255 so it replicates cheaply.
    pub progress: u8,
    /// The idle reason code, for the UI.
    pub idle_code: u8,
}

/// A machine's full slot contents, sent only to the client with that UI open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineContents {
    /// The machine entity this belongs to.
    pub machine: u64,
    /// Input slots.
    pub inputs: Vec<ItemStack>,
    /// Output slots.
    pub outputs: Vec<ItemStack>,
    /// Fuel slot.
    pub fuel: ItemStack,
    /// Installed modules.
    pub modules: (u8, u8),
}

/// The team-shared tech tree state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetTechState {
    /// Which nodes are researched.
    pub researched: Vec<TechId>,
    /// How much of each node's cost has been delivered, as `(node, progress)`.
    pub deposits: Vec<(TechId, Vec<(ItemId, u32)>)>,
}

/// Power/steam network statistics, sent once per second to clients viewing the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NetPowerStats {
    /// Network identifier.
    pub network: u32,
    /// Supply.
    pub supply: f32,
    /// Demand.
    pub demand: f32,
    /// Stored energy.
    pub stored: f32,
    /// Capacity.
    pub capacity: f32,
    /// Medium: 0 = steam, 1 = electric.
    pub medium: u8,
}

// ================================================================ client -> server intents

/// A movement intent, sent unreliably with a sequence number.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MoveInputMsg {
    /// Sequence number, increasing per input.
    pub seq: u32,
    /// Forward axis.
    pub forward: f32,
    /// Right axis.
    pub right: f32,
    /// Jump pressed.
    pub jump: bool,
    /// Sprint held.
    pub sprint: bool,
    /// Crouch held.
    pub crouch: bool,
    /// Absolute yaw.
    pub yaw: f32,
    /// Absolute pitch.
    pub pitch: f32,
}

/// The client's start-mining intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartMiningMsg {
    /// The block being mined.
    pub block: NetBlockPos,
    /// The face being hit.
    pub face: NetFace,
}

/// The client's place-block intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceBlockMsg {
    /// Where to place.
    pub pos: NetBlockPos,
    /// Which face was clicked.
    pub face: NetFace,
    /// Which hotbar or backpack slot supplies the block.
    pub item_slot: u8,
}

/// The client's interact intent, e.g. opening a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractMsg {
    /// The block interacted with.
    pub pos: NetBlockPos,
}

/// The client's item-move intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveItemsMsg {
    /// Source container: 0 = player, 1 = the machine in `container_id`.
    pub from_container: u8,
    /// Source slot.
    pub from_slot: u16,
    /// Destination container.
    pub to_container: u8,
    /// Destination slot.
    pub to_slot: u16,
    /// How many to move; 0 means "as many as fit".
    pub count: u32,
    /// The machine entity, when a container is a machine.
    pub container_id: u64,
    /// Whether to split the stack in half instead of moving it.
    pub split: bool,
}

/// The client's crafting intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CraftMsg {
    /// The recipe to craft.
    pub recipe: RecipeId,
    /// How many times.
    pub count: u32,
}

/// The client's intent to change what a machine is producing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetMachineRecipeMsg {
    /// The machine entity.
    pub machine: u64,
    /// The recipe to run, or `None` to clear.
    pub recipe: Option<RecipeId>,
}

/// The client's research deposit intent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResearchDepositMsg {
    /// The tech node.
    pub node: TechId,
    /// The items offered from the player's inventory.
    pub items: Vec<(ItemId, u32)>,
}

/// A chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMsg {
    /// The text.
    pub text: String,
}

/// The client's request to respawn after death.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RespawnMsg;

/// The client's reactor control intent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReactorControlMsg {
    /// The reactor console entity.
    pub console: u64,
    /// Start or stop the reactor.
    pub running: Option<bool>,
    /// Set rod withdrawal, 0..1.
    pub rods: Option<f32>,
}

// ================================================================ server -> client events

/// A notification shown in the HUD.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    /// The message text.
    pub text: String,
    /// 0 = info, 1 = warning, 2 = danger.
    pub level: u8,
}

/// A chat message from another player, or a system message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatBroadcast {
    /// Sender name, or "Server".
    pub from: String,
    /// The text.
    pub text: String,
}

/// The handshake request a client sends first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandshakeRequest {
    /// The client's protocol version.
    pub protocol: u32,
    /// The client's content hash.
    pub content_hash: String,
    /// The display name the player wants.
    pub display_name: String,
    /// An auth token from the lobby, when there is one.
    pub auth_token: Option<String>,
}

/// The handshake response the server sends back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandshakeResponse {
    /// Whether the connection was accepted.
    pub accepted: bool,
    /// Why it was refused, if it was.
    pub reason: String,
    /// The world seed, so the client can predict generation while chunks stream.
    pub world_seed: u64,
    /// The server tick rate.
    pub tick_hz: u32,
}

/// The result of a mining action, sent back to the miner for feedback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MiningFeedback {
    /// The block.
    pub block: NetBlockPos,
    /// 0 = started, 1 = completed, 2 = too hard, 3 = out of range.
    pub kind: u8,
    /// The required tier, when `kind == 2`.
    pub required_tier: u8,
}

/// The owner's inventory contents, sent on change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventoryUpdate {
    /// All slots, hotbar first.
    pub slots: Vec<ItemStack>,
    /// Which hotbar slot is selected.
    pub selected: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_messages_round_trip_through_postcard() {
        let msg = HandshakeRequest {
            protocol: crate::PROTOCOL_VERSION,
            content_hash: "abc123".to_string(),
            display_name: "Miner".to_string(),
            auth_token: None,
        };
        let bytes = postcard::to_allocvec(&msg).expect("serialize");
        let back: HandshakeRequest = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(msg, back);
    }

    #[test]
    fn block_positions_convert_both_ways() {
        let pos = BlockPos::new(-5, -120, 7);
        let net: NetBlockPos = pos.into();
        let back: BlockPos = net.into();
        assert_eq!(pos, back);
    }

    #[test]
    fn faces_convert_both_ways() {
        for face in [
            Face::East,
            Face::West,
            Face::Up,
            Face::Down,
            Face::South,
            Face::North,
        ] {
            let net: NetFace = face.into();
            let back: Face = net.into();
            assert_eq!(face, back);
        }
    }

    #[test]
    fn transform_is_compact_on_the_wire() {
        let transform = NetTransform {
            position: [1.0, 2.0, 3.0],
            velocity: [0.0, 0.0, 0.0],
            yaw: 0.5,
            pitch: -0.2,
            acked_seq: 42,
            server_tick: 1000,
        };
        let bytes = postcard::to_allocvec(&transform).expect("serialize");
        // 6 floats + 2 u32 = 32 bytes; the design budget is a few dozen bytes per player.
        assert!(bytes.len() <= 40, "transform took {} bytes", bytes.len());
    }
}
