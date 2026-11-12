//! Mining rules: break-time maths, tool gating and block-target raycasting.

use crate::content::types::{Area, ToolSpec};
use crate::voxel::BlockPos;

/// Result of evaluating whether a tool can break a block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MiningCheck {
    /// The block can be broken, taking this many seconds.
    Allowed {
        /// Seconds to break, rounded to whole server ticks.
        seconds: f32,
    },
    /// The tool tier is too low.
    TooHard {
        /// The tier the block requires.
        required_tier: u8,
    },
    /// The block cannot be mined at all (bedrock, air, fluids).
    Impossible,
}

/// Computes the break time in seconds for a block with a tool.
///
/// `break_time = hardness / (tool_speed * power_multiplier)`, rounded up to whole server
/// ticks so the server and the client's predicted crack animation agree exactly.
#[must_use]
pub fn break_time(hardness: f32, tool_speed: f32, power_multiplier: f32, tick_seconds: f32) -> f32 {
    if hardness <= 0.0 {
        return 0.0;
    }
    let speed = (tool_speed * power_multiplier).max(0.001);
    let raw = hardness / speed;
    let ticks = (raw / tick_seconds).ceil();
    ticks * tick_seconds
}

/// Evaluates whether `tool` can break a block of the given hardness and minimum tier.
#[must_use]
pub fn check_mining(
    tool: &ToolSpec,
    hardness: f32,
    min_tier: u8,
    power_multiplier: f32,
    tick_seconds: f32,
) -> MiningCheck {
    if hardness <= 0.0 || min_tier == 0 || min_tier > 90 {
        return MiningCheck::Impossible;
    }
    if tool.tier < min_tier {
        return MiningCheck::TooHard {
            required_tier: min_tier,
        };
    }
    MiningCheck::Allowed {
        seconds: break_time(hardness, tool.speed, power_multiplier, tick_seconds),
    }
}

/// The blocks affected by one mining action, given the target face and the tool's area.
///
/// A `(1, 1)` area returns just the target. A `(3, 3)` area (the Nuclear Drill) returns the
/// 3×3 face of blocks in the plane of the hit face.
#[must_use]
pub fn area_blocks(target: BlockPos, face: Face, area: Area) -> Vec<BlockPos> {
    if area.0 <= 1 && area.1 <= 1 {
        return vec![target];
    }
    let (u, v) = face.plane_axes();
    let half_u = area.0 / 2;
    let half_v = area.1 / 2;
    let mut out = Vec::with_capacity((area.0 * area.1) as usize);
    for du in -half_u..=half_u {
        for dv in -half_v..=half_v {
            let mut pos = target;
            pos = pos.offset(u.0 * du, u.1 * du, u.2 * du);
            pos = pos.offset(v.0 * dv, v.1 * dv, v.2 * dv);
            out.push(pos);
        }
    }
    out
}

/// The six block faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// +X
    East,
    /// -X
    West,
    /// +Y
    Up,
    /// -Y
    Down,
    /// +Z
    South,
    /// -Z
    North,
}

impl Face {
    /// The two in-plane axes for this face, used for area mining.
    #[must_use]
    pub fn plane_axes(self) -> ((i32, i32, i32), (i32, i32, i32)) {
        match self {
            Face::East | Face::West => ((0, 1, 0), (0, 0, 1)),
            Face::Up | Face::Down => ((1, 0, 0), (0, 0, 1)),
            Face::South | Face::North => ((1, 0, 0), (0, 1, 0)),
        }
    }

    /// The outward normal of this face.
    #[must_use]
    pub fn normal(self) -> (i32, i32, i32) {
        match self {
            Face::East => (1, 0, 0),
            Face::West => (-1, 0, 0),
            Face::Up => (0, 1, 0),
            Face::Down => (0, -1, 0),
            Face::South => (0, 0, 1),
            Face::North => (0, 0, -1),
        }
    }
}

/// A voxel raycast hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// The block that was hit.
    pub block: BlockPos,
    /// Which face of the block the ray entered through.
    pub face: Face,
    /// Distance from the ray origin, in blocks.
    pub distance: f32,
    /// The empty block in front of the hit face, where a placed block would go.
    pub place_position: BlockPos,
}

/// Casts a ray through the voxel grid and returns the first block that satisfies `hit`.
///
/// Uses the standard Amanatides-Woo voxel traversal, which is exact and integer-friendly.
/// `is_solid` decides what counts as a hit; `in_bounds` stops the ray at the world edge.
#[must_use]
pub fn raycast<F, B>(
    origin: [f32; 3],
    direction: [f32; 3],
    max_distance: f32,
    is_solid: F,
    in_bounds: B,
) -> Option<RayHit>
where
    F: Fn(BlockPos) -> bool,
    B: Fn(BlockPos) -> bool,
{
    let mut x = origin[0].floor() as i32;
    let mut y = origin[1].floor() as i32;
    let mut z = origin[2].floor() as i32;

    let step_x = if direction[0] > 0.0 { 1 } else { -1 };
    let step_y = if direction[1] > 0.0 { 1 } else { -1 };
    let step_z = if direction[2] > 0.0 { 1 } else { -1 };

    let delta_x = if direction[0].abs() < 1e-8 {
        f32::INFINITY
    } else {
        (1.0 / direction[0]).abs()
    };
    let delta_y = if direction[1].abs() < 1e-8 {
        f32::INFINITY
    } else {
        (1.0 / direction[1]).abs()
    };
    let delta_z = if direction[2].abs() < 1e-8 {
        f32::INFINITY
    } else {
        (1.0 / direction[2]).abs()
    };

    let mut t_max_x = if direction[0].abs() < 1e-8 {
        f32::INFINITY
    } else {
        let next = if step_x > 0 { (x + 1) as f32 } else { x as f32 };
        (next - origin[0]).abs() / direction[0].abs()
    };
    let mut t_max_y = if direction[1].abs() < 1e-8 {
        f32::INFINITY
    } else {
        let next = if step_y > 0 { (y + 1) as f32 } else { y as f32 };
        (next - origin[1]).abs() / direction[1].abs()
    };
    let mut t_max_z = if direction[2].abs() < 1e-8 {
        f32::INFINITY
    } else {
        let next = if step_z > 0 { (z + 1) as f32 } else { z as f32 };
        (next - origin[2]).abs() / direction[2].abs()
    };

    let mut last_face = Face::Up;
    let mut travelled = 0.0f32;

    // Guard against a degenerate direction.
    if direction.iter().all(|c| c.abs() < 1e-8) {
        return None;
    }

    for _ in 0..512 {
        let current = BlockPos::new(x, y, z);
        if !in_bounds(current) {
            return None;
        }
        if is_solid(current) {
            let normal = last_face.normal();
            return Some(RayHit {
                block: current,
                face: last_face,
                distance: travelled,
                place_position: current.offset(normal.0, normal.1, normal.2),
            });
        }

        if t_max_x < t_max_y && t_max_x < t_max_z {
            travelled = t_max_x;
            x += step_x;
            t_max_x += delta_x;
            last_face = if step_x > 0 { Face::West } else { Face::East };
        } else if t_max_y < t_max_z {
            travelled = t_max_y;
            y += step_y;
            t_max_y += delta_y;
            last_face = if step_y > 0 { Face::Down } else { Face::Up };
        } else {
            travelled = t_max_z;
            z += step_z;
            t_max_z += delta_z;
            last_face = if step_z > 0 { Face::North } else { Face::South };
        }

        if travelled > max_distance {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::types::{ToolEnergy, ToolSpec};

    fn tool(tier: u8, speed: f32) -> ToolSpec {
        ToolSpec {
            tier,
            speed,
            area: Area(1, 1),
            energy: ToolEnergy::Manual,
            reach: 5.0,
        }
    }

    #[test]
    fn break_time_scales_inversely_with_speed() {
        let t1 = break_time(2.0, 1.0, 1.0, 1.0 / 30.0);
        let t2 = break_time(2.0, 2.0, 1.0, 1.0 / 30.0);
        assert!((t1 - 2.0 * t2).abs() < 0.05, "{t1} vs {t2}");
    }

    #[test]
    fn break_time_rounds_up_to_whole_ticks() {
        let tick = 1.0 / 30.0;
        let t = break_time(1.0, 1.0, 1.0, tick);
        let ticks = t / tick;
        assert!(
            (ticks - ticks.round()).abs() < 1e-4,
            "not a whole number of ticks"
        );
        assert!(t >= 1.0, "must not round down below the true break time");
    }

    #[test]
    fn tier_gating_rejects_weak_tools() {
        let check = check_mining(&tool(1, 1.0), 2.5, 2, 1.0, 1.0 / 30.0);
        assert_eq!(check, MiningCheck::TooHard { required_tier: 2 });
    }

    #[test]
    fn tier_gating_allows_matching_tools() {
        let check = check_mining(&tool(2, 1.6), 2.5, 2, 1.0, 1.0 / 30.0);
        assert!(matches!(check, MiningCheck::Allowed { .. }));
    }

    #[test]
    fn bedrock_is_impossible() {
        let check = check_mining(&tool(5, 12.0), 1000.0, 99, 1.0, 1.0 / 30.0);
        assert_eq!(check, MiningCheck::Impossible);
    }

    #[test]
    fn power_multiplier_speeds_up_mining() {
        let plain = break_time(4.0, 2.0, 1.0, 1.0 / 30.0);
        let boosted = break_time(4.0, 2.0, 1.8, 1.0 / 30.0);
        assert!(boosted < plain);
    }

    #[test]
    fn nuclear_drill_breaks_a_three_by_three_face() {
        let target = BlockPos::new(10, -20, 10);
        let blocks = area_blocks(target, Face::East, Area(3, 3));
        assert_eq!(blocks.len(), 9);
        assert!(blocks.contains(&target));
        // All blocks share the same X, because the face is perpendicular to X.
        assert!(blocks.iter().all(|b| b.x == target.x));
    }

    #[test]
    fn single_area_returns_only_the_target() {
        let target = BlockPos::new(1, 2, 3);
        assert_eq!(area_blocks(target, Face::Up, Area(1, 1)), vec![target]);
    }

    #[test]
    fn raycast_hits_the_nearest_block() {
        let is_solid = |p: BlockPos| p.z == -5;
        let hit = raycast([0.5, 0.5, 0.5], [0.0, 0.0, -1.0], 10.0, is_solid, |_| true).unwrap();
        assert_eq!(hit.block, BlockPos::new(0, 0, -5));
        assert_eq!(hit.face, Face::South);
        assert_eq!(hit.place_position, BlockPos::new(0, 0, -4));
    }

    #[test]
    fn raycast_returns_none_when_nothing_is_hit() {
        let hit = raycast([0.5, 0.5, 0.5], [0.0, 1.0, 0.0], 3.0, |_| false, |_| true);
        assert!(hit.is_none());
    }

    #[test]
    fn raycast_respects_max_distance() {
        let is_solid = |p: BlockPos| p.z == -50;
        let hit = raycast([0.5, 0.5, 0.5], [0.0, 0.0, -1.0], 5.0, is_solid, |_| true);
        assert!(hit.is_none(), "ray should stop before a far block");
    }

    #[test]
    fn raycast_hits_a_floor_below_the_player() {
        let is_solid = |p: BlockPos| p.y < 0;
        let hit = raycast([0.5, 1.5, 0.5], [0.0, -1.0, 0.0], 5.0, is_solid, |_| true).unwrap();
        assert_eq!(hit.block, BlockPos::new(0, -1, 0));
        assert_eq!(hit.face, Face::Up);
        assert_eq!(hit.place_position, BlockPos::new(0, 0, 0));
    }
}
