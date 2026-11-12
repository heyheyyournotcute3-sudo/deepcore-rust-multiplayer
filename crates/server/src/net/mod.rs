//! Server networking: transport, handshake validation and per-client chunk streaming.

use std::{
    net::{Ipv4Addr, UdpSocket},
    time::SystemTime,
};

use bevy::{platform::collections::HashMap, prelude::*};
use bevy_replicon::prelude::*;
use bevy_replicon_renet::{
    RenetChannelsExt, RenetServer, RepliconRenetPlugins,
    netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig},
    renet::ConnectionConfig,
};
use deepcore_shared::{
    PROTOCOL_VERSION,
    content::ContentRegistry,
    protocol::{HandshakeRequest, HandshakeResponse},
    voxel::ChunkPos,
};
use tracing::{info, warn};

use crate::ServerArgs;

/// Netcode protocol ID. Fixed for all Deepcore servers so clients can talk to any of them.
const NETCODE_PROTOCOL_ID: u64 = 0x0000_DEE9_C0DE;

/// Server networking plugin.
///
/// Replicon authorizes clients automatically when the protocol hash matches; we additionally
/// require the game-level handshake (content hash + display name) before authorizing, so a
/// client with stale content never receives world data.
pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RepliconPlugins, RepliconRenetPlugins))
            .init_resource::<HandshakeState>()
            .add_systems(Startup, start_server)
            .add_systems(Update, on_handshake_request)
            .add_systems(Update, admit_new_clients);
        deepcore_shared::protocol::register_networking(app);
    }
}

/// Per-client handshake bookkeeping.
#[derive(Resource, Default)]
pub struct HandshakeState {
    /// Clients that completed the handshake successfully.
    pub admitted: HashMap<Entity, AdmittedClient>,
}

/// A client that passed the handshake.
#[derive(Debug, Clone)]
pub struct AdmittedClient {
    /// Display name.
    pub name: String,
    /// The chunk the client's player is centred on, for interest management.
    pub centre: ChunkPos,
}

/// Spawns the renet server transport and inserts the resources replicon needs.
fn start_server(mut commands: Commands, args: Res<ServerArgs>, channels: Res<RepliconChannels>) {
    let server = RenetServer::new(ConnectionConfig {
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
    let socket = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, args.port)) {
        Ok(s) => s,
        Err(e) => {
            error!("cannot bind UDP port {}: {e}", args.port);
            return;
        }
    };
    let server_config = ServerConfig {
        current_time,
        max_clients: 16,
        protocol_id: NETCODE_PROTOCOL_ID,
        authentication: ServerAuthentication::Unsecure,
        public_addresses: Default::default(),
    };
    let transport = match NetcodeServerTransport::new(server_config, socket) {
        Ok(t) => t,
        Err(e) => {
            error!("cannot create netcode transport: {e}");
            return;
        }
    };

    commands.insert_resource(server);
    commands.insert_resource(transport);
    info!(port = args.port, "listening for UDP connections");
}

/// Handles a client's handshake request: checks the protocol version and content hash.
fn on_handshake_request(
    mut requests: Option<MessageReader<FromClient<HandshakeRequest>>>,
    mut commands: Commands,
    registry: Res<ContentRegistry>,
    args: Res<ServerArgs>,
    mut state: ResMut<HandshakeState>,
) {
    let Some(requests) = requests.as_mut() else {
        return;
    };
    for FromClient {
        client_id,
        message: req,
    } in requests.read()
    {
        let client = *client_id;
        let Some(client_entity) = client.entity() else {
            warn!("handshake from a non-entity client id, ignoring");
            continue;
        };

        let (accepted, reason) = if req.protocol != PROTOCOL_VERSION {
            (
                false,
                format!(
                    "Protocol mismatch: server is {}, client is {}",
                    PROTOCOL_VERSION, req.protocol
                ),
            )
        } else if req.content_hash != registry.content_hash() {
            (
                false,
                "Content mismatch: update your game files".to_string(),
            )
        } else if req.display_name.trim().is_empty() {
            (false, "Please choose a display name".to_string())
        } else {
            (true, String::new())
        };

        if accepted {
            info!(client = ?client_entity, name = %req.display_name, "client admitted");
            // Authorize the client so replication and all other messages start flowing to it.
            commands.entity(client_entity).insert(AuthorizedClient);
            state.admitted.insert(
                client_entity,
                AdmittedClient {
                    name: req.display_name.clone(),
                    centre: ChunkPos::new(0, 0, 0),
                },
            );
        } else {
            warn!(client = ?client_entity, %reason, "client rejected");
        }

        commands.write_message(ToClients {
            targets: SendTargets::Single(client),
            message: HandshakeResponse {
                accepted,
                reason,
                world_seed: args.seed,
                tick_hz: deepcore_shared::SERVER_TICK_HZ,
            },
        });
    }
}

/// Marks clients that finished the handshake so the world can spawn a player for them.
#[allow(clippy::needless_pass_by_value)]
fn admit_new_clients(
    mut commands: Commands,
    clients: Query<Entity, With<ConnectedClient>>,
    state: Res<HandshakeState>,
    mut spawned: Local<std::collections::HashSet<Entity>>,
) {
    for client in &clients {
        if state.admitted.contains_key(&client) && spawned.insert(client) {
            commands.trigger(SpawnPlayerFor { client });
        }
    }
    // Forget clients that disconnected so a reconnect spawns a fresh player.
    spawned.retain(|e| clients.contains(*e));
}

/// Trigger fired when a client should get a player entity.
#[derive(Event, Debug)]
pub struct SpawnPlayerFor {
    /// The connected client entity.
    pub client: Entity,
}
