//! Verifies the network registration sequence is valid on both sides.
use bevy::prelude::*;
use bevy_replicon::prelude::*;

#[test]
fn registration_sequence_is_valid() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::state::app::StatesPlugin,
        RepliconPlugins,
    ));
    deepcore_shared::protocol::register_networking(&mut app);
    // If registration panicked we would not get here.
    assert!(app.world().contains_resource::<RepliconChannels>());
}
