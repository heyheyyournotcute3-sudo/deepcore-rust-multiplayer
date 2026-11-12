//! Persistence: region files for modified chunks and SQLite for everything else.
//!
//! Chunks are bulk binary data, so they go into region files of 8×8×8 chunks, compressed with
//! zstd. Players, machines and the tech tree are queryable records, so they go into SQLite.
//!
//! All writes are crash-safe: data is written to a temporary file (or a transaction) and only
//! then moved into place, so an interrupted save never leaves a corrupt world.

use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
};

use bevy::prelude::*;
use deepcore_shared::{
    content::ContentRegistry,
    inventory::ItemStack,
    voxel::{Chunk, ChunkPos},
};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use tracing::{info, warn};

use crate::world::ServerWorld;

/// Chunks per region edge.
pub const REGION_CHUNKS: i32 = 8;

/// Region file format version, so old files can be rejected rather than misread.
pub const REGION_FORMAT_VERSION: u32 = 1;

/// The save directory layout.
#[derive(Debug, Clone, Resource)]
pub struct SavePaths {
    /// Root save directory.
    pub root: PathBuf,
}

impl SavePaths {
    /// Creates the save directory layout under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self { root }
    }

    /// Where region files live.
    #[must_use]
    pub fn regions_dir(&self) -> PathBuf {
        self.root.join("regions")
    }

    /// The SQLite database path.
    #[must_use]
    pub fn database(&self) -> PathBuf {
        self.root.join("deepcore.sqlite")
    }

    /// Ensures the directories exist.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.regions_dir())
    }
}

/// A region file's contents: the modified chunks in one 8×8×8 region.
#[derive(Debug, Default)]
pub struct Region {
    /// Chunks by position.
    pub chunks: HashMap<ChunkPos, Chunk>,
}

impl Region {
    /// The region coordinate that contains a chunk.
    #[must_use]
    pub fn region_of(pos: ChunkPos) -> (i32, i32, i32) {
        (
            pos.x.div_euclid(REGION_CHUNKS),
            pos.y.div_euclid(REGION_CHUNKS),
            pos.z.div_euclid(REGION_CHUNKS),
        )
    }

    /// The file name for a region coordinate.
    #[must_use]
    pub fn file_name(region: (i32, i32, i32)) -> String {
        format!("r.{}.{}.{}.dcr", region.0, region.1, region.2)
    }
}

/// Serialises a region to bytes: a version header, then a zstd-compressed postcard payload.
pub fn encode_region(region: &Region) -> std::io::Result<Vec<u8>> {
    let entries: Vec<(ChunkPos, Chunk)> = region
        .chunks
        .iter()
        .map(|(pos, chunk)| (*pos, chunk.clone()))
        .collect();
    let raw = postcard::to_allocvec(&entries)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let compressed = zstd::encode_all(raw.as_slice(), 3)?;
    let mut out = Vec::with_capacity(compressed.len() + 8);
    out.extend_from_slice(&REGION_FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
}

/// Deserialises a region, rejecting unknown format versions.
pub fn decode_region(bytes: &[u8]) -> std::io::Result<Region> {
    if bytes.len() < 8 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "region file is too short",
        ));
    }
    let version = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if version != REGION_FORMAT_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported region format version {version}"),
        ));
    }
    let len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    let payload = bytes
        .get(8..8 + len)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "truncated region"))?;
    let raw = zstd::decode_all(payload)?;
    let entries: Vec<(ChunkPos, Chunk)> = postcard::from_bytes(&raw)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let mut region = Region::default();
    for (pos, chunk) in entries {
        region.chunks.insert(pos, chunk);
    }
    Ok(region)
}

/// Writes bytes to a file atomically: temp file first, then rename.
///
/// A crash mid-write leaves the old file intact and a stray `.tmp`, never a half-written save.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Saves every modified chunk in the world to its region file.
pub fn save_modified_chunks(world: &ServerWorld, paths: &SavePaths) -> std::io::Result<usize> {
    paths.ensure_dirs()?;
    // Group modified chunks by region.
    let mut regions: HashMap<(i32, i32, i32), Region> = HashMap::new();
    for (pos, chunk) in world.modified_chunk_list() {
        let region = Region::region_of(pos);
        regions.entry(region).or_default().chunks.insert(pos, chunk);
    }
    let mut saved = 0;
    for (region_pos, region) in regions {
        saved += region.chunks.len();
        let bytes = encode_region(&region)?;
        write_atomic(
            &paths.regions_dir().join(Region::file_name(region_pos)),
            &bytes,
        )?;
    }
    Ok(saved)
}

/// Loads every region file in the save directory into the world.
pub fn load_regions(world: &mut ServerWorld, paths: &SavePaths) -> std::io::Result<usize> {
    let dir = paths.regions_dir();
    if !dir.exists() {
        return Ok(0);
    }
    let mut loaded = 0;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("dcr") {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        match decode_region(&bytes) {
            Ok(region) => {
                for (pos, chunk) in region.chunks {
                    world.insert_loaded_chunk(pos, chunk);
                    loaded += 1;
                }
            }
            Err(e) => warn!(file = %path.display(), "skipping unreadable region: {e}"),
        }
    }
    Ok(loaded)
}

// ================================================================ SQLite records

/// A saved player record.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerRecord {
    /// Stable player id (the display name, until accounts exist).
    pub id: String,
    /// Position.
    pub position: [f32; 3],
    /// Yaw.
    pub yaw: f32,
    /// Pitch.
    pub pitch: f32,
    /// Health.
    pub health: f32,
    /// Radiation dose.
    pub dose: f32,
    /// Inventory slots.
    pub inventory: Vec<ItemStack>,
}

/// A saved machine record.
#[derive(Debug, Clone, PartialEq)]
pub struct MachineRecord {
    /// Machine kind string ID.
    pub kind: String,
    /// Anchor position.
    pub pos: (i32, i32, i32),
    /// Rotation.
    pub rotation: u8,
    /// Input slots.
    pub inputs: Vec<ItemStack>,
    /// Output slots.
    pub outputs: Vec<ItemStack>,
    /// Fuel slot.
    pub fuel: ItemStack,
    /// Pinned recipe string ID, if any.
    pub selected_recipe: Option<String>,
}

/// The database handle.
#[derive(Resource)]
pub struct SaveDb {
    /// The connection pool.
    pub pool: SqlitePool,
}

impl SaveDb {
    /// Opens (and creates if needed) the database, running the schema migration.
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await?;
        let db = Self { pool };
        db.migrate().await?;
        Ok(db)
    }

    /// Creates the schema. Safe to run on every start.
    pub async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS players (
                id TEXT PRIMARY KEY,
                x REAL NOT NULL, y REAL NOT NULL, z REAL NOT NULL,
                yaw REAL NOT NULL, pitch REAL NOT NULL,
                health REAL NOT NULL, dose REAL NOT NULL,
                inventory BLOB NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS machines (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                x INTEGER NOT NULL, y INTEGER NOT NULL, z INTEGER NOT NULL,
                rotation INTEGER NOT NULL,
                inputs BLOB NOT NULL, outputs BLOB NOT NULL, fuel BLOB NOT NULL,
                selected_recipe TEXT
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS research (
                node TEXT PRIMARY KEY,
                deposits BLOB NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Saves a player, replacing any existing record.
    pub async fn save_player(&self, record: &PlayerRecord) -> anyhow::Result<()> {
        let inventory = postcard::to_allocvec(&record.inventory)?;
        sqlx::query(
            "INSERT INTO players (id, x, y, z, yaw, pitch, health, dose, inventory)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                x=excluded.x, y=excluded.y, z=excluded.z,
                yaw=excluded.yaw, pitch=excluded.pitch,
                health=excluded.health, dose=excluded.dose,
                inventory=excluded.inventory",
        )
        .bind(&record.id)
        .bind(record.position[0])
        .bind(record.position[1])
        .bind(record.position[2])
        .bind(record.yaw)
        .bind(record.pitch)
        .bind(record.health)
        .bind(record.dose)
        .bind(inventory)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Loads every saved player.
    pub async fn load_players(&self) -> anyhow::Result<Vec<PlayerRecord>> {
        let rows =
            sqlx::query("SELECT id, x, y, z, yaw, pitch, health, dose, inventory FROM players")
                .fetch_all(&self.pool)
                .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let blob: Vec<u8> = row.try_get("inventory")?;
            let inventory: Vec<ItemStack> = postcard::from_bytes(&blob).unwrap_or_default();
            out.push(PlayerRecord {
                id: row.try_get("id")?,
                position: [row.try_get("x")?, row.try_get("y")?, row.try_get("z")?],
                yaw: row.try_get("yaw")?,
                pitch: row.try_get("pitch")?,
                health: row.try_get("health")?,
                dose: row.try_get("dose")?,
                inventory,
            });
        }
        Ok(out)
    }

    /// Saves a machine, replacing the record at the same position.
    pub async fn save_machine(&self, record: &MachineRecord) -> anyhow::Result<()> {
        // Delete any existing record at this position, then insert: keeps the schema simple and
        // works identically on SQLite and Postgres.
        sqlx::query("DELETE FROM machines WHERE x = ? AND y = ? AND z = ?")
            .bind(record.pos.0)
            .bind(record.pos.1)
            .bind(record.pos.2)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "INSERT INTO machines (kind, x, y, z, rotation, inputs, outputs, fuel, selected_recipe)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&record.kind)
        .bind(record.pos.0)
        .bind(record.pos.1)
        .bind(record.pos.2)
        .bind(i64::from(record.rotation))
        .bind(postcard::to_allocvec(&record.inputs)?)
        .bind(postcard::to_allocvec(&record.outputs)?)
        .bind(postcard::to_allocvec(&record.fuel)?)
        .bind(record.selected_recipe.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Loads every saved machine.
    pub async fn load_machines(&self) -> anyhow::Result<Vec<MachineRecord>> {
        let rows = sqlx::query(
            "SELECT kind, x, y, z, rotation, inputs, outputs, fuel, selected_recipe FROM machines",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let inputs: Vec<u8> = row.try_get("inputs")?;
            let outputs: Vec<u8> = row.try_get("outputs")?;
            let fuel: Vec<u8> = row.try_get("fuel")?;
            out.push(MachineRecord {
                kind: row.try_get("kind")?,
                pos: (row.try_get("x")?, row.try_get("y")?, row.try_get("z")?),
                rotation: row.try_get::<i64, _>("rotation")? as u8,
                inputs: postcard::from_bytes(&inputs).unwrap_or_default(),
                outputs: postcard::from_bytes(&outputs).unwrap_or_default(),
                fuel: postcard::from_bytes(&fuel).unwrap_or_default(),
                selected_recipe: row.try_get("selected_recipe")?,
            });
        }
        Ok(out)
    }

    /// Saves the set of researched tech nodes.
    pub async fn save_research(&self, nodes: &[String]) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM research")
            .execute(&mut *tx)
            .await?;
        for node in nodes {
            sqlx::query("INSERT INTO research (node, deposits) VALUES (?, ?)")
                .bind(node)
                .bind(Vec::<u8>::new())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Loads the researched tech nodes.
    pub async fn load_research(&self) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query("SELECT node FROM research")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>("node").ok())
            .collect())
    }

    /// Records a metadata value, e.g. the world seed.
    pub async fn set_meta(&self, key: &str, value: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO meta (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// The persistence plugin: loads on startup and autosaves on a timer.
pub struct PersistencePlugin;

impl Plugin for PersistencePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Autosave>()
            // The world must be built before saved chunks can be loaded into it.
            .add_systems(Startup, load_on_startup.after(crate::world::WorldInit))
            .add_systems(Update, autosave_tick);
    }
}

fn autosave_tick(
    time: Res<Time>,
    mut autosave: ResMut<Autosave>,
    mut saveables: Saveables,
    registry: Res<ContentRegistry>,
) {
    if !autosave_due(&mut autosave, time.delta_secs()) {
        return;
    }
    let Some(paths) = saveables.paths.as_ref() else {
        return;
    };

    // 1. Modified chunks go to region files.
    if let Some(world) = saveables.world.as_deref() {
        match save_modified_chunks(world, paths) {
            Ok(saved) => info!(saved, "autosave: chunks written"),
            Err(e) => warn!("autosave: chunk save failed: {e}"),
        }
    }
    if let Some(world) = saveables.world.as_mut() {
        world.mark_all_saved();
    }

    // 2. Players, machines and research go to SQLite.
    let Some(db) = saveables.db.as_ref() else {
        return;
    };
    let player_records: Vec<PlayerRecord> = saveables
        .players
        .iter()
        .map(|p| PlayerRecord {
            id: p.name.clone(),
            position: p.body.position,
            yaw: p.body.yaw,
            pitch: p.body.pitch,
            health: p.vitals.health,
            dose: p.vitals.dose,
            inventory: p.inventory.slots().to_vec(),
        })
        .collect();
    let machine_records: Vec<MachineRecord> = saveables
        .machines
        .iter()
        .filter_map(|m| {
            let def = registry.machine_def(m.kind)?;
            Some(MachineRecord {
                kind: def.id.clone(),
                pos: (m.pos.x, m.pos.y, m.pos.z),
                rotation: m.rotation,
                inputs: m.state.inputs.slots().to_vec(),
                outputs: m.state.outputs.slots().to_vec(),
                fuel: m.state.fuel,
                selected_recipe: m
                    .state
                    .selected_recipe
                    .and_then(|r| registry.recipe_def(r).map(|d| d.id.clone())),
            })
        })
        .collect();
    let researched: Vec<String> = saveables
        .research
        .as_ref()
        .map(|r| {
            r.researched
                .iter()
                .filter_map(|node| registry.tech_def(*node).map(|d| d.id.clone()))
                .collect()
        })
        .unwrap_or_default();

    // Database calls are async. A disk problem must never take the server down, so failures
    // are logged and the session continues.
    let outcome = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("cannot start a runtime: {e}"))
        .and_then(|runtime| {
            runtime.block_on(async {
                for player in &player_records {
                    db.save_player(player).await?;
                }
                for machine in &machine_records {
                    db.save_machine(machine).await?;
                }
                db.save_research(&researched).await?;
                Ok::<(), anyhow::Error>(())
            })
        });
    match outcome {
        Ok(()) => info!(
            players = player_records.len(),
            machines = machine_records.len(),
            research = researched.len(),
            "autosave: records written"
        ),
        Err(e) => warn!("autosave: record save failed: {e}"),
    }
}

/// Everything an autosave needs, grouped so the system signature stays readable.
#[derive(bevy::ecs::system::SystemParam)]
struct Saveables<'w, 's> {
    world: Option<ResMut<'w, ServerWorld>>,
    paths: Option<Res<'w, SavePaths>>,
    db: Option<Res<'w, SaveDb>>,
    research: Option<Res<'w, crate::research::ResearchState>>,
    players: Query<'w, 's, &'static crate::sim::ServerPlayer>,
    machines: Query<'w, 's, &'static crate::machines::MachineEntity>,
}

/// Loads saved regions into the world and opens the database at startup, so a restart restores
/// the mine, the machines and the team's research.
fn load_on_startup(
    mut commands: Commands,
    world: Option<ResMut<ServerWorld>>,
    paths: Option<Res<SavePaths>>,
    mut research: Option<ResMut<crate::research::ResearchState>>,
    registry: Res<ContentRegistry>,
) {
    let (Some(mut world), Some(paths)) = (world, paths) else {
        info!("persistence not configured; running without saved regions");
        return;
    };
    match load_regions(&mut world, &paths) {
        Ok(loaded) if loaded > 0 => info!(loaded, "loaded modified chunks from disk"),
        Ok(_) => info!("no saved regions found; the world will generate from the seed"),
        Err(e) => warn!("failed to load saved regions: {e}"),
    }

    if let Err(e) = paths.ensure_dirs() {
        warn!("cannot create the save directory: {e}");
        return;
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            warn!("cannot start a runtime for the database: {e}");
            return;
        }
    };
    let db = match runtime.block_on(SaveDb::open(&paths.database())) {
        Ok(db) => db,
        Err(e) => {
            warn!("cannot open the save database: {e}");
            return;
        }
    };
    let saved = runtime.block_on(db.load_research()).unwrap_or_default();
    if let Some(state) = research.as_mut() {
        for name in saved {
            if let Some(node) = registry.tech(&name) {
                state.researched.insert(node);
            }
        }
        if !state.researched.is_empty() {
            info!(nodes = state.researched.len(), "restored research");
        }
    }
    // Rebuild saved machines as entities, so a restart restores the base.
    let saved_machines = runtime.block_on(db.load_machines()).unwrap_or_default();
    let mut restored = 0;
    for record in saved_machines {
        let Some(kind) = registry.machine(&record.kind) else {
            continue;
        };
        let Some(def) = registry.machine_def(kind) else {
            continue;
        };
        let mut state = deepcore_shared::crafting::MachineState::for_machine(kind, def);
        for (index, stack) in record.inputs.iter().enumerate() {
            state.inputs.set_slot(index, *stack);
        }
        for (index, stack) in record.outputs.iter().enumerate() {
            state.outputs.set_slot(index, *stack);
        }
        state.fuel = record.fuel;
        state.selected_recipe = record
            .selected_recipe
            .as_deref()
            .and_then(|name| registry.recipe(name));
        let pos = deepcore_shared::voxel::BlockPos::new(record.pos.0, record.pos.1, record.pos.2);
        commands.spawn((
            crate::machines::MachineEntity {
                kind,
                pos,
                rotation: record.rotation,
                owner: None,
                state,
                cascade: Vec::new(),
                reactor: None,
                has_scram: false,
            },
            bevy_replicon::prelude::Replicated,
            deepcore_shared::protocol::net_register::ReplicatedMachine(
                deepcore_shared::protocol::NetMachine {
                    pos: deepcore_shared::protocol::NetBlockPos::from(pos),
                    kind,
                    rotation: record.rotation,
                    running: false,
                    progress: 0,
                    idle_code: 1,
                },
            ),
            Transform::from_xyz(pos.x as f32, pos.y as f32, pos.z as f32),
        ));
        restored += 1;
    }
    if restored > 0 {
        info!(restored, "restored machines");
    }
    record_world_seed(&db, world.seed());

    commands.insert_resource(db);
    info!("persistence ready");
}

/// Loads a saved player's record by display name, if one exists.
///
/// Called when a player joins, so position, health and inventory survive a restart.
#[must_use]
pub fn restore_player(db: &SaveDb, name: &str) -> Option<PlayerRecord> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let players = runtime.block_on(db.load_players()).ok()?;
    players.into_iter().find(|p| p.id == name)
}

/// Records the world seed in the database, so a save can be matched to its world.
pub fn record_world_seed(db: &SaveDb, seed: u64) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    if let Err(e) = runtime.block_on(db.set_meta("world_seed", &seed.to_string())) {
        warn!("cannot record the world seed: {e}");
    }
}

/// Saves on a clean shutdown. Called from the server's exit path.
pub fn save_on_shutdown(world: &ServerWorld, paths: &SavePaths) {
    match save_modified_chunks(world, paths) {
        Ok(saved) => info!(saved, "saved world on shutdown"),
        Err(e) => warn!("shutdown save failed: {e}"),
    }
}

/// Autosave timer: every five minutes, and on clean shutdown.
#[derive(Resource, Debug)]
pub struct Autosave {
    /// Seconds until the next save.
    pub remaining: f32,
}

impl Default for Autosave {
    fn default() -> Self {
        Self { remaining: 300.0 }
    }
}

/// Whether an autosave is due this tick.
pub fn autosave_due(autosave: &mut Autosave, dt: f32) -> bool {
    autosave.remaining -= dt;
    if autosave.remaining <= 0.0 {
        autosave.remaining = 300.0;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepcore_shared::content::ContentRegistry;
    use deepcore_shared::voxel::BlockPos;

    fn registry() -> ContentRegistry {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        ContentRegistry::load_from_dir(&dir).unwrap()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deepcore-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn region_round_trips_through_compression() {
        let reg = registry();
        let world = ServerWorld::new(&reg, 1337).unwrap();
        let mut region = Region::default();
        let chunk = reg
            .layers
            .first()
            .map(|_| deepcore_shared::worldgen::WorldGen::new(&reg, 1337).unwrap())
            .unwrap()
            .generate_chunk(ChunkPos::new(1, -1, 1));
        region.chunks.insert(ChunkPos::new(1, -1, 1), chunk.clone());

        let bytes = encode_region(&region).unwrap();
        let back = decode_region(&bytes).unwrap();
        assert_eq!(back.chunks.len(), 1);
        assert_eq!(
            back.chunks.get(&ChunkPos::new(1, -1, 1)).unwrap().to_vec(),
            chunk.to_vec()
        );
        // And it is compressed.
        let raw = postcard::to_allocvec(&vec![(ChunkPos::new(1, -1, 1), chunk)]).unwrap();
        assert!(bytes.len() < raw.len() + 8, "region should be compressed");
        let _ = world;
    }

    #[test]
    fn modified_chunks_survive_a_save_and_load() {
        let reg = registry();
        let mut world = ServerWorld::new(&reg, 1337).unwrap();
        let pos = BlockPos::new(5, -40, 5);
        let stone = reg.block("stone").unwrap();
        let before = world.block(pos);
        world.set_block(pos, stone);
        let expected = world.block(pos);

        let paths = SavePaths::new(temp_dir("chunks"));
        let saved = save_modified_chunks(&world, &paths).unwrap();
        assert_eq!(saved, 1, "one modified chunk should be written");

        // A fresh world, then load the saved regions back in.
        let mut reloaded = ServerWorld::new(&reg, 1337).unwrap();
        let loaded = load_regions(&mut reloaded, &paths).unwrap();
        assert_eq!(loaded, 1);
        assert_eq!(
            reloaded.peek(pos),
            expected,
            "the edit must survive the round trip"
        );
        assert_ne!(
            reloaded.peek(pos),
            before,
            "the edit must actually have changed the world"
        );
        let _ = std::fs::remove_dir_all(paths.root);
    }

    #[test]
    fn atomic_write_leaves_no_partial_file() {
        let dir = temp_dir("atomic");
        let path = dir.join("data.bin");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        // No temporary file is left behind.
        assert!(!path.with_extension("tmp").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bad_region_versions_are_rejected() {
        let mut bytes = vec![99u8, 0, 0, 0, 0, 0, 0, 0];
        bytes.extend_from_slice(b"junk");
        assert!(decode_region(&bytes).is_err());
        assert!(decode_region(&[]).is_err());
    }

    #[test]
    fn region_names_are_stable() {
        let pos = ChunkPos::new(9, -9, 0);
        let region = Region::region_of(pos);
        assert_eq!(region, (1, -2, 0));
        assert_eq!(Region::file_name(region), "r.1.-2.0.dcr");
    }

    #[test]
    fn autosave_fires_every_five_minutes() {
        let mut autosave = Autosave::default();
        // Not due yet.
        assert!(!autosave_due(&mut autosave, 1.0));
        // After five minutes it fires once, then waits again.
        assert!(autosave_due(&mut autosave, 300.0));
        assert!(!autosave_due(&mut autosave, 1.0));
    }
}
