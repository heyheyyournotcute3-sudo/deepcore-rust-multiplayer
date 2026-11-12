//! On-screen HUD: crosshair, hotbar, vitals, depth meter and the "too hard" warning.
//!
//! Built with Bevy UI rather than egui because it is drawn every frame and must be cheap.

use bevy::prelude::*;
use bevy::text::FontSize;
use deepcore_shared::{content::ContentRegistry, voxel::coords::SURFACE_Y};

use crate::{
    input::{
        controller::{FirstPersonCamera, LocalPlayer},
        mining::Targeting,
    },
    render::chunks::ChunkStats,
};

/// Plugin that builds and updates the HUD.
pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud)
            .add_systems(Update, (update_crosshair, update_depth, update_stats_text));
    }
}

/// The crosshair's two bars, recoloured when the target is too hard to mine.
#[derive(Component)]
pub struct CrosshairPart;

/// The depth meter text.
#[derive(Component)]
pub struct DepthText;

/// The debug/F3 stats text.
#[derive(Component)]
pub struct StatsText;

/// The "needs tier N" warning text.
#[derive(Component)]
pub struct WarningText;

fn spawn_hud(mut commands: Commands) {
    let crosshair_color = Color::srgba(1.0, 1.0, 1.0, 0.85);
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            Name::new("HUD"),
        ))
        .with_children(|root| {
            // Crosshair: a horizontal and a vertical bar centred on screen.
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    top: Val::Percent(50.0),
                    width: Val::Px(18.0),
                    height: Val::Px(2.0),
                    margin: UiRect::new(Val::Px(-9.0), Val::Auto, Val::Px(-1.0), Val::Auto),
                    ..default()
                },
                BackgroundColor(crosshair_color),
                CrosshairPart,
            ));
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    top: Val::Percent(50.0),
                    width: Val::Px(2.0),
                    height: Val::Px(18.0),
                    margin: UiRect::new(Val::Px(-1.0), Val::Auto, Val::Px(-9.0), Val::Auto),
                    ..default()
                },
                BackgroundColor(crosshair_color),
                CrosshairPart,
            ));

            // Warning text just under the crosshair.
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Percent(50.0),
                    top: Val::Percent(50.0),
                    margin: UiRect::new(Val::Px(-120.0), Val::Auto, Val::Px(28.0), Val::Auto),
                    width: Val::Px(240.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(17.0),
                    ..default()
                },
                TextColor(Color::srgb(1.0, 0.35, 0.3)),
                WarningText,
            ));

            // Depth meter, bottom left.
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(18.0),
                    bottom: Val::Px(18.0),
                    ..default()
                },
                Text::new("Depth 0 m"),
                TextFont {
                    font_size: FontSize::Px(20.0),
                    ..default()
                },
                TextColor(Color::srgb(0.85, 0.92, 1.0)),
                DepthText,
            ));

            // Debug stats, top left.
            root.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(18.0),
                    top: Val::Px(14.0),
                    ..default()
                },
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(15.0),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.9, 0.8, 0.85)),
                StatsText,
            ));
        });
}

/// Recolours the crosshair red when the target block needs a better tool.
fn update_crosshair(
    targeting: Res<Targeting>,
    mut parts: Query<&mut BackgroundColor, With<CrosshairPart>>,
) {
    let color = if targeting.hit.is_some() && !targeting.can_mine && targeting.required_tier > 0 {
        Color::srgba(1.0, 0.25, 0.2, 0.95)
    } else if targeting.can_mine {
        Color::srgba(0.75, 1.0, 0.75, 0.9)
    } else {
        Color::srgba(1.0, 1.0, 1.0, 0.85)
    };
    for mut bg in &mut parts {
        bg.0 = color;
    }
}

/// Updates the depth meter and the "too hard" warning.
fn update_depth(
    camera: Query<&Transform, With<FirstPersonCamera>>,
    mut depth_text: Query<&mut Text, (With<DepthText>, Without<WarningText>)>,
    mut warning: Query<&mut Text, (With<WarningText>, Without<DepthText>)>,
    targeting: Res<Targeting>,
) {
    if let Ok(transform) = camera.single()
        && let Ok(mut text) = depth_text.single_mut()
    {
        let depth = (SURFACE_Y as f32 - transform.translation.y).round() as i32;
        let layer = if depth <= 0 {
            "Surface".to_string()
        } else if depth <= 16 {
            "Topsoil".to_string()
        } else if depth <= 64 {
            "Shale Beds".to_string()
        } else if depth <= 160 {
            "Granite Depths".to_string()
        } else if depth <= 288 {
            "Basalt Furnace".to_string()
        } else if depth <= 448 {
            "Deep Crust".to_string()
        } else {
            "Mantle Edge".to_string()
        };
        **text = format!("Depth {} m  ·  {}", depth.max(0), layer);
    }

    if let Ok(mut text) = warning.single_mut() {
        **text = if targeting.hit.is_some() && !targeting.can_mine && targeting.required_tier > 0 {
            format!("Too hard — needs Tier {}", targeting.required_tier)
        } else {
            String::new()
        };
    }
}

/// Shows chunk counts and the world seed on the debug line.
fn update_stats_text(
    stats: Res<ChunkStats>,
    player: Res<LocalPlayer>,
    registry: Res<ContentRegistry>,
    mut prediction: ResMut<crate::prediction::PredictionState>,
    mut text: Query<&mut Text, With<StatsText>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut counter_second: Local<f32>,
) {
    // The correction counter is a per-second statistic, so reset it every second.
    *counter_second += time.delta_secs();
    if *counter_second >= 1.0 {
        *counter_second = 0.0;
        prediction.reset_correction_counter();
    }
    let Ok(mut text) = text.single_mut() else {
        return;
    };
    if keys.pressed(KeyCode::F3) {
        **text = format!(
            "chunks {} drawn {} (meshing {})  ·  pos {:.1}, {:.1}, {:.1}  ·  unacked {} corr {}  ·  content {}",
            stats.loaded,
            stats.drawn,
            stats.pending,
            player.body.position[0],
            player.body.position[1],
            player.body.position[2],
            prediction.unacked(),
            prediction.corrections_this_second,
            registry.content_hash().get(..8).unwrap_or("?"),
        );
    } else {
        text.clear();
    }
}
