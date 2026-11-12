//! Deterministic player movement and AABB-versus-voxel collision.
//!
//! This code is shared by the client (prediction) and the server (authority). It is pure and
//! deterministic: no randomness, no floating-point accumulation across frames beyond the
//! fixed timestep. That is what lets the client predict exactly what the server will compute.

use crate::content::types::BalanceDef;
use crate::voxel::{BlockPos, coords::SURFACE_Y};

/// A player's physical state inside the simulation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerBody {
    /// Feet position in world space.
    pub position: [f32; 3],
    /// Velocity in blocks per second.
    pub velocity: [f32; 3],
    /// Yaw in radians (0 looks down -Z).
    pub yaw: f32,
    /// Pitch in radians, clamped to +-89 degrees.
    pub pitch: f32,
    /// Whether the player is standing on ground.
    pub on_ground: bool,
}

impl PlayerBody {
    /// Creates a body at a spawn position.
    #[must_use]
    pub fn new(position: [f32; 3]) -> Self {
        Self {
            position,
            velocity: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }
    }

    /// The centre of the player's eyes, used for raycasts and the camera.
    #[must_use]
    pub fn eye_position(&self, balance: &BalanceDef) -> [f32; 3] {
        [
            self.position[0],
            self.position[1] + balance.player_height - 0.15,
            self.position[2],
        ]
    }

    /// The unit vector the player is looking along.
    #[must_use]
    pub fn look_direction(&self) -> [f32; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        // Yaw 0 looks down -Z; positive yaw turns right.
        [-sy * cp, sp, -cy * cp]
    }
}

/// One tick of movement intent, produced by the client and validated by the server.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MoveInput {
    /// Forward/back axis in -1..1.
    pub forward: f32,
    /// Right/left axis in -1..1.
    pub right: f32,
    /// Whether the player wants to jump.
    pub jump: bool,
    /// Whether the player wants to sprint.
    pub sprint: bool,
    /// Whether the player wants to crouch.
    pub crouch: bool,
    /// Absolute yaw in radians.
    pub yaw: f32,
    /// Absolute pitch in radians.
    pub pitch: f32,
}

/// A read-only view of the world for collision queries.
pub trait CollisionWorld {
    /// Whether the block at this position blocks movement.
    fn is_solid(&self, pos: BlockPos) -> bool;
}

/// Advances a body by one fixed timestep, resolving collisions against the world.
///
/// The player is modelled as an axis-aligned box: `player_half_width` wide and
/// `player_height` tall, with the position at the feet.
pub fn step_body<W: CollisionWorld>(
    body: &mut PlayerBody,
    input: &MoveInput,
    balance: &BalanceDef,
    world: &W,
    dt: f32,
) {
    body.yaw = input.yaw;
    body.pitch = input.pitch.clamp(-1.5533, 1.5533);

    // Desired horizontal velocity from the input axes, in the player's facing frame.
    let mut forward = input.forward.clamp(-1.0, 1.0);
    let mut right = input.right.clamp(-1.0, 1.0);
    let len = (forward * forward + right * right).sqrt();
    if len > 1.0 {
        forward /= len;
        right /= len;
    }

    let mut speed = balance.walk_speed;
    if input.sprint && !input.crouch {
        speed *= balance.sprint_multiplier;
    }
    if input.crouch {
        speed *= balance.crouch_multiplier;
    }

    let (sy, cy) = body.yaw.sin_cos();
    // Forward vector on the ground plane.
    let fx = -sy;
    let fz = -cy;
    // Right vector.
    let rx = cy;
    let rz = -sy;

    let target_x = (fx * forward + rx * right) * speed;
    let target_z = (fz * forward + rz * right) * speed;

    // Ground friction and air control: snap to target quickly on the ground, slower in air.
    let control = if body.on_ground { 18.0 } else { 4.0 };
    let blend = (control * dt).min(1.0);
    body.velocity[0] += (target_x - body.velocity[0]) * blend;
    body.velocity[2] += (target_z - body.velocity[2]) * blend;

    // Vertical motion: gravity, jumping, and standing still on the ground.
    if input.jump && body.on_ground {
        body.velocity[1] = balance.jump_speed;
        body.on_ground = false;
    }
    body.velocity[1] -= balance.gravity * dt;
    if body.velocity[1] < -60.0 {
        body.velocity[1] = -60.0;
    }

    // Integrate one axis at a time so a blocked axis does not cancel the others.
    body.on_ground = false;
    move_axis(body, balance, world, 0, body.velocity[0] * dt);
    move_axis(body, balance, world, 2, body.velocity[2] * dt);
    move_axis(body, balance, world, 1, body.velocity[1] * dt);
}

/// Moves the body along one axis and resolves penetration against solid blocks.
fn move_axis<W: CollisionWorld>(
    body: &mut PlayerBody,
    balance: &BalanceDef,
    world: &W,
    axis: usize,
    delta: f32,
) {
    if delta == 0.0 {
        return;
    }
    body.position[axis] += delta;

    let half = balance.player_half_width;
    let height = balance.player_height;
    let min = [
        body.position[0] - half,
        body.position[1],
        body.position[2] - half,
    ];
    let max = [
        body.position[0] + half,
        body.position[1] + height,
        body.position[2] + half,
    ];

    // Block range the AABB overlaps.
    let lo = [
        min[0].floor() as i32,
        min[1].floor() as i32,
        min[2].floor() as i32,
    ];
    let hi = [
        (max[0] - 1e-4).floor() as i32,
        (max[1] - 1e-4).floor() as i32,
        (max[2] - 1e-4).floor() as i32,
    ];

    for y in lo[1]..=hi[1] {
        for z in lo[2]..=hi[2] {
            for x in lo[0]..=hi[0] {
                if !world.is_solid(BlockPos::new(x, y, z)) {
                    continue;
                }
                // Push the body back out along the movement axis.
                if delta > 0.0 {
                    let block_min = match axis {
                        0 => x as f32,
                        1 => y as f32,
                        _ => z as f32,
                    };
                    let offset = match axis {
                        0 => half,
                        1 => height,
                        _ => half,
                    };
                    body.position[axis] = block_min - offset - 1e-3;
                } else {
                    let block_max = match axis {
                        0 => (x + 1) as f32,
                        1 => (y + 1) as f32,
                        _ => (z + 1) as f32,
                    };
                    let offset = match axis {
                        0 => half,
                        1 => 0.0,
                        _ => half,
                    };
                    body.position[axis] = block_max + offset + 1e-3;
                }
                if axis == 1 {
                    if delta < 0.0 {
                        body.on_ground = true;
                    }
                    body.velocity[1] = 0.0;
                } else {
                    body.velocity[axis] = 0.0;
                }
                return;
            }
        }
    }

    // Standing-on-ground detection when no vertical move happened this tick.
    if axis == 1 && delta <= 0.0 {
        let probe = BlockPos::new(
            body.position[0].floor() as i32,
            (body.position[1] - 0.05).floor() as i32,
            body.position[2].floor() as i32,
        );
        if world.is_solid(probe) {
            body.on_ground = true;
            body.velocity[1] = 0.0;
        }
    }
}

/// Returns true if the player's box at `position` intersects any solid block.
#[must_use]
pub fn box_collides<W: CollisionWorld>(
    position: [f32; 3],
    balance: &BalanceDef,
    world: &W,
) -> bool {
    let half = balance.player_half_width;
    let height = balance.player_height;
    let lo = [
        (position[0] - half).floor() as i32,
        position[1].floor() as i32,
        (position[2] - half).floor() as i32,
    ];
    let hi = [
        (position[0] + half - 1e-4).floor() as i32,
        (position[1] + height - 1e-4).floor() as i32,
        (position[2] + half - 1e-4).floor() as i32,
    ];
    for y in lo[1]..=hi[1] {
        for z in lo[2]..=hi[2] {
            for x in lo[0]..=hi[0] {
                if world.is_solid(BlockPos::new(x, y, z)) {
                    return true;
                }
            }
        }
    }
    false
}

/// A flat world where only blocks below `floor_y` are solid. Used by unit tests.
#[derive(Debug, Clone, Copy)]
pub struct FlatTestWorld {
    /// The Y coordinate of the first solid layer.
    pub floor_y: i32,
}

impl CollisionWorld for FlatTestWorld {
    fn is_solid(&self, pos: BlockPos) -> bool {
        pos.y < self.floor_y
    }
}

/// The default spawn position at the Base, standing on the surface.
#[must_use]
pub fn base_spawn(balance: &BalanceDef) -> [f32; 3] {
    let _ = balance;
    [0.5, SURFACE_Y as f32 + 0.05, 0.5]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn balance() -> BalanceDef {
        BalanceDef::default()
    }

    #[test]
    fn player_falls_onto_the_floor() {
        let world = FlatTestWorld { floor_y: 0 };
        let b = balance();
        let mut body = PlayerBody::new([0.5, 5.0, 0.5]);
        for _ in 0..120 {
            step_body(&mut body, &MoveInput::default(), &b, &world, 1.0 / 30.0);
        }
        assert!(body.on_ground, "player should land on the floor");
        assert!(
            (body.position[1] - 0.0).abs() < 0.05,
            "expected feet at y=0, got {}",
            body.position[1]
        );
    }

    #[test]
    fn player_cannot_walk_through_a_wall() {
        // Floor at y=0 plus a wall at x = 2.
        #[derive(Clone, Copy)]
        struct WallWorld;
        impl CollisionWorld for WallWorld {
            fn is_solid(&self, pos: BlockPos) -> bool {
                pos.y < 0 || (pos.x == 2 && pos.y < 2)
            }
        }
        let b = balance();
        let mut body = PlayerBody::new([0.5, 0.0, 0.5]);
        body.on_ground = true;
        // Face +X (yaw = -90 degrees) and run forward.
        let input = MoveInput {
            forward: 1.0,
            yaw: -std::f32::consts::FRAC_PI_2,
            ..Default::default()
        };
        for _ in 0..200 {
            step_body(&mut body, &input, &b, &WallWorld, 1.0 / 30.0);
        }
        assert!(
            body.position[0] < 2.0,
            "player walked through the wall to x={}",
            body.position[0]
        );
        assert!(
            body.position[0] > 1.0,
            "player should have reached the wall"
        );
    }

    #[test]
    fn jumping_leaves_the_ground_and_returns() {
        let world = FlatTestWorld { floor_y: 0 };
        let b = balance();
        let mut body = PlayerBody::new([0.5, 0.0, 0.5]);
        body.on_ground = true;
        let jump = MoveInput {
            jump: true,
            ..Default::default()
        };
        step_body(&mut body, &jump, &b, &world, 1.0 / 30.0);
        let peak_start = body.position[1];
        assert!(peak_start > 0.0, "jump did not lift the player");
        let idle = MoveInput::default();
        let mut peak = peak_start;
        for _ in 0..60 {
            step_body(&mut body, &idle, &b, &world, 1.0 / 30.0);
            peak = peak.max(body.position[1]);
        }
        assert!(peak > 1.0, "jump peak too low: {peak}");
        assert!(body.on_ground, "player did not land after jumping");
    }

    #[test]
    fn crouching_is_slower_than_walking() {
        let world = FlatTestWorld { floor_y: 0 };
        let b = balance();
        let run = |crouch: bool| {
            let mut body = PlayerBody::new([0.5, 0.0, 0.5]);
            body.on_ground = true;
            let input = MoveInput {
                forward: 1.0,
                crouch,
                ..Default::default()
            };
            for _ in 0..60 {
                step_body(&mut body, &input, &b, &world, 1.0 / 30.0);
            }
            body.position[2].abs()
        };
        let walking = run(false);
        let crouching = run(true);
        assert!(crouching < walking, "crouch {crouching} vs walk {walking}");
    }

    #[test]
    fn movement_is_deterministic() {
        let world = FlatTestWorld { floor_y: 0 };
        let b = balance();
        let input = MoveInput {
            forward: 1.0,
            right: 0.3,
            sprint: true,
            yaw: 0.7,
            ..Default::default()
        };
        let simulate = || {
            let mut body = PlayerBody::new([1.5, 3.0, 1.5]);
            for _ in 0..90 {
                step_body(&mut body, &input, &b, &world, 1.0 / 30.0);
            }
            body
        };
        assert_eq!(simulate(), simulate(), "movement must be deterministic");
    }

    #[test]
    fn look_direction_matches_yaw_convention() {
        let mut body = PlayerBody::new([0.0; 3]);
        body.yaw = 0.0;
        body.pitch = 0.0;
        let dir = body.look_direction();
        assert!((dir[2] + 1.0).abs() < 1e-5, "yaw 0 should look down -Z");
        body.yaw = std::f32::consts::FRAC_PI_2;
        let dir = body.look_direction();
        assert!(
            (dir[0] + 1.0).abs() < 1e-5,
            "yaw 90 degrees should look down -X"
        );
    }
}
