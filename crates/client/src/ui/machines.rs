//! Machine screens: slots, the idle reason, and reactor controls.
//!
//! The panel shows whatever the server last sent for the machine. Opening a machine asks the
//! server for its full contents; the server only sends them to the client that has it open.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use deepcore_shared::content::ContentRegistry;

use crate::ui::{UiState, inventory::ClientInventory, item_label};

/// The contents of the machine the player has open, as last sent by the server.
#[derive(Resource, Default, Debug)]
pub struct OpenMachine {
    /// The machine entity this panel is bound to, if any.
    pub entity: Option<u64>,
    /// Its name, for the window title.
    pub name: String,
    /// Input slots.
    pub inputs: Vec<deepcore_shared::inventory::ItemStack>,
    /// Output slots.
    pub outputs: Vec<deepcore_shared::inventory::ItemStack>,
    /// Why it is idle.
    pub idle: String,
    /// Progress, 0..1.
    pub progress: f32,
    /// Whether it is running.
    pub running: bool,
    /// Reactor fields, when this is a reactor console.
    pub reactor: Option<ReactorReadout>,
}

/// Reactor values shown on the console.
#[derive(Debug, Clone, Copy)]
pub struct ReactorReadout {
    /// Core temperature.
    pub temperature: f32,
    /// Rod withdrawal, 0..1.
    pub rods: f32,
    /// Coolant flow, 0..1.
    pub coolant: f32,
    /// Power output in kW.
    pub kw: f32,
    /// Whether it is running.
    pub running: bool,
}

/// Draws the machine panel if one is open.
pub fn draw_machine_screen(
    mut contexts: EguiContexts,
    mut state: ResMut<UiState>,
    machine: Res<OpenMachine>,
    _inv: ResMut<ClientInventory>,
    registry: Res<ContentRegistry>,
) -> Result {
    // The panel is only drawn for the machine the player actually opened.
    let Some(open_entity) = state.machine_open else {
        return Ok(());
    };
    if machine.entity != Some(open_entity) {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let mut open = true;
    let title = if machine.name.is_empty() {
        "Machine".to_string()
    } else {
        machine.name.clone()
    };
    egui::Window::new(title)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(360.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(if machine.running { "Running" } else { "Idle" });
                if let Some(reactor) = machine.reactor {
                    ui.label(if reactor.running {
                        "(core critical)"
                    } else {
                        "(core shut down)"
                    });
                }
                if !machine.running && !machine.idle.is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(230, 170, 60), &machine.idle);
                }
            });
            ui.add(
                egui::ProgressBar::new(machine.progress)
                    .show_percentage()
                    .desired_width(320.0),
            );
            ui.separator();

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Inputs").strong());
                    for (i, stack) in machine.inputs.iter().enumerate() {
                        ui.label(format!("{i}: {}", item_label(&registry, stack.item)));
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Outputs").strong());
                    for (i, stack) in machine.outputs.iter().enumerate() {
                        ui.label(format!("{i}: {}", item_label(&registry, stack.item)));
                    }
                });
            });

            if let Some(reactor) = machine.reactor {
                ui.separator();
                ui.label(egui::RichText::new("Reactor").strong());
                ui.label(format!("Core temperature: {:.0} C", reactor.temperature));
                ui.label(format!("Power: {:.0} kW", reactor.kw));
                ui.add(
                    egui::ProgressBar::new(reactor.rods)
                        .text("control rods withdrawn")
                        .desired_width(320.0),
                );
                ui.add(
                    egui::ProgressBar::new(reactor.coolant)
                        .text("coolant flow")
                        .desired_width(320.0),
                );
                if reactor.temperature > 800.0 {
                    ui.colored_label(
                        egui::Color32::from_rgb(240, 80, 60),
                        "WARNING: core above the warning line",
                    );
                }
            }
        });
    if !open {
        // Closing the panel releases the machine so the server stops sending its contents.
        state.machine_open = None;
    }
    Ok(())
}
