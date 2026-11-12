//! Shared UI helpers and the UI plugin: screen state, item colours and labels, and the
//! workbench crafting panel.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use deepcore_shared::{
    content::{ContentRegistry, ItemId, RecipeId, types::ItemCategory},
    crafting::try_hand_craft,
};

pub mod inventory;
pub mod machines;
pub mod tech;

/// Which UI screen is open.
#[derive(Resource, Debug, Default)]
pub struct UiState {
    /// Whether the inventory is open.
    pub inventory_open: bool,
    /// Whether the crafting panel is open.
    pub crafting_open: bool,
    /// Whether the tech tree is open.
    pub tech_open: bool,
    /// Whether the machine panel is open, and for which machine.
    pub machine_open: Option<u64>,
    /// A transient message shown at the bottom of the screen.
    pub toast: Option<(String, f32)>,
}

impl UiState {
    /// Shows a short-lived message.
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), 3.0));
    }

    /// Whether any screen is open, which should pause mouse capture.
    #[must_use]
    pub fn any_open(&self) -> bool {
        self.inventory_open || self.crafting_open || self.tech_open || self.machine_open.is_some()
    }
}

/// The client UI plugin.
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<UiState>()
            .init_resource::<inventory::ClientInventory>()
            .init_resource::<tech::ClientTech>()
            .init_resource::<machines::OpenMachine>()
            .add_systems(
                Update,
                (inventory::read_inventory_keys, read_screen_keys, tick_toast),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (
                    inventory::draw_inventory_screen,
                    draw_crafting_screen,
                    tech::draw_tech_screen,
                    machines::draw_machine_screen,
                    draw_toast,
                ),
            );
    }
}

/// Opens and closes the crafting and tech screens.
fn read_screen_keys(keys: Res<ButtonInput<KeyCode>>, mut state: ResMut<UiState>) {
    if keys.just_pressed(KeyCode::KeyC) {
        state.crafting_open = !state.crafting_open;
    }
    if keys.just_pressed(KeyCode::KeyT) {
        state.tech_open = !state.tech_open;
    }
    if keys.just_pressed(KeyCode::Escape) {
        state.crafting_open = false;
        state.tech_open = false;
        state.inventory_open = false;
        state.machine_open = None;
    }
}

/// Ages out the toast message.
fn tick_toast(time: Res<Time>, mut state: ResMut<UiState>) {
    if let Some((_, remaining)) = &mut state.toast {
        *remaining -= time.delta_secs();
        if *remaining <= 0.0 {
            state.toast = None;
        }
    }
}

/// Draws the toast, if any.
fn draw_toast(mut contexts: EguiContexts, state: Res<UiState>) -> Result {
    let Some((text, _)) = &state.toast else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    egui::Area::new(egui::Id::new("toast"))
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -90.0])
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(egui::RichText::new(text).size(15.0));
            });
        });
    Ok(())
}

/// The colour used to draw an item in the UI, taken from its block if it has one.
#[must_use]
pub fn item_color(registry: &ContentRegistry, item: Option<ItemId>) -> (f32, f32, f32) {
    let Some(item) = item else {
        return (0.2, 0.2, 0.2);
    };
    let Some(def) = registry.item_def(item) else {
        return (0.5, 0.5, 0.5);
    };
    match &def.category {
        ItemCategory::Block(block) | ItemCategory::Machine(block) => registry
            .block(block)
            .and_then(|b| registry.block_def(b))
            .map_or((0.5, 0.5, 0.5), |b| (b.color.0, b.color.1, b.color.2)),
        _ => {
            // Materials get a stable colour derived from their name, so they are distinguishable.
            let hash = def.id.bytes().fold(0u32, |acc, b| {
                acc.wrapping_mul(31).wrapping_add(u32::from(b))
            });
            let r = 0.35 + (hash & 0xFF) as f32 / 255.0 * 0.6;
            let g = 0.35 + ((hash >> 8) & 0xFF) as f32 / 255.0 * 0.6;
            let b = 0.35 + ((hash >> 16) & 0xFF) as f32 / 255.0 * 0.6;
            (r, g, b)
        }
    }
}

/// A human-readable item label, falling back to the raw ID.
#[must_use]
pub fn item_label(registry: &ContentRegistry, item: Option<ItemId>) -> String {
    match item {
        None => "Empty".to_string(),
        Some(id) => registry
            .item_def(id)
            .map_or_else(|| "Unknown".to_string(), |d| d.name.clone()),
    }
}

/// Draws the hand-crafting panel: every workbench recipe the player can afford.
fn draw_crafting_screen(
    mut contexts: EguiContexts,
    mut state: ResMut<UiState>,
    mut inv: ResMut<inventory::ClientInventory>,
    registry: Res<ContentRegistry>,
) -> Result {
    if !state.crafting_open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let mut open = true;
    let mut toast: Option<String> = None;

    egui::Window::new("Workbench")
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
        .default_width(430.0)
        .show(ctx, |ui| {
            ui.label("Hand crafting needs no machine. Machines need a powered structure.");
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    for (index, recipe) in registry.recipes.iter().enumerate() {
                        if recipe.machine != "hand" {
                            continue;
                        }
                        let id = RecipeId(index as u16);
                        let Some(resolved) = registry.resolve_recipe(id) else {
                            continue;
                        };
                        let affordable = inv.inventory.has_all(&resolved.inputs);
                        let header = format!(
                            "{}  ->  {}",
                            recipe
                                .inputs
                                .iter()
                                .map(|(i, c)| format!("{c} x {}", short_name(&registry, i)))
                                .collect::<Vec<_>>()
                                .join(", "),
                            recipe
                                .outputs
                                .iter()
                                .map(|(i, c)| format!("{c} x {}", short_name(&registry, i)))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(affordable, |ui| {
                                if ui.button(header).clicked() {
                                    match try_hand_craft(
                                        &registry,
                                        id,
                                        &mut inv.inventory,
                                        &|_| true,
                                        1,
                                    ) {
                                        Ok(made) => toast = Some(format!("Crafted {made}")),
                                        Err(reason) => toast = Some(reason.message()),
                                    }
                                }
                            });
                        });
                    }
                });
        });

    state.crafting_open = open;
    if let Some(message) = toast {
        state.toast(message);
    }
    Ok(())
}

/// The short item name used inside recipe lines.
fn short_name(registry: &ContentRegistry, id: &str) -> String {
    registry
        .item(id)
        .and_then(|i| registry.item_def(i))
        .map_or_else(|| id.to_string(), |d| d.name.clone())
}
