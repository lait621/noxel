//! Queries: ray casts, overlap tests and shape sweeps.
//!
//! Every query uses the spatial hash to gather candidates and then runs an
//! *analytic* test against the actual shape, so a result is never limited by
//! the broadphase's approximation. Candidates are sorted by [`BodyHandle`]
//! before testing, which makes ties resolve the same way on every run.

use noxel_core::math::{Aabb, Quat, Ray, Vec3};

use crate::body::{Body, BodyHandle, LAYER_ALL};
use crate::narrow::closest_features;
use crate::shape::ColliderShape;
use crate::world::PhysicsWorld;

/// Which bodies a query is allowed to see.
///
/// ```
/// use noxel_physics::{QueryFilter, LAYER_PLAYER};
///
/// // Only player-layer bodies, ignoring one specific body.
/// let f = QueryFilter::with_mask(LAYER_PLAYER).with_sensors(true);
/// assert!(f.include_sensors);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct QueryFilter {
    /// Layer bits to accept; a body matches when `body.layer & mask != 0`.
    pub mask: u32,
    /// A body to skip, usually the one casting the query.
    pub ignore: Option<BodyHandle>,
    /// Whether sensor bodies can be hit.
    pub include_sensors: bool,
    /// Whether static bodies can be hit.
    pub include_static: bool,
}

impl Default for QueryFilter {
    /// A filter that sees all solid, non-sensor geometry.
    ///
    /// This is deliberately *not* the derived all-zero value: a zero mask would
    /// reject every body, and excluding static geometry by default would make a
    /// line-of-sight ray pass straight through the level.
    fn default() -> Self {
        Self {
            mask: LAYER_ALL,
            ignore: None,
            include_sensors: false,
            include_static: true,
        }
    }
}

impl QueryFilter {
    /// Everything, including sensors.
    pub const ALL: Self = Self {
        mask: LAYER_ALL,
        ignore: None,
        include_sensors: true,
        include_static: true,
    };

    /// A filter restricted to `mask`, otherwise permissive.
    #[must_use]
    pub const fn with_mask(mask: u32) -> Self {
        Self {
            mask,
            ignore: None,
            include_sensors: true,
            include_static: true,
        }
    }

    /// Skips `handle` (usually the body casting the query).
    #[must_use]
    pub const fn ignoring(mut self, handle: BodyHandle) -> Self {
        self.ignore = Some(handle);
        self
    }

    /// Includes or excludes sensor bodies.
    #[must_use]
    pub const fn with_sensors(mut self, on: bool) -> Self {
        self.include_sensors = on;
        self
    }

    /// Includes or excludes static bodies.
    #[must_use]
    pub const fn with_static(mut self, on: bool) -> Self {
        self.include_static = on;
        self
    }

    /// True when `body` passes this filter.
    #[must_use]
    pub(crate) fn accepts(&self, body: &Body) -> bool {
        (body.layer & self.mask) != 0
            && (self.include_sensors || !body.is_sensor)
            && (self.include_static || !body.is_static())
    }
}

/// A ray hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RaycastHit {
    /// The body that was hit.
    pub body: BodyHandle,
    /// Distance along the ray in metres.
    pub distance: f32,
    /// World-space impact point.
    pub point: Vec3,
    /// World-space surface normal at the impact point.
    pub normal: Vec3,
    /// The body's `user_data`, echoed back for gameplay code.
    pub user_data: u64,
}

/// The result of a swept shape test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SweepHit {
    /// How far the shape may travel before touching, in metres. The value is
    /// already backed off by the collision skin, so it is safe to apply
    /// directly.
    pub distance: f32,
    /// Contact point on the obstacle.
    pub point: Vec3,
    /// Unit normal pointing from the obstacle towards the swept shape.
    pub normal: Vec3,
    /// The obstacle.
    pub body: BodyHandle,
}

/// The collision skin: how far above a surface a character comes to rest.
pub(crate) const SKIN: f32 = 0.005;

/// Iterations of conservative advancement before a sweep gives up.
const MAX_ADVANCE_STEPS: u32 = 12;

/// Distance at which conservative advancement declares contact.
const ADVANCE_CONVERGENCE: f32 = 1e-4;

/// Smallest approach speed worth advancing against.
const CLOSING_EPSILON: f32 = 1e-6;

impl PhysicsWorld {
    /// The nearest body a ray hits, or `None`.
    ///
    /// The ray direction is normalised internally, so `distance` is always in
    /// world units. A ray starting inside a shape reports a hit at distance `0`
    /// with a normal opposite the ray direction.
    #[must_use]
    pub fn raycast(&self, ray: &Ray, filter: QueryFilter) -> Option<RaycastHit> {
        let (origin, dir, max_t) = ray_parameters(ray)?;
        let mut candidates = Vec::new();
        self.gather_candidates(
            sweep_bounds(origin, dir, max_t, 0.0),
            &filter,
            &mut candidates,
        );
        let mut best: Option<RaycastHit> = None;
        for handle in candidates {
            let Some(body) = self.body(handle) else {
                continue;
            };
            let Some((distance, normal)) = ray_shape(
                &body.shape,
                body.position,
                body.rotation,
                origin,
                dir,
                max_t,
            ) else {
                continue;
            };
            if best.as_ref().is_none_or(|b| distance < b.distance) {
                best = Some(RaycastHit {
                    body: handle,
                    distance,
                    point: origin + dir * distance,
                    normal,
                    user_data: body.user_data,
                });
            }
        }
        best
    }

    /// Every ray hit, sorted by distance (ties broken by body handle).
    ///
    /// `out` is cleared first.
    pub fn raycast_all(&self, ray: &Ray, filter: QueryFilter, out: &mut Vec<RaycastHit>) {
        out.clear();
        let Some((origin, dir, max_t)) = ray_parameters(ray) else {
            return;
        };
        let mut candidates = Vec::new();
        self.gather_candidates(
            sweep_bounds(origin, dir, max_t, 0.0),
            &filter,
            &mut candidates,
        );
        for handle in candidates {
            let Some(body) = self.body(handle) else {
                continue;
            };
            if let Some((distance, normal)) = ray_shape(
                &body.shape,
                body.position,
                body.rotation,
                origin,
                dir,
                max_t,
            ) {
                out.push(RaycastHit {
                    body: handle,
                    distance,
                    point: origin + dir * distance,
                    normal,
                    user_data: body.user_data,
                });
            }
        }
        out.sort_unstable_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then_with(|| a.body.cmp(&b.body))
        });
    }

    /// Every body whose collider overlaps the sphere, sorted by handle.
    ///
    /// `out` is cleared first. Touching counts as overlapping.
    pub fn overlap_sphere(
        &self,
        center: Vec3,
        radius: f32,
        filter: QueryFilter,
        out: &mut Vec<BodyHandle>,
    ) {
        out.clear();
        if !center.is_finite() || !radius.is_finite() || radius < 0.0 {
            return;
        }
        let probe = ColliderShape::Sphere {
            radius: radius.max(1e-6),
        };
        let bounds = probe.aabb(center, Quat::IDENTITY);
        self.overlap_probe(&probe, center, Quat::IDENTITY, bounds, filter, out);
    }

    /// Every body whose collider overlaps the box, sorted by handle.
    ///
    /// `out` is cleared first. Touching counts as overlapping.
    pub fn overlap_aabb(&self, bounds: Aabb, filter: QueryFilter, out: &mut Vec<BodyHandle>) {
        out.clear();
        if !bounds.is_finite() {
            return;
        }
        let probe = ColliderShape::Box {
            half_extents: bounds.half_extents().max(Vec3::splat(1e-6)),
        };
        self.overlap_probe(&probe, bounds.center(), Quat::IDENTITY, bounds, filter, out);
    }

    /// Shared implementation of the two overlap queries.
    fn overlap_probe(
        &self,
        probe: &ColliderShape,
        center: Vec3,
        rotation: Quat,
        bounds: Aabb,
        filter: QueryFilter,
        out: &mut Vec<BodyHandle>,
    ) {
        self.gather_candidates(bounds, &filter, out);
        out.retain(|&handle| {
            let Some(body) = self.body(handle) else {
                return false;
            };
            let features = closest_features(
                probe,
                center,
                rotation,
                &body.shape,
                body.position,
                body.rotation,
            );
            features.separation <= 0.0
        });
    }

    /// Sweeps `shape` from `position` along `direction` and returns the first
    /// obstacle, if any.
    ///
    /// The sweep is a conservative-advancement shape cast: it never tunnels,
    /// however fast the shape moves, and it ignores obstacles it is moving away
    /// from or sliding along. The returned distance is backed off by [`SKIN`].
    pub(crate) fn sweep_shape(
        &self,
        shape: &ColliderShape,
        position: Vec3,
        rotation: Quat,
        direction: Vec3,
        max_distance: f32,
        filter: &QueryFilter,
    ) -> Option<SweepHit> {
        let dir = direction.normalize_or_zero();
        if dir == Vec3::ZERO
            || !position.is_finite()
            || !max_distance.is_finite()
            || max_distance <= 0.0
        {
            return None;
        }
        let end = position + dir * max_distance;
        let bounds = shape
            .aabb(position, rotation)
            .union(&shape.aabb(end, rotation));
        let mut candidates = Vec::new();
        self.gather_candidates(bounds.expanded(SKIN), filter, &mut candidates);

        let mut travelled = 0.0f32;
        let mut last: Option<SweepHit> = None;
        for _ in 0..MAX_ADVANCE_STEPS {
            let current = position + dir * travelled;
            let mut advance = f32::INFINITY;
            let mut nearest: Option<SweepHit> = None;
            for &handle in &candidates {
                let Some(body) = self.body(handle) else {
                    continue;
                };
                let features = closest_features(
                    shape,
                    current,
                    rotation,
                    &body.shape,
                    body.position,
                    body.rotation,
                );
                // `features.normal` points from the swept shape towards the
                // obstacle, so a positive dot product means closing in.
                let closing = dir.dot(features.normal);
                if closing <= CLOSING_EPSILON {
                    continue;
                }
                let step = (features.separation - SKIN) / closing;
                if step < advance {
                    advance = step;
                    nearest = Some(SweepHit {
                        distance: travelled,
                        point: features.point_b,
                        normal: -features.normal,
                        body: handle,
                    });
                }
            }
            let Some(hit) = nearest else {
                // Nothing ahead: the whole motion is clear.
                return None;
            };
            last = Some(hit);
            if advance <= ADVANCE_CONVERGENCE {
                return last.map(|hit| SweepHit {
                    distance: travelled,
                    ..hit
                });
            }
            travelled += advance;
            if travelled >= max_distance {
                return None;
            }
        }
        // Ran out of iterations: report the conservative position reached.
        last.map(|hit| SweepHit {
            distance: travelled.min(max_distance),
            ..hit
        })
    }
}

/// Normalised ray parameters, or `None` for a degenerate ray.
fn ray_parameters(ray: &Ray) -> Option<(Vec3, Vec3, f32)> {
    if !ray.origin.is_finite() {
        return None;
    }
    let dir = ray.dir.normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    let max_t = if ray.max_t.is_finite() {
        ray.max_t.max(0.0)
    } else {
        f32::INFINITY
    };
    Some((ray.origin, dir, max_t))
}

/// An AABB covering a segment of the ray, with a margin for grazing hits.
fn sweep_bounds(origin: Vec3, dir: Vec3, max_t: f32, margin: f32) -> Aabb {
    let reach = max_t.min(1.0e7);
    let end = origin + dir * reach;
    Aabb::new(origin, end).expanded(margin)
}

/// Analytic ray test against one placed shape.
///
/// Returns `(distance, world_normal)`.
fn ray_shape(
    shape: &ColliderShape,
    position: Vec3,
    rotation: Quat,
    origin: Vec3,
    dir: Vec3,
    max_t: f32,
) -> Option<(f32, Vec3)> {
    let inv = rotation.conjugate();
    let local_origin = inv.rotate_vec3(origin - position);
    let local_dir = inv.rotate_vec3(dir);
    let (t, normal) = shape.raycast_local(local_origin, local_dir, max_t)?;
    Some((t, rotation.rotate_vec3(normal)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::BodyDesc;

    fn world_with_floor() -> (PhysicsWorld, BodyHandle) {
        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        let floor = world.insert_static_aabb(
            Aabb::new(Vec3::new(-10.0, -1.0, -10.0), Vec3::new(10.0, 0.0, 10.0)),
            7,
        );
        (world, floor)
    }

    #[test]
    fn default_filter_sees_static_geometry() {
        let (world, floor) = world_with_floor();
        let ray = Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::DOWN);
        let hit = world.raycast(&ray, QueryFilter::default()).unwrap();
        assert_eq!(hit.body, floor);
        assert!((hit.distance - 5.0).abs() < 1e-4);
        assert!((hit.normal - Vec3::Y).length() < 1e-4);
        assert_eq!(hit.user_data, 7);
    }

    #[test]
    fn raycast_miss_returns_none() {
        let (world, _) = world_with_floor();
        let ray = Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::Y);
        assert!(world.raycast(&ray, QueryFilter::default()).is_none());
    }

    #[test]
    fn raycast_honours_max_t() {
        let (world, _) = world_with_floor();
        let ray = Ray::with_max_t(Vec3::new(0.0, 5.0, 0.0), Vec3::DOWN, 4.0);
        assert!(world.raycast(&ray, QueryFilter::default()).is_none());
    }

    #[test]
    fn raycast_ignore_skips_the_body() {
        let (world, floor) = world_with_floor();
        let ray = Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::DOWN);
        let filter = QueryFilter::default().ignoring(floor);
        assert!(world.raycast(&ray, filter).is_none());
    }

    #[test]
    fn raycast_hits_each_shape_at_the_right_distance() {
        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        let sphere = world.insert(
            BodyDesc::static_body(ColliderShape::Sphere { radius: 1.0 })
                .at(Vec3::new(0.0, 0.0, 0.0)),
        );
        let hit = world
            .raycast(
                &Ray::new(Vec3::new(5.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)),
                QueryFilter::default(),
            )
            .unwrap();
        assert_eq!(hit.body, sphere);
        assert!((hit.distance - 4.0).abs() < 1e-4);
        assert!((hit.normal - Vec3::X).length() < 1e-4);
    }

    #[test]
    fn raycast_hits_capsule_cylinder_and_box() {
        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        let capsule = world.insert(BodyDesc::static_body(ColliderShape::Capsule {
            radius: 0.5,
            half_height: 1.0,
        }));
        let hit = world
            .raycast(
                &Ray::new(Vec3::new(5.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)),
                QueryFilter::default(),
            )
            .unwrap();
        assert_eq!(hit.body, capsule);
        assert!((hit.distance - 4.5).abs() < 1e-4);

        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        let cylinder = world.insert(BodyDesc::static_body(ColliderShape::Cylinder {
            radius: 0.25,
            half_height: 1.0,
        }));
        let hit = world
            .raycast(
                &Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::DOWN),
                QueryFilter::default(),
            )
            .unwrap();
        assert_eq!(hit.body, cylinder);
        assert!((hit.distance - 4.0).abs() < 1e-4);
        assert!((hit.normal - Vec3::Y).length() < 1e-4);

        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        let boxy = world.insert(BodyDesc::static_body(ColliderShape::Box {
            half_extents: Vec3::splat(0.5),
        }));
        let hit = world
            .raycast(
                &Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::DOWN),
                QueryFilter::default(),
            )
            .unwrap();
        assert_eq!(hit.body, boxy);
        assert!((hit.distance - 4.5).abs() < 1e-4);
    }

    #[test]
    fn raycast_all_is_sorted_and_complete() {
        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        for y in [1.0f32, 3.0, 5.0] {
            world.insert(
                BodyDesc::static_body(ColliderShape::Sphere { radius: 0.25 })
                    .at(Vec3::new(0.0, y, 0.0)),
            );
        }
        let mut hits = Vec::new();
        world.raycast_all(
            &Ray::new(Vec3::new(0.0, 10.0, 0.0), Vec3::DOWN),
            QueryFilter::ALL,
            &mut hits,
        );
        assert_eq!(hits.len(), 3);
        assert!((hits[0].distance - 4.75).abs() < 1e-4, "{:?}", hits[0]);
        assert!((hits[1].distance - 6.75).abs() < 1e-4);
        assert!((hits[2].distance - 8.75).abs() < 1e-4);
        // Nearest first, regardless of insertion order.
        assert!(hits[0].distance < hits[1].distance && hits[1].distance < hits[2].distance);
    }

    #[test]
    fn raycast_respects_layer_masks() {
        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        let prop = world.insert(
            BodyDesc::static_body(ColliderShape::Sphere { radius: 0.5 })
                .with_layer(crate::body::LAYER_PROP, LAYER_ALL),
        );
        let ray = Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::DOWN);
        assert!(
            world
                .raycast(&ray, QueryFilter::with_mask(crate::body::LAYER_WORLD))
                .is_none()
        );
        let hit = world
            .raycast(&ray, QueryFilter::with_mask(crate::body::LAYER_PROP))
            .unwrap();
        assert_eq!(hit.body, prop);
    }

    #[test]
    fn sensors_are_excluded_unless_requested() {
        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        let trigger =
            world.insert(BodyDesc::static_body(ColliderShape::Sphere { radius: 1.0 }).as_sensor());
        let ray = Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::DOWN);
        assert!(world.raycast(&ray, QueryFilter::default()).is_none());
        let hit = world
            .raycast(&ray, QueryFilter::default().with_sensors(true))
            .unwrap();
        assert_eq!(hit.body, trigger);
    }

    #[test]
    fn overlap_sphere_finds_touching_bodies() {
        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        let near = world.insert(BodyDesc::static_body(ColliderShape::Sphere { radius: 0.5 }));
        let far = world.insert(
            BodyDesc::static_body(ColliderShape::Sphere { radius: 0.5 })
                .at(Vec3::new(10.0, 0.0, 0.0)),
        );
        let mut out = Vec::new();
        world.overlap_sphere(
            Vec3::new(0.0, 1.0, 0.0),
            0.5,
            QueryFilter::default(),
            &mut out,
        );
        assert_eq!(out, vec![near], "exactly touching counts");
        world.overlap_sphere(
            Vec3::new(0.0, 1.01, 0.0),
            0.5,
            QueryFilter::default(),
            &mut out,
        );
        assert!(out.is_empty());
        world.overlap_sphere(
            Vec3::new(10.0, 0.0, 0.0),
            0.5,
            QueryFilter::default(),
            &mut out,
        );
        assert_eq!(out, vec![far]);
    }

    #[test]
    fn overlap_aabb_matches_containment() {
        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        let inside = world.insert(
            BodyDesc::static_body(ColliderShape::Box {
                half_extents: Vec3::splat(0.25),
            })
            .at(Vec3::new(0.0, 0.0, 0.0)),
        );
        let outside = world.insert(
            BodyDesc::static_body(ColliderShape::Box {
                half_extents: Vec3::splat(0.25),
            })
            .at(Vec3::new(5.0, 0.0, 0.0)),
        );
        let mut out = Vec::new();
        world.overlap_aabb(
            Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0)),
            QueryFilter::default(),
            &mut out,
        );
        assert_eq!(out, vec![inside]);
        assert_ne!(out, vec![outside]);
    }

    #[test]
    fn sweep_reports_the_gap_to_a_wall() {
        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        world.insert_static_aabb(
            Aabb::new(Vec3::new(2.0, -1.0, -5.0), Vec3::new(3.0, 5.0, 5.0)),
            0,
        );
        let shape = ColliderShape::Sphere { radius: 0.5 };
        // Starting at x = 0 moving +X: the wall starts at x = 2, so the sphere
        // touches it at 1.5, i.e. after 1.5 - SKIN of travel.
        let hit = world
            .sweep_shape(
                &shape,
                Vec3::ZERO,
                Quat::IDENTITY,
                Vec3::X,
                10.0,
                &QueryFilter::default(),
            )
            .unwrap();
        assert!((hit.distance - (1.5 - SKIN)).abs() < 1e-3, "{hit:?}");
        assert!((hit.normal + Vec3::X).length() < 1e-3, "{hit:?}");
    }

    #[test]
    fn sweep_ignores_parallel_surfaces() {
        let (world, _) = world_with_floor();
        let shape = ColliderShape::Sphere { radius: 0.5 };
        // Resting on the floor (centre y = 0.505), sliding sideways: the floor
        // must not block the slide.
        let hit = world.sweep_shape(
            &shape,
            Vec3::new(0.0, 0.5 + SKIN, 0.0),
            Quat::IDENTITY,
            Vec3::X,
            3.0,
            &QueryFilter::default(),
        );
        assert!(hit.is_none(), "{hit:?}");
    }

    #[test]
    fn sweep_never_tunnels_a_thin_wall() {
        let mut world = PhysicsWorld::new(crate::config::PhysicsConfig::default());
        world.insert_static_aabb(
            Aabb::new(Vec3::new(0.9, -1.0, -1.0), Vec3::new(1.1, 1.0, 1.0)),
            0,
        );
        let shape = ColliderShape::Sphere { radius: 0.1 };
        let hit = world
            .sweep_shape(
                &shape,
                Vec3::new(-5.0, 0.0, 0.0),
                Quat::IDENTITY,
                Vec3::X,
                10.0,
                &QueryFilter::default(),
            )
            .unwrap();
        // The sphere (r = 0.1) touches the near face at x = 0.8, i.e. after
        // 5.8 m of travel, minus the skin.
        assert!((hit.distance - (5.8 - SKIN)).abs() < 1e-2, "{hit:?}");
    }
}
