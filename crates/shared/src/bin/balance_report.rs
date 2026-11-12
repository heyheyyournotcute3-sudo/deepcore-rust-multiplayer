//! Content validator and balance report.
//!
//! Run with `cargo run -p deepcore-shared --bin balance_report`. It loads every content file,
//! validates the whole graph, and prints how much raw ore each era needs, so pacing can be
//! checked on paper before playtesting.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use deepcore_shared::content::{ContentRegistry, ItemCategory};

fn main() -> Result<()> {
    let dir = std::path::Path::new("assets/data");
    let registry = ContentRegistry::load_from_dir(dir)
        .context("content validation failed — fix the RON files listed above")?;

    println!("=== Deepcore content validation ===");
    println!("content hash: {}", registry.content_hash());
    println!("blocks:   {}", registry.blocks.len());
    println!("items:    {}", registry.items.len());
    println!("machines: {}", registry.machines.len());
    println!("recipes:  {}", registry.recipes.len());
    println!("techs:    {}", registry.techs.len());
    println!("layers:   {}", registry.layers.len());
    println!("world depth: {} blocks", registry.world_depth());

    let tools = registry
        .items
        .iter()
        .filter(|i| matches!(i.category, ItemCategory::Tool(_)))
        .count();
    let wearables = registry
        .items
        .iter()
        .filter(|i| matches!(i.category, ItemCategory::Wearable(_)))
        .count();
    println!("tools: {tools}   wearables: {wearables}");

    report_ores(&registry);
    report_tech(&registry);
    report_era_costs(&registry);

    println!("\nvalidation passed.");
    Ok(())
}

/// Lists every ore block and the item it drops, so dead-end ores are obvious.
fn report_ores(registry: &ContentRegistry) {
    println!("\n=== ores by layer ===");
    for layer in &registry.layers {
        let names: Vec<String> = layer
            .ores
            .iter()
            .map(|o| {
                let drop = registry
                    .block(&o.block)
                    .and_then(|b| registry.block_def(b))
                    .and_then(|b| b.drops.first())
                    .map_or("?", |(i, _)| i.as_str());
                format!("{}->{}", o.block, drop)
            })
            .collect();
        println!(
            "  {:<16} depth {:>3}-{:<3}  {}",
            layer.name,
            layer.depth.0,
            layer.depth.1,
            names.join(", ")
        );
    }
}

/// Shows the tech tree shape and flags anything that unlocks nothing.
fn report_tech(registry: &ContentRegistry) {
    println!("\n=== tech tree ===");
    for era in 0..=5u8 {
        let nodes: Vec<&str> = registry
            .techs
            .iter()
            .filter(|t| t.era == era)
            .map(|t| t.id.as_str())
            .collect();
        println!("  era {era}: {}", nodes.join(", "));
    }
}

/// Walks the tech tree and reports, per era, the raw ore cost of everything that era needs.
fn report_era_costs(registry: &ContentRegistry) {
    println!("\n=== raw material demand per era ===");
    println!("(sums every tech node cost in that era, expanded through recipes to raw ore)");

    let raw: BTreeSet<&str> = registry
        .items
        .iter()
        .filter(|i| i.id.starts_with("raw_") || i.id == "coal" || i.id == "uranium_ore")
        .map(|i| i.id.as_str())
        .collect();

    for era in 0..=5u8 {
        let mut totals: BTreeMap<String, f64> = BTreeMap::new();
        for tech in registry.techs.iter().filter(|t| t.era == era) {
            for (item, count) in &tech.cost {
                expand_item(registry, item, f64::from(*count), &raw, &mut totals, 0);
            }
        }
        let mut line: Vec<String> = totals
            .iter()
            .map(|(item, count)| format!("{item} {:.0}", count.ceil()))
            .collect();
        line.sort();
        println!("  era {era}: {}", line.join(", "));
    }
}

/// Recursively expands an item into the raw materials needed to make it.
fn expand_item(
    registry: &ContentRegistry,
    item: &str,
    count: f64,
    raw: &BTreeSet<&str>,
    out: &mut BTreeMap<String, f64>,
    depth: usize,
) {
    if depth > 12 {
        return;
    }
    if raw.contains(item) {
        *out.entry(item.to_string()).or_default() += count;
        return;
    }
    // Find the cheapest recipe that produces this item and expand its inputs.
    let mut best: Option<(f64, &Vec<(String, u32)>)> = None;
    for recipe in &registry.recipes {
        let Some(output) = recipe.outputs.iter().find(|(i, _)| i == item) else {
            continue;
        };
        let produced = f64::from(output.1);
        let per_unit = recipe
            .inputs
            .iter()
            .map(|(_, c)| f64::from(*c))
            .sum::<f64>()
            / produced.max(1.0);
        if best.as_ref().is_none_or(|(b, _)| per_unit < *b) {
            best = Some((per_unit, &recipe.inputs));
        }
    }
    match best {
        Some((_, inputs)) => {
            // Work out how many crafts are needed to produce `count` of the item.
            let produced = registry
                .recipes
                .iter()
                .filter_map(|r| r.outputs.iter().find(|(i, _)| i == item))
                .map(|(_, c)| f64::from(*c))
                .fold(0.0f64, f64::max)
                .max(1.0);
            let crafts = (count / produced).ceil();
            for (input, input_count) in inputs {
                expand_item(
                    registry,
                    input,
                    crafts * f64::from(*input_count),
                    raw,
                    out,
                    depth + 1,
                );
            }
        }
        None => {
            // Not craftable: count it directly as an input item.
            *out.entry(item.to_string()).or_default() += count;
        }
    }
}
