//! Towns: deterministic settlements with a plaza, a street grid and buildings.
//!
//! A town exists at every `town_spacing_chunks`-th chunk-lattice position. Its
//! style, name, street layout and buildings are all drawn from
//! `RngStream::for_chunk(seed, "town/…", lattice_x, lattice_y)`, so a town is a
//! pure function of the seed and its lattice cell: two neighbouring chunks that
//! both touch the same town compute byte-identical plans and therefore agree
//! about where the walls are.
//!
//! Buildings are placed from the prefab library when one fits the plot; when
//! none does — including when the library is empty — the generator emits a
//! procedural box-and-gable house, so a town is never an empty field of plots.
//!
//! ```
//! use noxel_core::math::ChunkPos;
//! use noxel_world::WorldConfig;
//! use noxel_world::town::TownPlan;
//!
//! let config = WorldConfig::new(4);
//! let plan = TownPlan::generate(&config, ChunkPos::new(0, 0), &[], |_, _| 0.0);
//! assert_eq!(plan.center_chunk, ChunkPos::new(0, 0));
//! assert!(!plan.name.is_empty());
//!
//! // An empty prefab library still yields a town: the fallback house fills in.
//! let buildings = plan.instantiate(&config, &[], |_, _| 0.0);
//! assert!(!buildings.is_empty());
//! ```

use std::collections::HashSet;
use std::sync::Arc;

use noxel_asset::format::{Prefab, PrefabVoxel};
use noxel_core::math::{Aabb, ChunkPos, Rect, Vec2, Vec3};
use noxel_core::rng::{RngStream, hash_combine, hash_str};

use crate::r#gen::WorldConfig;
use crate::road::{RoadSegment, macro_line_x, macro_line_z, nearest_line_index};

/// Which way a building's front faces, in world axes.
///
/// The engine's convention is right-handed with `-Z` forward, so [`Facing::North`]
/// is `-Z` and a yaw of `0`. The four values map onto the four cardinal yaws
/// exactly, which is why a building's world bounds stay axis-aligned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    /// Towards `-Z`, yaw `0`.
    North,
    /// Towards `+X`, yaw `-90°` (equivalently `270°`).
    East,
    /// Towards `+Z`, yaw `180°`.
    South,
    /// Towards `-X`, yaw `90°`.
    West,
}

impl Facing {
    /// Every facing, in a stable order.
    pub const ALL: [Self; 4] = [Self::North, Self::East, Self::South, Self::West];

    /// The yaw that rotates the model's front onto this facing, in radians.
    #[must_use]
    pub fn yaw(self) -> f32 {
        match self {
            Self::North => 0.0,
            Self::West => core::f32::consts::FRAC_PI_2,
            Self::South => core::f32::consts::PI,
            Self::East => -core::f32::consts::FRAC_PI_2,
        }
    }

    /// The outward unit direction in the XZ plane.
    #[must_use]
    pub fn vector(self) -> Vec3 {
        match self {
            Self::North => Vec3::new(0.0, 0.0, -1.0),
            Self::South => Vec3::new(0.0, 0.0, 1.0),
            Self::East => Vec3::new(1.0, 0.0, 0.0),
            Self::West => Vec3::new(-1.0, 0.0, 0.0),
        }
    }

    /// The facing directly opposite this one.
    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::East => Self::West,
            Self::West => Self::East,
        }
    }

    /// The cardinal facing nearest to `yaw`.
    ///
    /// Any angle is accepted; ties go to the X axis so the result is total and
    /// deterministic.
    #[must_use]
    pub fn from_yaw(yaw: f32) -> Self {
        if !yaw.is_finite() {
            return Self::North;
        }
        Self::from_vector(Vec3::from_yaw(yaw))
    }

    /// The cardinal facing nearest to a direction, ignoring Y.
    ///
    /// A zero or non-finite direction yields [`Facing::North`].
    #[must_use]
    pub fn from_vector(v: Vec3) -> Self {
        if !v.is_finite() || (v.x == 0.0 && v.z == 0.0) {
            return Self::North;
        }
        if v.x.abs() > v.z.abs() {
            if v.x > 0.0 {
                Facing::East
            } else {
                Facing::West
            }
        } else if v.z > 0.0 {
            Facing::South
        } else {
            Facing::North
        }
    }

    /// The facing whose direction is `dir` rotated 90° about `+Y`.
    #[must_use]
    pub fn turned_left(self) -> Self {
        Self::from_vector(Vec3::new(self.vector().z, 0.0, -self.vector().x))
    }
}

/// How large and how dense a town is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TownStyle {
    /// A handful of buildings around a crossroad.
    Hamlet,
    /// A compact village with a plaza and two side streets.
    Village,
    /// A dense town: three streets per axis and wall-to-wall frontage.
    Town,
    /// A village built around the meeting of two through-roads.
    Crossroads,
}

impl TownStyle {
    /// Every style, in a stable order.
    pub const ALL: [Self; 4] = [Self::Hamlet, Self::Village, Self::Town, Self::Crossroads];

    /// The style's lowercase name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Hamlet => "hamlet",
            Self::Village => "village",
            Self::Town => "town",
            Self::Crossroads => "crossroads",
        }
    }

    /// How built-up the town is, in `0..1`.
    ///
    /// Drives the gap left between two neighbouring plots along a street: a
    /// hamlet of `0.35` has room to breathe, a town of `0.8` is nearly terraced.
    #[must_use]
    pub fn building_density(self) -> f32 {
        match self {
            Self::Hamlet => 0.35,
            Self::Village => 0.55,
            Self::Town => 0.8,
            Self::Crossroads => 0.65,
        }
    }

    /// Fraction of `buildings_per_town` this style actually builds.
    #[must_use]
    pub fn building_scale(self) -> f32 {
        match self {
            Self::Hamlet => 0.4,
            Self::Village => 0.7,
            Self::Town => 1.0,
            Self::Crossroads => 0.85,
        }
    }

    /// Side streets laid out on each side of the plaza, per axis.
    #[must_use]
    pub fn side_streets(self) -> i32 {
        match self {
            Self::Hamlet => 0,
            Self::Village => 1,
            Self::Town => 3,
            Self::Crossroads => 1,
        }
    }

    /// Gap between plots along a street, in tiles.
    #[must_use]
    pub fn plot_gap_tiles(self) -> f32 {
        // Dense towns almost touch; hamlets leave two tiles of garden.
        crate::r#gen::lerp_f32(2.0, 0.4, self.building_density())
    }
}

/// A site a building can occupy.
///
/// `position` is the minimum corner of the plot's world-space XZ footprint and
/// `size_tiles` is its extent in world X and Z (so a plot is always
/// axis-aligned, whatever the building's facing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuildingPlot {
    /// World position of the plot's minimum corner, on the ground.
    pub position: Vec3,
    /// Extent in tiles along world X and world Z.
    pub size_tiles: (u32, u32),
    /// Which way a building on this plot faces (towards its street).
    pub facing: Facing,
    /// True when the generator placed a building here; false for the reserved
    /// plots a game can hand to the player.
    pub taken: bool,
}

impl BuildingPlot {
    /// The plot's world-space XZ rectangle.
    #[must_use]
    pub fn rect(&self, tile_size: f32) -> Rect {
        let ts = if tile_size.is_finite() && tile_size > 0.0 {
            tile_size
        } else {
            1.0
        };
        let x1 = self.position.x + self.size_tiles.0 as f32 * ts;
        let z1 = self.position.z + self.size_tiles.1 as f32 * ts;
        Rect::new(
            Vec2::new(self.position.x, self.position.z),
            Vec2::new(x1, z1),
        )
    }

    /// The plot's centre on the ground.
    #[must_use]
    pub fn center(&self, tile_size: f32) -> Vec3 {
        let r = self.rect(tile_size);
        let c = r.center();
        Vec3::new(c.x, self.position.y, c.y)
    }

    /// The plot's world bounds, `height` metres tall.
    #[must_use]
    pub fn bounds(&self, tile_size: f32, height: f32) -> Aabb {
        let r = self.rect(tile_size);
        Aabb::from_footprint(r.min.x, r.min.y, r.max.x, r.max.y, self.position.y, height)
    }
}

/// A building placed in a town.
#[derive(Clone, Debug)]
pub struct BuildingInstance {
    /// Name of the prefab this building was instanced from, or
    /// `"procedural_house"` for the generator's fallback box-and-gable.
    pub prefab: String,
    /// World position of the minimum corner of the building's world bounds.
    ///
    /// Because every facing is a multiple of 90°, the bounds below are exact:
    /// the model's XZ footprint is rotated about its own centre, so a renderer
    /// should rotate about `bounds.center()` in XZ.
    pub origin: Vec3,
    /// Rotation about the up axis, in radians: one of the four cardinal yaws.
    pub yaw: f32,
    /// Extent of the placed building in world X and world Z, in tiles.
    pub size_tiles: (u32, u32),
    /// The building's exact world bounds.
    pub bounds: Aabb,
    /// Street-facing side, used to place doors and signs.
    pub facing: Facing,
    /// Voxel occupancy grid for this building, if it was generated from a prefab.
    ///
    /// Merged into as few boxes as the shape allows: a solid house is one or two
    /// boxes, not one per voxel, because the occlusion system ray-tests these
    /// every frame.
    pub occluders: Vec<Aabb>,
}

impl BuildingInstance {
    /// A stable id for this building, used as collider and occluder `user_data`.
    #[must_use]
    pub fn id(&self) -> u64 {
        let tag = hash_str(&self.prefab) ^ 0x8D00_0000;
        let mut h = crate::chunk::stable_id(0, self.origin.x, self.origin.z, tag);
        h = hash_combine(h, self.yaw.to_bits() as u64);
        h
    }

    /// The building's XZ footprint, for point-in-building tests.
    #[must_use]
    pub fn footprint(&self) -> Rect {
        Rect::from_min_max(
            Vec2::new(self.bounds.min.x, self.bounds.min.z),
            Vec2::new(self.bounds.max.x, self.bounds.max.z),
        )
    }

    /// True when the building's footprint contains a world XZ position.
    #[must_use]
    pub fn contains_xz(&self, p: Vec3) -> bool {
        self.footprint().contains(Vec2::new(p.x, p.z))
    }

    /// Height of the building above its own ground, in metres.
    #[must_use]
    pub fn height(&self) -> f32 {
        self.bounds.max.y - self.bounds.min.y
    }

    /// The building's solid volumes clipped to a chunk footprint.
    ///
    /// Clipping is what keeps a building that straddles a chunk border from
    /// being reported twice by a query that touches both chunks.
    #[must_use]
    pub fn colliders_in(&self, region: &Aabb) -> Vec<(Aabb, u64)> {
        let mut out = Vec::new();
        for box_ in &self.occluders {
            if let Some(clipped) = clip_xz(box_, region) {
                out.push((clipped, self.id()));
            }
        }
        out
    }
}

/// A town's layout: where the streets and the building plots are.
#[derive(Clone, Debug)]
pub struct TownPlan {
    /// The chunk at the town's centre.
    pub center_chunk: ChunkPos,
    /// How big and how dense the town is.
    pub style: TownStyle,
    /// Radius of the built-up area, in chunks.
    pub radius_chunks: i32,
    /// Every plot in the layout, built or reserved.
    pub building_plots: Vec<BuildingPlot>,
    /// Street tiles in world space, including the connector towards the nearest
    /// macro road. The connector is clipped to the town radius; its approach
    /// beyond that is stamped from the analytic lattice by the generator.
    pub streets: Vec<RoadSegment>,
    /// The town's plaza centre in world space.
    pub plaza_center: Vec3,
    /// Deterministic, hand-written-sounding name, e.g. `"Thornwick"`.
    pub name: String,
}

impl TownPlan {
    /// Radius of the built-up area, in metres.
    #[must_use]
    pub fn radius_world(&self, config: &WorldConfig) -> f32 {
        self.radius_chunks.max(0) as f32 * config.chunk_world_size()
    }

    /// True when a world position lies inside the built-up disc.
    #[must_use]
    pub fn contains_world(&self, config: &WorldConfig, p: Vec3) -> bool {
        let r = self.radius_world(config);
        let d = Vec2::new(p.x - self.plaza_center.x, p.z - self.plaza_center.z).length();
        d <= r
    }

    /// XZ distance from `p` to the nearest street centre line.
    #[must_use]
    pub fn distance_to_street(&self, p: Vec3) -> f32 {
        let mut best = f32::INFINITY;
        for street in &self.streets {
            let d = street.distance_to(p);
            if d < best {
                best = d;
            }
        }
        best
    }

    /// True when `p` is on a paved street.
    #[must_use]
    pub fn is_on_street(&self, p: Vec3) -> bool {
        self.streets
            .iter()
            .any(|street| street.distance_to(p) <= street.half_width())
    }

    /// The plots nobody has built on yet.
    pub fn free_plots(&self) -> impl Iterator<Item = &BuildingPlot> {
        self.building_plots.iter().filter(|plot| !plot.taken)
    }

    /// Approximate heap footprint in bytes.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        core::mem::size_of::<Self>()
            + self.name.capacity()
            + self.building_plots.capacity() * core::mem::size_of::<BuildingPlot>()
            + self.streets.capacity() * core::mem::size_of::<RoadSegment>()
    }

    /// Builds the plan for the town centred on `center_chunk`.
    ///
    /// `ground` supplies the terrain height at a world XZ position; the caller
    /// passes the generator's own height field so the plaza and the buildings
    /// sit exactly on the ground the terrain mesh shows. `prefabs` may be empty,
    /// in which case every plot is filled with the procedural fallback house.
    #[must_use]
    pub fn generate<F>(
        config: &WorldConfig,
        center_chunk: ChunkPos,
        prefabs: &[Arc<Prefab>],
        ground: F,
    ) -> Self
    where
        F: Fn(f32, f32) -> f32,
    {
        let cs = config.chunk_world_size();
        let radius_chunks = config.town_radius_chunks.max(1);
        let radius = radius_chunks as f32 * cs;
        let centre = center_chunk.to_world_center(cs, 0.0);
        let centre = Vec3::new(centre.x, ground(centre.x, centre.z), centre.z);

        let style = town_style_at(config, center_chunk);
        let name = town_name(config, center_chunk);
        // Lift the analytic grid onto the ground the terrain actually has.
        let streets: Vec<RoadSegment> = town_streets(config, center_chunk)
            .into_iter()
            .map(|street| {
                RoadSegment::new(
                    Vec3::new(street.from.x, centre.y, street.from.z),
                    Vec3::new(street.to.x, centre.y, street.to.z),
                    street.width,
                    street.is_main,
                )
            })
            .collect();

        let mut rng =
            RngStream::for_chunk(config.seed, "town/plots", center_chunk.x, center_chunk.y).rng();
        let plots = layout_plots(config, centre, radius, style, prefabs, &streets, &mut rng);

        Self {
            center_chunk,
            style,
            radius_chunks,
            building_plots: plots,
            streets,
            plaza_center: centre,
            name,
        }
    }

    /// Instantiates the buildings for this plan.
    ///
    /// Every plot the generator claimed (`taken`) becomes a real building: a
    /// prefab when one fits, and the procedural fallback otherwise. Reserved
    /// plots stay empty.
    ///
    /// The building is inset half a tile into its plot, so the one-tile margin
    /// the layout reserved stays a visible gap between neighbours.
    #[must_use]
    pub fn instantiate<F>(
        &self,
        config: &WorldConfig,
        prefabs: &[Arc<Prefab>],
        ground: F,
    ) -> Vec<BuildingInstance>
    where
        F: Fn(f32, f32) -> f32,
    {
        let ts = sanitize_tile_size(config.tile_size);
        let mut rng = RngStream::for_chunk(
            config.seed,
            "town/build",
            self.center_chunk.x,
            self.center_chunk.y,
        )
        .rng();
        let mut out = Vec::new();
        for plot in &self.building_plots {
            if !plot.taken {
                continue;
            }
            let origin = Vec3::new(
                plot.position.x + ts * 0.5,
                plot_ground(plot, ts, &ground),
                plot.position.z + ts * 0.5,
            );
            match fitting_prefab(prefabs, plot.facing, plot.size_tiles) {
                Some(prefab) => out.push(instance_prefab(prefab, origin, plot.facing, ts)),
                None => out.push(procedural_building(&mut rng, plot, origin, ts)),
            }
        }
        out
    }
}

/// The ground height a building should stand on: the highest sample over the
/// plot, so a building on a slope is never buried in the terrain.
#[must_use]
fn plot_ground<F>(plot: &BuildingPlot, ts: f32, ground: &F) -> f32
where
    F: Fn(f32, f32) -> f32,
{
    let w = plot.size_tiles.0 as f32 * ts;
    let d = plot.size_tiles.1 as f32 * ts;
    let corners = [
        (plot.position.x, plot.position.z),
        (plot.position.x + w, plot.position.z),
        (plot.position.x, plot.position.z + d),
        (plot.position.x + w, plot.position.z + d),
        (plot.position.x + w * 0.5, plot.position.z + d * 0.5),
    ];
    let mut best = f32::NEG_INFINITY;
    for (x, z) in corners {
        let h = ground(x, z);
        if h.is_finite() && h > best {
            best = h;
        }
    }
    if best.is_finite() { best } else { 0.0 }
}

/// The prefab footprint `(x, z)` in tiles once rotated onto `facing`.
///
/// A quarter turn swaps the two axes.
#[must_use]
pub fn rotated_footprint(footprint: (u32, u32), facing: Facing) -> (u32, u32) {
    match facing {
        Facing::North | Facing::South => footprint,
        Facing::East | Facing::West => (footprint.1, footprint.0),
    }
}

/// The town-lattice cell whose centre is nearest to a world XZ position.
///
/// This is a pure function of the position and the configuration — no plan, no
/// prefabs — which is what lets the height field flatten a town disc, and the
/// tile pass stamp its streets, without building anything.
#[must_use]
pub fn nearest_town_cell(config: &WorldConfig, x: f32, z: f32) -> ChunkPos {
    let spacing = config.town_spacing().max(1);
    let step = spacing as f32 * config.chunk_world_size();
    let i = if x.is_finite() {
        (x / step).round().clamp(-4_000_000.0, 4_000_000.0) as i32
    } else {
        0
    };
    let j = if z.is_finite() {
        (z / step).round().clamp(-4_000_000.0, 4_000_000.0) as i32
    } else {
        0
    };
    ChunkPos::new(i.saturating_mul(spacing), j.saturating_mul(spacing))
}

/// The world-space centre of a town's lattice cell, on the ground plane.
///
/// The `y` component is `0`; callers fill it from the height field.
#[must_use]
pub fn town_centre(config: &WorldConfig, cell: ChunkPos) -> Vec3 {
    cell.to_world_center(config.chunk_world_size(), 0.0)
}

/// The style of the town at a lattice cell.
///
/// Deterministic and cheap: one draw from the cell's own stream.
#[must_use]
pub fn town_style_at(config: &WorldConfig, cell: ChunkPos) -> TownStyle {
    let mut rng = RngStream::for_chunk(config.seed, "town/layout", cell.x, cell.y).rng();
    pick_style(&mut rng)
}

/// The street grid of the town at a lattice cell, with `y == 0`.
///
/// The grid is analytic: it depends only on the seed, the cell and the
/// configuration, so the terrain carve, the tile pass and the town plan all see
/// exactly the same streets.
#[must_use]
pub fn town_streets(config: &WorldConfig, cell: ChunkPos) -> Vec<RoadSegment> {
    let ts = sanitize_tile_size(config.tile_size);
    let radius = config.town_radius_chunks.max(1) as f32 * config.chunk_world_size();
    let style = town_style_at(config, cell);
    layout_streets(
        config,
        town_centre(config, cell),
        radius,
        config.road_width_tiles.max(1) as f32 * ts * 1.5,
        config.road_width_tiles.max(1) as f32 * ts,
        style,
    )
}

/// A prefab that fits a plot of `plot_size` tiles with the given facing.
#[must_use]
fn fitting_prefab(
    prefabs: &[Arc<Prefab>],
    facing: Facing,
    plot_size: (u32, u32),
) -> Option<&Arc<Prefab>> {
    let inner = (
        plot_size.0.saturating_sub(1).max(1),
        plot_size.1.saturating_sub(1).max(1),
    );
    // Prefer prefabs the asset authors tagged as buildings; fall back to any
    // prefab that fits and is tall enough to be a structure.
    let mut best: Option<&Arc<Prefab>> = None;
    for prefab in prefabs {
        let rot = rotated_footprint(prefab.footprint(), facing);
        if rot.0 == 0 || rot.1 == 0 || prefab.size[1] == 0 {
            continue;
        }
        if rot.0 > inner.0 || rot.1 > inner.1 {
            continue;
        }
        let tagged = prefab.has_tag("building") || prefab.has_tag("house");
        let usable = prefab.volume() >= 4;
        if !tagged && !usable {
            continue;
        }
        match best {
            None => best = Some(prefab),
            Some(current) => {
                let current_tagged = current.has_tag("building") || current.has_tag("house");
                let better = (tagged && !current_tagged)
                    || (tagged == current_tagged && prefab.footprint() > current.footprint());
                if better {
                    best = Some(prefab);
                }
            }
        }
    }
    best
}

/// Turns a prefab into a placed building: exact bounds, merged occluders.
#[must_use]
fn instance_prefab(prefab: &Prefab, origin: Vec3, facing: Facing, ts: f32) -> BuildingInstance {
    let rot = rotated_footprint(prefab.footprint(), facing);
    let height = prefab.size[1] as f32 * ts;
    let bounds = Aabb::from_footprint(
        origin.x,
        origin.z,
        origin.x + rot.0 as f32 * ts,
        origin.z + rot.1 as f32 * ts,
        origin.y,
        height,
    );
    let yaw = facing.yaw();
    let centre = Vec2::new(
        origin.x + rot.0 as f32 * ts * 0.5,
        origin.z + rot.1 as f32 * ts * 0.5,
    );
    let occluders = if prefab.occluders.is_empty() {
        merge_voxel_runs(&prefab.voxels)
            .into_iter()
            .filter_map(|b| voxel_box_to_world(b, origin, centre, yaw, ts))
            .collect()
    } else {
        prefab
            .occluders
            .iter()
            .filter_map(|b| voxel_box_to_world(normalise_voxel_box(*b), origin, centre, yaw, ts))
            .collect()
    };
    BuildingInstance {
        prefab: prefab.name.clone(),
        origin,
        yaw,
        size_tiles: rot,
        bounds,
        facing,
        occluders,
    }
}

/// The generator's fallback building: a solid box with a gable roof.
///
/// It exists so that a town is never empty when the prefab library has nothing
/// that fits — which is also the case when the library is empty entirely.
#[must_use]
fn procedural_building(
    rng: &mut noxel_core::rng::Pcg32,
    plot: &BuildingPlot,
    origin: Vec3,
    ts: f32,
) -> BuildingInstance {
    let w = plot.size_tiles.0.saturating_sub(1).max(2);
    let d = plot.size_tiles.1.saturating_sub(1).max(2);
    // Two to four storeys of 1 tile keeps the skyline varied but bounded.
    let storeys = rng.range_i32(2, 4).max(1) as u32;
    let wall_height = storeys as f32 * ts;
    let roof_height = (ts * 1.5).max(1.0);
    let x0 = origin.x;
    let z0 = origin.z;
    let bounds = Aabb::from_footprint(
        x0,
        z0,
        x0 + w as f32 * ts,
        z0 + d as f32 * ts,
        origin.y,
        wall_height + roof_height,
    );
    // One box for the whole solid run of walls (a storey run, not a voxel run)
    // and one for the roof volume: two occluders for a building that would
    // otherwise be hundreds of voxels.
    let walls = Aabb::from_footprint(
        x0,
        z0,
        x0 + w as f32 * ts,
        z0 + d as f32 * ts,
        origin.y,
        wall_height,
    );
    let overhang = (ts * 0.25).max(0.1);
    let roof = Aabb::from_footprint(
        x0 - overhang,
        z0 - overhang,
        x0 + w as f32 * ts + overhang,
        z0 + d as f32 * ts + overhang,
        origin.y + wall_height,
        roof_height,
    );
    BuildingInstance {
        prefab: PROCEDURAL_PREFAB.to_string(),
        origin,
        yaw: plot.facing.yaw(),
        size_tiles: (w, d),
        bounds,
        facing: plot.facing,
        occluders: vec![walls, roof],
    }
}

/// Name given to a building produced by the generator's fallback.
pub const PROCEDURAL_PREFAB: &str = "procedural_house";

/// The rotated world box for a voxel-space box.
///
/// The rotation is about the footprint centre, and every facing is a multiple of
/// 90°, so the result is exact rather than a conservative fit.
#[must_use]
fn voxel_box_to_world(b: [u8; 6], origin: Vec3, centre: Vec2, yaw: f32, ts: f32) -> Option<Aabb> {
    let x0 = origin.x + b[0] as f32 * ts;
    let x1 = origin.x + (b[3] as f32 + 1.0) * ts;
    let z0 = origin.z + b[2] as f32 * ts;
    let z1 = origin.z + (b[5] as f32 + 1.0) * ts;
    let y0 = origin.y + b[1] as f32 * ts;
    let y1 = origin.y + (b[4] as f32 + 1.0) * ts;
    if !(x1 > x0 && z1 > z0 && y1 > y0) {
        return None;
    }
    let c = rotate_about(Vec2::new(x0, z0), centre, yaw);
    let d = rotate_about(Vec2::new(x1, z0), centre, yaw);
    let e = rotate_about(Vec2::new(x0, z1), centre, yaw);
    let f = rotate_about(Vec2::new(x1, z1), centre, yaw);
    let min_x = c.x.min(d.x).min(e.x).min(f.x);
    let max_x = c.x.max(d.x).max(e.x).max(f.x);
    let min_z = c.y.min(d.y).min(e.y).min(f.y);
    let max_z = c.y.max(d.y).max(e.y).max(f.y);
    if !(max_x > min_x && max_z > min_z) || !min_x.is_finite() {
        return None;
    }
    Some(Aabb::from_footprint(
        min_x,
        min_z,
        max_x,
        max_z,
        y0,
        y1 - y0,
    ))
}

/// Rotates an XZ point about `centre` by `yaw`.
#[must_use]
fn rotate_about(p: Vec2, centre: Vec2, yaw: f32) -> Vec2 {
    let (s, c) = yaw.sin_cos();
    let d = p - centre;
    centre + Vec2::new(d.x * c + d.y * s, -d.x * s + d.y * c)
}

/// Normalises an authored `[x0,y0,z0,x1,y1,z1]` box so the maximum corner is not
/// below the minimum one.
#[must_use]
fn normalise_voxel_box(b: [u8; 6]) -> [u8; 6] {
    [
        b[0].min(b[3]),
        b[1].min(b[4]),
        b[2].min(b[5]),
        b[0].max(b[3]),
        b[1].max(b[4]),
        b[2].max(b[5]),
    ]
}

/// Merges a prefab's voxels into as few axis-aligned boxes as a greedy sweep
/// can manage.
///
/// The algorithm is the classic greedy voxel mesh: collapse each `(x, z)` column
/// into vertical runs, group the runs by their height span, then grow each run
/// along `+X` and then `+Z` while the neighbours agree. A solid house becomes
/// one box; an L-shaped one becomes two or three.
#[must_use]
pub fn merge_voxel_runs(voxels: &[PrefabVoxel]) -> Vec<[u8; 6]> {
    if voxels.is_empty() {
        return Vec::new();
    }
    let mut occupied: HashSet<(u8, u8, u8)> = HashSet::with_capacity(voxels.len());
    for voxel in voxels {
        occupied.insert((voxel.x, voxel.y, voxel.z));
    }
    // Per-column vertical runs.
    let mut runs: Vec<[u8; 6]> = Vec::new();
    let mut columns: Vec<(u8, u8)> = Vec::new();
    for &(x, _, z) in &occupied {
        columns.push((x, z));
    }
    columns.sort_unstable();
    columns.dedup();
    let max_y: u16 = occupied
        .iter()
        .map(|&(_, y, _)| y as u16)
        .max()
        .unwrap_or(0)
        + 1;
    for (x, z) in columns {
        let mut y: u16 = 0;
        while y < max_y {
            if y <= 255 && occupied.contains(&(x, y as u8, z)) {
                let start = y;
                while y < max_y && y <= 255 && occupied.contains(&(x, y as u8, z)) {
                    y += 1;
                }
                let end = (y.saturating_sub(1)).min(255) as u8;
                runs.push([x, start as u8, z, x, end, z]);
            } else {
                y += 1;
            }
        }
    }
    // Greedy merge in the XZ plane, per identical height span.
    runs.sort_unstable_by_key(|b| (b[1], b[4], b[2], b[0]));
    let mut used = vec![false; runs.len()];
    let mut out: Vec<[u8; 6]> = Vec::new();
    for i in 0..runs.len() {
        if used[i] {
            continue;
        }
        let base = runs[i];
        used[i] = true;
        let x0 = base[0];
        let mut x1 = base[3];
        let z0 = base[2];
        let mut z1 = base[5];
        // Grow along +X.
        while let Some(nx) = x1.checked_add(1) {
            match find_run(&runs, &used, [nx, base[1], z0, nx, base[4], z0]) {
                Some(j) => {
                    used[j] = true;
                    x1 = nx;
                }
                None => break,
            }
        }
        // Grow along +Z, requiring the whole X span to be free.
        'grow_z: while let Some(nz) = z1.checked_add(1) {
            let mut found = Vec::new();
            for x in x0..=x1 {
                match find_run(&runs, &used, [x, base[1], nz, x, base[4], nz]) {
                    Some(j) => found.push(j),
                    None => break 'grow_z,
                }
            }
            for j in found {
                used[j] = true;
            }
            z1 = nz;
        }
        out.push([x0, base[1], z0, x1, base[4], z1]);
    }
    out
}

/// Finds an unused run with exactly the given span.
#[must_use]
fn find_run(runs: &[[u8; 6]], used: &[bool], want: [u8; 6]) -> Option<usize> {
    runs.iter().enumerate().position(|(i, b)| {
        !used[i] && b[1] == want[1] && b[4] == want[4] && b[0] == want[0] && b[2] == want[2]
    })
}

/// Clips a box to a region in XZ, keeping the full Y extent.
///
/// Returns `None` when the overlap is empty or degenerate, so callers never see
/// a zero-area collider.
#[must_use]
pub(crate) fn clip_xz(box_: &Aabb, region: &Aabb) -> Option<Aabb> {
    let x0 = box_.min.x.max(region.min.x);
    let x1 = box_.max.x.min(region.max.x);
    let z0 = box_.min.z.max(region.min.z);
    let z1 = box_.max.z.min(region.max.z);
    if x1 - x0 <= 1e-4 || z1 - z0 <= 1e-4 {
        return None;
    }
    Some(Aabb::from_footprint(
        x0,
        z0,
        x1,
        z1,
        box_.min.y,
        box_.max.y - box_.min.y,
    ))
}

/// Picks a style with fixed weights, deterministically.
#[must_use]
fn pick_style(rng: &mut noxel_core::rng::Pcg32) -> TownStyle {
    let weights = [0.30, 0.35, 0.20, 0.15];
    match rng.weighted_index(&weights) {
        Some(1) => TownStyle::Village,
        Some(2) => TownStyle::Town,
        Some(3) => TownStyle::Crossroads,
        _ => TownStyle::Hamlet,
    }
}

/// A deterministic name built from two word lists.
///
/// The point is that the result reads like a place — `"Thornwick"`,
/// `"Aldermoor"`, `"Rookhaven"` — rather than like an index.
#[must_use]
pub fn town_name(config: &WorldConfig, center_chunk: ChunkPos) -> String {
    const FIRST: [&str; 32] = [
        "Thorn", "Alder", "Bramble", "Cold", "Dun", "Elder", "Fern", "Gale", "Harrow", "Iron",
        "Juniper", "Kestrel", "Lark", "Mire", "Nettle", "Oak", "Pine", "Quarry", "Rook", "Salt",
        "Thistle", "Umber", "Vale", "Willow", "Yarrow", "Ashen", "Bracken", "Cinder", "Dusk",
        "Ember", "Fox", "Grim",
    ];
    const SECOND: [&str; 24] = [
        "wick", "ford", "haven", "moor", "stead", "bury", "gate", "holm", "crest", "dale", "fell",
        "garth", "hollow", "mere", "reach", "ridge", "thorpe", "wold", "brook", "stone", "barrow",
        "combe", "shaw", "worth",
    ];
    const QUALIFIER: [&str; 8] = [
        "Little", "Great", "Upper", "Lower", "Old", "New", "East", "West",
    ];
    let mut rng =
        RngStream::for_chunk(config.seed, "town/name", center_chunk.x, center_chunk.y).rng();
    let first = FIRST[rng.range_usize(0, FIRST.len())];
    let second = SECOND[rng.range_usize(0, SECOND.len())];
    // One name in six gets a qualifier, which makes the map read less uniformly.
    if rng.chance(1.0 / 6.0) {
        let qualifier = QUALIFIER[rng.range_usize(0, QUALIFIER.len())];
        format!("{qualifier} {first}{second}")
    } else {
        format!("{first}{second}")
    }
}

/// The street grid: a main cross through the plaza, side streets parallel to it,
/// and a connector aiming at the nearest macro road.
#[must_use]
fn layout_streets(
    config: &WorldConfig,
    centre: Vec3,
    radius: f32,
    main_width: f32,
    side_width: f32,
    style: TownStyle,
) -> Vec<RoadSegment> {
    let mut streets = Vec::new();
    let y = centre.y;
    // The main cross spans the whole disc.
    streets.push(RoadSegment::new(
        Vec3::new(centre.x - radius, y, centre.z),
        Vec3::new(centre.x + radius, y, centre.z),
        main_width,
        true,
    ));
    streets.push(RoadSegment::new(
        Vec3::new(centre.x, y, centre.z - radius),
        Vec3::new(centre.x, y, centre.z + radius),
        main_width,
        true,
    ));
    let spacing = radius * 0.5;
    for k in 1..=style.side_streets() {
        let offset = spacing * k as f32;
        let reach = (radius * radius - offset * offset).max(0.0).sqrt();
        let inner = main_width * 0.5 + side_width * 0.5;
        if reach <= inner + 1.0 {
            continue;
        }
        for sign in [-1.0, 1.0] {
            streets.push(RoadSegment::new(
                Vec3::new(centre.x - reach + inner, y, centre.z + sign * offset),
                Vec3::new(centre.x + reach - inner, y, centre.z + sign * offset),
                side_width,
                false,
            ));
            streets.push(RoadSegment::new(
                Vec3::new(centre.x + sign * offset, y, centre.z - reach + inner),
                Vec3::new(centre.x + sign * offset, y, centre.z + reach - inner),
                side_width,
                false,
            ));
        }
    }
    // Connector towards the nearest macro road, clipped to the disc.
    let i = nearest_line_index(config, centre.x);
    let j = nearest_line_index(config, centre.z);
    let lx = macro_line_x(config, i);
    let lz = macro_line_z(config, j);
    let dx = lx - centre.x;
    let dz = lz - centre.z;
    let reach = (radius - main_width * 0.5).max(0.0);
    if reach > 1.0 {
        if dx.abs() <= dz.abs() {
            let end = lx.clamp(centre.x - reach, centre.x + reach);
            streets.push(RoadSegment::new(
                Vec3::new(centre.x, y, centre.z),
                Vec3::new(end, y, centre.z),
                main_width,
                true,
            ));
        } else {
            let end = lz.clamp(centre.z - reach, centre.z + reach);
            streets.push(RoadSegment::new(
                Vec3::new(centre.x, y, centre.z),
                Vec3::new(centre.x, y, end),
                main_width,
                true,
            ));
        }
    }
    streets
}

/// Lays out building plots along both sides of every street.
#[must_use]
fn layout_plots(
    config: &WorldConfig,
    centre: Vec3,
    radius: f32,
    style: TownStyle,
    prefabs: &[Arc<Prefab>],
    streets: &[RoadSegment],
    rng: &mut noxel_core::rng::Pcg32,
) -> Vec<BuildingPlot> {
    let ts = sanitize_tile_size(config.tile_size);
    let built = ((config.buildings_per_town as f32) * style.building_scale()).round() as u32;
    let (built, slots) = if config.buildings_per_town == 0 {
        // No buildings were asked for, but the layout still exists: every plot
        // is reserved (`taken == false`) for the game to build on.
        (0, 8)
    } else {
        let built = built.max(1);
        // Over-provision by a quarter so a game has free plots to hand out.
        (built, built + (built / 4).max(1))
    };
    let plaza_reach = plaza_radius(config);
    let plaza_centre = Vec2::new(centre.x, centre.z);
    let gap = style.plot_gap_tiles().max(0.0) * ts;

    let mut plots: Vec<BuildingPlot> = Vec::new();
    let mut rects: Vec<Rect> = Vec::new();
    let mut claimed = 0u32;

    for street in streets {
        if plots.len() as u32 >= slots {
            break;
        }
        let from = street.from;
        let length = street.length();
        let dir = street.direction();
        let normal = Vec3::new(-dir.z, 0.0, dir.x);
        let half = street.half_width();
        for side in [1.0f32, -1.0] {
            let mut t = half + gap;
            while t < length - gap && (plots.len() as u32) < slots {
                let point = from + dir * t;
                let facing = Facing::from_vector(-normal * side);
                let (rw, rd) = match choose_footprint(prefabs, facing, rng) {
                    Some(fp) => rotated_footprint(fp, facing),
                    None => (0, 0),
                };
                let (w, d) = if rw == 0 {
                    // Nothing in the library fits: reserve a procedural plot in
                    // the range the fallback house can fill.
                    let a = rng.range_i32(4, 7) as u32;
                    let b = rng.range_i32(4, 6) as u32;
                    match facing {
                        Facing::North | Facing::South => (a, b),
                        Facing::East | Facing::West => (b, a),
                    }
                } else {
                    // One tile of margin over the rotated footprint.
                    (rw + 1, rd + 1)
                };
                let plot_w = w as f32 * ts;
                let plot_d = d as f32 * ts;
                // The plot's extent along the street is the `dir` component.
                let along = if dir.x.abs() > dir.z.abs() {
                    plot_w
                } else {
                    plot_d
                };
                let across = if dir.x.abs() > dir.z.abs() {
                    plot_d
                } else {
                    plot_w
                };
                let centre_point = Vec3::new(point.x, centre.y, point.z)
                    + normal * side * (half + across * 0.5 + gap * 0.5);
                let rect = Rect::new(
                    Vec2::new(centre_point.x - plot_w * 0.5, centre_point.z - plot_d * 0.5),
                    Vec2::new(centre_point.x + plot_w * 0.5, centre_point.z + plot_d * 0.5),
                );
                let half_diagonal = Vec2::new(plot_w * 0.5, plot_d * 0.5).length();
                let ok = within_radius(rect, centre, radius)
                    && !in_plaza(plaza_centre, rect.center(), half_diagonal, plaza_reach)
                    && !streets
                        .iter()
                        .any(|s| s.aabb(0.0).intersects(&rect_aabb(rect)))
                    && !rects
                        .iter()
                        .any(|other| rect.intersects(&other.expanded(Vec2::splat(ts * 0.25))));
                if ok {
                    let taken = claimed < built;
                    if taken {
                        claimed += 1;
                    }
                    plots.push(BuildingPlot {
                        position: Vec3::new(rect.min.x, centre.y, rect.min.y),
                        size_tiles: (w.max(1), d.max(1)),
                        facing,
                        taken,
                    });
                    rects.push(rect);
                    t += along + gap;
                } else {
                    t += ts * 2.0;
                }
            }
        }
    }
    plots
}

/// The plot size in tiles for a candidate, preferring a prefab that fits.
#[must_use]
fn choose_footprint(
    prefabs: &[Arc<Prefab>],
    facing: Facing,
    rng: &mut noxel_core::rng::Pcg32,
) -> Option<(u32, u32)> {
    if prefabs.is_empty() {
        return None;
    }
    let start = rng.range_usize(0, prefabs.len());
    for k in 0..prefabs.len() {
        let prefab = &prefabs[(start + k) % prefabs.len()];
        if prefab.size[1] == 0 {
            continue;
        }
        let fp = prefab.footprint();
        if fp.0 == 0 || fp.1 == 0 {
            continue;
        }
        let rot = rotated_footprint(fp, facing);
        // Keep plots inside the town: a huge prefab would swallow the disc.
        if rot.0 <= 12 && rot.1 <= 12 {
            return Some(fp);
        }
    }
    None
}

/// The radius of a town's plaza, in metres.
///
/// Roughly a third of a chunk: big enough to read as a town square, small enough
/// that the chunk at the town's centre still has street frontage to build on.
#[must_use]
pub fn plaza_radius(config: &WorldConfig) -> f32 {
    config.chunk_world_size() * 0.35
}

/// True when a footprint of `half_diagonal` metres around `p` would overlap the
/// plaza.
#[must_use]
pub fn in_plaza(plaza_centre: Vec2, p: Vec2, half_diagonal: f32, radius: f32) -> bool {
    let d = Vec2::new(p.x - plaza_centre.x, p.y - plaza_centre.y).length();
    d < radius + half_diagonal.max(0.0)
}

/// True when the whole rectangle is inside the town disc.
#[must_use]
fn within_radius(rect: Rect, centre: Vec3, radius: f32) -> bool {
    let c = rect.center();
    let d = Vec2::new(c.x - centre.x, c.y - centre.z).length();
    let r = Vec2::new(rect.width() * 0.5, rect.height() * 0.5).length();
    d + r <= radius
}

/// The AABB of a rectangle, for overlap tests against road bounds.
#[must_use]
fn rect_aabb(rect: Rect) -> Aabb {
    Aabb::from_footprint(
        rect.min.x, rect.min.y, rect.max.x, rect.max.y, -1000.0, 2000.0,
    )
}

/// Sanitises a tile size for the town layout.
#[must_use]
fn sanitize_tile_size(tile_size: f32) -> f32 {
    if tile_size.is_finite() && tile_size > 0.0 {
        tile_size
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> WorldConfig {
        WorldConfig::new(1234)
    }

    fn prefab(name: &str, w: u8, h: u8, d: u8) -> Arc<Prefab> {
        let mut voxels = Vec::new();
        for x in 0..w {
            for y in 0..h {
                for z in 0..d {
                    voxels.push(PrefabVoxel { x, y, z, tile: 1 });
                }
            }
        }
        Arc::new(Prefab {
            name: name.to_string(),
            size: [w, h, d],
            tags: vec!["building".to_string()],
            voxels,
            props: Vec::new(),
            spawns: Vec::new(),
            occluders: Vec::new(),
        })
    }

    #[test]
    fn facing_yaws_are_cardinal() {
        for facing in Facing::ALL {
            let v = facing.vector();
            assert!(
                Vec3::from_yaw(facing.yaw()).approx_eq(v, 1e-5),
                "{facing:?}: {:?} vs {v:?}",
                Vec3::from_yaw(facing.yaw())
            );
            assert_eq!(Facing::from_yaw(facing.yaw()), facing);
            assert_eq!(Facing::from_vector(v), facing);
            // Every cardinal yaw is a whole number of quarter turns.
            let quarters = facing.yaw() / core::f32::consts::FRAC_PI_2;
            assert!((quarters - quarters.round()).abs() < 1e-6, "{facing:?}");
        }
    }

    #[test]
    fn facing_from_yaw_is_total() {
        assert_eq!(Facing::from_yaw(f32::NAN), Facing::North);
        assert_eq!(Facing::from_vector(Vec3::ZERO), Facing::North);
        assert_eq!(Facing::from_vector(Vec3::new(0.0, 5.0, 0.0)), Facing::North);
        // Slightly-off angles snap to the nearest cardinal.
        assert_eq!(Facing::from_yaw(0.2), Facing::North);
        assert_eq!(Facing::from_yaw(-1.4), Facing::East);
    }

    #[test]
    fn facing_opposite_and_turn_are_involutions() {
        for facing in Facing::ALL {
            assert_eq!(facing.opposite().opposite(), facing);
            assert_eq!(facing.turned_left().turned_left().opposite(), facing);
            assert_eq!(facing.opposite().vector(), -facing.vector());
        }
    }

    #[test]
    fn style_names_and_densities_are_ordered() {
        assert_eq!(TownStyle::Hamlet.name(), "hamlet");
        assert_eq!(TownStyle::Town.name(), "town");
        for style in TownStyle::ALL {
            assert!((0.0..=1.0).contains(&style.building_density()));
            assert!(style.plot_gap_tiles() > 0.0);
        }
        assert!(TownStyle::Town.building_density() > TownStyle::Hamlet.building_density());
        assert!(TownStyle::Town.plot_gap_tiles() < TownStyle::Hamlet.plot_gap_tiles());
        assert!(TownStyle::Town.building_scale() > TownStyle::Hamlet.building_scale());
    }

    #[test]
    fn rotated_footprint_swaps_on_quarter_turns() {
        assert_eq!(rotated_footprint((5, 3), Facing::North), (5, 3));
        assert_eq!(rotated_footprint((5, 3), Facing::South), (5, 3));
        assert_eq!(rotated_footprint((5, 3), Facing::East), (3, 5));
        assert_eq!(rotated_footprint((5, 3), Facing::West), (3, 5));
    }

    #[test]
    fn names_are_hand_written_not_indices() {
        let c = config();
        let mut names = std::collections::HashSet::new();
        for i in 0..40 {
            let name = town_name(&c, ChunkPos::new(i * 10, -i * 10));
            assert!(name.len() >= 6, "{name}");
            assert!(!name.chars().any(|ch| ch.is_ascii_digit()), "{name}");
            assert_eq!(name, town_name(&c, ChunkPos::new(i * 10, -i * 10)));
            names.insert(name);
        }
        assert!(names.len() > 30, "names must vary: {}", names.len());
    }

    #[test]
    fn plan_is_deterministic_and_lives_at_the_lattice_centre() {
        let c = config();
        let a = TownPlan::generate(&c, ChunkPos::new(10, -20), &[], |_, _| 2.0);
        let b = TownPlan::generate(&c, ChunkPos::new(10, -20), &[], |_, _| 2.0);
        assert_eq!(a.center_chunk, ChunkPos::new(10, -20));
        assert_eq!(a.name, b.name);
        assert_eq!(a.style, b.style);
        assert_eq!(a.building_plots.len(), b.building_plots.len());
        assert_eq!(a.streets.len(), b.streets.len());
        assert_eq!(a.plaza_center.y, 2.0);
        for (p, q) in a.building_plots.iter().zip(b.building_plots.iter()) {
            assert_eq!(p.position, q.position);
            assert_eq!(p.facing, q.facing);
            assert_eq!(p.taken, q.taken);
        }
    }

    #[test]
    fn different_seeds_give_different_towns() {
        let a = TownPlan::generate(&WorldConfig::new(1), ChunkPos::new(0, 0), &[], |_, _| 0.0);
        let b = TownPlan::generate(&WorldConfig::new(2), ChunkPos::new(0, 0), &[], |_, _| 0.0);
        assert!(
            a.name != b.name || a.building_plots.len() != b.building_plots.len(),
            "{} vs {}",
            a.name,
            b.name
        );
    }

    #[test]
    fn empty_prefab_list_still_builds_a_town() {
        let c = config();
        let plan = TownPlan::generate(&c, ChunkPos::new(0, 0), &[], |_, _| 0.0);
        let buildings = plan.instantiate(&c, &[], |_, _| 0.0);
        let taken = plan.building_plots.iter().filter(|p| p.taken).count();
        assert!(taken > 0, "a town must never be empty");
        assert_eq!(buildings.len(), taken);
        for b in &buildings {
            assert_eq!(b.prefab, PROCEDURAL_PREFAB);
            assert_eq!(b.occluders.len(), 2, "one wall run and one roof");
            assert!(b.bounds.is_finite());
        }
    }

    #[test]
    fn every_plot_is_inside_the_radius_and_out_of_the_plaza() {
        let c = config();
        for i in -3..3 {
            let plan = TownPlan::generate(&c, ChunkPos::new(i * 10, 0), &[], |_, _| 0.0);
            let radius = plan.radius_world(&c);
            assert_eq!(plan.radius_chunks, c.town_radius_chunks);
            let plaza_centre = Vec2::new(plan.plaza_center.x, plan.plaza_center.z);
            let reach = plaza_radius(&c);
            for plot in &plan.building_plots {
                let centre = plot.center(c.tile_size);
                let d = Vec2::new(
                    centre.x - plan.plaza_center.x,
                    centre.z - plan.plaza_center.z,
                )
                .length();
                assert!(d <= radius, "plot at {d} m, radius {radius} m");
                let rect = plot.rect(c.tile_size);
                let half_diagonal = Vec2::new(rect.width() * 0.5, rect.height() * 0.5).length();
                assert!(
                    !in_plaza(plaza_centre, rect.center(), half_diagonal, reach),
                    "plot in plaza: {plot:?}"
                );
            }
        }
    }

    #[test]
    fn plots_do_not_overlap() {
        let c = config();
        for i in 0..6 {
            let plan = TownPlan::generate(&c, ChunkPos::new(i * 10, i * 10), &[], |_, _| 0.0);
            let rects: Vec<Rect> = plan
                .building_plots
                .iter()
                .map(|p| p.rect(c.tile_size))
                .collect();
            for a in 0..rects.len() {
                for b in (a + 1)..rects.len() {
                    assert!(
                        !rects[a].intersects(&rects[b]),
                        "{} vs {} in {}",
                        a,
                        b,
                        plan.name
                    );
                }
            }
        }
    }

    #[test]
    fn plots_avoid_streets() {
        let c = config();
        let plan = TownPlan::generate(&c, ChunkPos::new(0, 0), &[], |_, _| 0.0);
        for plot in &plan.building_plots {
            for street in &plan.streets {
                assert!(
                    !street
                        .aabb(0.0)
                        .intersects(&rect_aabb(plot.rect(c.tile_size))),
                    "plot {} overlaps {}",
                    plan.building_plots.len(),
                    plan.streets.len()
                );
            }
        }
    }

    #[test]
    fn every_building_faces_a_street() {
        let c = config();
        let plan = TownPlan::generate(&c, ChunkPos::new(0, 0), &[], |_, _| 0.0);
        let buildings = plan.instantiate(&c, &[], |_, _| 0.0);
        assert!(!buildings.is_empty());
        for building in &buildings {
            let front = Vec3::new(
                building.bounds.center().x,
                building.origin.y,
                building.bounds.center().z,
            ) + building.facing.vector()
                * (building.size_tiles.0 as f32 * c.tile_size * 0.5);
            let d = plan.distance_to_street(front);
            assert!(
                d < c.chunk_world_size() * 0.5,
                "{} faces {d} m from a street",
                plan.name
            );
        }
    }

    #[test]
    fn prefabs_are_used_when_they_fit() {
        let c = config();
        let prefabs = vec![prefab("house_small", 5, 3, 4)];
        let plan = TownPlan::generate(&c, ChunkPos::new(0, 0), &prefabs, |_, _| 0.0);
        let buildings = plan.instantiate(&c, &prefabs, |_, _| 0.0);
        assert!(!buildings.is_empty());
        for building in &buildings {
            assert_eq!(building.prefab, "house_small");
            assert!(building.bounds.is_finite());
            assert!(!building.occluders.is_empty());
            // Merged: 60 voxels must not become 60 boxes.
            assert!(building.occluders.len() < 5, "{}", building.occluders.len());
        }
    }

    #[test]
    fn merging_a_solid_box_yields_one_box() {
        let voxels: Vec<PrefabVoxel> = (0..4)
            .flat_map(|x| {
                (0..3).flat_map(move |y| (0..4).map(move |z| PrefabVoxel { x, y, z, tile: 1 }))
            })
            .collect();
        let boxes = merge_voxel_runs(&voxels);
        assert_eq!(boxes.len(), 1, "{boxes:?}");
        assert_eq!(boxes[0], [0, 0, 0, 3, 2, 3]);
        assert!(boxes.len() < voxels.len());
    }

    #[test]
    fn merging_two_separate_walls_yields_two_boxes() {
        let mut voxels = Vec::new();
        for x in [0u8, 5] {
            for y in 0..3 {
                for z in 0..4 {
                    voxels.push(PrefabVoxel { x, y, z, tile: 1 });
                }
            }
        }
        let boxes = merge_voxel_runs(&voxels);
        assert_eq!(boxes.len(), 2, "{boxes:?}");
        assert!(boxes.len() < voxels.len());
    }

    #[test]
    fn merging_empty_voxels_is_empty() {
        assert!(merge_voxel_runs(&[]).is_empty());
    }

    #[test]
    fn buildings_per_town_is_respected() {
        let c = config();
        let plan = TownPlan::generate(&c, ChunkPos::new(0, 0), &[], |_, _| 0.0);
        let taken = plan.building_plots.iter().filter(|p| p.taken).count() as u32;
        assert!(taken <= c.buildings_per_town.max(1));
        assert!(plan.building_plots.len() as u32 >= taken);
        assert!(
            plan.free_plots().count() > 0,
            "the plan reserves plots for the player"
        );
    }

    #[test]
    fn zero_buildings_per_town_gives_a_reserved_layout() {
        let mut c = config();
        c.buildings_per_town = 0;
        let plan = TownPlan::generate(&c, ChunkPos::new(0, 0), &[], |_, _| 0.0);
        assert!(!plan.building_plots.is_empty(), "the layout still exists");
        assert!(
            plan.building_plots.iter().all(|plot| !plot.taken),
            "nothing may be built when the config asks for no buildings"
        );
        assert_eq!(plan.free_plots().count(), plan.building_plots.len());
        assert!(plan.instantiate(&c, &[], |_, _| 0.0).is_empty());
        assert!(!plan.streets.is_empty(), "streets still exist");
    }

    #[test]
    fn streets_include_a_connector_towards_the_lattice() {
        let c = config();
        let plan = TownPlan::generate(&c, ChunkPos::new(0, 0), &[], |_, _| 0.0);
        let main: Vec<_> = plan.streets.iter().filter(|s| s.is_main).collect();
        assert!(main.len() >= 3, "main cross plus connector");
        // The connector starts at the plaza.
        assert!(plan.is_on_street(plan.plaza_center));
        // And it stays inside the disc.
        for street in &plan.streets {
            for p in [street.from, street.to] {
                let d = Vec2::new(p.x - plan.plaza_center.x, p.z - plan.plaza_center.z).length();
                assert!(d <= plan.radius_world(&c) + 1e-3, "{d}");
            }
        }
    }

    #[test]
    fn contains_world_matches_the_radius() {
        let c = config();
        let plan = TownPlan::generate(&c, ChunkPos::new(0, 0), &[], |_, _| 0.0);
        assert!(plan.contains_world(&c, plan.plaza_center));
        let outside = plan.plaza_center + Vec3::new(plan.radius_world(&c) + 5.0, 0.0, 0.0);
        assert!(!plan.contains_world(&c, outside));
    }

    #[test]
    fn clip_xz_drops_degenerate_slivers() {
        let b = Aabb::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 4.0, 10.0));
        let far = Aabb::new(Vec3::new(20.0, 0.0, 20.0), Vec3::new(30.0, 4.0, 30.0));
        assert!(clip_xz(&b, &far).is_none());
        let half = Aabb::new(Vec3::new(5.0, -10.0, 5.0), Vec3::new(30.0, 10.0, 30.0));
        let clipped = clip_xz(&b, &half).unwrap();
        assert_eq!(clipped.min.x, 5.0);
        assert_eq!(clipped.max.x, 10.0);
        assert_eq!(clipped.min.y, 0.0);
        assert_eq!(clipped.max.y, 4.0);
    }

    #[test]
    fn building_helpers_are_consistent() {
        let c = config();
        let plan = TownPlan::generate(&c, ChunkPos::new(0, 0), &[], |_, _| 1.0);
        let buildings = plan.instantiate(&c, &[], |_, _| 1.0);
        for b in &buildings {
            assert!(b.bounds.is_finite());
            assert!(b.height() > 0.0);
            assert!(b.contains_xz(b.bounds.center()));
            assert_ne!(b.id(), 0);
            let region = b.bounds.expanded(100.0);
            assert!(!b.colliders_in(&region).is_empty());
            assert!(
                b.colliders_in(&Aabb::new(
                    Vec3::new(1e6, 1e6, 1e6),
                    Vec3::new(1e6 + 1.0, 1e6 + 1.0, 1e6 + 1.0)
                ))
                .is_empty()
            );
        }
    }
}
