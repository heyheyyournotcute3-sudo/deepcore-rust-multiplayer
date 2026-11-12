//! Client networking: connect, handshake, receive chunks and send intents.
//!
//! The client never decides anything. It sends intents, applies the authoritative block changes
//! and player transforms it receives, and reconciles its own predicted movement.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    str::FromStr,
    time::SystemTime,
};

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon_renet::{
    RenetChannelsExt, RenetClient, RepliconRenetPlugins,
    netcode::{ClientAuthentication, NetcodeClientTransport},
    renet::ConnectionConfig,
};
use deepcore_shared::{
    PROTOCOL_VERSION,
    content::ContentRegistry,
    protocol::{
        BlockChange, ChunkMessage, HandshakeRequest, HandshakeResponse, MiningFeedback,
        StartMiningMsg,
        net_register::{ReplicatedTransform, register_networking},
    },
    voxel::ChunkPos,
};
use tracing::{info, warn};

use crate::{
    ClientArgs,
    input::controller::{FirstPersonCamera, LocalPlayer},
    render::{chunks::ClientChunks, world_stream::WorldStreamer},
};

/// Netcode protocol ID, matching the server.
const NETCODE_PROTOCOL_ID: u64 = 0x0000_DEE9_C0DE;

/// Client networking plugin.
pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RepliconPlugins, RepliconRenetPlugins))
            .init_resource::<ConnectionState>()
            .add_systems(Startup, connect)
            .add_systems(
                Update,
                (
                    send_handshake,
                    sync_remote_players,
                    send_mining_intent,
                    send_move_input,
                )
                    .chain(),
            )
            // Server events arrive as observers, not buffered messages.
            .add_systems(Update, on_handshake_response)
            .add_observer(on_chunk_message)
            .add_observer(on_block_change)
            .add_observer(on_mining_feedback)
            .add_observer(apply_inventory_update);
        register_networking(app);
    }
}

/// Where the client is in the connection lifecycle.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Nothing has been attempted (playing the local world).
    #[default]
    Local,
    /// Connecting and waiting for the handshake response.
    Handshaking,
    /// Connected and playing on a server.
    Playing,
    /// Refused, with the reason to show the player.
    Refused(String),
}

/// Connects to the configured server, if any.
fn connect(
    mut commands: Commands,
    args: Res<ClientArgs>,
    mut state: ResMut<ConnectionState>,
    channels: Res<RepliconChannels>,
    mut streamer: ResMut<WorldStreamer>,
) {
    let Some(address) = args.connect.clone() else {
        info!("no --connect given: playing the local world");
        *state = ConnectionState::Local;
        return;
    };

    let ip = IpAddr::from_str(&address).unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let server_addr = SocketAddr::new(ip, args.port);

    let client = RenetClient::new(ConnectionConfig {
        server_channels_config: channels.server_configs(),
        client_channels_config: channels.client_configs(),
        ..Default::default()
    });

    let current_time = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(t) => t,
        Err(e) => {
            error!("system clock is before the unix epoch: {e}");
            return;
        }
    };
    let client_id = current_time.as_millis() as u64;
    let socket = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        Ok(s) => s,
        Err(e) => {
            error!("cannot bind a local UDP socket: {e}");
            *state = ConnectionState::Refused(format!("Cannot open a network socket: {e}"));
            return;
        }
    };
    let authentication = ClientAuthentication::Unsecure {
        client_id,
        protocol_id: NETCODE_PROTOCOL_ID,
        server_addr,
        user_data: None,
    };
    let transport = match NetcodeClientTransport::new(current_time, authentication, socket) {
        Ok(t) => t,
        Err(e) => {
            error!("cannot create the netcode transport: {e}");
            *state = ConnectionState::Refused(format!("Cannot start networking: {e}"));
            return;
        }
    };

    commands.insert_resource(client);
    commands.insert_resource(transport);
    // Chunks come from the server now, not from local generation.
    streamer.local_generation = false;
    streamer.generator = None;
    *state = ConnectionState::Handshaking;
    info!(%server_addr, "connecting");
}

/// Sends the handshake once the transport reports a connection.
fn send_handshake(
    mut commands: Commands,
    state: Res<ConnectionState>,
    client: Option<Res<RenetClient>>,
    registry: Res<ContentRegistry>,
    args: Res<ClientArgs>,
    mut sent: Local<bool>,
) {
    if *state != ConnectionState::Handshaking || *sent {
        return;
    }
    let Some(client) = client else {
        return;
    };
    if !client.is_connected() {
        return;
    }
    *sent = true;
    commands.write_message(HandshakeRequest {
        protocol: PROTOCOL_VERSION,
        content_hash: registry.content_hash().to_string(),
        display_name: args.name.clone(),
        auth_token: None,
    });
    info!("handshake sent");
}

/// Handles the server's handshake response.
fn on_handshake_response(
    mut responses: Option<MessageReader<HandshakeResponse>>,
    mut state: ResMut<ConnectionState>,
    mut streamer: ResMut<WorldStreamer>,
    registry: Res<ContentRegistry>,
) {
    let Some(responses) = responses.as_mut() else {
        return;
    };
    for response in responses.read() {
        if response.accepted {
            info!(seed = response.world_seed, "handshake accepted");
            // Adopt the server's seed so any locally predicted generation matches.
            if let Ok(generator) =
                deepcore_shared::worldgen::WorldGen::new(&registry, response.world_seed)
            {
                streamer.generator = Some(generator);
            }
            *state = ConnectionState::Playing;
        } else {
            warn!(reason = %response.reason, "connection refused");
            *state = ConnectionState::Refused(response.reason.clone());
        }
    }
}

/// Decodes chunk messages into the client's chunk store.
fn on_chunk_message(message: On<ChunkMessage>, mut chunks: ResMut<ClientChunks>) {
    let message = message.event();
    let Some(chunk) = crate::render::world_stream::decode_chunk_payload(&message.payload) else {
        warn!(
            x = message.x,
            y = message.y,
            z = message.z,
            "bad chunk payload"
        );
        return;
    };
    let expected = ChunkPos::new(message.x, message.y, message.z);
    if chunk.pos() != expected {
        warn!(?expected, got = ?chunk.pos(), "chunk position mismatch");
        return;
    }
    chunks.insert(chunk);
}

/// Applies authoritative block changes.
fn on_block_change(change: On<BlockChange>, mut chunks: ResMut<ClientChunks>) {
    let change = change.event();
    let pos: deepcore_shared::voxel::BlockPos = change.pos.into();
    chunks.set_block(pos, change.block);
}

/// Shows mining feedback (e.g. "too hard") to the player.
fn on_mining_feedback(feedback: On<MiningFeedback>) {
    let message = feedback.event();
    match message.kind {
        2 => debug!(
            tier = message.required_tier,
            "server refused mining: tier too low"
        ),
        1 => debug!("server confirmed a block break"),
        _ => {}
    }
}

/// Applies the owner's inventory snapshot from the server.
fn apply_inventory_update(
    update: On<deepcore_shared::protocol::InventoryUpdate>,
    mut inv: ResMut<crate::ui::inventory::ClientInventory>,
) {
    let update = update.event();
    for (index, stack) in update.slots.iter().enumerate() {
        inv.inventory.set_slot(index, *stack);
    }
    inv.selected = update.selected;
}

/// Interpolates other players' transforms onto their entities.
fn sync_remote_players(
    transforms: Query<(&ReplicatedTransform, &mut Transform), Without<FirstPersonCamera>>,
) {
    for (net, mut transform) in transforms {
        let p = net.0.position;
        transform.translation = Vec3::new(p[0], p[1] + 1.2, p[2]);
        transform.rotation = Quat::from_rotation_y(net.0.yaw);
    }
}

/// Sends a mining intent when the player starts mining a new block.
fn send_mining_intent(
    mut commands: Commands,
    state: Res<ConnectionState>,
    targeting: Res<crate::input::mining::Targeting>,
    mut last_target: Local<Option<deepcore_shared::voxel::BlockPos>>,
) {
    if *state != ConnectionState::Playing {
        return;
    }
    let current = targeting.hit.map(|h| h.block);
    if current != *last_target {
        *last_target = current;
        if let Some(hit) = targeting.hit {
            commands.client_trigger(StartMiningMsg {
                block: hit.block.into(),
                face: hit.face.into(),
            });
        }
    }
}

/// Sends the player's movement input to the server.
fn send_move_input(
    mut commands: Commands,
    state: Res<ConnectionState>,
    player: Res<LocalPlayer>,
    mut prediction: ResMut<crate::prediction::PredictionState>,
) {
    if *state != ConnectionState::Playing {
        return;
    }
    let seq = prediction.record(player.body);
    commands.client_trigger(deepcore_shared::protocol::MoveInputMsg {
        seq,
        forward: player.input.forward,
        right: player.input.right,
        jump: player.input.jump,
        sprint: player.input.sprint,
        crouch: player.input.crouch,
        yaw: player.body.yaw,
        pitch: player.body.pitch,
    });
}
