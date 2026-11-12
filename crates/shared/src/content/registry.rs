//! Content registries: resolve string IDs from the RON files into compact numeric IDs.
//!
//! Loading is all-or-nothing. If any reference is dangling, the loader returns an error that
//! names the offending entry, so content bugs are caught at startup rather than in play.

use std::collections::{BTreeMap, HashMap};

use bevy::prelude::Resource;
use thiserror::Error;

use super::types::*;

/// Compact numeric block ID.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct BlockId(pub u16);

/// Compact numeric item ID.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ItemId(pub u16);

/// Compact numeric machine ID.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct MachineId(pub u16);

/// Compact numeric recipe ID.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct RecipeId(pub u16);

/// Compact numeric tech node ID.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct TechId(pub u16);

/// Errors produced while loading or validating content.
#[derive(Debug, Error)]
pub enum ContentError {
    /// A RON file could not be parsed.
    #[error("failed to parse {file}: {source}")]
    Parse {
        /// File name that failed.
        file: String,
        /// Underlying parse error.
        source: Box<ron::error::SpannedError>,
    },
    /// A string ID was referenced but never defined.
    #[error("unknown {kind} reference \"{id}\" in {context}")]
    UnknownRef {
        /// Kind of thing being referenced, e.g. "item".
        kind: &'static str,
        /// The dangling ID.
        id: String,
        /// Where the bad reference was found.
        context: String,
    },
    /// Two entries share the same ID.
    #[error("duplicate {kind} id \"{id}\"")]
    Duplicate {
        /// Kind of thing duplicated.
        kind: &'static str,
        /// The duplicated ID.
        id: String,
    },
    /// A structural rule was violated.
    #[error("{0}")]
    Invalid(String),
}

/// Every piece of content, loaded and validated, indexed by numeric ID.
#[derive(Debug, Resource, Default)]
pub struct ContentRegistry {
    /// All block definitions, indexed by [`BlockId`].
    pub blocks: Vec<BlockDef>,
    /// All item definitions, indexed by [`ItemId`].
    pub items: Vec<ItemDef>,
    /// All machine definitions, indexed by [`MachineId`].
    pub machines: Vec<MachineDef>,
    /// All recipe definitions, indexed by [`RecipeId`].
    pub recipes: Vec<RecipeDef>,
    /// All tech nodes, indexed by [`TechId`].
    pub techs: Vec<TechDef>,
    /// All world layers, ordered from the surface down.
    pub layers: Vec<LayerDef>,
    /// Global tunables.
    pub balance: BalanceDef,
    /// String to numeric block lookup.
    block_ids: HashMap<String, BlockId>,
    /// String to numeric item lookup.
    item_ids: HashMap<String, ItemId>,
    /// String to numeric machine lookup.
    machine_ids: HashMap<String, MachineId>,
    /// String to numeric recipe lookup.
    recipe_ids: HashMap<String, RecipeId>,
    /// String to numeric tech lookup.
    tech_ids: HashMap<String, TechId>,
    /// SHA-256 of the content files, exchanged during the handshake.
    content_hash: String,
}

/// A recipe with its references already resolved to numeric IDs.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRecipe {
    /// The recipe's own ID.
    pub id: RecipeId,
    /// Machine that runs it; `None` means hand crafting at a workbench.
    pub machine: Option<MachineId>,
    /// Seconds at 1x speed.
    pub time: f32,
    /// Resolved inputs.
    pub inputs: Vec<(ItemId, u32)>,
    /// Resolved outputs.
    pub outputs: Vec<(ItemId, u32)>,
    /// Resolved catalysts.
    pub catalysts: Vec<(ItemId, u32)>,
    /// Tech node required, if any.
    pub tech: Option<TechId>,
    /// Power draw override.
    pub power_kw: Option<f32>,
}

impl ContentRegistry {
    /// Loads and validates every content file from a directory.
    pub fn load_from_dir(dir: &std::path::Path) -> Result<Self, ContentError> {
        let mut raw: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for name in [
            "blocks.ron",
            "items.ron",
            "machines.ron",
            "recipes.ron",
            "tech_tree.ron",
            "layers.ron",
            "balance.ron",
        ] {
            let path = dir.join(name);
            let bytes = std::fs::read(&path).map_err(|e| {
                ContentError::Invalid(format!("cannot read {}: {e}", path.display()))
            })?;
            raw.insert(name.to_string(), bytes);
        }
        Self::load_from_bytes(&raw)
    }

    /// Loads and validates content from in-memory file bytes, keyed by file name.
    ///
    /// This is the form the tests use, and the form the client uses to hash its content.
    pub fn load_from_bytes(raw: &BTreeMap<String, Vec<u8>>) -> Result<Self, ContentError> {
        let parse = |name: &str| -> Result<&[u8], ContentError> {
            raw.get(name)
                .map(Vec::as_slice)
                .ok_or_else(|| ContentError::Invalid(format!("missing content file {name}")))
        };
        // Allow `field: "value"` for `Option` fields so the data files stay readable.
        let options = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let de =
            |name: &'static str| -> Result<Vec<u8>, ContentError> { Ok(parse(name)?.to_vec()) };
        let _ = &de;

        let blocks: Vec<BlockDef> =
            options
                .from_bytes(parse("blocks.ron")?)
                .map_err(|e| ContentError::Parse {
                    file: "blocks.ron".into(),
                    source: Box::new(e),
                })?;
        let items: Vec<ItemDef> =
            options
                .from_bytes(parse("items.ron")?)
                .map_err(|e| ContentError::Parse {
                    file: "items.ron".into(),
                    source: Box::new(e),
                })?;
        let machines: Vec<MachineDef> =
            options
                .from_bytes(parse("machines.ron")?)
                .map_err(|e| ContentError::Parse {
                    file: "machines.ron".into(),
                    source: Box::new(e),
                })?;
        let recipes: Vec<RecipeDef> =
            options
                .from_bytes(parse("recipes.ron")?)
                .map_err(|e| ContentError::Parse {
                    file: "recipes.ron".into(),
                    source: Box::new(e),
                })?;
        let techs: Vec<TechDef> =
            options
                .from_bytes(parse("tech_tree.ron")?)
                .map_err(|e| ContentError::Parse {
                    file: "tech_tree.ron".into(),
                    source: Box::new(e),
                })?;
        let layers: Vec<LayerDef> =
            options
                .from_bytes(parse("layers.ron")?)
                .map_err(|e| ContentError::Parse {
                    file: "layers.ron".into(),
                    source: Box::new(e),
                })?;
        let balance: BalanceDef =
            options
                .from_bytes(parse("balance.ron")?)
                .map_err(|e| ContentError::Parse {
                    file: "balance.ron".into(),
                    source: Box::new(e),
                })?;

        Self::build(
            blocks, items, machines, recipes, techs, layers, balance, raw,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        blocks: Vec<BlockDef>,
        items: Vec<ItemDef>,
        machines: Vec<MachineDef>,
        recipes: Vec<RecipeDef>,
        techs: Vec<TechDef>,
        layers: Vec<LayerDef>,
        balance: BalanceDef,
        raw: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, ContentError> {
        let block_ids = index_ids(&blocks, |b| b.id.as_str(), "block")?;
        let item_ids = index_ids(&items, |i| i.id.as_str(), "item")?;
        let machine_ids = index_ids(&machines, |m| m.id.as_str(), "machine")?;
        let recipe_ids = index_ids(&recipes, |r| r.id.as_str(), "recipe")?;
        let tech_ids = index_ids(&techs, |t| t.id.as_str(), "tech")?;

        let registry = Self {
            blocks,
            items,
            machines,
            recipes,
            techs,
            layers,
            balance,
            block_ids,
            item_ids,
            machine_ids,
            recipe_ids,
            tech_ids,
            content_hash: hash_content(raw),
        };

        registry.validate()?;
        Ok(registry)
    }

    /// The SHA-256 of all content files, used to reject mismatched clients.
    #[must_use]
    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }

    /// Looks up a block by string ID.
    #[must_use]
    pub fn block(&self, id: &str) -> Option<BlockId> {
        self.block_ids.get(id).copied()
    }

    /// Looks up an item by string ID.
    #[must_use]
    pub fn item(&self, id: &str) -> Option<ItemId> {
        self.item_ids.get(id).copied()
    }

    /// Looks up a machine by string ID.
    #[must_use]
    pub fn machine(&self, id: &str) -> Option<MachineId> {
        self.machine_ids.get(id).copied()
    }

    /// Looks up a recipe by string ID.
    #[must_use]
    pub fn recipe(&self, id: &str) -> Option<RecipeId> {
        self.recipe_ids.get(id).copied()
    }

    /// Looks up a tech node by string ID.
    #[must_use]
    pub fn tech(&self, id: &str) -> Option<TechId> {
        self.tech_ids.get(id).copied()
    }

    /// Returns the block definition for a numeric ID.
    #[must_use]
    pub fn block_def(&self, id: BlockId) -> Option<&BlockDef> {
        self.blocks.get(id.0 as usize)
    }

    /// Returns the item definition for a numeric ID.
    #[must_use]
    pub fn item_def(&self, id: ItemId) -> Option<&ItemDef> {
        self.items.get(id.0 as usize)
    }

    /// Returns the machine definition for a numeric ID.
    #[must_use]
    pub fn machine_def(&self, id: MachineId) -> Option<&MachineDef> {
        self.machines.get(id.0 as usize)
    }

    /// Returns the recipe definition for a numeric ID.
    #[must_use]
    pub fn recipe_def(&self, id: RecipeId) -> Option<&RecipeDef> {
        self.recipes.get(id.0 as usize)
    }

    /// Returns the tech definition for a numeric ID.
    #[must_use]
    pub fn tech_def(&self, id: TechId) -> Option<&TechDef> {
        self.techs.get(id.0 as usize)
    }

    /// Returns the string ID of an item.
    #[must_use]
    pub fn item_name(&self, id: ItemId) -> &str {
        self.items
            .get(id.0 as usize)
            .map_or("<missing item>", |d| d.id.as_str())
    }

    /// Returns the string ID of a block.
    #[must_use]
    pub fn block_name(&self, id: BlockId) -> &str {
        self.blocks
            .get(id.0 as usize)
            .map_or("<missing block>", |d| d.id.as_str())
    }

    /// Resolves a recipe's string references into numeric IDs.
    pub fn resolve_recipe(&self, id: RecipeId) -> Option<ResolvedRecipe> {
        let def = self.recipe_def(id)?;
        let machine = if def.machine == "hand" {
            None
        } else {
            Some(self.machine(&def.machine)?)
        };
        let mut inputs = Vec::new();
        for (name, count) in &def.inputs {
            inputs.push((self.item(name)?, *count));
        }
        let mut outputs = Vec::new();
        for (name, count) in &def.outputs {
            outputs.push((self.item(name)?, *count));
        }
        let mut catalysts = Vec::new();
        for (name, count) in &def.catalysts {
            catalysts.push((self.item(name)?, *count));
        }
        let tech = match &def.tech {
            Some(t) => Some(self.tech(t)?),
            None => None,
        };
        Some(ResolvedRecipe {
            id,
            machine,
            time: def.time,
            inputs,
            outputs,
            catalysts,
            tech,
            power_kw: def.power_kw,
        })
    }

    /// Resolves every recipe, panicking only on internal invariant violations.
    #[must_use]
    pub fn resolved_recipes(&self) -> Vec<ResolvedRecipe> {
        (0..self.recipes.len())
            .filter_map(|i| self.resolve_recipe(RecipeId(i as u16)))
            .collect()
    }

    /// Returns the layer that contains the given depth below the surface.
    #[must_use]
    pub fn layer_at_depth(&self, depth: i32) -> Option<&LayerDef> {
        self.layers.iter().find(|l| l.depth.contains(depth))
    }

    /// Every recipe a given machine can run.
    #[must_use]
    pub fn recipes_for_machine(&self, machine: MachineId) -> Vec<RecipeId> {
        self.recipes
            .iter()
            .enumerate()
            .filter(|(_, r)| r.machine == self.machine_def(machine).map_or("", |m| &m.id))
            .map(|(i, _)| RecipeId(i as u16))
            .collect()
    }

    /// Total depth of the world, taken from the deepest layer.
    #[must_use]
    pub fn world_depth(&self) -> i32 {
        self.layers.iter().map(|l| l.depth.1).max().unwrap_or(0)
    }

    fn validate(&self) -> Result<(), ContentError> {
        // --- every item reference in blocks must exist ---
        for b in &self.blocks {
            for (item, _) in &b.drops {
                if !self.item_ids.contains_key(item) {
                    return Err(ContentError::UnknownRef {
                        kind: "item",
                        id: item.clone(),
                        context: format!("blocks.ron block \"{}\" drops", b.id),
                    });
                }
            }
            if let Some(m) = &b.machine
                && !self.machine_ids.contains_key(m)
            {
                return Err(ContentError::UnknownRef {
                    kind: "machine",
                    id: m.clone(),
                    context: format!("blocks.ron block \"{}\"", b.id),
                });
            }
        }

        // --- item references inside items.ron ---
        for i in &self.items {
            match &i.category {
                ItemCategory::Block(b) => {
                    if !self.block_ids.contains_key(b) {
                        return Err(ContentError::UnknownRef {
                            kind: "block",
                            id: b.clone(),
                            context: format!("items.ron item \"{}\"", i.id),
                        });
                    }
                }
                ItemCategory::Machine(m) if !self.machine_ids.contains_key(m) => {
                    return Err(ContentError::UnknownRef {
                        kind: "machine",
                        id: m.clone(),
                        context: format!("items.ron item \"{}\"", i.id),
                    });
                }
                _ => {}
            }
            if i.stack == 0 {
                return Err(ContentError::Invalid(format!(
                    "items.ron item \"{}\" has stack size 0",
                    i.id
                )));
            }
        }

        // --- machines ---
        for m in &self.machines {
            for (item, _) in &m.build_cost {
                if !self.item_ids.contains_key(item) {
                    return Err(ContentError::UnknownRef {
                        kind: "item",
                        id: item.clone(),
                        context: format!("machines.ron machine \"{}\" build_cost", m.id),
                    });
                }
            }
            if let Some(t) = &m.tech
                && !self.tech_ids.contains_key(t)
            {
                return Err(ContentError::UnknownRef {
                    kind: "tech",
                    id: t.clone(),
                    context: format!("machines.ron machine \"{}\"", m.id),
                });
            }
            if m.input_slots == 0 && m.output_slots == 0 && m.role == MachineRole::Processor {
                return Err(ContentError::Invalid(format!(
                    "machines.ron machine \"{}\" is a processor with no slots",
                    m.id
                )));
            }
        }

        // --- recipes: every referenced item, machine and tech must exist ---
        for r in &self.recipes {
            if r.machine != "hand" && !self.machine_ids.contains_key(&r.machine) {
                return Err(ContentError::Invalid(format!(
                    "recipes.ron recipe \"{}\" names machine \"{}\", which no machine can run",
                    r.id, r.machine
                )));
            }
            if r.inputs.is_empty() && r.catalysts.is_empty() {
                return Err(ContentError::Invalid(format!(
                    "recipes.ron recipe \"{}\" has no inputs",
                    r.id
                )));
            }
            if r.outputs.is_empty() {
                return Err(ContentError::Invalid(format!(
                    "recipes.ron recipe \"{}\" produces nothing",
                    r.id
                )));
            }
            if r.time <= 0.0 {
                return Err(ContentError::Invalid(format!(
                    "recipes.ron recipe \"{}\" has non-positive time",
                    r.id
                )));
            }
            for (item, count) in r.inputs.iter().chain(&r.outputs).chain(&r.catalysts) {
                if !self.item_ids.contains_key(item) {
                    return Err(ContentError::UnknownRef {
                        kind: "item",
                        id: item.clone(),
                        context: format!("recipes.ron recipe \"{}\"", r.id),
                    });
                }
                if *count == 0 {
                    return Err(ContentError::Invalid(format!(
                        "recipes.ron recipe \"{}\" lists item \"{}\" with count 0",
                        r.id, item
                    )));
                }
            }
            if let Some(t) = &r.tech
                && !self.tech_ids.contains_key(t)
            {
                return Err(ContentError::UnknownRef {
                    kind: "tech",
                    id: t.clone(),
                    context: format!("recipes.ron recipe \"{}\"", r.id),
                });
            }
        }

        // --- tech tree: parents exist, no cycles, reachable from the roots ---
        for t in &self.techs {
            for p in &t.parents {
                if !self.tech_ids.contains_key(p) {
                    return Err(ContentError::Invalid(format!(
                        "tech_tree.ron node \"{}\" has missing parent \"{}\"",
                        t.id, p
                    )));
                }
            }
            for (item, _) in &t.cost {
                if !self.item_ids.contains_key(item) {
                    return Err(ContentError::UnknownRef {
                        kind: "item",
                        id: item.clone(),
                        context: format!("tech_tree.ron node \"{}\" cost", t.id),
                    });
                }
            }
        }
        self.check_tech_acyclic()?;

        // --- layers: blocks and ores exist, depth ranges are sane and contiguous ---
        for l in &self.layers {
            if !self.block_ids.contains_key(&l.base_block) {
                return Err(ContentError::UnknownRef {
                    kind: "block",
                    id: l.base_block.clone(),
                    context: format!("layers.ron layer \"{}\"", l.id),
                });
            }
            for (b, _) in &l.filler_blocks {
                if !self.block_ids.contains_key(b) {
                    return Err(ContentError::UnknownRef {
                        kind: "block",
                        id: b.clone(),
                        context: format!("layers.ron layer \"{}\" filler", l.id),
                    });
                }
            }
            for ore in &l.ores {
                if !self.block_ids.contains_key(&ore.block) {
                    return Err(ContentError::UnknownRef {
                        kind: "block",
                        id: ore.block.clone(),
                        context: format!("layers.ron layer \"{}\" ores", l.id),
                    });
                }
                if ore.vein_size.0 == 0 || ore.vein_size.0 > ore.vein_size.1 {
                    return Err(ContentError::Invalid(format!(
                        "layers.ron layer \"{}\" ore \"{}\" has bad vein_size {:?}",
                        l.id, ore.block, ore.vein_size
                    )));
                }
            }
            if l.depth.0 > l.depth.1 {
                return Err(ContentError::Invalid(format!(
                    "layers.ron layer \"{}\" has inverted depth range {:?}",
                    l.id, l.depth
                )));
            }
        }

        // --- every ore block must be used for something (no dead-end ores) ---
        let mut used_items: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for r in &self.recipes {
            for (i, _) in &r.inputs {
                used_items.insert(i.as_str());
            }
        }
        for t in &self.techs {
            for (i, _) in &t.cost {
                used_items.insert(i.as_str());
            }
        }
        for m in &self.machines {
            for (i, _) in &m.build_cost {
                used_items.insert(i.as_str());
            }
        }
        for b in &self.blocks {
            for (item, _) in &b.drops {
                let is_ore = self
                    .layers
                    .iter()
                    .any(|l| l.ores.iter().any(|o| o.block == b.id));
                if is_ore && !used_items.contains(item.as_str()) {
                    return Err(ContentError::Invalid(format!(
                        "ore block \"{}\" drops item \"{}\", which no recipe, tech node or machine uses",
                        b.id, item
                    )));
                }
            }
        }

        Ok(())
    }

    fn check_tech_acyclic(&self) -> Result<(), ContentError> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Unvisited,
            InProgress,
            Done,
        }
        let mut marks = vec![Mark::Unvisited; self.techs.len()];
        fn visit(
            registry: &ContentRegistry,
            idx: usize,
            marks: &mut [Mark],
        ) -> Result<(), ContentError> {
            match marks[idx] {
                Mark::Done => return Ok(()),
                Mark::InProgress => {
                    return Err(ContentError::Invalid(format!(
                        "tech_tree.ron has a cycle through \"{}\"",
                        registry.techs[idx].id
                    )));
                }
                Mark::Unvisited => {}
            }
            marks[idx] = Mark::InProgress;
            for parent in &registry.techs[idx].parents {
                let pidx = registry
                    .tech_ids
                    .get(parent)
                    .map(|id| id.0 as usize)
                    .ok_or_else(|| {
                        ContentError::Invalid(format!(
                            "tech_tree.ron node \"{}\" has missing parent \"{}\"",
                            registry.techs[idx].id, parent
                        ))
                    })?;
                visit(registry, pidx, marks)?;
            }
            marks[idx] = Mark::Done;
            Ok(())
        }
        for i in 0..self.techs.len() {
            visit(self, i, &mut marks)?;
        }
        Ok(())
    }
}

fn index_ids<T, I, F>(
    entries: &[T],
    id_of: F,
    kind: &'static str,
) -> Result<HashMap<String, I>, ContentError>
where
    F: Fn(&T) -> &str,
    I: From<usize>,
{
    let mut map = HashMap::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        let id = id_of(entry);
        if map.insert(id.to_string(), I::from(i)).is_some() {
            return Err(ContentError::Duplicate {
                kind,
                id: id.to_string(),
            });
        }
    }
    Ok(map)
}

/// Computes a stable SHA-256 over the content files, including their names.
fn hash_content(raw: &BTreeMap<String, Vec<u8>>) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (name, bytes) in raw {
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        hasher.update(bytes);
        hasher.update([0u8]);
    }
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

impl From<usize> for BlockId {
    fn from(v: usize) -> Self {
        Self(v as u16)
    }
}
impl From<usize> for ItemId {
    fn from(v: usize) -> Self {
        Self(v as u16)
    }
}
impl From<usize> for MachineId {
    fn from(v: usize) -> Self {
        Self(v as u16)
    }
}
impl From<usize> for RecipeId {
    fn from(v: usize) -> Self {
        Self(v as u16)
    }
}
impl From<usize> for TechId {
    fn from(v: usize) -> Self {
        Self(v as u16)
    }
}
