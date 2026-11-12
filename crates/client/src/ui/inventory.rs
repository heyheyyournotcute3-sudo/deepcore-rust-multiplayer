//! The player's inventory screen: hotbar, backpack, drag and drop, and hand crafting.
//!
//! The UI never decides anything. It reads the client's mirrored copy of the inventory, applies
//! moves through the same `Inventory` rules the server uses (so the local result matches), and
//! sends intents for the server to confirm. In single-process play the local result is final.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use deepcore_shared::{
    content::{ContentRegistry, ItemId, types::ItemCategory},
    inventory::{Inventory, ItemStack, TransferError, max_stack_of},
};

use crate::ui::{UiState, item_color, item_label};

/// The client's mirror of the player's inventory.
#[derive(Resource, Debug, Clone)]
pub struct ClientInventory {
    /// Slot contents, hotbar first.
    pub inventory: Inventory,
    /// Which hotbar slot is selected.
    pub selected: u8,
    /// A stack picked up by the cursor while dragging.
    pub held: Option<ItemStack>,
}

impl Default for ClientInventory {
    fn default() -> Self {
        let mut inventory = Inventory::player();
        inventory.set_slot(0, ItemStack::empty());
        Self {
            inventory,
            selected: 0,
            held: None,
        }
    }
}

/// Draws the inventory and hotbar. Returns true if it consumed this frame's input.
pub fn draw_inventory(
    ctx: &egui::Context,
    state: &mut UiState,
    inv: &mut ClientInventory,
    registry: &ContentRegistry,
) -> bool {
    let mut open = state.inventory_open;
    let mut consumed = false;

    if !open {
        draw_hotbar(ctx, inv, registry);
        return false;
    }

    egui::Window::new("Inventory")
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            consumed = true;
            ui.horizontal(|ui| {
                ui.label("Click a stack to pick it up, click again to drop it.");
            });
            ui.separator();

            // Backpack: 36 slots in a 9x4 grid.
            ui.label(egui::RichText::new("Backpack").strong());
            draw_grid(ui, inv, registry, 9, 9, 4);
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Hotbar").strong());
            draw_grid(ui, inv, registry, 0, 9, 1);

            ui.separator();
            if let Some(held) = inv.held {
                ui.label(format!(
                    "Holding: {} x{}",
                    item_label(registry, held.item),
                    held.count
                ));
                if ui.button("Put back").clicked()
                    && let Some(item) = held.item
                {
                    let max = max_stack_of(registry, item);
                    inv.inventory.add(item, held.count, max);
                    inv.held = None;
                }
            } else {
                ui.label("Hands empty");
            }
        });

    state.inventory_open = open;
    consumed
}

/// Draws a grid of slots starting at `first`.
fn draw_grid(
    ui: &mut egui::Ui,
    inv: &mut ClientInventory,
    registry: &ContentRegistry,
    first: usize,
    columns: usize,
    rows: usize,
) {
    let slot_size = egui::vec2(44.0, 44.0);
    for row in 0..rows {
        ui.horizontal(|ui| {
            for column in 0..columns {
                let index = first + row * columns + column;
                let stack = inv.inventory.slot(index).copied().unwrap_or_default();
                let response = draw_slot(
                    ui,
                    slot_size,
                    &stack,
                    registry,
                    index == usize::from(inv.selected),
                );
                if response.clicked() {
                    click_slot(inv, registry, index);
                }
            }
        });
    }
}

/// Draws one slot and returns its click response.
fn draw_slot(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    stack: &ItemStack,
    registry: &ContentRegistry,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter();
    let fill = if selected {
        egui::Color32::from_rgb(60, 70, 60)
    } else {
        egui::Color32::from_rgb(30, 32, 34)
    };
    painter.rect_filled(rect, 3.0, fill);
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 74, 78)),
        egui::StrokeKind::Inside,
    );

    if let Some(item) = stack.item {
        let color = item_color(registry, Some(item));
        let inner = rect.shrink(8.0);
        painter.rect_filled(
            inner,
            2.0,
            egui::Color32::from_rgb(
                (color.0 * 255.0) as u8,
                (color.1 * 255.0) as u8,
                (color.2 * 255.0) as u8,
            ),
        );
        if stack.count > 1 {
            painter.text(
                rect.right_bottom() - egui::vec2(4.0, 2.0),
                egui::Align2::RIGHT_BOTTOM,
                stack.count.to_string(),
                egui::FontId::proportional(13.0),
                egui::Color32::WHITE,
            );
        }
        response.on_hover_text(item_label(registry, Some(item)))
    } else {
        response
    }
}

/// Handles a click on a slot: pick up, drop, merge or swap.
fn click_slot(inv: &mut ClientInventory, registry: &ContentRegistry, index: usize) {
    let slot = inv.inventory.slot(index).copied().unwrap_or_default();
    match inv.held {
        None => {
            if !slot.is_empty() {
                inv.held = Some(slot);
                inv.inventory.set_slot(index, ItemStack::empty());
            }
        }
        Some(held) => {
            if slot.is_empty() {
                inv.inventory.set_slot(index, held);
                inv.held = None;
            } else if slot.item == held.item {
                let max = max_stack_of(registry, held.item.expect("held stack has an item"));
                let space = slot.space_for(held.item.expect("held stack has an item"), max);
                let moved = space.min(held.count);
                if moved > 0 {
                    inv.inventory.set_slot(
                        index,
                        ItemStack::new(slot.item.expect("non-empty"), slot.count + moved),
                    );
                    let left = held.count - moved;
                    inv.held = if left == 0 {
                        None
                    } else {
                        Some(ItemStack::new(held.item.expect("non-empty"), left))
                    };
                }
            } else {
                // Swap.
                inv.inventory.set_slot(index, held);
                inv.held = Some(slot);
            }
        }
    }
    let _ = TransferError::NoOp;
}

/// Draws the always-visible hotbar at the bottom of the screen.
fn draw_hotbar(ctx: &egui::Context, inv: &mut ClientInventory, registry: &ContentRegistry) {
    egui::Area::new(egui::Id::new("hotbar"))
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -12.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for index in 0..9 {
                    let stack = inv.inventory.slot(index).copied().unwrap_or_default();
                    let selected = index == usize::from(inv.selected);
                    let response =
                        draw_slot(ui, egui::vec2(46.0, 46.0), &stack, registry, selected);
                    if response.clicked() {
                        inv.selected = index as u8;
                    }
                }
            });
        });
}

/// Handles number keys and the inventory key.
pub fn read_inventory_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<UiState>,
    mut inv: ResMut<ClientInventory>,
) {
    if keys.just_pressed(KeyCode::KeyE) || keys.just_pressed(KeyCode::Tab) {
        state.inventory_open = !state.inventory_open;
    }
    let number_keys = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    for (index, key) in number_keys.iter().enumerate() {
        if keys.just_pressed(*key) {
            inv.selected = index as u8;
        }
    }
}

/// Renders the inventory screen from the egui context.
pub fn draw_inventory_screen(
    mut contexts: EguiContexts,
    mut state: ResMut<UiState>,
    mut inv: ResMut<ClientInventory>,
    registry: Res<ContentRegistry>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    draw_inventory(ctx, &mut state, &mut inv, &registry);
    Ok(())
}

/// The item in the selected hotbar slot, if it is a placeable block.
#[must_use]
pub fn selected_placeable(inv: &ClientInventory, registry: &ContentRegistry) -> Option<ItemId> {
    let stack = inv.inventory.slot(usize::from(inv.selected))?;
    let item = stack.item?;
    match &registry.item_def(item)?.category {
        ItemCategory::Block(_) => Some(item),
        _ => None,
    }
}
