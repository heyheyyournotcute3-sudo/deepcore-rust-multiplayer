//! Replicon registration shared by both sides.
//!
//! The client and the server must register exactly the same components and messages, in the same
//! order, or the replication protocol hashes will not match. Keeping that registration in one
//! place makes a mismatch impossible.

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::protocol::*;

/// Channel carrying chunk data (reliable, ordered).
pub const CHUNK_CHANNEL: Channel = Channel::Ordered;

/// Channel carrying block-change batches (reliable, ordered).
pub const BLOCK_CHANNEL: Channel = Channel::Ordered;

/// Channel carrying the handshake and other session messages (reliable, ordered).
pub const SESSION_CHANNEL: Channel = Channel::Ordered;

/// Channel carrying player transforms (unreliable).
pub const MOVE_CHANNEL: Channel = Channel::Unordered;

/// Channel carrying the client's movement intents (unreliable, sequenced by `seq`).
pub const MOVE_INPUT_CHANNEL: Channel = Channel::Unordered;

/// Channel carrying power/steam overlay stats (unreliable).
pub const STATS_CHANNEL: Channel = Channel::Unordered;

/// The replicated player transform.
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ReplicatedTransform(pub NetTransform);

/// A replicated player's identity and appearance.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
pub struct ReplicatedPlayerInfo(pub NetPlayerInfo);

/// A replicated machine's visible state.
#[derive(Component, Debug, Clone, Serialize, Deserialize)]
pub struct ReplicatedMachine(pub NetMachine);

/// Registers every replicated component and remote message. Must be called on both sides.
pub fn register_networking(app: &mut App) {
    app.replicate::<ReplicatedTransform>()
        .replicate::<ReplicatedPlayerInfo>()
        .replicate::<ReplicatedMachine>()
        // The handshake is a *message*, not an event: messages are not gated by client
        // authorization, so a client can ask to connect before it has been authorized.
        .add_client_message::<HandshakeRequest>(SESSION_CHANNEL)
        // Player intents: client -> server.
        .add_client_event::<MoveInputMsg>(MOVE_INPUT_CHANNEL)
        .add_client_event::<StartMiningMsg>(Channel::Ordered)
        .add_client_event::<InteractMsg>(Channel::Ordered)
        .add_client_event::<PlaceBlockMsg>(Channel::Ordered)
        .add_client_event::<MoveItemsMsg>(Channel::Ordered)
        .add_client_event::<CraftMsg>(Channel::Ordered)
        .add_client_event::<SetMachineRecipeMsg>(Channel::Ordered)
        .add_client_event::<ResearchDepositMsg>(Channel::Ordered)
        .add_client_event::<ChatMsg>(Channel::Ordered)
        .add_client_event::<RespawnMsg>(Channel::Ordered)
        .add_client_event::<ReactorControlMsg>(Channel::Ordered)
        // Server -> client notifications.
        .add_server_event::<Notification>(Channel::Ordered)
        .add_server_event::<ChatBroadcast>(Channel::Ordered)
        .add_server_event::<MiningFeedback>(Channel::Ordered)
        .add_server_event::<InventoryUpdate>(Channel::Ordered)
        .add_server_message::<HandshakeResponse>(SESSION_CHANNEL)
        .make_message_independent::<HandshakeResponse>()
        .add_server_event::<BlockChange>(BLOCK_CHANNEL)
        .add_server_event::<ChunkMessage>(CHUNK_CHANNEL)
        .add_server_event::<ChunkUnload>(CHUNK_CHANNEL)
        .add_server_event::<NetTechState>(Channel::Ordered)
        .add_server_event::<MachineContents>(Channel::Ordered)
        .add_server_event::<NetPowerStats>(STATS_CHANNEL)
        // Notifications reach clients before authorization so they can show a "connecting"
        // message; everything else waits until authorization succeeds.
        .make_event_independent::<Notification>();
}
