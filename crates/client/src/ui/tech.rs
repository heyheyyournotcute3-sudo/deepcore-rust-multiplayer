//! The tech tree screen: a node graph with locked, available and researched states.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use deepcore_shared::content::ContentRegistry;

use crate::ui::UiState;

/// The team's research state as the client knows it.
#[derive(Resource, Default, Debug)]
pub struct ClientTech {
    /// Researched node IDs (indices into the content registry's tech list).
    pub researched: std::collections::BTreeSet<u16>,
}

/// Draws the tech tree.
pub fn draw_tech_screen(
    mut contexts: EguiContexts,
    mut state: ResMut<UiState>,
    tech: Res<ClientTech>,
    registry: Res<ContentRegistry>,
) -> Result {
    if !state.tech_open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let mut open = true;

    egui::Window::new("Tech Tree")
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(720.0)
        .default_height(460.0)
        .show(ctx, |ui| {
            ui.label("Research is shared by the whole team. Deliver items at the Research Desk.");
            ui.separator();
            egui::ScrollArea::both().show(ui, |ui| {
                for era in 0..=5u8 {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("Era {era}"))
                                .strong()
                                .size(16.0),
                        );
                        for (index, node) in registry.techs.iter().enumerate() {
                            if node.era != era {
                                continue;
                            }
                            let researched = tech.researched.contains(&(index as u16));
                            let available = node.parents.iter().all(|p| {
                                registry
                                    .tech(p)
                                    .is_some_and(|t| tech.researched.contains(&t.0))
                            });
                            let (fill, text_color) = if researched {
                                (egui::Color32::from_rgb(46, 92, 54), egui::Color32::WHITE)
                            } else if available {
                                (egui::Color32::from_rgb(70, 74, 40), egui::Color32::WHITE)
                            } else {
                                (egui::Color32::from_rgb(40, 40, 44), egui::Color32::GRAY)
                            };
                            let label = egui::RichText::new(&node.name).color(text_color);
                            let button = egui::Button::new(label).fill(fill);
                            ui.add(button).on_hover_text(format!(
                                "{}\nCost: {}",
                                node.description,
                                node.cost
                                    .iter()
                                    .map(|(i, c)| format!("{c} x {i}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                        }
                    });
                }
            });
        });
    state.tech_open = open;
    Ok(())
}
