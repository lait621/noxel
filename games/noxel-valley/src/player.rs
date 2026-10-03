//! The player: movement, facing, the tool swing, and which tile a click lands on.
//!
//! # Coordinates
//!
//! The player's position is in **world units**, where one unit is one tile, and
//! the centre of tile `(x, y)` is at `(x + 0.5, y + 0.5)`. Everything the
//! player touches converts to tile coordinates at the last moment, through
//! [`Player::aim_tile`], so there is exactly one place that can be off by half a
//! tile — and it is a function with a test.
//!
//! # Collision
//!
//! A tile grid rather than the physics crate. The world is a grid, the player is
//! a circle, and resolving against the four neighbouring tiles is four lines;
//! building rigid bodies for it would mean maintaining a second representation of
//! the same map, which is how the two drift apart. `noxel-physics` is the right
//! tool for a game whose world is not a grid; this one's is.

use noxel_core::math::Vec2;

use crate::config::{RUN_SPEED, TILE, TOOL_SWING_SECONDS, WALK_SPEED};
use crate::world::FarmMap;

/// Which way the player is looking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Facing {
    /// Towards the bottom of the screen.
    #[default]
    Down,
    /// Towards the top of the screen.
    Up,
    /// Towards the left.
    Left,
    /// Towards the right.
    Right,
}

impl Facing {
    /// The direction as a unit vector in tile space.
    #[must_use]
    pub const fn delta(self) -> (i32, i32) {
        match self {
            Self::Down => (0, 1),
            Self::Up => (0, -1),
            Self::Left => (-1, 0),
            Self::Right => (1, 0),
        }
    }

    /// The name used in character sprite regions.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Up => "up",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    /// The facing that best matches a movement vector.
    ///
    /// Ties go to the horizontal axis, because a player holding two keys is
    /// almost always moving diagonally and the horizontal component is the one
    /// that reads as "which way am I going" on a wide screen.
    #[must_use]
    pub fn from_vector(x: f32, y: f32) -> Self {
        if x.abs() >= y.abs() {
            if x < 0.0 { Self::Left } else { Self::Right }
        } else if y < 0.0 {
            Self::Up
        } else {
            Self::Down
        }
    }
}

/// The player character.
#[derive(Clone, Debug)]
pub struct Player {
    /// Centre position in tiles.
    pub position: Vec2,
    /// Which way they are looking.
    pub facing: Facing,
    /// Movement speed actually achieved last frame, for the walk animation.
    speed: f32,
    /// Seconds spent walking, for the animation phase.
    walk_phase: f32,
    /// Seconds left in the current tool swing, or zero.
    swing: f32,
    /// The tile the current swing is aimed at.
    swing_target: (i32, i32),
}

impl Default for Player {
    fn default() -> Self {
        Self {
            position: Vec2::new(13.5, 10.5),
            facing: Facing::Down,
            speed: 0.0,
            walk_phase: 0.0,
            swing: 0.0,
            swing_target: (0, 0),
        }
    }
}

impl Player {
    /// A player standing at a tile's centre.
    #[must_use]
    pub fn at(tile: (i32, i32)) -> Self {
        Self {
            position: Vec2::new(tile.0 as f32 + 0.5, tile.1 as f32 + 0.5),
            ..Self::default()
        }
    }

    /// The tile the player's feet are in.
    #[must_use]
    pub fn tile(&self) -> (i32, i32) {
        (
            self.position.x.floor() as i32,
            self.position.y.floor() as i32,
        )
    }

    /// The tile the player is facing, which is what a tool acts on.
    #[must_use]
    pub fn aim_tile(&self) -> (i32, i32) {
        let (dx, dy) = self.facing.delta();
        let tile = self.tile();
        (tile.0 + dx, tile.1 + dy)
    }

    /// Whether a swing is in progress.
    #[must_use]
    pub fn is_swinging(&self) -> bool {
        self.swing > 0.0
    }

    /// How far through the swing, `0..=1`, for the animation.
    #[must_use]
    pub fn swing_fraction(&self) -> f32 {
        if self.swing <= 0.0 {
            return 1.0;
        }
        1.0 - (self.swing / TOOL_SWING_SECONDS).clamp(0.0, 1.0)
    }

    /// The tile a swing was aimed at when it started.
    #[must_use]
    pub fn swing_target(&self) -> (i32, i32) {
        self.swing_target
    }

    /// Starts a tool swing at the current aim tile.
    pub fn start_swing(&mut self) {
        self.swing = TOOL_SWING_SECONDS;
        self.swing_target = self.aim_tile();
    }

    /// Which sprite frame to draw.
    ///
    /// Frame 0 is the idle pose, so a standing player does not shuffle. The walk
    /// cycle is a three-frame `1, 2, 3, 2` loop, which reads as a step rather
    /// than as a slide.
    #[must_use]
    pub fn frame(&self) -> u32 {
        if self.speed < 0.05 {
            return 0;
        }
        let phase = (self.walk_phase * 6.0) as u32 % 4;
        [1, 2, 3, 2][phase as usize]
    }

    /// The atlas region for the current pose.
    #[must_use]
    pub fn sprite(&self) -> String {
        format!("player_{}_{}", self.facing.key(), self.frame())
    }

    /// Moves the player, resolving against the map.
    ///
    /// Returns the distance actually travelled, so the caller can charge energy
    /// for walking rather than for pushing into a wall.
    pub fn update(&mut self, map: &FarmMap, axis: (f32, f32), running: bool, dt: f32) -> f32 {
        self.swing = (self.swing - dt).max(0.0);

        // Normalise so diagonals are not faster than the axes. Without this a
        // player crossing the farm diagonally arrives 41% sooner.
        let length = (axis.0 * axis.0 + axis.1 * axis.1).sqrt();
        let (mut dx, mut dy) = if length > 1e-4 {
            (axis.0 / length, axis.1 / length)
        } else {
            (0.0, 0.0)
        };

        if dx.abs() > 1e-4 || dy.abs() > 1e-4 {
            self.facing = Facing::from_vector(dx, dy);
        }
        // A swinging tool roots the player, which is what makes the swing read
        // as an action rather than as decoration.
        if self.is_swinging() {
            dx = 0.0;
            dy = 0.0;
        }

        let speed = if running { RUN_SPEED } else { WALK_SPEED };
        let step = speed * dt;
        let before = self.position;
        self.move_axis(map, dx * step, dy * step);

        let travelled = (self.position.x - before.x).hypot(self.position.y - before.y);
        self.speed = travelled / dt.max(1e-5);
        self.walk_phase = if self.speed > 0.05 {
            self.walk_phase + dt
        } else {
            0.0
        };
        travelled
    }

    /// Moves along one axis at a time, so sliding along a wall works.
    ///
    /// Resolving both axes at once is what makes a player stick on a corner
    /// instead of sliding past it, and it is the single most noticeable
    /// difference between movement that feels good and movement that does not.
    fn move_axis(&mut self, map: &FarmMap, dx: f32, dy: f32) {
        const RADIUS: f32 = 0.32;
        if dx != 0.0 {
            let next = self.position.x + dx;
            let probe = next + RADIUS * dx.signum();
            let tile_y = self.position.y.floor() as i32;
            let blocked = !map.is_walkable(probe.floor() as i32, tile_y);
            if !blocked {
                self.position.x = next;
            }
        }
        if dy != 0.0 {
            let next = self.position.y + dy;
            let probe = next + RADIUS * dy.signum();
            let tile_x = self.position.x.floor() as i32;
            let blocked = !map.is_walkable(tile_x, probe.floor() as i32);
            if !blocked {
                self.position.y = next;
            }
        }
        self.clamp_to_map(map);
    }

    /// Keeps the player inside the map even if a resolution rounding error put
    /// them a hair outside.
    fn clamp_to_map(&mut self, map: &FarmMap) {
        let max_x = map.width() as f32 - 0.5;
        let max_y = map.height() as f32 - 0.5;
        self.position.x = self.position.x.clamp(0.5, max_x.max(0.5));
        self.position.y = self.position.y.clamp(0.5, max_y.max(0.5));
    }

    /// The world position to aim the camera at.
    #[must_use]
    pub fn camera_focus(&self) -> Vec2 {
        self.position
    }

    /// Converts a screen-space point to the tile under it.
    ///
    /// This is the bridge between the UI, which works in framebuffer pixels, and
    /// the world, which works in tiles. `screen` is the point in framebuffer
    /// pixels, `camera_center` is the world position at the centre of the view,
    /// and `viewport` is the framebuffer's size.
    #[must_use]
    pub fn screen_to_tile(
        screen: (i32, i32),
        camera_center: Vec2,
        viewport: (u32, u32),
    ) -> (i32, i32) {
        let half_w = viewport.0 as f32 / 2.0;
        let half_h = viewport.1 as f32 / 2.0;
        let world_x = camera_center.x + (screen.0 as f32 - half_w) / TILE as f32;
        let world_y = camera_center.y + (screen.1 as f32 - half_h) / TILE as f32;
        (world_x.floor() as i32, world_y.floor() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::build_farm;

    #[test]
    fn a_player_at_a_tile_stands_in_its_centre() {
        let player = Player::at((4, 7));
        assert_eq!(player.tile(), (4, 7));
        assert_eq!(player.position, Vec2::new(4.5, 7.5));
    }

    #[test]
    fn the_aim_tile_is_the_one_in_front() {
        let mut player = Player::at((5, 5));
        player.facing = Facing::Down;
        assert_eq!(player.aim_tile(), (5, 6));
        player.facing = Facing::Up;
        assert_eq!(player.aim_tile(), (5, 4));
        player.facing = Facing::Left;
        assert_eq!(player.aim_tile(), (4, 5));
        player.facing = Facing::Right;
        assert_eq!(player.aim_tile(), (6, 5));
    }

    #[test]
    fn moving_turns_the_player_to_face_the_movement() {
        let map = build_farm();
        // The open field: dirt, and the scatter only places props on grass.
        let mut player = Player::at((25, 20));
        player.update(&map, (1.0, 0.0), false, 1.0 / 60.0);
        assert_eq!(player.facing, Facing::Right);
        player.update(&map, (-1.0, 0.0), false, 1.0 / 60.0);
        assert_eq!(player.facing, Facing::Left);
        player.update(&map, (0.0, -1.0), false, 1.0 / 60.0);
        assert_eq!(player.facing, Facing::Up);
        player.update(&map, (0.0, 1.0), false, 1.0 / 60.0);
        assert_eq!(player.facing, Facing::Down);
    }

    #[test]
    fn a_diagonal_is_not_faster_than_a_straight_line() {
        // The classic bug: not normalising the input, so W+D crosses the farm
        // 41% faster than W. Measured in the open field, where neither player
        // can hit anything and stop early — an earlier version of this test ran
        // in the plaza, hit a wall on the second step, and asserted about two
        // players who had not moved.
        let map = build_farm();
        let mut straight = Player::at((25, 20));
        let mut diagonal = Player::at((25, 20));
        for _ in 0..45 {
            straight.update(&map, (1.0, 0.0), false, 1.0 / 60.0);
            diagonal.update(&map, (1.0, 1.0), false, 1.0 / 60.0);
        }
        let straight_distance = (straight.position.x - 25.5).hypot(straight.position.y - 20.5);
        let diagonal_distance = (diagonal.position.x - 25.5).hypot(diagonal.position.y - 20.5);
        assert!(straight_distance > 1.0, "the straight walker never moved");
        assert!(
            (straight_distance - diagonal_distance).abs() < 0.2,
            "straight {straight_distance:.3} vs diagonal {diagonal_distance:.3}"
        );
    }

    #[test]
    fn the_player_never_ends_up_inside_something_solid() {
        // Walk hard in every direction from the open field and check the
        // invariant each time, rather than picking one route and one obstacle.
        let map = build_farm();
        for axis in [
            (0.0, 1.0),
            (0.0, -1.0),
            (1.0, 0.0),
            (-1.0, 0.0),
            (1.0, 1.0),
            (-1.0, -1.0),
        ] {
            let mut player = Player::at((25, 20));
            for _ in 0..600 {
                player.update(&map, axis, true, 1.0 / 60.0);
            }
            let tile = player.tile();
            assert!(
                map.is_walkable(tile.0, tile.1),
                "walking {axis:?} ended inside a solid tile at {tile:?}"
            );
        }
    }

    #[test]
    fn the_starting_position_is_walkable() {
        // A player who spawns inside a tree cannot move and does not know why.
        let map = build_farm();
        let player = Player::default();
        let tile = player.tile();
        assert!(
            map.is_walkable(tile.0, tile.1),
            "the player starts inside a solid tile at {tile:?}"
        );
    }

    #[test]
    fn the_player_cannot_leave_the_map() {
        let map = build_farm();
        let mut player = Player::at((25, 20));
        for _ in 0..900 {
            player.update(&map, (-1.0, -1.0), true, 1.0 / 60.0);
        }
        assert!(
            player.position.x >= 0.5 && player.position.y >= 0.5,
            "{:?}",
            player.position
        );
        assert!(player.position.x < map.width() as f32);
        assert!(player.position.y < map.height() as f32);
    }

    #[test]
    fn walking_slides_along_a_wall_instead_of_sticking() {
        // Resolving both axes at once is what makes a player jam on a corner.
        // The search is the test being honest about the farm: the scatter is
        // procedural, so which tiles are blocked is not something to hard-code.
        let map = build_farm();
        let mut start = None;
        for y in 3..14 {
            for x in 3..20 {
                if !map.is_walkable(x + 1, y) && map.is_walkable(x, y) && map.is_walkable(x, y + 2)
                {
                    start = Some((x, y));
                    break;
                }
            }
            if start.is_some() {
                break;
            }
        }
        let (x, y) = start.expect("the farm has a wall to slide along");
        let mut player = Player::at((x, y));
        let before_y = player.position.y;
        // Push diagonally into the wall and down.
        for _ in 0..40 {
            player.update(&map, (1.0, 1.0), false, 1.0 / 60.0);
        }
        assert!(
            player.position.y > before_y + 0.2,
            "the player stuck on the wall instead of sliding: {before_y} -> {}",
            player.position.y
        );
    }

    #[test]
    fn a_swing_roots_the_player_and_expires() {
        let map = build_farm();
        let mut player = Player::at((25, 20));
        player.start_swing();
        assert!(player.is_swinging());
        let before = player.position;
        player.update(&map, (1.0, 0.0), true, 1.0 / 60.0);
        assert_eq!(player.position, before, "a swinging player must not slide");

        for _ in 0..40 {
            player.update(&map, (0.0, 0.0), false, 1.0 / 60.0);
        }
        assert!(!player.is_swinging(), "the swing never ended");
    }

    #[test]
    fn the_swing_target_is_captured_when_it_starts() {
        // Turning mid-swing must not move the tool: the player aimed, and the
        // aim should be what the tool acts on.
        let mut player = Player::at((5, 5));
        player.facing = Facing::Right;
        player.start_swing();
        assert_eq!(player.swing_target(), (6, 5));
        player.facing = Facing::Left;
        assert_eq!(player.swing_target(), (6, 5));
    }

    #[test]
    fn the_idle_pose_is_frame_zero_and_walking_cycles() {
        let map = build_farm();
        let mut player = Player::at((25, 20));
        assert_eq!(player.frame(), 0, "a standing player must not shuffle");
        let mut seen = std::collections::HashSet::new();
        for _ in 0..60 {
            player.update(&map, (1.0, 0.0), false, 1.0 / 60.0);
            seen.insert(player.frame());
        }
        assert!(seen.len() > 1, "the walk cycle never advanced: {seen:?}");
        assert!(!seen.contains(&0), "walking should not use the idle pose");
    }

    #[test]
    fn screen_to_tile_centres_the_view_on_the_camera() {
        let viewport = (480, 270);
        let centre = Vec2::new(20.0, 15.0);
        // The middle of the screen is the camera's world position.
        assert_eq!(
            Player::screen_to_tile((240, 135), centre, viewport),
            (20, 15)
        );
        // One tile to the right is 16 pixels to the right.
        assert_eq!(
            Player::screen_to_tile((240 + 16, 135), centre, viewport),
            (21, 15)
        );
        // And 8 pixels is still inside the same tile, because tiles are 16 wide.
        assert_eq!(
            Player::screen_to_tile((240 + 8, 135), centre, viewport),
            (20, 15)
        );
        // Up the screen is towards smaller y.
        assert_eq!(
            Player::screen_to_tile((240, 135 - 16), centre, viewport),
            (20, 14)
        );
    }
}
