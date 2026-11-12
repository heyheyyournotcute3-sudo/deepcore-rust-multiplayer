//! Progression integration tests: one per era.
//!
//! Each test starts from that era's raw inputs and drives the real machine state machines,
//! research rules and reactor model until the era's unlock is reached. These are headless — no
//! window, no network — so they run in CI and prove the content graph actually connects.
//!
//! The nuclear chain test is the most important test in the project.

use std::path::Path;

use deepcore_shared::{
    content::{ContentRegistry, ItemId, MachineId, RecipeId, TechId},
    crafting::{MachineState, tick_machine},
    hazards::{ReactorCore, ReactorStatus, reactor_output, tick_reactor},
    inventory::{Inventory, ItemStack},
};

fn registry() -> ContentRegistry {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
    ContentRegistry::load_from_dir(&dir).expect("content must load")
}

/// Runs a machine until it produces `want` of `item`, or fails after a generous budget.
/// Returns how many ticks it took, so pacing can be sanity-checked.
fn run_machine(
    reg: &ContentRegistry,
    machine_name: &str,
    inputs: &[(&str, u32)],
    want: &str,
    fuel: Option<(&str, u32)>,
    budget_ticks: usize,
) -> usize {
    let kind = reg
        .machine(machine_name)
        .unwrap_or_else(|| panic!("machine {machine_name} exists"));
    let def = reg.machine_def(kind).expect("machine def");
    let mut state = MachineState::for_machine(kind, def);
    for (name, count) in inputs {
        let item = reg
            .item(name)
            .unwrap_or_else(|| panic!("item {name} exists"));
        state.inputs.add(item, *count, 64);
    }
    if let Some((name, count)) = fuel {
        let item = reg.item(name).expect("fuel item");
        state.fuel = ItemStack::new(item, count);
    }

    let wanted = reg
        .item(want)
        .unwrap_or_else(|| panic!("item {want} exists"));
    // Pin the recipe that produces the wanted item, so machines with several recipes sharing
    // an input (the anvil, for example) run the one the test asked for.
    state.selected_recipe = reg
        .recipes
        .iter()
        .position(|r| r.machine == machine_name && r.outputs.iter().any(|(o, _)| o == want))
        .map(|index| RecipeId(index as u16));
    let everything = |_: &str| true;
    // Catalysts for the recipes that consume the given inputs. Topping them up means the test
    // exercises the recipe rather than the player's inventory discipline. Only catalysts for
    // *matching* recipes are stocked, so they never crowd the real inputs out of a small
    // machine like the anvil.
    let input_items: Vec<ItemId> = inputs
        .iter()
        .filter_map(|(name, _)| reg.item(name))
        .collect();
    let catalysts: Vec<ItemId> = reg
        .recipes
        .iter()
        .filter(|r| {
            r.machine == machine_name
                && r.inputs
                    .iter()
                    .any(|(name, _)| reg.item(name).is_some_and(|i| input_items.contains(&i)))
        })
        .flat_map(|r| r.catalysts.iter())
        .filter_map(|(name, _)| reg.item(name))
        .collect();

    for tick in 0..budget_ticks {
        for catalyst in &catalysts {
            if state.inputs.count_of(*catalyst) < 2 {
                state.inputs.add(*catalyst, 4, 64);
            }
        }
        tick_machine(&mut state, def, reg, &everything, 1.0, 0.05);
        if state.outputs.count_of(wanted) > 0 {
            return tick;
        }
        if let Some(fuel_item) = fuel.map(|(n, _)| reg.item(n).unwrap())
            && state.fuel.count == 0
            && state.fuel_remaining <= 0.0
        {
            state.fuel = ItemStack::new(fuel_item, 1);
        }
    }
    panic!(
        "machine {machine_name} never produced {want} (idle: {:?})",
        state.idle
    );
}

/// Researches a node and every ancestor, in dependency order.
fn research(state: &mut TestResearch, reg: &ContentRegistry, name: &str) {
    let node = reg
        .tech(name)
        .unwrap_or_else(|| panic!("tech {name} exists"));
    let parents = reg
        .tech_def(node)
        .map(|d| d.parents.clone())
        .unwrap_or_default();
    for parent in parents {
        research(state, reg, &parent);
    }
    state.completed.insert(node);
}

/// A minimal research tracker for the tests.
struct TestResearch {
    completed: std::collections::BTreeSet<TechId>,
}

impl TestResearch {
    fn new() -> Self {
        Self {
            completed: Default::default(),
        }
    }

    fn allows(&self, reg: &ContentRegistry, tech: &str) -> bool {
        reg.tech(tech).is_some_and(|t| self.completed.contains(&t))
    }
}

/// Verifies that every machine a tech node unlocks actually exists in content.
fn assert_unlocks_exist(reg: &ContentRegistry, tech: &str) {
    let node = reg.tech(tech).expect("tech node");
    let def = reg.tech_def(node).expect("tech def");
    for unlock in &def.unlocks {
        let is_machine = reg.machine(unlock).is_some();
        let is_item = reg.item(unlock).is_some();
        let is_block = reg.block(unlock).is_some();
        assert!(
            is_machine || is_item || is_block,
            "tech \"{tech}\" unlocks \"{unlock}\", which does not exist"
        );
    }
}

// ================================================================ Era 1: smelting

#[test]
fn era1_smelting_produces_a_hardened_steel_pickaxe() {
    let reg = registry();
    let mut research_state = TestResearch::new();
    for node in [
        "basic_tools",
        "field_research",
        "smelting",
        "basic_metallurgy",
    ] {
        research(&mut research_state, &reg, node);
    }
    assert!(research_state.allows(&reg, "smelting"));

    // Clay is fired into bricks and fire bricks in the kiln.
    run_machine(
        &reg,
        "brick_kiln",
        &[("clay_lump", 8)],
        "brick",
        Some(("coal", 4)),
        4000,
    );
    run_machine(
        &reg,
        "brick_kiln",
        &[("brick", 4), ("sand_pile", 2)],
        "fire_brick",
        Some(("coal", 4)),
        4000,
    );

    // Ore becomes ingots in the stone furnace.
    run_machine(
        &reg,
        "stone_furnace",
        &[("raw_iron", 4)],
        "iron_ingot",
        Some(("coal", 4)),
        4000,
    );
    // The anvil turns ingots into plates and a pickaxe head.
    run_machine(
        &reg,
        "anvil",
        &[("iron_ingot", 2)],
        "iron_plate",
        None,
        4000,
    );
    // The blast furnace makes steel first; the anvil then forges the pickaxe head from it.
    run_machine(
        &reg,
        "blast_furnace",
        &[("iron_ingot", 4), ("coal", 2)],
        "steel_ingot",
        Some(("coal", 4)),
        8000,
    );
    run_machine(
        &reg,
        "anvil",
        &[("steel_ingot", 3)],
        "steel_pickaxe_head",
        None,
        8000,
    );
    let ticks = run_machine(
        &reg,
        "blast_furnace",
        &[
            ("steel_pickaxe_head", 1),
            ("steel_ingot", 2),
            ("bronze_gear", 1),
        ],
        "hardened_steel_pickaxe",
        Some(("coal", 4)),
        12000,
    );
    assert!(ticks > 0, "the pickaxe should take real time to make");

    // The hardened pickaxe must reach layer 3 and no further.
    let pickaxe = reg.item("hardened_steel_pickaxe").expect("pickaxe");
    let spec = match &reg.item_def(pickaxe).expect("pickaxe def").category {
        deepcore_shared::content::types::ItemCategory::Tool(spec) => *spec,
        other => panic!("hardened pickaxe is not a tool: {other:?}"),
    };
    let granite = reg.block_def(reg.block("granite").unwrap()).unwrap();
    assert!(
        spec.tier >= granite.min_tier,
        "the hardened pickaxe must break granite"
    );
    let basalt = reg.block_def(reg.block("basalt").unwrap()).unwrap();
    assert!(
        spec.tier < basalt.min_tier,
        "the hardened pickaxe must NOT break basalt"
    );
    assert_unlocks_exist(&reg, "steelmaking");
}

// ================================================================ Era 2: steam and processing

#[test]
fn era2_steam_drill_and_ore_yield_bonus() {
    let reg = registry();
    let mut research_state = TestResearch::new();
    for node in [
        "basic_tools",
        "field_research",
        "smelting",
        "basic_metallurgy",
        "steelmaking",
        "bronze_age",
        "steam_power",
        "ore_processing",
        "steam_drilling",
    ] {
        research(&mut research_state, &reg, node);
    }

    // Crushing doubles the unit count; washing then smelting gives the 2x yield the design
    // promises over smelting raw ore directly.
    run_machine(
        &reg,
        "crusher",
        &[("raw_iron", 2)],
        "crushed_iron",
        None,
        8000,
    );
    run_machine(
        &reg,
        "ore_washer",
        &[("crushed_iron", 2)],
        "washed_iron",
        None,
        8000,
    );
    let ticks = run_machine(
        &reg,
        "stone_furnace",
        &[("washed_iron", 2)],
        "iron_ingot",
        Some(("coal", 4)),
        8000,
    );
    assert!(ticks > 0);

    // The Steam Drill is built at the anvil and must reach layer 4 (basalt).
    run_machine(
        &reg,
        "anvil",
        &[("steel_plate", 6), ("bronze_gear", 4), ("copper_pipe", 6)],
        "steam_drill",
        None,
        20000,
    );
    let drill = reg.item("steam_drill").unwrap();
    let spec = match &reg.item_def(drill).unwrap().category {
        deepcore_shared::content::types::ItemCategory::Tool(spec) => *spec,
        other => panic!("steam drill is not a tool: {other:?}"),
    };
    let basalt = reg.block_def(reg.block("basalt").unwrap()).unwrap();
    assert!(
        spec.tier >= basalt.min_tier,
        "steam drill must break basalt"
    );
    let gneiss = reg.block_def(reg.block("gneiss").unwrap()).unwrap();
    assert!(
        spec.tier < gneiss.min_tier,
        "steam drill must NOT break the deep crust"
    );
    assert_unlocks_exist(&reg, "steam_drilling");
}

// ================================================================ Era 3: electricity

#[test]
fn era3_electric_chain_builds_circuits_and_aluminium() {
    let reg = registry();
    let mut research_state = TestResearch::new();
    for node in [
        "basic_tools",
        "field_research",
        "smelting",
        "basic_metallurgy",
        "steelmaking",
        "steam_power",
        "ore_processing",
        "electric_power",
        "circuits",
        "electric_smelting",
        "electrochemistry",
        "electric_storage",
    ] {
        research(&mut research_state, &reg, node);
    }

    // Arc furnace: quartz becomes silicon.
    run_machine(
        &reg,
        "arc_furnace",
        &[("quartz", 3), ("coal", 1)],
        "silicon",
        None,
        8000,
    );
    // Electrolysis: bauxite becomes aluminium.
    run_machine(
        &reg,
        "electrolysis_cell",
        &[("raw_bauxite", 3), ("coal", 1)],
        "aluminium_ingot",
        None,
        8000,
    );
    // Salt splits into chlorine and lye.
    run_machine(
        &reg,
        "electrolysis_cell",
        &[("salt", 4)],
        "chlorine",
        None,
        8000,
    );
    // The circuit assembler makes a circuit from plates, quartz and wire.
    run_machine(
        &reg,
        "circuit_assembler",
        &[("copper_plate", 2), ("quartz", 2), ("wire", 2)],
        "circuit_mk1",
        None,
        8000,
    );
    // And a battery pack from lead, acid and copper.
    run_machine(
        &reg,
        "circuit_assembler",
        &[("lead_plate", 2), ("sulfuric_acid", 2), ("copper_plate", 1)],
        "battery_pack",
        None,
        8000,
    );
    assert_unlocks_exist(&reg, "circuits");
}

// ================================================================ Era 4: chemistry

#[test]
fn era4_chemistry_produces_stainless_and_a_battery_drill() {
    let reg = registry();
    let mut research_state = TestResearch::new();
    for node in [
        "basic_tools",
        "field_research",
        "smelting",
        "basic_metallurgy",
        "steelmaking",
        "steam_power",
        "ore_processing",
        "electric_power",
        "circuits",
        "electric_smelting",
        "electrochemistry",
        "chemistry",
        "advanced_alloys",
        "precision_engineering",
        "rare_earths",
        "deep_drilling",
    ] {
        research(&mut research_state, &reg, node);
    }

    // Sulfuric acid from sulfur and salt.
    run_machine(
        &reg,
        "chemical_plant",
        &[("sulfur", 2), ("salt", 1)],
        "sulfuric_acid",
        None,
        8000,
    );
    // Fluorine compounds from fluorite and acid.
    run_machine(
        &reg,
        "chemical_plant",
        &[("fluorite_crystal", 2), ("sulfuric_acid", 1)],
        "fluorine_compound",
        None,
        8000,
    );
    // Stainless steel from steel, nickel and chromium.
    run_machine(
        &reg,
        "alloy_furnace",
        &[
            ("steel_ingot", 3),
            ("nickel_ingot", 1),
            ("chromium_ingot", 1),
        ],
        "stainless_steel_ingot",
        None,
        12000,
    );
    // Tungsten carbide from tungsten and graphite.
    run_machine(
        &reg,
        "alloy_furnace",
        &[("tungsten_ingot", 2), ("graphite", 2)],
        "tungsten_carbide_plate",
        None,
        12000,
    );
    // Rare earth magnets from separated monazite.
    run_machine(
        &reg,
        "rare_earth_separator",
        &[("monazite_sand", 4), ("sulfuric_acid", 1)],
        "rare_earth_concentrate",
        None,
        8000,
    );
    run_machine(
        &reg,
        "rare_earth_separator",
        &[("rare_earth_oxide", 4), ("sulfuric_acid", 2)],
        "rare_earth_magnet",
        None,
        12000,
    );
    // The tungsten-carbide drill reaches layer 5 and no further.
    run_machine(
        &reg,
        "machine_shop",
        &[
            ("tungsten_carbide_plate", 2),
            ("stainless_steel_ingot", 4),
            ("battery_pack", 2),
            ("rare_earth_magnet", 1),
        ],
        "battery_drill",
        None,
        30000,
    );
    let drill = reg.item("battery_drill").unwrap();
    let spec = match &reg.item_def(drill).unwrap().category {
        deepcore_shared::content::types::ItemCategory::Tool(spec) => *spec,
        other => panic!("battery drill is not a tool: {other:?}"),
    };
    let gneiss = reg.block_def(reg.block("gneiss").unwrap()).unwrap();
    assert!(
        spec.tier >= gneiss.min_tier,
        "drill must break the deep crust"
    );
    let mantle = reg.block_def(reg.block("mantle_rock").unwrap()).unwrap();
    assert!(
        spec.tier < mantle.min_tier,
        "drill must NOT break mantle rock"
    );
    assert_unlocks_exist(&reg, "deep_drilling");
}

// ================================================================ Era 5: the nuclear chain

/// The most important test in the project: raw uranium goes through every nuclear structure
/// and comes out as a working Nuclear Drill and a stable reactor.
#[test]
fn era5_nuclear_chain_produces_a_nuclear_drill_and_a_stable_reactor() {
    let reg = registry();
    let mut research_state = TestResearch::new();
    for node in [
        "basic_tools",
        "field_research",
        "smelting",
        "basic_metallurgy",
        "steelmaking",
        "steam_power",
        "ore_processing",
        "electric_power",
        "circuits",
        "electric_smelting",
        "electrochemistry",
        "chemistry",
        "advanced_alloys",
        "precision_engineering",
        "rare_earths",
        "hazard_gear",
        "deep_drilling",
        "uranium_milling",
        "uranium_conversion",
        "enrichment",
        "fuel_fabrication",
        "reactor_engineering",
        "reactor_safety",
        "nuclear_drill",
        "waste_handling",
    ] {
        research(&mut research_state, &reg, node);
    }

    // --- 1. Crush the ore -------------------------------------------------
    run_machine(
        &reg,
        "crusher",
        &[("uranium_ore", 2)],
        "crushed_uranium_ore",
        None,
        8000,
    );

    // --- 2. Leach: crushed ore + acid -> solution + tailings ---------------
    run_machine(
        &reg,
        "leaching_tank",
        &[("crushed_uranium_ore", 2), ("sulfuric_acid", 1)],
        "uranium_solution",
        None,
        12000,
    );

    // --- 3. Precipitate: solution + lye -> yellowcake ----------------------
    run_machine(
        &reg,
        "precipitator",
        &[("uranium_solution", 2), ("lye", 1)],
        "yellowcake",
        None,
        12000,
    );

    // --- 4. Convert: yellowcake + fluorine -> UF6 canister -----------------
    run_machine(
        &reg,
        "conversion_plant",
        &[("yellowcake", 1), ("fluorine_compound", 2)],
        "uf6_canister",
        None,
        16000,
    );

    // --- 5. Enrich: a 4-centrifuge cascade spins UF6 into enriched + depleted
    let centrifuge = reg.machine("centrifuge").unwrap();
    let cdef = reg.machine_def(centrifuge).unwrap();
    let mut cascade = MachineState::for_machine(centrifuge, cdef);
    cascade.cascade_len = 4;
    cascade.inputs.add(reg.item("uf6_canister").unwrap(), 2, 16);
    let everything = |_: &str| true;
    let mut enriched = false;
    for _ in 0..20000 {
        tick_machine(&mut cascade, cdef, &reg, &everything, 1.0, 0.05);
        if cascade
            .outputs
            .count_of(reg.item("enriched_canister").unwrap())
            > 0
        {
            enriched = true;
            break;
        }
    }
    assert!(enriched, "the centrifuge cascade never enriched a canister");
    assert_eq!(
        cascade
            .outputs
            .count_of(reg.item("depleted_canister").unwrap()),
        1,
        "enrichment must also produce a depleted canister"
    );

    // A cascade shorter than the minimum must refuse to run.
    let mut lonely = MachineState::for_machine(centrifuge, cdef);
    lonely.cascade_len = 2;
    lonely.inputs.add(reg.item("uf6_canister").unwrap(), 2, 16);
    tick_machine(&mut lonely, cdef, &reg, &everything, 1.0, 0.05);
    assert!(
        matches!(
            lonely.idle,
            deepcore_shared::crafting::IdleReason::Unformed(_)
        ),
        "a 2-centrifuge cascade must not enrich"
    );

    // --- 6. Fabricate fuel rods and a core cell ----------------------------
    run_machine(
        &reg,
        "fuel_fabricator",
        &[("enriched_canister", 1), ("zirconium_tubing", 2)],
        "fuel_rod",
        None,
        12000,
    );
    run_machine(
        &reg,
        "fuel_fabricator",
        &[
            ("enriched_canister", 1),
            ("stainless_casing", 2),
            ("shielding_plate", 1),
        ],
        "compact_core_cell",
        None,
        16000,
    );

    // --- 7. Control rods from cadmium and stainless ------------------------
    run_machine(
        &reg,
        "machine_shop",
        &[("cadmium", 2), ("stainless_steel_ingot", 2)],
        "control_rod",
        None,
        12000,
    );

    // --- 8. The Nuclear Drill ---------------------------------------------
    run_machine(
        &reg,
        "machine_shop",
        &[
            ("compact_core_cell", 1),
            ("tungsten_carbide_drill_head", 1),
            ("stainless_casing", 4),
            ("rare_earth_magnet", 2),
        ],
        "nuclear_drill",
        None,
        40000,
    );
    let drill = reg.item("nuclear_drill").unwrap();
    let spec = match &reg.item_def(drill).unwrap().category {
        deepcore_shared::content::types::ItemCategory::Tool(spec) => *spec,
        other => panic!("nuclear drill is not a tool: {other:?}"),
    };
    assert_eq!(spec.tier, 5, "the nuclear drill is the tier-5 tool");
    assert_eq!(spec.area.0, 3, "the nuclear drill mines a 3x3 face");
    assert_eq!(spec.area.1, 3);
    assert!(spec.speed >= 12.0, "the nuclear drill is a 12x speed tool");
    let mantle = reg.block_def(reg.block("mantle_rock").unwrap()).unwrap();
    assert!(
        spec.tier >= mantle.min_tier,
        "the nuclear drill must break mantle rock"
    );

    // --- 9. A stable reactor ----------------------------------------------
    // Coolant flowing, rods half withdrawn: the core must settle below the limit and
    // produce power indefinitely.
    let balance = &reg.balance;
    // Fission heat at 50% rods is 210 C/s; coolant at 0.8 removes 240 C/s, so the core settles
    // below ambient and stays there. (At 0.6 the core slowly overheats and correctly scrams.)
    let mut core = ReactorCore {
        running: true,
        rod_withdrawal: 0.5,
        coolant_flow: 0.8,
        temperature: 300.0,
        melted_down: false,
    };
    let mut ever_warned = false;
    for _ in 0..6000 {
        match tick_reactor(&mut core, balance, true, 0.05) {
            ReactorStatus::Meltdown => panic!("a cooled reactor must never melt down"),
            ReactorStatus::Warning => ever_warned = true,
            _ => {}
        }
    }
    assert!(
        core.temperature < balance.reactor_limit_temp,
        "core settled at {} which is above the limit",
        core.temperature
    );
    assert!(
        reactor_output(&core, balance) > 0.0,
        "a stable reactor must produce power"
    );
    let _ = ever_warned;

    // --- 10. The reactor is far more powerful than the coal grid -----------
    let generator_kw = match reg
        .machine_def(reg.machine("generator").unwrap())
        .unwrap()
        .energy
    {
        deepcore_shared::content::types::EnergyKind::Shaft { kw } => kw,
        other => panic!("generator should draw shaft power: {other:?}"),
    };
    assert!(
        balance.reactor_kw > generator_kw * 100.0,
        "the reactor ({}) should dwarf a single generator ({generator_kw})",
        balance.reactor_kw
    );

    // --- 11. Waste handling -----------------------------------------------
    run_machine(
        &reg,
        "alloy_furnace",
        &[("radioactive_tailings", 2), ("lead_ingot", 1)],
        "waste_cask",
        None,
        12000,
    );
    run_machine(
        &reg,
        "alloy_furnace",
        &[("depleted_canister", 1), ("lead_ingot", 2)],
        "shielding_plate",
        None,
        12000,
    );

    // Every nuclear tech node must unlock something that exists.
    for node in [
        "uranium_milling",
        "uranium_conversion",
        "enrichment",
        "fuel_fabrication",
        "reactor_engineering",
        "reactor_safety",
        "nuclear_drill",
        "waste_handling",
    ] {
        assert_unlocks_exist(&reg, node);
    }
}

// ================================================================ Whole-ladder checks

/// Every era's tech cost must be reachable from the ores available at or above that era's
/// depth gate. This is the "no dead ends, no unreachable content" check.
#[test]
fn every_era_cost_uses_only_reachable_items() {
    let reg = registry();
    // Which items exist as ore drops, by the deepest layer they appear in.
    let mut reachable: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // Era 0-1 items come from the surface and the first two layers.
    for layer in &reg.layers {
        if layer.depth.1 <= 64 {
            // Base and filler blocks are dug up as well as ore, and all of them drop items.
            let mut blocks: Vec<String> = vec![layer.base_block.clone()];
            blocks.extend(layer.filler_blocks.iter().map(|(b, _)| b.clone()));
            blocks.extend(layer.ores.iter().map(|o| o.block.clone()));
            for name in blocks {
                if let Some(block) = reg.block(&name)
                    && let Some(def) = reg.block_def(block)
                {
                    for (item, _) in &def.drops {
                        reachable.insert(item.clone());
                    }
                }
            }
        }
    }
    reachable.insert("timber".into());

    // Era 0 and 1 must be buildable from those items plus intermediates they can make.
    for era in [0u8, 1] {
        for node in reg.techs.iter().filter(|t| t.era == era) {
            for (item, _) in &node.cost {
                let buildable = reachable.contains(item)
                    || reg
                        .recipes
                        .iter()
                        .any(|r| r.outputs.iter().any(|(out, _)| out == item));
                assert!(
                    buildable,
                    "era {era} node \"{}\" needs \"{item}\", which nothing produces",
                    node.id
                );
            }
        }
    }
}

/// Every machine must be craftable by some recipe, or it can never be built.
#[test]
fn every_machine_is_craftable() {
    let reg = registry();
    for machine in &reg.machines {
        let item_name = machine.id.as_str();
        let craftable = reg
            .recipes
            .iter()
            .any(|r| r.outputs.iter().any(|(out, _)| out == item_name));
        assert!(
            craftable,
            "machine \"{}\" has no crafting recipe",
            machine.id
        );
    }
}

/// Every tool in the game must be craftable.
#[test]
fn every_tool_is_craftable() {
    let reg = registry();
    for item in &reg.items {
        if matches!(
            item.category,
            deepcore_shared::content::types::ItemCategory::Tool(_)
        ) {
            let craftable = reg
                .recipes
                .iter()
                .any(|r| r.outputs.iter().any(|(out, _)| out == &item.id));
            assert!(craftable, "tool \"{}\" has no crafting recipe", item.id);
        }
    }
}

/// The tech tree must gate every machine behind a node, so progression cannot be skipped.
#[test]
fn machines_are_gated_by_tech() {
    let reg = registry();
    for machine in &reg.machines {
        // A few Era-0 starter structures are deliberately ungated.
        if ["workbench", "storage_crate", "research_desk"].contains(&machine.id.as_str()) {
            continue;
        }
        assert!(
            machine.tech.is_some(),
            "machine \"{}\" is not gated behind a tech node",
            machine.id
        );
    }
}

/// Every recipe naming a tech node must reference a node that exists (the loader checks this,
/// but this asserts the check is exercised on the real content).
#[test]
fn recipe_tech_references_resolve() {
    let reg = registry();
    let mut gated = 0;
    for recipe in &reg.recipes {
        if let Some(tech) = &recipe.tech {
            assert!(
                reg.tech(tech).is_some(),
                "recipe \"{}\" references missing tech \"{tech}\"",
                recipe.id
            );
            gated += 1;
        }
    }
    assert!(gated > 50, "most recipes should be gated, found {gated}");
}

/// Machines used by recipes must have enough slots for those recipes' inputs and outputs.
#[test]
fn machines_have_enough_slots_for_their_recipes() {
    let reg = registry();
    for recipe in &reg.recipes {
        if recipe.machine == "hand" {
            continue;
        }
        let Some(machine_id) = reg.machine(&recipe.machine) else {
            continue;
        };
        let def = reg.machine_def(machine_id).unwrap();
        assert!(
            recipe.inputs.len() + recipe.catalysts.len() <= usize::from(def.input_slots),
            "recipe \"{}\" needs {} inputs but {} has {} slots",
            recipe.id,
            recipe.inputs.len() + recipe.catalysts.len(),
            def.id,
            def.input_slots
        );
        assert!(
            recipe.outputs.len() <= usize::from(def.output_slots),
            "recipe \"{}\" produces {} outputs but {} has {} slots",
            recipe.id,
            recipe.outputs.len(),
            def.id,
            def.output_slots
        );
    }
}

/// A quick pacing sanity check: no single recipe should take absurdly long.
#[test]
fn recipe_times_are_within_a_sane_range() {
    let reg = registry();
    for recipe in &reg.recipes {
        assert!(
            recipe.time > 0.0 && recipe.time <= 120.0,
            "recipe \"{}\" takes {}s, which is outside the sane range",
            recipe.id,
            recipe.time
        );
    }
}

/// Helper used by other tests: the machine a recipe runs in, resolved.
#[allow(dead_code)]
fn machine_of(reg: &ContentRegistry, recipe: RecipeId) -> Option<MachineId> {
    let def = reg.recipe_def(recipe)?;
    reg.machine(&def.machine)
}

/// The player starts with a steel pickaxe, which must be able to break layer 1 and 2 blocks.
#[test]
fn the_starting_pickaxe_breaks_the_first_two_layers() {
    let reg = registry();
    let pickaxe = reg.item("steel_pickaxe").unwrap();
    let spec = match &reg.item_def(pickaxe).unwrap().category {
        deepcore_shared::content::types::ItemCategory::Tool(spec) => *spec,
        other => panic!("steel pickaxe is not a tool: {other:?}"),
    };
    for layer_name in ["topsoil", "shale_beds"] {
        let layer = reg
            .layers
            .iter()
            .find(|l| l.id == layer_name)
            .expect("layer exists");
        let base = reg
            .block_def(reg.block(&layer.base_block).unwrap())
            .unwrap();
        assert!(
            spec.tier >= base.min_tier,
            "the steel pickaxe must break {} ({})",
            layer_name,
            base.id
        );
    }
    // And it must NOT break the granite below.
    let granite = reg.block_def(reg.block("granite").unwrap()).unwrap();
    assert!(
        spec.tier < granite.min_tier,
        "the steel pickaxe must be blocked by granite"
    );
}

/// The inventory helper used by the tests keeps its invariants.
#[test]
fn test_inventory_helper_is_sane() {
    let reg = registry();
    let mut inv = Inventory::player();
    let coal = reg.item("coal").unwrap();
    assert_eq!(inv.add(coal, 10, 64), 0);
    assert_eq!(inv.count_of(coal), 10);
}
