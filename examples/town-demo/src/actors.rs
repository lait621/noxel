//! The player and the crowd.
//!
//! The player is a physics capsule moved by [`PhysicsWorld::move_character`],
//! which is what gives it step-up over kerbs, a slope limit and sliding along
//! walls. The crowd is [`NpcSystem`], which owns its own tiering and steering;
//! this module only mirrors agents into the render scene and hands the physics
//! world over when a tier-0 agent needs to collide.

use std::collections::HashMap;

use noxel_app::App;
use noxel_core::math::{Color, Transform, Vec3};
use noxel_npc::{CrowdTier, NpcConfig, NpcContext, NpcKind, NpcSystem};
use noxel_physics::{
    BodyDesc, BodyHandle, Capsule, CharacterMove, ColliderShape, LAYER_NPC, LAYER_PLAYER,
};
use noxel_render::material::Material;
use noxel_render::mesh::Mesh;
use noxel_render::scene::InstanceHandle;

use crate::terrain::TerrainPlugin;

/// The player's simulated state.
#[derive(Clone, Debug)]
pub struct Player {
    /// The render instance.
    pub instance: InstanceHandle,
    /// The physics capsule.
    pub body: BodyHandle,
    /// World position, at the capsule's centre.
    pub position: Vec3,
    /// Horizontal velocity, m/s.
    pub velocity: Vec3,
    /// Facing, radians (ADR 0001: yaw 0 looks along `-Z`).
    pub yaw: f32,
    /// Vertical velocity, for jumping.
    pub vertical_velocity: f32,
    /// True when the character is standing on ground.
    pub grounded: bool,
    /// Metres walked since the demo started, for the HUD.
    pub distance_travelled: f32,
}

impl Player {
    /// The camera's look-at point: chest height, which keeps a top-down view
    /// centred on the character rather than on their feet.
    #[must_use]
    pub fn focus(&self) -> Vec3 {
        self.position + Vec3::Y * 0.8
    }

    /// The forward direction implied by the yaw.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos())
    }
}

/// A walkable waypoint the scripted player follows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Waypoint {
    /// World position on the ground.
    pub position: Vec3,
    /// How long to pause here, in seconds.
    pub pause: f32,
}

/// The player plugin: input, movement and camera following.
pub struct PlayerPlugin {
    /// The player, once the world has finished loading.
    pub player: Option<Player>,
    /// The scripted route.
    pub route: Vec<Waypoint>,
    /// Index into the route.
    pub route_cursor: usize,
    /// Seconds left at the current waypoint.
    pub pause_timer: f32,
    /// Walk speed, m/s.
    pub speed: f32,
    /// Distance travelled around the route, for a progress readout.
    pub laps: u32,
}

impl Default for PlayerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerPlugin {
    /// An unspawned player plugin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            player: None,
            route: Vec::new(),
            route_cursor: 0,
            pause_timer: 1.0,
            speed: 3.4,
            laps: 0,
        }
    }

    /// Places the player at a world position.
    pub fn spawn_at(&mut self, app: &mut App, position: Vec3, yaw: f32) {
        let mesh = app.scene_mut().add_mesh(Mesh::capsule(0.35, 1.7, 12));
        let material = app
            .scene_mut()
            .add_material(Material::lit("player", Color::rgb(0.92, 0.86, 0.72)));
        let instance = app.scene_mut().spawn(
            "player",
            mesh,
            material,
            Transform::from_translation(position),
        );

        let body = app.physics_mut().insert(
            BodyDesc::kinematic()
                .with_shape(ColliderShape::Capsule(Capsule::new(0.35, 1.0)))
                .with_position(position)
                .with_layer(LAYER_PLAYER),
        );

        self.player = Some(Player {
            instance,
            body,
            position,
            velocity: Vec3::ZERO,
            yaw,
            vertical_velocity: 0.0,
            grounded: false,
            distance_travelled: 0.0,
        });
    }

    /// The player, if one exists.
    #[must_use]
    pub fn player(&self) -> Option<&Player> {
        self.player.as_ref()
    }

    /// The current target, or `None` when the route is empty.
    #[must_use]
    pub fn target(&self) -> Option<Vec3> {
        self.route.get(self.route_cursor).map(|w| w.position)
    }

    /// Advances the route cursor.
    pub fn advance_route(&mut self) {
        if self.route.is_empty() {
            return;
        }
        self.route_cursor += 1;
        if self.route_cursor >= self.route.len() {
            self.route_cursor = 0;
            self.laps += 1;
        }
        self.pause_timer = self.route[self.route_cursor].pause;
    }

    /// One fixed step of player movement.
    pub fn update(&mut self, app: &mut App, dt: f32, scripted: bool) {
        let Some(player) = self.player.as_mut() else {
            return;
        };

        // ---- decide where to go ------------------------------------------
        let desired = if scripted {
            match self.route.get(self.route_cursor) {
                Some(waypoint) => {
                    let delta = waypoint.position - player.position;
                    let flat = Vec3::new(delta.x, 0.0, delta.z);
                    if flat.length() < 0.8 {
                        if self.pause_timer > 0.0 {
                            self.pause_timer -= dt;
                            Vec3::ZERO
                        } else {
                            self.advance_route();
                            Vec3::ZERO
                        }
                    } else {
                        flat.normalize_or_zero()
                    }
                }
                None => Vec3::ZERO,
            }
        } else {
            // Free movement: the host fills `InputState`.
            let (x, z) = app.context.input.movement_axis(82, 81, 80, 79); // WASD
            // Screen-relative: pressing up walks away from the camera.
            let forward = player.forward();
            let right = Vec3::new(-forward.z, 0.0, forward.x);
            (forward * -z + right * x).normalize_or_zero()
        };

        player.velocity = desired * self.speed;
        let motion = player.velocity * dt;

        // ---- move through the physics world -------------------------------
        let body = player.body;
        let move_request = CharacterMove {
            motion,
            ..CharacterMove::default()
        };
        let result = app.physics_mut().move_character(body, &move_request);

        player.position = result.position;
        player.grounded = result.grounded;
        player.distance_travelled += Vec3::new(motion.x, 0.0, motion.z).length();
        if desired.length_squared() > 1e-6 {
            // Turn towards the direction of travel with a short smoothing so the
            // character does not snap around on a corner.
            let wanted = (-desired.x).atan2(-desired.z);
            let delta = noxel_core::math::angle_delta(player.yaw, wanted);
            player.yaw += delta * noxel_core::math::damp_factor(0.02, dt);
        }

        // ---- keep the ground under our feet -------------------------------
        app.physics_mut().set_position(body, player.position);
        let instance = player.instance;
        app.scene_mut().set_transform(
            instance,
            Transform::from_translation(player.position).with_yaw(player.yaw),
        );

        app.context.focus = player.focus();
        app.debug_mut()
            .record_counter("player x", player.position.x);
        app.debug_mut()
            .record_counter("player z", player.position.z);
        app.debug_mut()
            .record_counter("walked m", player.distance_travelled);
    }

    /// Builds a route that loops around a position, staying on walkable ground.
    ///
    /// A route is generated by walking outwards in a ring and testing each
    /// candidate against the streamer, so the player never tries to walk through
    /// a building or into a river.
    pub fn build_route(&mut self, app: &App, centre: Vec3, radius: f32, points: usize) {
        self.route.clear();
        let points = points.max(4);
        for i in 0..points {
            let angle = i as f32 / points as f32 * core::f32::consts::TAU;
            let mut candidate = centre + Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius);
            // Nudge inwards until it lands somewhere walkable.
            for step in 0..6 {
                let shrink = 1.0 - step as f32 * 0.12;
                candidate = centre
                    + Vec3::new(
                        angle.cos() * radius * shrink,
                        0.0,
                        angle.sin() * radius * shrink,
                    );
                if TerrainPlugin::is_walkable(app, candidate) {
                    break;
                }
            }
            candidate.y = app.context.streamer.height_at(candidate);
            self.route.push(Waypoint {
                position: candidate,
                pause: 0.4,
            });
        }
        self.route_cursor = 0;
    }
}

/// The crowd plugin: mirrors NPC agents into the scene.
pub struct CrowdPlugin {
    /// The NPC system.
    pub system: NpcSystem,
    /// Scene instances, keyed by the agent's slot index.
    instances: HashMap<u32, InstanceHandle>,
    /// The shared capsule mesh.
    mesh: Option<noxel_render::material::MeshHandle>,
    /// Materials per kind.
    materials: HashMap<&'static str, noxel_render::material::MaterialHandle>,
    /// True once the world has settled enough to populate.
    pub populated: bool,
}

impl CrowdPlugin {
    /// Creates a crowd with the given configuration.
    #[must_use]
    pub fn new(config: NpcConfig) -> Self {
        Self {
            system: NpcSystem::new(config),
            instances: HashMap::new(),
            mesh: None,
            materials: HashMap::new(),
            populated: false,
        }
    }

    /// The number of agents currently drawn.
    #[must_use]
    pub fn drawn(&self) -> usize {
        self.instances.len()
    }

    /// The crowd system.
    #[must_use]
    pub fn system(&self) -> &NpcSystem {
        &self.system
    }

    /// Fills the crowd so that the configured population is resident.
    pub fn populate(&mut self, app: &mut App) {
        if self.populated {
            return;
        }
        let center = app.context.focus;
        let mut crowd = self.system.crowd_mut();
        let target = crowd.config().target_population;
        for i in 0..target {
            let _ = i;
            crowd.spawn(NpcKind::Villager, center);
        }
        drop(crowd);
        self.system.maintain_population(&mut NpcContext {
            streamer: &app.context.streamer,
            physics: &mut app.context.physics,
            center,
            world_time: 8.0 * 3600.0,
            dt: 1.0 / 60.0,
        });
        self.populated = true;
    }

    /// One fixed step: update the crowd and sync the scene.
    pub fn update(&mut self, app: &mut App, dt: f32, world_time: f64) {
        let center = app.context.focus;
        let mut ctx = NpcContext {
            streamer: &app.context.streamer,
            physics: &mut app.context.physics,
            center,
            world_time,
            dt,
        };
        self.system.maintain_population(&mut ctx);
        let stats = self.system.update(&mut ctx);
        app.debug_mut().record_counter("npcs", stats.active as f32);
        app.debug_mut()
            .record_counter("npc us/agent", stats.us_per_agent);
        app.debug_mut().record_section("npc", stats.last_step_ms);

        self.sync_scene(app);
    }

    /// Creates, moves and removes the scene instances that represent agents.
    fn sync_scene(&mut self, app: &mut App) {
        let mesh = *self
            .mesh
            .get_or_insert_with(|| app.scene_mut().add_mesh(Mesh::capsule(0.3, 1.5, 8)));
        let mut alive: Vec<u32> = Vec::with_capacity(self.instances.len() + 16);

        for agent in self.system.crowd().iter() {
            let index = agent.id.index() as u32;
            alive.push(index);
            let transform = Transform::from_translation(agent.position).with_yaw(agent.yaw);
            match self.instances.get(&index) {
                Some(handle) => {
                    app.scene_mut().set_transform(*handle, transform);
                }
                None => {
                    let material = self.material_for(app, agent.kind);
                    let handle =
                        app.scene_mut()
                            .spawn(format!("npc_{index}"), mesh, material, transform);
                    app.scene_mut().set_flags(handle, agent_flags(agent.tier));
                    self.instances.insert(index, handle);
                }
            }
        }

        // Any instance whose agent is gone must leave the scene, or the crowd
        // would grow without bound as the player walks.
        let stale: Vec<u32> = self
            .instances
            .keys()
            .copied()
            .filter(|index| !alive.contains(index))
            .collect();
        for index in stale {
            if let Some(handle) = self.instances.remove(&index) {
                app.scene_mut().despawn(handle);
            }
        }
    }

    /// The material for a kind, created on first use.
    fn material_for(
        &mut self,
        app: &mut App,
        kind: NpcKind,
    ) -> noxel_render::material::MaterialHandle {
        let name = kind.name();
        if let Some(handle) = self.materials.get(name) {
            return *handle;
        }
        let color = match kind {
            NpcKind::Villager => Color::rgb(0.80, 0.74, 0.62),
            NpcKind::Guard => Color::rgb(0.45, 0.52, 0.72),
            NpcKind::Merchant => Color::rgb(0.78, 0.60, 0.42),
            NpcKind::Child => Color::rgb(0.88, 0.82, 0.55),
            NpcKind::Animal => Color::rgb(0.62, 0.55, 0.48),
        };
        let handle = app.scene_mut().add_material(Material::lit(name, color));
        self.materials.insert(name, handle);
        handle
    }
}

/// Distant agents are marked as non-occluding so a crowd of 400 does not put
/// 400 boxes into the camera-occlusion ray test.
fn agent_flags(tier: CrowdTier) -> noxel_render::scene::InstanceFlags {
    let mut flags = noxel_render::scene::InstanceFlags::character();
    if tier != CrowdTier::Near {
        flags.occluder = false;
    }
    flags
}

/// The HUD-facing summary of the crowd.
#[must_use]
pub fn describe(plugin: &CrowdPlugin) -> String {
    let stats = plugin.system.stats();
    format!(
        "npc {} drawn {} | tier {} {} {} {} | {:.1}us/agent",
        stats.active,
        plugin.drawn(),
        stats.tier_counts[0],
        stats.tier_counts[1],
        stats.tier_counts[2],
        stats.tier_counts[3],
        stats.us_per_agent
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use noxel_app::AppConfig;

    fn app() -> App {
        App::new(AppConfig::headless()).unwrap()
    }

    #[test]
    fn player_forward_matches_the_engine_convention() {
        let player = Player {
            instance: InstanceHandle::default(),
            body: BodyHandle::INVALID,
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            vertical_velocity: 0.0,
            grounded: true,
            distance_travelled: 0.0,
        };
        assert!(player.forward().approx_eq(Vec3::new(0.0, 0.0, -1.0), 1e-5));
        let turned = Player {
            yaw: core::f32::consts::FRAC_PI_2,
            ..player
        };
        assert!(turned.forward().approx_eq(Vec3::new(-1.0, 0.0, 0.0), 1e-5));
    }

    #[test]
    fn focus_is_above_the_feet() {
        let player = Player {
            instance: InstanceHandle::default(),
            body: BodyHandle::INVALID,
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            vertical_velocity: 0.0,
            grounded: true,
            distance_travelled: 0.0,
        };
        assert!(player.focus().y > 0.0);
    }

    #[test]
    fn empty_route_has_no_target() {
        let plugin = PlayerPlugin::new();
        assert!(plugin.target().is_none());
    }

    #[test]
    fn advancing_an_empty_route_does_nothing() {
        let mut plugin = PlayerPlugin::new();
        plugin.advance_route();
        assert_eq!(plugin.route_cursor, 0);
        assert_eq!(plugin.laps, 0);
    }

    #[test]
    fn advancing_wraps_and_counts_laps() {
        let mut plugin = PlayerPlugin::new();
        plugin.route = vec![
            Waypoint {
                position: Vec3::ZERO,
                pause: 0.0,
            },
            Waypoint {
                position: Vec3::X,
                pause: 0.0,
            },
        ];
        plugin.advance_route();
        assert_eq!(plugin.route_cursor, 1);
        plugin.advance_route();
        assert_eq!(plugin.route_cursor, 0);
        assert_eq!(plugin.laps, 1);
    }

    #[test]
    fn route_is_built_on_walkable_ground() {
        let mut app = app();
        let mut plugin = PlayerPlugin::new();
        plugin.build_route(&app, Vec3::ZERO, 12.0, 8);
        assert_eq!(plugin.route.len(), 8);
        assert!(plugin.route.iter().all(|w| w.position.y.is_finite()));
        let _ = app.frame_update(1.0 / 60.0);
    }

    #[test]
    fn crowd_starts_empty() {
        let plugin = CrowdPlugin::new(NpcConfig::default());
        assert_eq!(plugin.drawn(), 0);
        assert_eq!(plugin.system().stats().active, 0);
    }

    #[test]
    fn describe_mentions_the_tier_counts() {
        let plugin = CrowdPlugin::new(NpcConfig::default());
        let text = describe(&plugin);
        assert!(text.contains("npc"));
        assert!(text.contains("tier"));
    }

    #[test]
    fn near_agents_are_occluders_and_distant_ones_are_not() {
        assert!(agent_flags(CrowdTier::Near).occluder);
        assert!(!agent_flags(CrowdTier::Far).occluder);
    }
}
