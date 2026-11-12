//! Inventory: item stacks, slots and transfer rules.
//!
//! An inventory is a flat list of slots. The player has 9 hotbar slots (0..9) followed by 36
//! backpack slots (9..45). Machines have their own slot layouts. All moves are validated here
//! so the server and the client agree on what a legal move is.

use serde::{Deserialize, Serialize};

use crate::content::{ContentRegistry, ItemId};

/// One slot's contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ItemStack {
    /// The item, or `None` for an empty slot.
    pub item: Option<ItemId>,
    /// How many are in the stack.
    pub count: u32,
}

impl ItemStack {
    /// An empty stack.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            item: None,
            count: 0,
        }
    }

    /// A stack of `count` of `item`.
    #[must_use]
    pub fn new(item: ItemId, count: u32) -> Self {
        Self {
            item: Some(item),
            count,
        }
    }

    /// Whether this slot holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.item.is_none() || self.count == 0
    }

    /// How many more of `item` this stack can accept, given its max stack size.
    #[must_use]
    pub fn space_for(&self, item: ItemId, max_stack: u32) -> u32 {
        match self.item {
            Some(existing) if existing == item => max_stack.saturating_sub(self.count),
            None => max_stack,
            _ => 0,
        }
    }
}

/// A fixed-size inventory of slots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    slots: Vec<ItemStack>,
    /// Number of hotbar slots at the front, which are never dropped on death.
    hotbar: usize,
}

/// Errors a transfer can produce, reported to the UI as the reason a move failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferError {
    /// The source slot is empty.
    EmptySource,
    /// The slot index is out of range.
    BadSlot,
    /// The destination cannot accept this item.
    NoRoom,
    /// The move would not change anything.
    NoOp,
}

impl Inventory {
    /// Creates an inventory with `hotbar` hotbar slots and `backpack` backpack slots.
    #[must_use]
    pub fn new(hotbar: usize, backpack: usize) -> Self {
        Self {
            slots: vec![ItemStack::empty(); hotbar + backpack],
            hotbar,
        }
    }

    /// A player inventory: 9 hotbar + 36 backpack slots.
    #[must_use]
    pub fn player() -> Self {
        Self::new(9, 36)
    }

    /// Number of slots.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether the inventory has no slots.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Number of hotbar slots.
    #[must_use]
    pub fn hotbar_len(&self) -> usize {
        self.hotbar
    }

    /// Reads a slot.
    #[must_use]
    pub fn slot(&self, index: usize) -> Option<&ItemStack> {
        self.slots.get(index)
    }

    /// Reads all slots.
    #[must_use]
    pub fn slots(&self) -> &[ItemStack] {
        &self.slots
    }

    /// Writes a slot directly. Used by the server when applying validated changes.
    pub fn set_slot(&mut self, index: usize, stack: ItemStack) {
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = if stack.is_empty() {
                ItemStack::empty()
            } else {
                stack
            };
        }
    }

    /// Total number of a given item across all slots.
    #[must_use]
    pub fn count_of(&self, item: ItemId) -> u32 {
        self.slots
            .iter()
            .filter(|s| s.item == Some(item))
            .map(|s| s.count)
            .sum()
    }

    /// Whether the inventory holds at least `count` of `item`.
    #[must_use]
    pub fn has(&self, item: ItemId, count: u32) -> bool {
        self.count_of(item) >= count
    }

    /// Adds items, filling existing stacks first and then empty slots.
    ///
    /// Returns how many items could not be added.
    pub fn add(&mut self, item: ItemId, count: u32, max_stack: u32) -> u32 {
        let mut remaining = count;
        // First pass: top up existing stacks.
        for slot in &mut self.slots {
            if remaining == 0 {
                break;
            }
            if slot.item == Some(item) && slot.count < max_stack {
                let space = max_stack - slot.count;
                let moved = space.min(remaining);
                slot.count += moved;
                remaining -= moved;
            }
        }
        // Second pass: use empty slots.
        for slot in &mut self.slots {
            if remaining == 0 {
                break;
            }
            if slot.is_empty() {
                let moved = max_stack.min(remaining);
                *slot = ItemStack::new(item, moved);
                remaining -= moved;
            }
        }
        remaining
    }

    /// Removes up to `count` of an item. Returns how many were actually removed.
    pub fn remove(&mut self, item: ItemId, count: u32) -> u32 {
        let mut remaining = count;
        for slot in &mut self.slots {
            if remaining == 0 {
                break;
            }
            if slot.item == Some(item) {
                let taken = slot.count.min(remaining);
                slot.count -= taken;
                remaining -= taken;
                if slot.count == 0 {
                    *slot = ItemStack::empty();
                }
            }
        }
        count - remaining
    }

    /// Whether every listed input is present.
    #[must_use]
    pub fn has_all(&self, inputs: &[(ItemId, u32)]) -> bool {
        inputs.iter().all(|(item, count)| self.has(*item, *count))
    }

    /// Removes every listed input, or nothing if any is missing.
    pub fn remove_all(&mut self, inputs: &[(ItemId, u32)]) -> bool {
        if !self.has_all(inputs) {
            return false;
        }
        for (item, count) in inputs {
            self.remove(*item, *count);
        }
        true
    }

    /// Whether the inventory can accept all of these outputs.
    #[must_use]
    pub fn can_accept_all(&self, outputs: &[(ItemId, u32)], registry: &ContentRegistry) -> bool {
        // Simulate on a copy so partial fills are accounted for.
        let mut copy = self.clone();
        for (item, count) in outputs {
            let max = max_stack_of(registry, *item);
            if copy.add(*item, *count, max) > 0 {
                return false;
            }
        }
        true
    }

    /// Moves `count` items from one slot to another, merging stacks where possible.
    ///
    /// This is the single validated move used by drag-and-drop, shift-click and quick-move.
    pub fn transfer(
        &mut self,
        from: usize,
        to: usize,
        count: u32,
        registry: &ContentRegistry,
    ) -> Result<u32, TransferError> {
        if from >= self.slots.len() || to >= self.slots.len() {
            return Err(TransferError::BadSlot);
        }
        if from == to {
            return Err(TransferError::NoOp);
        }
        let source = self.slots[from];
        if source.is_empty() {
            return Err(TransferError::EmptySource);
        }
        let item = source.item.expect("non-empty stack has an item");
        let max_stack = max_stack_of(registry, item);
        let dest = self.slots[to];
        let space = dest.space_for(item, max_stack);
        if space == 0 {
            return Err(TransferError::NoRoom);
        }
        let moved = space.min(count).min(source.count);
        if moved == 0 {
            return Err(TransferError::NoOp);
        }

        // Apply.
        self.slots[to] = match dest.item {
            Some(_) => ItemStack::new(item, dest.count + moved),
            None => ItemStack::new(item, moved),
        };
        let left = source.count - moved;
        self.slots[from] = if left == 0 {
            ItemStack::empty()
        } else {
            ItemStack::new(item, left)
        };
        Ok(moved)
    }

    /// Splits a stack in half into another (empty) slot, the classic right-click behaviour.
    pub fn split(&mut self, from: usize, to: usize) -> Result<u32, TransferError> {
        if from >= self.slots.len() || to >= self.slots.len() {
            return Err(TransferError::BadSlot);
        }
        let source = self.slots[from];
        if source.is_empty() {
            return Err(TransferError::EmptySource);
        }
        if !self.slots[to].is_empty() {
            return Err(TransferError::NoRoom);
        }
        let item = source.item.expect("non-empty stack has an item");
        let half = source.count.div_ceil(2);
        self.slots[to] = ItemStack::new(item, half);
        let left = source.count - half;
        self.slots[from] = if left == 0 {
            ItemStack::empty()
        } else {
            ItemStack::new(item, left)
        };
        Ok(half)
    }

    /// Shift-click behaviour: move a stack between the hotbar and the backpack.
    pub fn quick_move(
        &mut self,
        from: usize,
        registry: &ContentRegistry,
    ) -> Result<u32, TransferError> {
        if from >= self.slots.len() {
            return Err(TransferError::BadSlot);
        }
        let source = self.slots[from];
        if source.is_empty() {
            return Err(TransferError::EmptySource);
        }
        let (start, end) = if from < self.hotbar {
            (self.hotbar, self.slots.len())
        } else {
            (0, self.hotbar)
        };
        let item = source.item.expect("non-empty stack has an item");
        let max_stack = max_stack_of(registry, item);
        let mut moved = 0;
        // Merge into existing stacks first.
        for i in start..end {
            if self.slots[i].item == Some(item) && self.slots[i].count < max_stack {
                let space = max_stack - self.slots[i].count;
                let take = space.min(source.count - moved);
                self.slots[i].count += take;
                moved += take;
                if moved >= source.count {
                    break;
                }
            }
        }
        // Then use empty slots.
        if moved < source.count {
            for i in start..end {
                if self.slots[i].is_empty() {
                    let take = max_stack.min(source.count - moved);
                    self.slots[i] = ItemStack::new(item, take);
                    moved += take;
                    if moved >= source.count {
                        break;
                    }
                }
            }
        }
        if moved == 0 {
            return Err(TransferError::NoRoom);
        }
        let left = source.count - moved;
        self.slots[from] = if left == 0 {
            ItemStack::empty()
        } else {
            ItemStack::new(item, left)
        };
        Ok(moved)
    }

    /// Drops everything outside the hotbar, returning it as a list of stacks.
    ///
    /// Used by the death crate: the backpack is recoverable, the hotbar is not.
    pub fn take_backpack(&mut self) -> Vec<ItemStack> {
        let mut dropped = Vec::new();
        for i in self.hotbar..self.slots.len() {
            if !self.slots[i].is_empty() {
                dropped.push(self.slots[i]);
                self.slots[i] = ItemStack::empty();
            }
        }
        dropped
    }

    /// Puts stacks back into the inventory, returning whatever did not fit.
    pub fn restore(&mut self, stacks: &[ItemStack], registry: &ContentRegistry) -> Vec<ItemStack> {
        let mut leftover = Vec::new();
        for stack in stacks {
            let Some(item) = stack.item else { continue };
            let max = max_stack_of(registry, item);
            let remaining = self.add(item, stack.count, max);
            if remaining > 0 {
                leftover.push(ItemStack::new(item, remaining));
            }
        }
        leftover
    }
}

/// The max stack size of an item, defaulting to 1 if the item is unknown.
#[must_use]
pub fn max_stack_of(registry: &ContentRegistry, item: ItemId) -> u32 {
    registry.item_def(item).map_or(1, |d| d.stack)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn registry() -> ContentRegistry {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        ContentRegistry::load_from_dir(&dir).unwrap()
    }

    fn item(reg: &ContentRegistry, name: &str) -> ItemId {
        reg.item(name)
            .unwrap_or_else(|| panic!("missing item {name}"))
    }

    #[test]
    fn adding_fills_stacks_then_slots() {
        let reg = registry();
        let iron = item(&reg, "iron_ingot");
        let mut inv = Inventory::player();
        let leftover = inv.add(iron, 100, 64);
        assert_eq!(leftover, 0);
        assert_eq!(inv.count_of(iron), 100);
        assert_eq!(inv.slot(0).unwrap().count, 64);
        assert_eq!(inv.slot(1).unwrap().count, 36);
    }

    #[test]
    fn adding_beyond_capacity_reports_leftover() {
        let reg = registry();
        let iron = item(&reg, "iron_ingot");
        let mut inv = Inventory::new(0, 1);
        let leftover = inv.add(iron, 100, 64);
        assert_eq!(leftover, 36);
        assert_eq!(inv.count_of(iron), 64);
    }

    #[test]
    fn removing_only_takes_what_exists() {
        let reg = registry();
        let iron = item(&reg, "iron_ingot");
        let mut inv = Inventory::player();
        inv.add(iron, 10, 64);
        assert_eq!(inv.remove(iron, 4), 4);
        assert_eq!(inv.count_of(iron), 6);
        assert_eq!(inv.remove(iron, 100), 6);
        assert_eq!(inv.count_of(iron), 0);
    }

    #[test]
    fn transfer_merges_into_matching_stacks() {
        let reg = registry();
        let iron = item(&reg, "iron_ingot");
        let mut inv = Inventory::player();
        inv.set_slot(0, ItemStack::new(iron, 40));
        inv.set_slot(1, ItemStack::new(iron, 10));
        // Source has 40, destination has 10 and a max stack of 64: only 54 fit, so 40 move.
        let moved = inv.transfer(0, 1, 64, &reg).unwrap();
        assert_eq!(
            moved, 40,
            "destination can only take 54, source only has 40"
        );
        assert_eq!(inv.slot(1).unwrap().count, 50);
        assert_eq!(inv.slot(0).unwrap().count, 0);
    }

    #[test]
    fn transfer_refuses_when_destination_is_full() {
        let reg = registry();
        let iron = item(&reg, "iron_ingot");
        let copper = item(&reg, "copper_ingot");
        let mut inv = Inventory::player();
        inv.set_slot(0, ItemStack::new(iron, 10));
        inv.set_slot(1, ItemStack::new(copper, 10));
        assert_eq!(inv.transfer(0, 1, 10, &reg), Err(TransferError::NoRoom));
    }

    #[test]
    fn split_halves_a_stack() {
        let reg = registry();
        let iron = item(&reg, "iron_ingot");
        let mut inv = Inventory::player();
        inv.set_slot(0, ItemStack::new(iron, 7));
        let half = inv.split(0, 1).unwrap();
        assert_eq!(half, 4);
        assert_eq!(inv.slot(0).unwrap().count, 3);
        assert_eq!(inv.slot(1).unwrap().count, 4);
        let _ = &reg;
    }

    #[test]
    fn quick_move_moves_between_hotbar_and_backpack() {
        let reg = registry();
        let iron = item(&reg, "iron_ingot");
        let mut inv = Inventory::player();
        inv.set_slot(0, ItemStack::new(iron, 20));
        let moved = inv.quick_move(0, &reg).unwrap();
        assert_eq!(moved, 20);
        assert!(inv.slot(0).unwrap().is_empty());
        assert_eq!(inv.slot(9).unwrap().count, 20);
        // And back again.
        inv.quick_move(9, &reg).unwrap();
        assert_eq!(inv.slot(0).unwrap().count, 20);
    }

    #[test]
    fn death_drops_the_backpack_but_not_the_hotbar() {
        let reg = registry();
        let iron = item(&reg, "iron_ingot");
        let mut inv = Inventory::player();
        inv.set_slot(0, ItemStack::new(iron, 5));
        inv.set_slot(10, ItemStack::new(iron, 30));
        let dropped = inv.take_backpack();
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].count, 30);
        assert_eq!(inv.count_of(iron), 5, "hotbar survives");
        // Restoring puts it back.
        let leftover = inv.restore(&dropped, &reg);
        assert!(leftover.is_empty());
        assert_eq!(inv.count_of(iron), 35);
    }

    #[test]
    fn remove_all_is_all_or_nothing() {
        let reg = registry();
        let iron = item(&reg, "iron_ingot");
        let coal = item(&reg, "coal");
        let mut inv = Inventory::player();
        inv.set_slot(0, ItemStack::new(iron, 5));
        inv.set_slot(1, ItemStack::new(coal, 1));
        assert!(!inv.remove_all(&[(iron, 5), (coal, 5)]));
        assert_eq!(inv.count_of(iron), 5, "nothing may be consumed on failure");
        assert!(inv.remove_all(&[(iron, 5), (coal, 1)]));
        assert_eq!(inv.count_of(iron), 0);
    }

    #[test]
    fn can_accept_all_accounts_for_partial_fills() {
        let reg = registry();
        let iron = item(&reg, "iron_ingot");
        let mut inv = Inventory::new(0, 1);
        assert!(inv.can_accept_all(&[(iron, 64)], &reg));
        assert!(!inv.can_accept_all(&[(iron, 65)], &reg));
        inv.add(iron, 32, 64);
        assert!(inv.can_accept_all(&[(iron, 32)], &reg));
        assert!(!inv.can_accept_all(&[(iron, 33)], &reg));
    }

    proptest! {
        /// Whatever sequence of adds and removes happens, the count never goes negative
        /// and never exceeds what was added.
        #[test]
        fn add_remove_conserves_items(ops in prop::collection::vec((any::<bool>(), 0u32..200), 0..40)) {
            let reg = registry();
            let iron = item(&reg, "iron_ingot");
            let mut inv = Inventory::player();
            let mut expected: i64 = 0;
            for (is_add, amount) in ops {
                if is_add {
                    let leftover = inv.add(iron, amount, 64);
                    expected += i64::from(amount - leftover);
                } else {
                    let removed = inv.remove(iron, amount);
                    expected -= i64::from(removed);
                }
                prop_assert!(expected >= 0);
                prop_assert_eq!(i64::from(inv.count_of(iron)), expected);
            }
        }

        /// Any two slots in a player inventory can be transferred between without
        /// losing or duplicating items.
        #[test]
        fn transfer_conserves_items(from in 0usize..45, to in 0usize..45, count in 1u32..64) {
            let reg = registry();
            let iron = item(&reg, "iron_ingot");
            let mut inv = Inventory::player();
            inv.set_slot(from, ItemStack::new(iron, 32));
            let before = inv.count_of(iron);
            if from != to {
                let _ = inv.transfer(from, to, count, &reg);
                prop_assert_eq!(inv.count_of(iron), before);
            }
        }
    }
}
