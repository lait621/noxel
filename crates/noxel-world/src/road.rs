//! Roads: the macro lattice that ties the world together.
//!
//! The network is generated, never authored. A road runs along every
//! `road_grid_chunks`-th chunk boundary, offset along the boundary by a jitter
//! drawn from `(seed, boundary index)` — so it is straight, reproducible and
//! *connected by construction*: every vertical road crosses every horizontal
//! one, which makes the whole lattice a single graph without any path-finding
//! at generation time.
//!
//! The same lattice is available in closed form through [`macro_line_x`] and
//! [`macro_line_z`], which is what lets the terrain height function carve a road
//! without building (or storing) the segment list first.
//!
//! ```
//! use noxel_core::math::{ChunkPos, Vec3};
//! use noxel_world::WorldConfig;
//! use noxel_world::road::{RoadNetwork, macro_line_x};
//!
//! let config = WorldConfig::new(1);
//! let net = RoadNetwork::generate(&config, ChunkPos::new(0, 0), ChunkPos::new(4, 4));
//! assert!(!net.segments.is_empty());
//!
//! // A point on a lattice line is on a road.
//! let x = macro_line_x(&config, 0);
//! assert!(net.is_on_road(Vec3::new(x, 0.0, 16.0), 0.0));
//! ```

use noxel_core::math::noise::distance_to_segment_2d;
use noxel_core::math::{Aabb, ChunkPos, Vec2, Vec3};
use noxel_core::rng::RngStream;

use crate::r#gen::WorldConfig;

/// A straight piece of road, in world space.
///
/// Towns and the macro lattice both produce these, so the query helpers here are
/// shared by prop rejection, tile stamping and the road-distance query.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadSegment {
    /// Start point, on the road surface.
    pub from: Vec3,
    /// End point, on the road surface.
    pub to: Vec3,
    /// Full width of the paved surface, in metres.
    pub width: f32,
    /// True for the macro lattice and for a town's main street; false for the
    /// smaller connector streets.
    pub is_main: bool,
}

impl RoadSegment {
    /// Builds a segment. `width` is sanitised to a non-negative, finite value.
    #[must_use]
    pub fn new(from: Vec3, to: Vec3, width: f32, is_main: bool) -> Self {
        Self {
            from,
            to,
            width: if width.is_finite() {
                width.max(0.0)
            } else {
                0.0
            },
            is_main,
        }
    }

    /// Length of the segment in the XZ plane, in metres.
    ///
    /// Roads are laid out on the ground plane; a vertical difference between
    /// the endpoints is a grade, not extra length.
    #[must_use]
    pub fn length(&self) -> f32 {
        xz(self.to).distance(xz(self.from))
    }

    /// Half the paved width.
    #[must_use]
    pub fn half_width(&self) -> f32 {
        self.width * 0.5
    }

    /// Unit direction in the XZ plane, or `+X` for a degenerate segment.
    #[must_use]
    pub fn direction(&self) -> Vec3 {
        let d = Vec3::new(self.to.x - self.from.x, 0.0, self.to.z - self.from.z);
        d.try_normalize().unwrap_or(Vec3::new(1.0, 0.0, 0.0))
    }

    /// True when the two endpoints coincide.
    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        self.length() <= 1e-4
    }

    /// The point on the centre line closest to `p`, in the XZ plane.
    ///
    /// The returned point's height is interpolated along the segment, so a
    /// caller can place something on the road surface directly.
    #[must_use]
    pub fn closest_point(&self, p: Vec3) -> Vec3 {
        let a = xz(self.from);
        let b = xz(self.to);
        let q = xz(p);
        let ab = b - a;
        let len_sq = ab.length_squared();
        let t = if len_sq < 1e-9 {
            0.0
        } else {
            ((q - a).dot(ab) / len_sq).clamp(0.0, 1.0)
        };
        Vec3::new(
            a.x + ab.x * t,
            self.from.y + (self.to.y - self.from.y) * t,
            a.y + ab.y * t,
        )
    }

    /// XZ distance from `p` to the centre line.
    #[must_use]
    pub fn distance_to(&self, p: Vec3) -> f32 {
        distance_to_segment_2d(xz(p), xz(self.from), xz(self.to))
    }

    /// The segment's world bounds, grown by `margin` and by half its width.
    ///
    /// The Y range covers both endpoints with a small pad so the box is never
    /// degenerate in Y: an empty AABB breaks culling and collision code that
    /// assumes a positive extent.
    #[must_use]
    pub fn aabb(&self, margin: f32) -> Aabb {
        let margin = if margin.is_finite() {
            margin.max(0.0)
        } else {
            0.0
        };
        let half = self.half_width();
        let pad = margin + half;
        let y_lo = self.from.y.min(self.to.y) - 1.0;
        let y_hi = self.from.y.max(self.to.y) + 1.0;
        Aabb::from_footprint(
            self.from.x.min(self.to.x) - pad,
            self.from.z.min(self.to.z) - pad,
            self.from.x.max(self.to.x) + pad,
            self.from.z.max(self.to.z) + pad,
            y_lo,
            y_hi - y_lo,
        )
    }

    /// The piece of this road that lies inside an XZ box, or `None` when it
    /// misses entirely.
    ///
    /// Used to hand each chunk the part of a road that crosses it, so the pieces
    /// tile the network exactly and neighbouring chunks never report the same
    /// pavement twice. Heights are interpolated along the clipped piece, and the
    /// width is preserved.
    #[must_use]
    pub fn clip_to_aabb(&self, bounds: &Aabb) -> Option<Self> {
        let (t0, t1) = clip_parameters(
            xz(self.from),
            xz(self.to),
            Vec2::new(bounds.min.x, bounds.min.z),
            Vec2::new(bounds.max.x, bounds.max.z),
        )?;
        let from = self.from.lerp(self.to, t0);
        let to = self.from.lerp(self.to, t1);
        let clipped = Self {
            from,
            to,
            width: self.width,
            is_main: self.is_main,
        };
        if clipped.length() <= 1e-4 {
            return None;
        }
        Some(clipped)
    }
}

/// Effective spacing between macro roads, in metres.
///
/// `road_grid_chunks` is clamped to at least one chunk so a hand-edited config
/// cannot produce a division by zero.
#[must_use]
pub fn macro_spacing(config: &WorldConfig) -> f32 {
    let chunks = config.road_grid_chunks.max(1) as f32;
    (chunks * config.chunk_world_size()).max(config.chunk_world_size())
}

/// World X of the vertical macro road at lattice index `index`.
///
/// Vertical roads run along Z; two vertical roads are `macro_spacing()` apart
/// before jitter.
#[must_use]
pub fn macro_line_x(config: &WorldConfig, index: i32) -> f32 {
    let spacing = macro_spacing(config);
    let jitter = road_jitter(config, index, 0);
    index as f32 * spacing + jitter
}

/// World Z of the horizontal macro road at lattice index `index`.
#[must_use]
pub fn macro_line_z(config: &WorldConfig, index: i32) -> f32 {
    let spacing = macro_spacing(config);
    let jitter = road_jitter(config, index, 1);
    index as f32 * spacing + jitter
}

/// The deterministic offset of a lattice line from its nominal position.
///
/// `axis` is `0` for vertical (constant X) and `1` for horizontal (constant Z).
/// `road_jitter` is the fraction of a chunk the line may move.
#[must_use]
pub fn road_jitter(config: &WorldConfig, index: i32, axis: i32) -> f32 {
    let amount = if config.road_jitter.is_finite() {
        config.road_jitter.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let stream = RngStream::for_chunk(config.seed, "road/jitter", index, axis);
    stream.rng().range_f32(-1.0, 1.0) * amount * config.chunk_world_size()
}

/// The macro lattice index whose line is nearest to `coordinate`.
///
/// Clamped so a huge or non-finite coordinate cannot overflow the `i32`
/// multiply that turns an index back into a world position.
#[must_use]
pub fn nearest_line_index(config: &WorldConfig, coordinate: f32) -> i32 {
    let spacing = macro_spacing(config);
    let raw = coordinate / spacing;
    if !raw.is_finite() {
        return 0;
    }
    // 4_000_000 * 256 m is still comfortably inside f32's exact-integer range.
    raw.round().clamp(-4_000_000.0, 4_000_000.0) as i32
}

/// A world vector's XZ components, as a `Vec2` whose `y` is world Z.
///
/// `Vec3::truncate` drops Z and keeps XY, which is the wrong plane for a
/// top-down world; this is the conversion the road maths actually wants.
#[inline]
#[must_use]
fn xz(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z)
}

/// Liang–Barsky clip of the segment `a`–`b` against an XZ box.
///
/// Returns the two parameters along the segment that lie inside the box, or
/// `None` when the segment misses it. Degenerate boxes are treated as misses
/// rather than as everything.
#[must_use]
fn clip_parameters(a: Vec2, b: Vec2, min: Vec2, max: Vec2) -> Option<(f32, f32)> {
    let (ax, az) = (a.x, a.y);
    let (bx, bz) = (b.x, b.y);
    let (x0, z0) = (min.x, min.y);
    let (x1, z1) = (max.x, max.y);
    if x1 <= x0 || z1 <= z0 {
        return None;
    }
    let dx = bx - ax;
    let dz = bz - az;
    if !dx.is_finite() || !dz.is_finite() {
        return None;
    }
    let mut t0 = 0.0f32;
    let mut t1 = 1.0f32;
    for (p, q) in [(-dx, ax - x0), (dx, x1 - ax), (-dz, az - z0), (dz, z1 - az)] {
        if p.abs() < 1e-9 {
            // Parallel to this edge: inside only if the origin already is.
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return None;
            }
            if r > t0 {
                t0 = r;
            }
        } else {
            if r < t0 {
                return None;
            }
            if r < t1 {
                t1 = r;
            }
        }
    }
    if t1 < t0 {
        return None;
    }
    Some((t0, t1))
}

/// A set of road segments, queried by distance and by bounds.
#[derive(Clone, Debug, Default)]
pub struct RoadNetwork {
    /// Every segment in the network, in generation order.
    pub segments: Vec<RoadSegment>,
}

impl RoadNetwork {
    /// An empty network.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the macro road lattice covering `min_chunk..=max_chunk`.
    ///
    /// The lattice is extended one cell beyond the requested region on every
    /// side so the region is crossed by complete roads rather than by stubs.
    /// `min_chunk` and `max_chunk` are ordered internally, and the segment count
    /// is capped (see [`RoadNetwork::MAX_SEGMENTS`]) so a caller that asks for a
    /// continent-sized region gets a truncated network instead of an allocation
    /// failure.
    #[must_use]
    pub fn generate(config: &WorldConfig, min_chunk: ChunkPos, max_chunk: ChunkPos) -> Self {
        let spacing = macro_spacing(config);
        let cs = config.chunk_world_size();
        let width = config.road_width_tiles as f32 * config.tile_size;
        let (lo, hi) = (min_chunk.min(max_chunk), min_chunk.max(max_chunk));

        let i0 = nearest_line_index(config, lo.x as f32 * cs) - 1;
        let i1 = nearest_line_index(config, (hi.x + 1) as f32 * cs) + 1;
        let j0 = nearest_line_index(config, lo.y as f32 * cs) - 1;
        let j1 = nearest_line_index(config, (hi.y + 1) as f32 * cs) + 1;

        let z_start = j0 as f32 * spacing;
        let z_end = (j1 + 1) as f32 * spacing;
        let x_start = i0 as f32 * spacing;
        let x_end = (i1 + 1) as f32 * spacing;

        let mut segments = Vec::new();
        if i1 >= i0 && j1 >= j0 {
            let estimate = ((i1 - i0 + 1) as usize + (j1 - j0 + 1) as usize).min(4096);
            segments.reserve(estimate);
        }
        // Vertical roads (constant X, running along Z).
        for i in i0..=i1 {
            if segments.len() >= Self::MAX_SEGMENTS {
                break;
            }
            let x = macro_line_x(config, i);
            segments.push(RoadSegment::new(
                Vec3::new(x, 0.0, z_start),
                Vec3::new(x, 0.0, z_end),
                width,
                true,
            ));
        }
        // Horizontal roads (constant Z, running along X).
        for j in j0..=j1 {
            if segments.len() >= Self::MAX_SEGMENTS {
                break;
            }
            let z = macro_line_z(config, j);
            segments.push(RoadSegment::new(
                Vec3::new(x_start, 0.0, z),
                Vec3::new(x_end, 0.0, z),
                width,
                true,
            ));
        }
        Self { segments }
    }

    /// The segments intersecting `bounds`.
    ///
    /// The returned iterator borrows only the network, so it can outlive the
    /// bounds value it was filtered by — handy for
    /// `net.segments_in(&chunk.bounds())`.
    pub fn segments_in<'a>(&'a self, bounds: &Aabb) -> impl Iterator<Item = &'a RoadSegment> + 'a {
        let bounds = *bounds;
        self.segments
            .iter()
            .filter(move |segment| segment.aabb(0.0).intersects(&bounds))
    }

    /// XZ distance from `p` to the nearest road centre line.
    ///
    /// Returns [`f32::INFINITY`] for an empty network.
    #[must_use]
    pub fn distance_to_nearest(&self, p: Vec3) -> f32 {
        let mut best = f32::INFINITY;
        for segment in &self.segments {
            let d = segment.distance_to(p);
            if d < best {
                best = d;
            }
        }
        best
    }

    /// True when `p` lies on a road, allowing `extra` metres of slack beyond the
    /// paved half width.
    #[must_use]
    pub fn is_on_road(&self, p: Vec3, extra: f32) -> bool {
        let extra = if extra.is_finite() {
            extra.max(0.0)
        } else {
            0.0
        };
        self.segments
            .iter()
            .any(|segment| segment.distance_to(p) <= segment.half_width() + extra)
    }

    /// True when nothing is in the network.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Number of segments.
    #[must_use]
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Sum of every segment's length, in metres.
    #[must_use]
    pub fn total_length(&self) -> f32 {
        self.segments.iter().map(RoadSegment::length).sum()
    }

    /// Approximate heap footprint in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        core::mem::size_of::<Self>()
            + self.segments.capacity() * core::mem::size_of::<RoadSegment>()
    }

    /// Upper bound on how many segments [`RoadNetwork::generate`] will build.
    pub const MAX_SEGMENTS: usize = 200_000;
}

/// The four lattice lines that can reach a point, nearest first.
///
/// Shared by the analytic road carve and by anything that wants to reason about
/// "the road I am standing on" without building a network: index 0 is the
/// vertical line's X, index 1 the horizontal line's Z.
#[must_use]
pub(crate) fn nearest_lines(config: &WorldConfig, p: Vec3) -> (f32, f32) {
    let i = nearest_line_index(config, p.x);
    let j = nearest_line_index(config, p.z);
    (macro_line_x(config, i), macro_line_z(config, j))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> WorldConfig {
        WorldConfig::new(7)
    }

    #[test]
    fn segment_length_is_xz() {
        let s = RoadSegment::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(3.0, 100.0, 4.0),
            2.0,
            true,
        );
        assert!((s.length() - 5.0).abs() < 1e-4);
        assert_eq!(s.half_width(), 1.0);
    }

    #[test]
    fn degenerate_segment_is_flagged() {
        let s = RoadSegment::new(Vec3::ZERO, Vec3::ZERO, 3.0, false);
        assert!(s.is_degenerate());
        assert_eq!(s.direction(), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(s.distance_to(Vec3::new(0.0, 0.0, 5.0)), 5.0);
    }

    #[test]
    fn closest_point_clamps_to_the_ends() {
        let s = RoadSegment::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            2.0,
            true,
        );
        assert!((s.closest_point(Vec3::new(5.0, 3.0, 4.0)).x - 5.0).abs() < 1e-5);
        assert!((s.closest_point(Vec3::new(-5.0, 0.0, 0.0)).x - 0.0).abs() < 1e-5);
        assert!((s.closest_point(Vec3::new(50.0, 0.0, 0.0)).x - 10.0).abs() < 1e-5);
        assert_eq!(s.distance_to(Vec3::new(5.0, 0.0, 4.0)), 4.0);
    }

    #[test]
    fn closest_point_interpolates_height() {
        let s = RoadSegment::new(
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(10.0, 6.0, 0.0),
            2.0,
            true,
        );
        let mid = s.closest_point(Vec3::new(5.0, 0.0, 0.0));
        assert!((mid.y - 4.0).abs() < 1e-4, "{mid:?}");
    }

    #[test]
    fn aabb_covers_the_width_and_the_endpoints() {
        let s = RoadSegment::new(
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(10.0, 3.0, 0.0),
            4.0,
            true,
        );
        let b = s.aabb(0.5);
        assert!(b.contains_point(Vec3::new(0.0, 1.0, 0.0)));
        assert!(b.contains_point(Vec3::new(10.0, 3.0, 0.0)));
        assert!(b.min.z <= -2.0 - 0.5 + 1e-4);
        assert!(b.max.z >= 2.0 + 0.5 - 1e-4);
        assert!(b.max.y > b.min.y, "Y extent must be positive");
        assert!(b.is_finite());
    }

    #[test]
    fn nan_inputs_do_not_produce_nan_bounds() {
        let s = RoadSegment::new(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), f32::NAN, true);
        assert!(s.aabb(f32::NAN).is_finite());
        assert_eq!(s.width, 0.0);
    }

    #[test]
    fn macro_lines_are_spaced_and_jittered() {
        let c = config();
        let spacing = macro_spacing(&c);
        assert!((spacing - 256.0).abs() < 1e-3, "8 chunks of 32 m");
        let a = macro_line_x(&c, 0);
        let b = macro_line_x(&c, 1);
        assert!((b - a - spacing).abs() < spacing * 0.3);
        assert!(a.abs() <= c.chunk_world_size() * c.road_jitter + 1e-3);
        // Deterministic across calls.
        assert_eq!(a, macro_line_x(&c, 0));
    }

    #[test]
    fn jitter_differs_per_axis_and_index() {
        let c = config();
        assert_ne!(road_jitter(&c, 3, 0), road_jitter(&c, 3, 1));
        assert_ne!(road_jitter(&c, 3, 0), road_jitter(&c, 4, 0));
        assert_eq!(road_jitter(&c, 3, 0), road_jitter(&c, 3, 0));
    }

    #[test]
    fn nearest_line_index_handles_extremes() {
        let c = config();
        assert_eq!(nearest_line_index(&c, 0.0), 0);
        assert_eq!(nearest_line_index(&c, f32::NAN), 0);
        assert_eq!(nearest_line_index(&c, f32::INFINITY), 0);
        assert!(nearest_line_index(&c, 1e30).abs() <= 4_000_000);
        assert_eq!(nearest_line_index(&c, 300.0), 1);
    }

    #[test]
    fn generated_lattice_covers_the_region_and_crosses() {
        let c = config();
        let net = RoadNetwork::generate(&c, ChunkPos::new(0, 0), ChunkPos::new(4, 4));
        assert!(net.len() >= 4, "expected at least two lines per axis");
        assert!(net.total_length() > 0.0);
        // Every segment is on the road by its own test.
        for segment in &net.segments {
            let mid = (segment.from + segment.to) * 0.5;
            assert!(net.is_on_road(mid, 0.0), "{segment:?}");
            assert!(net.is_on_road(segment.from, 0.1));
            assert!(net.is_on_road(segment.to, 0.1));
        }
        // A vertical and a horizontal road cross inside the region.
        let vx = macro_line_x(&c, 0);
        let hz = macro_line_z(&c, 0);
        assert!(
            net.is_on_road(Vec3::new(vx, 0.0, hz), 0.0),
            "the junction must be on both roads"
        );
        assert!(net.distance_to_nearest(Vec3::new(vx, 0.0, hz)) < 1e-3);
    }

    #[test]
    fn generated_lattice_is_connected_through_junctions() {
        let c = config();
        let net = RoadNetwork::generate(&c, ChunkPos::new(0, 0), ChunkPos::new(3, 3));
        // Walk every segment; a point on it must reach a crossing with every
        // perpendicular line that spans it.
        for segment in &net.segments {
            let steps = (segment.length() / 4.0).ceil().max(1.0) as i32;
            for k in 0..=steps {
                let t = k as f32 / steps as f32;
                let p = segment.from.lerp(segment.to, t);
                assert!(net.is_on_road(p, 1e-3), "gap at {p:?}");
            }
        }
    }

    #[test]
    fn distant_points_are_not_on_a_road() {
        let c = config();
        let net = RoadNetwork::generate(&c, ChunkPos::new(0, 0), ChunkPos::new(3, 3));
        // Midway between four lattice lines, 128 m from each.
        let spacing = macro_spacing(&c);
        let x = macro_line_x(&c, 0) + spacing * 0.5;
        let z = macro_line_z(&c, 0) + spacing * 0.5;
        assert!(!net.is_on_road(Vec3::new(x, 0.0, z), 0.0));
        assert!(net.distance_to_nearest(Vec3::new(x, 0.0, z)) > 100.0);
    }

    #[test]
    fn segments_in_filters_by_bounds() {
        let c = config();
        let net = RoadNetwork::generate(&c, ChunkPos::new(0, 0), ChunkPos::new(9, 9));
        let all = net.len();
        let bounds = Aabb::new(Vec3::new(0.0, -100.0, 0.0), Vec3::new(32.0, 100.0, 32.0));
        let inside: Vec<_> = net.segments_in(&bounds).collect();
        assert!(!inside.is_empty());
        assert!(inside.len() < all, "{} of {}", inside.len(), all);
        for segment in inside {
            assert!(segment.aabb(0.0).intersects(&bounds));
        }
    }

    #[test]
    fn empty_network_is_harmless() {
        let net = RoadNetwork::new();
        assert!(net.is_empty());
        assert_eq!(net.len(), 0);
        assert!(net.distance_to_nearest(Vec3::ZERO).is_infinite());
        assert!(!net.is_on_road(Vec3::ZERO, 1000.0));
        assert_eq!(net.total_length(), 0.0);
        assert_eq!(net.segments_in(&Aabb::unit_cube()).count(), 0);
    }

    #[test]
    fn inverted_region_is_normalised() {
        let c = config();
        let a = RoadNetwork::generate(&c, ChunkPos::new(4, 4), ChunkPos::new(0, 0));
        let b = RoadNetwork::generate(&c, ChunkPos::new(0, 0), ChunkPos::new(4, 4));
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn region_is_capped() {
        let c = config();
        let net = RoadNetwork::generate(
            &c,
            ChunkPos::new(-100_000, -100_000),
            ChunkPos::new(100_000, 100_000),
        );
        assert!(net.len() <= RoadNetwork::MAX_SEGMENTS);
    }

    #[test]
    fn memory_bytes_grows_with_segments() {
        let c = config();
        let small = RoadNetwork::generate(&c, ChunkPos::new(0, 0), ChunkPos::new(1, 1));
        let big = RoadNetwork::generate(&c, ChunkPos::new(0, 0), ChunkPos::new(16, 16));
        assert!(big.memory_bytes() > small.memory_bytes());
    }

    #[test]
    fn nearest_lines_sit_on_the_lattice() {
        let c = config();
        let p = Vec3::new(37.0, 0.0, -84.0);
        let (x, z) = nearest_lines(&c, p);
        assert!((x - macro_line_x(&c, 0)).abs() <= macro_spacing(&c));
        assert!((z - macro_line_z(&c, 0)).abs() <= macro_spacing(&c));
        assert_eq!((x, z), nearest_lines(&c, p));
    }
}
