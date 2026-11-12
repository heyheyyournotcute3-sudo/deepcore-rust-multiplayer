//! Crafting and machine processing.
//!
//! A machine is a small state machine: it holds a set of input and output stacks, an optional
//! fuel slot and an optional upgrade module pair. Each tick it looks for a recipe it can run,
//! advances a progress bar and, when finished, moves items from input to output.

use crate::content::types::EnergyKind;
use crate::content::{ContentRegistry, ItemId, MachineDef, MachineId, RecipeId, ResolvedRecipe};
use crate::inventory::{Inventory, ItemStack, max_stack_of};

/// Why a machine is not running. Shown verbatim in the machine UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdleReason {
    /// No recipe matches the current inputs.
    NoRecipe,
    /// A recipe matched but its output slots are full.
    OutputFull,
    /// The machine needs fuel and the fuel slot is empty.
    NoFuel,
    /// The machine needs power and the network is not supplying it.
    NoPower,
    /// A required catalyst (acid, flux, mould) is missing.
    MissingCatalyst(String),
    /// The recipe's tech node is not researched.
    Locked,
    /// The machine is working.
    Running,
    /// The machine is a multi-block that is not assembled correctly.
    Unformed(String),
}

impl IdleReason {
    /// A short human-readable message for the machine UI.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NoRecipe => "No matching recipe".to_string(),
            Self::OutputFull => "Output full".to_string(),
            Self::NoFuel => "No fuel".to_string(),
            Self::NoPower => "No power".to_string(),
            Self::MissingCatalyst(item) => format!("Missing {item}"),
            Self::Locked => "Recipe not researched".to_string(),
            Self::Running => "Running".to_string(),
            Self::Unformed(what) => format!("Not formed: {what}"),
        }
    }
}

/// The runtime state of one machine block entity.
#[derive(Debug, Clone)]
pub struct MachineState {
    /// Which machine this is.
    pub kind: MachineId,
    /// Input slots.
    pub inputs: Inventory,
    /// Output slots.
    pub outputs: Inventory,
    /// Fuel slot contents (used by fuel machines).
    pub fuel: ItemStack,
    /// Seconds of burn time remaining in the current fuel unit.
    pub fuel_remaining: f32,
    /// The recipe currently running, if any.
    pub current: Option<RecipeId>,
    /// Progress in seconds toward the current recipe.
    pub progress: f32,
    /// Why the machine is idle, or [`IdleReason::Running`].
    pub idle: IdleReason,
    /// Speed modules installed (0..=2).
    pub speed_modules: u8,
    /// Efficiency modules installed (0..=2).
    pub efficiency_modules: u8,
    /// How many linked blocks the cascade has (centrifuges only).
    pub cascade_len: u8,
}

impl MachineState {
    /// Creates an empty machine of the given kind.
    #[must_use]
    pub fn new(def: &MachineDef) -> Self {
        Self {
            kind: MachineId(0),
            inputs: Inventory::new(usize::from(def.input_slots), 0),
            outputs: Inventory::new(0, usize::from(def.output_slots)),
            fuel: ItemStack::empty(),
            fuel_remaining: 0.0,
            current: None,
            progress: 0.0,
            idle: IdleReason::NoRecipe,
            speed_modules: 0,
            efficiency_modules: 0,
            cascade_len: 0,
        }
    }

    /// Creates a machine state bound to a specific machine kind.
    #[must_use]
    pub fn for_machine(id: MachineId, def: &MachineDef) -> Self {
        let mut state = Self::new(def);
        state.kind = id;
        state
    }

    /// Speed multiplier from installed modules.
    #[must_use]
    pub fn speed_multiplier(&self) -> f32 {
        1.0 + 0.5 * f32::from(self.speed_modules)
    }

    /// Power multiplier from installed modules.
    #[must_use]
    pub fn power_multiplier(&self) -> f32 {
        let speed_cost = 1.0 + 0.8 * f32::from(self.speed_modules);
        let efficiency = (1.0 - 0.3 * f32::from(self.efficiency_modules)).max(0.1);
        speed_cost * efficiency
    }

    /// Whether the machine is currently producing.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.idle == IdleReason::Running
    }

    /// Progress from 0 to 1 for the UI.
    #[must_use]
    pub fn progress_fraction(&self, registry: &ContentRegistry) -> f32 {
        let Some(recipe) = self.current else {
            return 0.0;
        };
        let Some(def) = registry.recipe_def(recipe) else {
            return 0.0;
        };
        (self.progress / def.time.max(0.001)).clamp(0.0, 1.0)
    }
}

/// Finds a recipe the machine can run with its current inputs.
///
/// Returns the recipe plus the inputs it would consume. Chooses the first match in content
/// order, which is deterministic.
#[must_use]
pub fn find_recipe(
    registry: &ContentRegistry,
    machine: MachineId,
    inputs: &Inventory,
    researched: &dyn Fn(&str) -> bool,
) -> Option<(RecipeId, ResolvedRecipe)> {
    let machine_name = registry.machine_def(machine)?.id.as_str();
    for (index, def) in registry.recipes.iter().enumerate() {
        if def.machine != machine_name {
            continue;
        }
        if let Some(tech) = &def.tech
            && !researched(tech)
        {
            continue;
        }
        let id = RecipeId(index as u16);
        let Some(resolved) = registry.resolve_recipe(id) else {
            continue;
        };
        if resolved.inputs.is_empty() {
            continue;
        }
        if inputs.has_all(&resolved.inputs) {
            return Some((id, resolved));
        }
    }
    None
}

/// Ticks one machine by `dt` seconds.
///
/// `power_available` is the fraction of requested power the network can supply, 0..1.
/// Returns true if the machine's state changed in a way the client needs to know about
/// (started, finished, or ran out of something).
pub fn tick_machine(
    state: &mut MachineState,
    def: &MachineDef,
    registry: &ContentRegistry,
    researched: &dyn Fn(&str) -> bool,
    power_available: f32,
    dt: f32,
) -> bool {
    let mut changed = false;

    // Multi-block machines that need neighbours report their own idle reasons upstream.
    if let Some(cascade) = def.cascade
        && state.cascade_len < cascade.min
    {
        state.idle = IdleReason::Unformed(format!(
            "needs {} linked centrifuges, has {}",
            cascade.min, state.cascade_len
        ));
        state.current = None;
        state.progress = 0.0;
        return false;
    }

    // Fuel machines burn one unit at a time.
    if matches!(def.energy, EnergyKind::Fuel { .. }) && state.fuel_remaining <= 0.0 {
        match take_fuel(&mut state.fuel, registry) {
            Some(seconds) => {
                state.fuel_remaining = seconds;
                changed = true;
            }
            None => {
                if state.current.is_some() || state.inputs.slots().iter().any(|s| !s.is_empty()) {
                    state.idle = IdleReason::NoFuel;
                    state.current = None;
                    state.progress = 0.0;
                    return true;
                }
                state.idle = IdleReason::NoFuel;
                return false;
            }
        }
    }

    // Power gate: electric and shaft machines stall without supply.
    let needs_power = matches!(
        def.energy,
        EnergyKind::Electric { .. } | EnergyKind::Shaft { .. } | EnergyKind::Steam { .. }
    );
    if needs_power && power_available <= 0.001 {
        if state.current.is_some() {
            changed = true;
        }
        state.idle = IdleReason::NoPower;
        state.current = None;
        state.progress = 0.0;
        return changed;
    }

    // Continue the current recipe if there is one.
    if let Some(recipe_id) = state.current {
        let Some(recipe) = registry.resolve_recipe(recipe_id) else {
            state.current = None;
            state.progress = 0.0;
            return true;
        };
        // Verify inputs are still present (they can be pulled out by a player).
        if !state.inputs.has_all(&recipe.inputs) {
            state.current = None;
            state.progress = 0.0;
            return true;
        }
        if !state.outputs.can_accept_all(&recipe.outputs, registry) {
            state.idle = IdleReason::OutputFull;
            return true;
        }

        state.idle = IdleReason::Running;
        state.progress += dt * state.speed_multiplier() * power_available;

        if state.progress >= recipe.time {
            // Finish: consume inputs and catalysts, produce outputs.
            state.inputs.remove_all(&recipe.inputs);
            for (catalyst, count) in &recipe.catalysts {
                state.inputs.remove(*catalyst, *count);
            }
            for (item, count) in &recipe.outputs {
                let max = max_stack_of(registry, *item);
                let leftover = state.outputs.add(*item, *count, max);
                debug_assert_eq!(leftover, 0, "output space was checked before finishing");
            }
            state.progress = 0.0;
            state.current = None;
            changed = true;
        }
        if let EnergyKind::Fuel { .. } = def.energy {
            state.fuel_remaining = (state.fuel_remaining - dt).max(0.0);
        }
        return changed;
    }

    // Nothing running: look for something to start.
    match find_recipe(registry, state.kind, &state.inputs, researched) {
        Some((id, recipe)) => {
            if !state.outputs.can_accept_all(&recipe.outputs, registry) {
                state.idle = IdleReason::OutputFull;
                return false;
            }
            for (catalyst, count) in &recipe.catalysts {
                if !state.inputs.has(*catalyst, *count) {
                    state.idle =
                        IdleReason::MissingCatalyst(registry.item_name(*catalyst).to_string());
                    return false;
                }
            }
            state.current = Some(id);
            state.progress = 0.0;
            state.idle = IdleReason::Running;
            true
        }
        None => {
            // Nothing matches: either there are no inputs, or the recipe is locked or the
            // inputs do not form a recipe for this machine. Both read the same to the player.
            state.idle = IdleReason::NoRecipe;
            false
        }
    }
}

/// Consumes one unit of fuel and returns how many seconds it burns for.
fn take_fuel(slot: &mut ItemStack, registry: &ContentRegistry) -> Option<f32> {
    let item = slot.item?;
    if slot.count == 0 {
        return None;
    }
    let def = registry.item_def(item)?;
    let seconds = match def.category {
        crate::content::types::ItemCategory::Fuel { burn_seconds } => burn_seconds,
        _ => return None,
    };
    slot.count -= 1;
    if slot.count == 0 {
        *slot = ItemStack::empty();
    }
    Some(seconds)
}

/// Hand crafting: checks that a workbench recipe can be made from an inventory and makes it.
pub fn try_hand_craft(
    registry: &ContentRegistry,
    recipe: RecipeId,
    inventory: &mut Inventory,
    researched: &dyn Fn(&str) -> bool,
    count: u32,
) -> Result<u32, IdleReason> {
    let Some(def) = registry.recipe_def(recipe) else {
        return Err(IdleReason::NoRecipe);
    };
    if def.machine != "hand" {
        return Err(IdleReason::NoRecipe);
    }
    if let Some(tech) = &def.tech
        && !researched(tech)
    {
        return Err(IdleReason::Locked);
    }
    let Some(resolved) = registry.resolve_recipe(recipe) else {
        return Err(IdleReason::NoRecipe);
    };

    // Craft as many as possible, up to `count`.
    let mut made = 0;
    for _ in 0..count {
        if !inventory.has_all(&resolved.inputs) {
            break;
        }
        if !inventory.can_accept_all(&resolved.outputs, registry) {
            if made == 0 {
                return Err(IdleReason::OutputFull);
            }
            break;
        }
        inventory.remove_all(&resolved.inputs);
        for (item, amount) in &resolved.outputs {
            let max = max_stack_of(registry, *item);
            inventory.add(*item, *amount, max);
        }
        made += 1;
    }
    if made == 0 {
        return Err(IdleReason::NoRecipe);
    }
    Ok(made)
}

/// Lists every recipe the given machine can run, for the machine UI's recipe picker.
#[must_use]
pub fn recipes_for(registry: &ContentRegistry, machine: MachineId) -> Vec<RecipeId> {
    registry.recipes_for_machine(machine)
}

/// Item counts needed to build a machine.
#[must_use]
pub fn build_cost(registry: &ContentRegistry, machine: MachineId) -> Vec<(ItemId, u32)> {
    let Some(def) = registry.machine_def(machine) else {
        return Vec::new();
    };
    def.build_cost
        .iter()
        .filter_map(|(name, count)| registry.item(name).map(|id| (id, *count)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::ContentRegistry;

    fn registry() -> ContentRegistry {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        ContentRegistry::load_from_dir(&dir).unwrap()
    }

    fn everything(_: &str) -> bool {
        true
    }

    fn machine<'a>(reg: &'a ContentRegistry, name: &str) -> (MachineId, &'a MachineDef) {
        let id = reg.machine(name).expect("machine exists");
        (id, reg.machine_def(id).unwrap())
    }

    #[test]
    fn furnace_smelts_iron_into_an_ingot() {
        let reg = registry();
        let (id, def) = machine(&reg, "stone_furnace");
        let mut state = MachineState::for_machine(id, def);
        let raw = reg.item("raw_iron").unwrap();
        let ingot = reg.item("iron_ingot").unwrap();
        let coal = reg.item("coal").unwrap();
        state.inputs.add(raw, 1, 64);
        state.fuel = ItemStack::new(coal, 1);

        // Run for long enough to finish a 6 second recipe.
        let mut finished = false;
        for _ in 0..400 {
            if tick_machine(&mut state, def, &reg, &everything, 1.0, 0.05)
                && state.outputs.count_of(ingot) > 0
            {
                finished = true;
                break;
            }
        }
        assert!(finished, "furnace never produced an ingot");
        assert_eq!(state.outputs.count_of(ingot), 1);
        assert_eq!(state.inputs.count_of(raw), 0, "input must be consumed");
    }

    #[test]
    fn furnace_without_fuel_reports_no_fuel() {
        let reg = registry();
        let (id, def) = machine(&reg, "stone_furnace");
        let mut state = MachineState::for_machine(id, def);
        state.inputs.add(reg.item("raw_iron").unwrap(), 1, 64);
        tick_machine(&mut state, def, &reg, &everything, 1.0, 0.05);
        assert_eq!(state.idle, IdleReason::NoFuel);
    }

    #[test]
    fn electric_machine_without_power_stalls() {
        let reg = registry();
        let (id, def) = machine(&reg, "arc_furnace");
        let mut state = MachineState::for_machine(id, def);
        state.inputs.add(reg.item("quartz").unwrap(), 3, 64);
        state.inputs.add(reg.item("coal").unwrap(), 1, 64);
        tick_machine(&mut state, def, &reg, &everything, 0.0, 0.05);
        assert_eq!(state.idle, IdleReason::NoPower);
        assert!(state.current.is_none());
    }

    #[test]
    fn locked_recipes_are_not_started() {
        let reg = registry();
        let (id, def) = machine(&reg, "crusher");
        let mut state = MachineState::for_machine(id, def);
        state.inputs.add(reg.item("raw_iron").unwrap(), 1, 64);
        let nothing = |_: &str| false;
        tick_machine(&mut state, def, &reg, &nothing, 1.0, 0.05);
        assert!(state.current.is_none(), "locked recipe must not start");
    }

    #[test]
    fn full_output_stops_the_machine() {
        let reg = registry();
        let (id, def) = machine(&reg, "stone_furnace");
        let mut state = MachineState::for_machine(id, def);
        let ingot = reg.item("iron_ingot").unwrap();
        // Fill the output slots completely.
        for i in 0..usize::from(def.output_slots) {
            state.outputs.set_slot(i, ItemStack::new(ingot, 64));
        }
        state.inputs.add(reg.item("raw_iron").unwrap(), 1, 64);
        state.fuel = ItemStack::new(reg.item("coal").unwrap(), 1);
        tick_machine(&mut state, def, &reg, &everything, 1.0, 0.05);
        assert_eq!(state.idle, IdleReason::OutputFull);
    }

    #[test]
    fn modules_change_speed_and_power() {
        let reg = registry();
        let (id, def) = machine(&reg, "arc_furnace");
        let mut state = MachineState::for_machine(id, def);
        assert!((state.speed_multiplier() - 1.0).abs() < 1e-6);
        state.speed_modules = 1;
        assert!((state.speed_multiplier() - 1.5).abs() < 1e-6);
        assert!(state.power_multiplier() > 1.0, "speed costs power");
        state.efficiency_modules = 1;
        assert!(state.power_multiplier() < 1.8, "efficiency reduces power");
    }

    #[test]
    fn centrifuge_needs_a_cascade() {
        let reg = registry();
        let (id, def) = machine(&reg, "centrifuge");
        let mut state = MachineState::for_machine(id, def);
        state.cascade_len = 1;
        tick_machine(&mut state, def, &reg, &everything, 1.0, 0.05);
        assert!(matches!(state.idle, IdleReason::Unformed(_)));
        state.cascade_len = 4;
        state.inputs.add(reg.item("uf6_canister").unwrap(), 2, 16);
        tick_machine(&mut state, def, &reg, &everything, 1.0, 0.05);
        assert_eq!(state.idle, IdleReason::Running);
    }

    #[test]
    fn hand_crafting_consumes_and_produces() {
        let reg = registry();
        let planks = reg.recipe("hand_planks").unwrap();
        let timber = reg.item("timber").unwrap();
        let plank_item = reg.item("planks").unwrap();
        let mut inv = Inventory::player();
        inv.add(timber, 3, 64);
        let made = try_hand_craft(&reg, planks, &mut inv, &everything, 1).unwrap();
        assert_eq!(made, 1);
        assert_eq!(inv.count_of(timber), 2);
        assert_eq!(inv.count_of(plank_item), 4);
    }

    #[test]
    fn hand_crafting_batches_until_inputs_run_out() {
        let reg = registry();
        let planks = reg.recipe("hand_planks").unwrap();
        let timber = reg.item("timber").unwrap();
        let mut inv = Inventory::player();
        inv.add(timber, 3, 64);
        let made = try_hand_craft(&reg, planks, &mut inv, &everything, 10).unwrap();
        assert_eq!(made, 3);
        assert_eq!(inv.count_of(timber), 0);
    }

    #[test]
    fn hand_crafting_refuses_machine_recipes() {
        let reg = registry();
        let smelt = reg.recipe("smelt_iron").unwrap();
        let mut inv = Inventory::player();
        inv.add(reg.item("raw_iron").unwrap(), 5, 64);
        assert_eq!(
            try_hand_craft(&reg, smelt, &mut inv, &everything, 1),
            Err(IdleReason::NoRecipe)
        );
    }

    #[test]
    fn fuel_burns_down_over_time() {
        let reg = registry();
        let (id, def) = machine(&reg, "stone_furnace");
        let mut state = MachineState::for_machine(id, def);
        state.fuel = ItemStack::new(reg.item("coal").unwrap(), 1);
        state.inputs.add(reg.item("raw_iron").unwrap(), 1, 64);
        tick_machine(&mut state, def, &reg, &everything, 1.0, 1.0);
        assert!(state.fuel_remaining > 0.0, "coal should be burning");
        assert_eq!(state.fuel.count, 0, "coal is consumed when lit");
    }
}
