//! Serde types mirroring the RON content files in `assets/data/`.
//!
//! These types are the *file format*. They reference other content by string ID so the files
//! stay human-editable; [`crate::content::registry`] resolves those strings to compact numeric
//! IDs at load time.

use serde::{Deserialize, Serialize};

/// An RGB colour in linear-ish 0..1 space, written as `(r, g, b)` in RON.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Color3(pub f32, pub f32, pub f32);

impl Color3 {
    /// Converts to an sRGB colour triple for rendering.
    #[must_use]
    pub fn srgb(self) -> [f32; 3] {
        [self.0, self.1, self.2]
    }
}

/// A 3D integer size, written as `(x, y, z)` in RON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size3(pub i32, pub i32, pub i32);

/// A 2D integer area, written as `(x, y)` in RON. `(1, 1)` is a single block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Area(pub i32, pub i32);

/// An inclusive depth range in blocks below the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepthRange(pub i32, pub i32);

impl DepthRange {
    /// True if `depth` falls inside this range.
    #[must_use]
    pub fn contains(&self, depth: i32) -> bool {
        depth >= self.0 && depth <= self.1
    }
}

/// Procedural texture pattern used by the atlas generator. No art assets are required.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BlockPattern {
    /// Flat colour.
    Solid,
    /// Fine random speckle, for stone and dirt.
    Speckle,
    /// Bright flecks of the given colour on the base colour, for ore blocks.
    OreFleck(Color3),
    /// Loose gravel-like blobs.
    Gravel,
    /// Horizontal strata, for shale and basalt.
    Strata,
    /// Brick courses, for fired bricks and casing.
    Brick,
    /// Wood grain with rings, for timber.
    Wood,
    /// Emissive glow with a hot core, for uranium and running machines.
    Glow,
    /// Metal plating with rivets.
    Plate,
}

/// Radiation emitted by a block standing in the world.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RadiationSpec {
    /// Radius in blocks within which dose accumulates.
    pub radius: f32,
    /// Dose added per second at the source, falling off with distance.
    pub dose_per_sec: f32,
}

/// A block type, from `blocks.ron`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockDef {
    /// Stable string ID, e.g. `"granite"`.
    pub id: String,
    /// Human-readable name for tooltips.
    pub name: String,
    /// Seconds to break with a tier-1 tool.
    pub hardness: f32,
    /// Minimum tool tier required; a lower-tier tool cannot break it at all.
    pub min_tier: u8,
    /// Items dropped when mined, as `(item_id, count)`.
    pub drops: Vec<(String, u32)>,
    /// Base colour for the procedural texture.
    pub color: Color3,
    /// Procedural texture pattern.
    pub pattern: BlockPattern,
    /// Emissive strength 0..1 for the atlas and lighting.
    #[serde(default)]
    pub emissive: f32,
    /// Radiation emitted while this block is exposed.
    #[serde(default)]
    pub radiation: Option<RadiationSpec>,
    /// Which layer this block mostly appears in, for the atlas and debugging.
    #[serde(default)]
    pub layer_hint: Option<u8>,
    /// Whether the block is solid (blocks movement) or a fluid/air.
    #[serde(default = "default_true")]
    pub solid: bool,
    /// Whether the block lets light through.
    #[serde(default)]
    pub transparent: bool,
    /// For fluid blocks: which fluid this is.
    #[serde(default)]
    pub fluid: Option<FluidKind>,
    /// If set, this block is a placeable machine whose behaviour comes from `machines.ron`.
    #[serde(default)]
    pub machine: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Fluids that can occupy a voxel and flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FluidKind {
    /// Water: drowns the player, released by flooding pockets.
    Water,
    /// Methane: drains oxygen and explodes near flame.
    Gas,
    /// Lava: extreme heat and damage.
    Lava,
}

/// Energy a machine needs to run.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EnergyKind {
    /// No power needed (hand-cranked or passive).
    None,
    /// Burns solid fuel from a dedicated fuel slot.
    Fuel {
        /// Number of fuel slots.
        slots: u8,
    },
    /// Draws shaft power from a connected steam engine network.
    Shaft {
        /// Shaft power required.
        kw: f32,
    },
    /// Draws electricity from a cable network.
    Electric {
        /// Kilowatts required.
        kw: f32,
    },
    /// Draws steam from a pipe network.
    Steam {
        /// Steam units per second required.
        rate: f32,
    },
}

/// How a tool is powered.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ToolEnergy {
    /// No energy: swings forever.
    Manual,
    /// Refillable steam canister, refilled at a boiler.
    Steam {
        /// Canister capacity in steam units.
        capacity: f32,
    },
    /// Rechargeable battery, refilled at a charging station.
    Battery {
        /// Battery capacity in charge units.
        capacity: f32,
    },
    /// Consumable core cell, swapped when spent.
    CoreCell {
        /// Block breaks per cell.
        breaks: u32,
    },
}

/// Tool behaviour, for items that mine.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Tool tier; must meet a block's `min_tier` to break it.
    pub tier: u8,
    /// Speed multiplier in the break-time formula.
    pub speed: f32,
    /// Mining area on the block face, e.g. `(3, 3)` for the Nuclear Drill.
    pub area: Area,
    /// How the tool is powered.
    pub energy: ToolEnergy,
    /// Reach in blocks when mining with this tool.
    #[serde(default = "default_reach")]
    pub reach: f32,
}

fn default_reach() -> f32 {
    5.0
}

/// Category of an item, which decides how it can be used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ItemCategory {
    /// A raw material or intermediate product; only used in recipes.
    Material,
    /// A tool that mines blocks.
    Tool(ToolSpec),
    /// A placeable block, referencing a block ID.
    Block(String),
    /// A placeable machine, referencing a machine ID.
    Machine(String),
    /// A machine upgrade module.
    Module(ModuleKind),
    /// A burnable fuel.
    Fuel {
        /// Seconds one unit burns for.
        burn_seconds: f32,
    },
    /// Wearable hazard protection.
    Wearable(WearableSpec),
}

/// Upgrade module kinds that slot into machines from Era 3.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ModuleKind {
    /// +50% speed, +80% power use.
    Speed,
    /// -30% power use.
    Efficiency,
}

/// Protection a wearable provides.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WearableSpec {
    /// Fraction of radiation dose blocked, 0..1.
    #[serde(default)]
    pub radiation_resist: f32,
    /// Fraction of heat gain blocked, 0..1.
    #[serde(default)]
    pub heat_resist: f32,
    /// Whether it supplies breathable air in gas or water.
    #[serde(default)]
    pub breathing: bool,
    /// Whether it lets the player see in the dark (headlamp).
    #[serde(default)]
    pub light: bool,
}

/// An item type, from `items.ron`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemDef {
    /// Stable string ID, e.g. `"steel_ingot"`.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Maximum stack size.
    pub stack: u32,
    /// What the item is and how it can be used.
    pub category: ItemCategory,
    /// Optional flavour text for tooltips.
    #[serde(default)]
    pub description: String,
}

/// A machine type, from `machines.ron`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineDef {
    /// Stable string ID.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Footprint in blocks `(x, y, z)`.
    pub size: Size3,
    /// Energy requirement.
    pub energy: EnergyKind,
    /// Number of input slots.
    pub input_slots: u8,
    /// Number of output slots.
    pub output_slots: u8,
    /// Items needed to build it.
    pub build_cost: Vec<(String, u32)>,
    /// Tech node that unlocks it, if any.
    #[serde(default)]
    pub tech: Option<String>,
    /// For cascades: how many linked blocks are required.
    #[serde(default)]
    pub cascade: Option<CascadeSpec>,
    /// Maximum number of upgrade modules this machine accepts.
    #[serde(default)]
    pub modules: u8,
    /// How this machine interacts with the world, when it is not a simple processor.
    #[serde(default)]
    pub role: MachineRole,
    /// Fraction of heat blocked for players standing next to it, 0..1.
    #[serde(default)]
    pub heat_output: f32,
}

/// A centrifuge cascade spec.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CascadeSpec {
    /// Minimum linked centrifuges.
    pub min: u8,
    /// Maximum linked centrifuges.
    pub max: u8,
}

/// Special behaviours a machine can have beyond "items in, timer, items out".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MachineRole {
    /// Plain processor.
    #[default]
    Processor,
    /// Produces steam from fuel and water.
    Boiler,
    /// Produces shaft power from steam.
    SteamEngine,
    /// Produces electricity from steam.
    Turbine,
    /// Produces electricity from shaft power.
    Generator,
    /// Stores electricity for the network.
    Battery,
    /// Refills steam canisters and batteries held by players.
    ChargingStation,
    /// Draws water out of flooded areas.
    Pump,
    /// Clears gas from a volume.
    VentFan,
    /// Clears radiation dose from nearby players.
    DeconShower,
    /// The reactor core: heat, control rods, scram.
    ReactorCore,
    /// Reactor console used to operate the core.
    ReactorConsole,
    /// Scram panel that makes the reactor fail safe.
    ScramPanel,
    /// Cools spent fuel over time.
    CoolingPool,
    /// Stores radioactive waste safely.
    WasteVault,
    /// The team research desk.
    ResearchDesk,
    /// Workbench for hand crafting.
    Workbench,
    /// Point where the player mines or interacts with a network overlay.
    NetworkNode,
}

/// A crafting or processing recipe, from `recipes.ron`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeDef {
    /// Stable string ID.
    pub id: String,
    /// Machine that runs it, or `"hand"` for workbench hand crafting.
    pub machine: String,
    /// Seconds to complete at 1x speed.
    pub time: f32,
    /// Input items as `(item_id, count)`.
    pub inputs: Vec<(String, u32)>,
    /// Output items as `(item_id, count)`.
    pub outputs: Vec<(String, u32)>,
    /// Tech node that must be researched first.
    #[serde(default)]
    pub tech: Option<String>,
    /// Power draw override in kW; defaults to the machine's requirement.
    #[serde(default)]
    pub power_kw: Option<f32>,
    /// Extra items consumed but not transformed, e.g. flux.
    #[serde(default)]
    pub catalysts: Vec<(String, u32)>,
}

/// A tech tree node, from `tech_tree.ron`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TechDef {
    /// Stable string ID.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Era this node belongs to (0..5).
    pub era: u8,
    /// Parent nodes that must be researched first.
    #[serde(default)]
    pub parents: Vec<String>,
    /// Items the team must deliver to the Research Desk.
    pub cost: Vec<(String, u32)>,
    /// What this node unlocks, for display.
    #[serde(default)]
    pub unlocks: Vec<String>,
    /// One-line description shown in the tree UI.
    #[serde(default)]
    pub description: String,
}

/// An ore vein rule inside a layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OreRule {
    /// Block ID of the ore block.
    pub block: String,
    /// Vein frequency, 0..1. Rarer ores use smaller values.
    pub frequency: f32,
    /// Minimum and maximum vein size in blocks.
    pub vein_size: (u32, u32),
    /// Depth range within the layer where this ore spawns.
    #[serde(default)]
    pub depth_within: Option<(i32, i32)>,
}

/// A hazard type present in a layer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum HazardRule {
    /// Water pockets that flood when breached.
    Flooding {
        /// Chance per chunk of a water pocket.
        pocket_chance: f32,
        /// Maximum water blocks per pocket.
        max_volume: u32,
    },
    /// Methane pockets that drain oxygen and explode.
    Gas {
        /// Chance per chunk of a gas pocket.
        pocket_chance: f32,
        /// Maximum gas blocks per pocket.
        max_volume: u32,
    },
    /// Ambient heat that rises with depth.
    Heat {
        /// Body heat added per second at the layer's top.
        heat_per_sec: f32,
    },
    /// Ceiling spans that collapse.
    CaveIn {
        /// Widest unsupported span before collapse.
        max_span: i32,
    },
    /// Ambient radiation from the rock itself.
    Radiation {
        /// Dose per second added in this layer.
        dose_per_sec: f32,
    },
}

/// A world layer, from `layers.ron`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerDef {
    /// Stable string ID.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Depth range below the surface.
    pub depth: DepthRange,
    /// The filler block for this layer.
    pub base_block: String,
    /// Secondary filler blocks scattered through the layer.
    #[serde(default)]
    pub filler_blocks: Vec<(String, f32)>,
    /// Ores found here.
    #[serde(default)]
    pub ores: Vec<OreRule>,
    /// Hazards present here.
    #[serde(default)]
    pub hazards: Vec<HazardRule>,
    /// Ambient temperature in degrees Celsius.
    #[serde(default)]
    pub ambient_temp: f32,
    /// How much cave carving happens here, 0..1.
    #[serde(default = "default_cave_density")]
    pub cave_density: f32,
}

fn default_cave_density() -> f32 {
    0.5
}

/// A reaction in the nuclear chain, which is more scripted than a normal recipe.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ReactionKind {
    /// Uranium ore becomes crushed uranium ore.
    Crush,
    /// Crushed ore plus acid becomes solution plus tailings.
    Leach,
    /// Solution plus lye becomes yellowcake.
    Precipitate,
    /// Yellowcake plus fluorine becomes a UF6 canister.
    Convert,
    /// A canister is enriched by the cascade.
    Enrich,
    /// Enriched canister becomes fuel rods.
    FabricateFuel,
    /// Enriched canister becomes a compact core cell.
    FabricateCore,
    /// Cadmium plus stainless becomes a control rod.
    ControlRod,
}

/// Global tunables, from `balance.ron`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BalanceDef {
    /// Seconds one coal burns for in a fuel machine.
    pub coal_burn_seconds: f32,
    /// Ore yield multiplier per ore once crushed.
    pub crushed_yield: f32,
    /// Ore yield multiplier once crushed and washed.
    pub washed_yield: f32,
    /// Player walk speed in blocks per second.
    pub walk_speed: f32,
    /// Player sprint multiplier.
    pub sprint_multiplier: f32,
    /// Player crouch multiplier.
    pub crouch_multiplier: f32,
    /// Jump velocity in blocks per second.
    pub jump_speed: f32,
    /// Gravity in blocks per second squared.
    pub gravity: f32,
    /// Player collision box half-width.
    pub player_half_width: f32,
    /// Player collision box height.
    pub player_height: f32,
    /// Base reach in blocks.
    pub reach: f32,
    /// Player max health.
    pub max_health: f32,
    /// Player max stamina.
    pub max_stamina: f32,
    /// Stamina drained per second while sprinting.
    pub stamina_sprint_drain: f32,
    /// Stamina regenerated per second while resting.
    pub stamina_regen: f32,
    /// Max oxygen in seconds of breath.
    pub max_oxygen: f32,
    /// Oxygen drained per second underwater.
    pub oxygen_drain: f32,
    /// Radiation dose at which the screen grains.
    pub dose_grain: f32,
    /// Radiation dose at which health stops regenerating.
    pub dose_no_regen: f32,
    /// Radiation dose at which damage over time starts.
    pub dose_damage: f32,
    /// Health lost per second at the damage threshold.
    pub dose_damage_rate: f32,
    /// Body heat at which health starts draining.
    pub heat_limit: f32,
    /// Health lost per second above the heat limit.
    pub heat_damage_rate: f32,
    /// Reactor power output in kW.
    pub reactor_kw: f32,
    /// Reactor core temperature warning line.
    pub reactor_warn_temp: f32,
    /// Reactor core temperature at which it scrams or melts down.
    pub reactor_limit_temp: f32,
    /// Minutes a radiation zone lingers after a meltdown.
    pub meltdown_zone_minutes: f32,
    /// Block breaks one nuclear core cell provides.
    pub core_cell_breaks: u32,
}

impl Default for BalanceDef {
    fn default() -> Self {
        Self {
            coal_burn_seconds: 80.0,
            crushed_yield: 1.5,
            washed_yield: 2.0,
            walk_speed: 4.3,
            sprint_multiplier: 1.6,
            crouch_multiplier: 0.45,
            jump_speed: 8.4,
            gravity: 24.0,
            player_half_width: 0.3,
            player_height: 1.8,
            reach: 5.0,
            max_health: 100.0,
            max_stamina: 100.0,
            stamina_sprint_drain: 8.0,
            stamina_regen: 14.0,
            max_oxygen: 45.0,
            oxygen_drain: 6.0,
            dose_grain: 100.0,
            dose_no_regen: 300.0,
            dose_damage: 600.0,
            dose_damage_rate: 2.0,
            heat_limit: 45.0,
            heat_damage_rate: 3.0,
            reactor_kw: 20_000.0,
            reactor_warn_temp: 800.0,
            reactor_limit_temp: 1000.0,
            meltdown_zone_minutes: 20.0,
            core_cell_breaks: 2000,
        }
    }
}
