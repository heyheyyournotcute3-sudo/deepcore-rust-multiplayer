//! egui screens: inventory, crafting, machines and the tech tree.
//!
//! The inventory grid lands with the content milestone; this plugin currently owns the egui
//! context setup and the small always-available overlay (help text and connection status).

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

/// Client UI plugin.
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        // Egui is wired up but no window is drawn yet: the inventory, crafting, machine and tech
        // tree screens arrive with the content milestone. Keeping the plugin registered here
        // means those screens only need systems added, not plumbing.
        app.add_plugins(EguiPlugin::default())
            .add_systems(EguiPrimaryContextPass, draw_overlay);
    }
}

/// Draws the small status overlay and the controls hint.
fn draw_overlay(mut contexts: EguiContexts) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Deepcore")
        .anchor(egui::Align2::RIGHT_TOP, [-12.0, 12.0])
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("WASD move · Shift sprint · Ctrl crouch · Space jump");
            ui.label("Left click mine · Esc release mouse · F3 debug");
        });
    Ok(())
}
