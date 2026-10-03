//! The software rasterizer.
//!
//! # Pipeline
//!
//! ```text
//! scene ─▶ per-instance vertex transform ─▶ near-plane clip ─▶ backface cull
//!       ─▶ screen-space triangles ─▶ tile binning ─▶ per-tile raster
//!       ─▶ fragment shading (lights + shadow map + fog + texture)
//!       ─▶ transparent pass (sorted back to front)
//!       ─▶ post-process (linear buffer)
//! ```
//!
//! # Why tile binning
//!
//! Even at 320×180 a frame can contain tens of thousands of triangles, and
//! walking every triangle for every pixel of its bounding box wastes most of
//! the work on overdraw. Splitting the screen into 32×32 tiles and bucketing
//! triangles into the tiles they touch means each tile's inner loop only visits
//! triangles that can actually cover it, the depth buffer stays in cache, and
//! the tile list is already in the shape a parallel rasterizer needs: each tile
//! writes a disjoint pixel rectangle, so tiles can be handed to
//! [`noxel_core::jobs::JobPool`] with no synchronisation at all.
//!
//! # Why it is pixel-art friendly
//!
//! * Textures sample with **nearest** filtering and a half-texel inset, so a
//!   16×16 tile never bleeds into its atlas neighbour.
//! * Depth comes straight from the projection's `[0, 1]` range, so the test is
//!   a plain `<`.
//! * Fragments are linear and immediately multiplied by the material's
//!   hand-picked colour, so an unlit sprite round-trips to exactly the bytes the
//!   artist drew (see [`crate::Framebuffer::resolve`]).

pub mod post;
pub mod shadow;

use std::sync::atomic::{AtomicU8, Ordering};

use noxel_core::jobs::JobPool;
use noxel_core::math::{Vec2, Vec3, Vec4};
use noxel_core::time::Stopwatch;

use crate::framebuffer::Framebuffer;
use crate::material::{AlphaMode, Material, MaterialHandle};
use crate::renderer::{
    CameraView, CullCounts, FrameTimer, RenderSettings, RenderStats, Renderer, ShadingMode,
    VisibleSet,
};
use crate::scene::{InstanceHandle, Scene};
use noxel_asset::texture::Texture;

use self::shadow::ShadowMap;

/// Tile edge length in pixels.
///
/// 32 fits a 320-wide pixel-art frame into ten columns and keeps each tile's
/// triangle list comfortably inside L1.
pub const TILE_SIZE: u32 = 32;

/// A vertex in clip space with everything the fragment stage needs.
#[derive(Clone, Copy, Debug)]
struct ClipVertex {
    /// Clip-space position.
    clip: Vec4,
    /// World-space position, for lighting and fog.
    world: Vec3,
    /// Texture coordinate (already UV-transformed by the material).
    uv: Vec2,
    /// World-space normal.
    normal: Vec3,
    /// `1 / clip.w`, precomputed for perspective-correct interpolation.
    inv_w: f32,
}

impl ClipVertex {
    /// Point on the segment between two vertices at parameter `t`.
    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        Self {
            clip: a.clip + (b.clip - a.clip) * t,
            world: a.world.lerp(b.world, t),
            uv: a.uv.lerp(b.uv, t),
            normal: a.normal.lerp(b.normal, t).normalize_or_zero(),
            inv_w: a.inv_w + (b.inv_w - a.inv_w) * t,
        }
    }
}

/// A triangle ready to rasterize, in screen space.
#[derive(Clone, Copy, Debug)]
struct ScreenTriangle {
    /// Screen x, screen y and normalised device depth for each vertex.
    screen: [[f32; 3]; 3],
    /// `1 / w` per vertex, for perspective-correct interpolation.
    inv_w: [f32; 3],
    /// World position per vertex.
    world: [Vec3; 3],
    /// Texture coordinate per vertex.
    uv: [Vec2; 3],
    /// Normal per vertex.
    normal: [Vec3; 3],
    /// Index into the frame's material table.
    material: u32,
    /// Instance id written to the ID buffer.
    instance: u32,
    /// Signed screen-space area (`< 0` means front-facing).
    area: f32,
}

impl ScreenTriangle {
    /// True when the triangle has no area on screen.
    #[inline]
    fn is_degenerate(&self) -> bool {
        self.area.abs() < 1e-7
    }

    /// Screen-space bounding box clamped to `width × height`.
    #[inline]
    fn bounds(&self, width: u32, height: u32) -> (i32, i32, i32, i32) {
        let min_x = self.screen[0][0]
            .min(self.screen[1][0])
            .min(self.screen[2][0])
            .floor()
            .max(0.0);
        let min_y = self.screen[0][1]
            .min(self.screen[1][1])
            .min(self.screen[2][1])
            .floor()
            .max(0.0);
        let max_x = (self.screen[0][0]
            .max(self.screen[1][0])
            .max(self.screen[2][0])
            .ceil())
        .min(width as f32);
        let max_y = (self.screen[0][1]
            .max(self.screen[1][1])
            .max(self.screen[2][1])
            .ceil())
        .min(height as f32);
        (min_x as i32, min_y as i32, max_x as i32, max_y as i32)
    }

    /// The inclusive tile rectangle the triangle touches.
    #[inline]
    fn tiles(&self, width: u32, height: u32, tile_size: u32) -> (u32, u32, u32, u32) {
        let (min_x, min_y, max_x, max_y) = self.bounds(width, height);
        if max_x <= min_x || max_y <= min_y {
            return (0, 0, 0, 0);
        }
        (
            (min_x.max(0) as u32) / tile_size,
            (min_y.max(0) as u32) / tile_size,
            ((max_x - 1).max(0) as u32) / tile_size,
            ((max_y - 1).max(0) as u32) / tile_size,
        )
    }
}

/// The per-instance material summary the fragment loop reads, copied out of the
/// scene so shading never touches the slot maps.
#[derive(Clone, Copy, Debug)]
struct ShadeMaterial {
    /// The scene material, used to fetch the texture.
    handle: MaterialHandle,
    /// Linear base colour.
    base_color: [f32; 3],
    /// Linear emissive.
    emissive: [f32; 3],
    /// Combined material + instance alpha.
    alpha: f32,
    /// Skip lighting entirely.
    unlit: bool,
    /// Shade both faces.
    double_sided: bool,
    /// Transparency handling.
    alpha_mode: AlphaMode,
    /// Whether the surface receives shadows.
    receive_shadow: bool,
}

/// A triangle-list bucket per screen tile, built with a counting sort.
#[derive(Clone, Debug, Default)]
pub struct TileBins {
    cols: u32,
    rows: u32,
    tile_size: u32,
    /// `offsets[i]..offsets[i + 1]` is tile `i`'s slice of `indices`.
    offsets: Vec<u32>,
    /// Per-tile fill cursor, used only while building.
    cursors: Vec<u32>,
    /// Triangle indices, grouped by tile.
    indices: Vec<u32>,
}

impl TileBins {
    /// An empty bin set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cols: 0,
            rows: 0,
            tile_size: TILE_SIZE,
            offsets: vec![0],
            cursors: Vec::new(),
            indices: Vec::new(),
        }
    }

    /// Number of tiles (including empty ones).
    #[must_use]
    pub fn tile_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Total triangle references held across all tiles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// True when nothing was binned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// The triangle indices for tile `index` (row-major).
    #[must_use]
    pub fn tile(&self, index: usize) -> &[u32] {
        if index + 1 >= self.offsets.len() {
            return &[];
        }
        let a = self.offsets[index] as usize;
        let b = self.offsets[index + 1] as usize;
        &self.indices[a..b]
    }

    /// Grid dimensions `(columns, rows)`.
    #[must_use]
    pub fn grid(&self) -> (u32, u32) {
        (self.cols, self.rows)
    }

    /// Rebins the triangles for a frame of the given size.
    pub fn build(&mut self, width: u32, height: u32, tile_size: u32, triangles: &[ScreenTriangle]) {
        self.tile_size = tile_size.max(8);
        self.cols = width.div_ceil(self.tile_size).max(1);
        self.rows = height.div_ceil(self.tile_size).max(1);
        let count = (self.cols as usize) * (self.rows as usize);
        self.offsets.clear();
        self.offsets.resize(count + 1, 0);
        self.cursors.clear();
        self.cursors.resize(count, 0);

        // Pass 1: count.
        for t in triangles {
            if t.is_degenerate() {
                continue;
            }
            let (tx0, ty0, tx1, ty1) = t.tiles(width, height, self.tile_size);
            for ty in ty0..=ty1 {
                for tx in tx0..=tx1 {
                    self.cursors[(ty * self.cols + tx) as usize] += 1;
                }
            }
        }
        // Prefix sum.
        let mut running = 0u32;
        for i in 0..count {
            self.offsets[i] = running;
            running += self.cursors[i];
            self.cursors[i] = self.offsets[i];
        }
        self.offsets[count] = running;
        self.indices.clear();
        self.indices.resize(running as usize, 0);

        // Pass 2: scatter.
        for (i, t) in triangles.iter().enumerate() {
            if t.is_degenerate() {
                continue;
            }
            let (tx0, ty0, tx1, ty1) = t.tiles(width, height, self.tile_size);
            for ty in ty0..=ty1 {
                for tx in tx0..=tx1 {
                    let slot = &mut self.cursors[(ty * self.cols + tx) as usize];
                    self.indices[*slot as usize] = i as u32;
                    *slot += 1;
                }
            }
        }
    }

    /// Releases the triangle list but keeps the tile grid.
    pub fn clear(&mut self) {
        self.indices.clear();
        self.offsets.clear();
        self.offsets.push(0);
        self.cursors.clear();
    }

    /// Bytes of heap used.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.offsets.capacity() * 4 + self.indices.capacity() * 4 + self.cursors.capacity() * 4
    }

    /// The number of triangles that ended up in a given tile.
    #[must_use]
    pub fn tile_load(&self, index: usize) -> usize {
        self.tile(index).len()
    }
}

/// Everything the fragment stage reads, bundled so the parallel tile loop can
/// borrow it immutably while the framebuffer is borrowed mutably.
struct ShadeContext<'a> {
    scene: &'a Scene,
    camera: &'a CameraView,
    settings: &'a RenderSettings,
    materials: &'a [ShadeMaterial],
    shadow: &'a ShadowMap,
    /// Framebuffer width, i.e. the row stride of the full buffer.
    width: u32,
}

/// One row band of the framebuffer, owned exclusively by one worker.
///
/// Both the single-threaded and the parallel path use this: with a [`JobPool`]
/// there is one band per worker, without one there is a single band covering the
/// whole frame. One code path means the two can never diverge.
struct Band<'a> {
    color: &'a mut [f32],
    depth: &'a mut [f32],
    ids: &'a mut [u32],
    /// First tile row this band owns.
    tile_row_start: u32,
    /// Number of tile rows in this band.
    tile_rows: u32,
    /// First framebuffer row this band owns (`tile_row_start * TILE_SIZE`).
    row_start: u32,
    /// Number of framebuffer rows this band owns.
    rows: u32,
    /// Framebuffer width, the row stride of the full buffer.
    stride: u32,
}

impl Band<'_> {
    /// Index of a band-local pixel in this band's slices.
    #[inline]
    fn pixel_index(&self, x: i32, local_y: i32) -> Option<usize> {
        if local_y < 0 || x < 0 || x as u32 >= self.stride || local_y as u32 >= self.rows {
            return None;
        }
        Some((local_y as usize) * (self.stride as usize) + x as usize)
    }
}

/// The software rasterizer.
pub struct RasterRenderer {
    width: u32,
    height: u32,
    shadow_map: ShadowMap,
    bins: TileBins,
    clip_vertices: Vec<ClipVertex>,
    triangles: Vec<ScreenTriangle>,
    opaque_triangles: Vec<ScreenTriangle>,
    materials: Vec<ShadeMaterial>,
    /// `(instance order, material index)` for the transparent pass.
    translucent_orders: Vec<(u32, u32)>,
    jobs: Option<JobPool>,
    last_stats: RenderStats,
    last_counts: CullCounts,
}

impl RasterRenderer {
    /// Creates a rasterizer for the given target size.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
            shadow_map: ShadowMap::new(1024),
            bins: TileBins::new(),
            clip_vertices: Vec::new(),
            triangles: Vec::new(),
            opaque_triangles: Vec::new(),
            materials: Vec::new(),
            translucent_orders: Vec::new(),
            jobs: None,
            last_stats: RenderStats::default(),
            last_counts: CullCounts::default(),
        }
    }

    /// Enables multi-threaded tile rasterization.
    ///
    /// The result is bit-identical to the single-threaded path (the bands are
    /// disjoint and no float operation is reassociated), which the test suite
    /// asserts.
    #[must_use]
    pub fn with_jobs(mut self, jobs: JobPool) -> Self {
        self.jobs = Some(jobs);
        self
    }

    /// Resizes the shadow map.
    pub fn set_shadow_map_size(&mut self, size: u32) {
        self.shadow_map.resize(size);
    }

    /// Culling statistics from the most recent frame.
    #[must_use]
    pub fn last_counts(&self) -> CullCounts {
        self.last_counts
    }

    /// The shadow map, for the debug overlay and for tests.
    #[must_use]
    pub fn shadow_map(&self) -> &ShadowMap {
        &self.shadow_map
    }

    /// The tile bins from the last frame.
    #[must_use]
    pub fn tile_bins(&self) -> &TileBins {
        &self.bins
    }

    /// Number of screen-space triangles emitted by the last frame.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// Statistics from the most recent frame.
    #[must_use]
    pub fn last_stats(&self) -> RenderStats {
        self.last_stats
    }

    /// Outlines every non-empty tile, for the debug overlay.
    pub fn debug_draw_tiles(&self, target: &mut Framebuffer) {
        let (cols, rows) = self.bins.grid();
        let size = self.bins.tile_size.max(1);
        for ty in 0..rows {
            for tx in 0..cols {
                if self.bins.tile((ty * cols + tx) as usize).is_empty() {
                    continue;
                }
                let x0 = tx * size;
                let y0 = ty * size;
                let x1 = (x0 + size - 1).min(target.width().saturating_sub(1));
                let y1 = (y0 + size - 1).min(target.height().saturating_sub(1));
                for x in x0..=x1 {
                    target.add(x, y0, [0.08, 0.0, 0.0]);
                    target.add(x, y1, [0.08, 0.0, 0.0]);
                }
                for y in y0..=y1 {
                    target.add(x0, y, [0.08, 0.0, 0.0]);
                    target.add(x1, y, [0.08, 0.0, 0.0]);
                }
            }
        }
    }
}

impl Renderer for RasterRenderer {
    fn name(&self) -> &'static str {
        "software-raster"
    }

    fn mode(&self) -> ShadingMode {
        ShadingMode::Raster
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
    }

    fn release_cached(&mut self) {
        self.bins.clear();
        self.triangles.clear();
        self.opaque_triangles.clear();
        self.clip_vertices.clear();
        self.materials.clear();
    }

    fn render(
        &mut self,
        scene: &Scene,
        camera: &CameraView,
        visible: Option<&VisibleSet>,
        target: &mut Framebuffer,
        settings: &RenderSettings,
    ) -> RenderStats {
        let mut timer = FrameTimer::start();
        let mut stats = RenderStats {
            mode: ShadingMode::Raster,
            ..Default::default()
        };
        if self.width != target.width() || self.height != target.height() {
            self.resize(target.width(), target.height());
        }
        target.clear([scene.background.r, scene.background.g, scene.background.b]);

        // ---- 1. Collect the instances to draw ------------------------------
        let mut counts = CullCounts::default();
        let handles: Vec<(InstanceHandle, f32)> = match visible {
            Some(set) => {
                counts = set.counts;
                set.items.iter().map(|i| (i.instance, i.alpha)).collect()
            }
            None => {
                let all: Vec<(InstanceHandle, f32)> = scene
                    .instances()
                    .filter(|(_, i)| i.visible)
                    .map(|(h, _)| (h, 1.0))
                    .collect();
                counts.considered = all.len();
                counts.drawn = all.len();
                all
            }
        };
        if counts.considered == 0 {
            counts.considered = handles.len();
            counts.drawn = handles.len();
        }
        stats.instances = handles.len();

        // ---- 2. Shadows ----------------------------------------------------
        if settings.shadows {
            if self.shadow_map.size() != settings.shadow_map_size {
                self.shadow_map.resize(settings.shadow_map_size);
            }
            if let Some(direction) = scene.primary_sun().and_then(|l| l.direction()) {
                let (extent, range) = match scene.primary_sun() {
                    Some(crate::light::Light::Directional { shadow_extent, .. }) => {
                        (*shadow_extent, shadow_extent * 4.0)
                    }
                    _ => (40.0, 160.0),
                };
                self.shadow_map
                    .begin(direction, camera.position, extent, range, true);
                self.render_shadow_pass(scene, &handles, &mut stats);
                stats.shadow_map_size = self.shadow_map.size();
            }
        }

        // ---- 3. Transform, clip and bin ------------------------------------
        self.materials.clear();
        self.triangles.clear();
        self.opaque_triangles.clear();
        self.clip_vertices.clear();
        self.translucent_orders.clear();

        let view_projection = camera.view_projection;
        let width = target.width();
        let height = target.height();

        for (order, (handle, alpha)) in handles.iter().enumerate() {
            let Some(instance) = scene.instance(*handle) else {
                continue;
            };
            let Some(mesh) = scene.mesh(instance.mesh) else {
                continue;
            };
            let Some(material) = scene.material(instance.material) else {
                continue;
            };
            if mesh.is_empty() {
                continue;
            }
            let index = self.materials.len() as u32;
            self.materials
                .push(shade_material(material, instance.material, *alpha));
            let translucent = material.alpha_mode.is_transparent();
            if translucent {
                // Pack (order, material index) so the transparent pass can sort
                // by distance while still finding its triangles.
                self.translucent_orders.push((order as u32, index));
            }

            let model = instance.transform.to_mat4();
            let mvp = view_projection * model;
            let normal_matrix = model
                .inverse()
                .unwrap_or(noxel_core::math::Mat4::IDENTITY)
                .transpose();
            let base = self.clip_vertices.len();
            for v in &mesh.vertices {
                let world = model.transform_point3(v.position);
                let clip = mvp.transform_point4(v.position.extend(1.0));
                let normal = normal_matrix
                    .transform_vector3(v.normal)
                    .normalize_or_zero();
                self.clip_vertices.push(ClipVertex {
                    clip,
                    world,
                    uv: material.transform_uv(v.uv),
                    normal: if normal.length_squared() > 1e-8 {
                        normal
                    } else {
                        v.normal
                    },
                    inv_w: if clip.w.abs() > 1e-6 {
                        1.0 / clip.w
                    } else {
                        0.0
                    },
                });
            }
            let out = if translucent {
                &mut self.triangles
            } else {
                &mut self.opaque_triangles
            };
            for tri in 0..mesh.triangle_count() {
                let a = self.clip_vertices[base + mesh.indices[tri * 3] as usize];
                let b = self.clip_vertices[base + mesh.indices[tri * 3 + 1] as usize];
                let c = self.clip_vertices[base + mesh.indices[tri * 3 + 2] as usize];
                emit_clipped(
                    a,
                    b,
                    c,
                    index,
                    instance.user_data as u32,
                    material.double_sided,
                    width,
                    height,
                    settings.backface_culling,
                    out,
                );
            }
            stats.triangles_submitted += mesh.triangle_count();
        }

        // ---- 4. Opaque pass -------------------------------------------------
        self.bins
            .build(width, height, TILE_SIZE, &self.opaque_triangles);
        stats.ms_geometry = timer.geometry.elapsed_ms() as f32;
        timer.shade = Stopwatch::start();
        let (pixels, triangles_drawn) = rasterize(
            &mut self.jobs,
            &mut self.bins,
            &self.opaque_triangles,
            &self.materials,
            &self.shadow_map,
            scene,
            camera,
            settings,
            target,
            true,
        );
        stats.pixels_shaded += pixels;
        stats.triangles_drawn += triangles_drawn;

        // ---- 5. Transparent pass, back to front ------------------------------
        if !self.triangles.is_empty() {
            // Sort the *instances* back to front, then draw each instance's
            // triangles. Sorting per triangle would be more accurate but would
            // also break the batching that keeps this pass cheap.
            let mut order: Vec<(f32, u32)> = self
                .translucent_orders
                .iter()
                .map(|(order, material)| {
                    let distance = handles
                        .get(*order as usize)
                        .and_then(|(h, _)| scene.instance(*h))
                        .map(|i| camera.position.distance(i.bounds().center()))
                        .unwrap_or(0.0);
                    (distance, *material)
                })
                .collect();
            order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(core::cmp::Ordering::Equal));
            for (_, material) in order {
                let subset: Vec<ScreenTriangle> = self
                    .triangles
                    .iter()
                    .filter(|t| t.material == material)
                    .copied()
                    .collect();
                if subset.is_empty() {
                    continue;
                }
                let (pixels, triangles_drawn) = rasterize(
                    &mut self.jobs,
                    &mut self.bins,
                    &subset,
                    &self.materials,
                    &self.shadow_map,
                    scene,
                    camera,
                    settings,
                    target,
                    false,
                );
                stats.pixels_shaded += pixels;
                stats.triangles_drawn += triangles_drawn;
            }
        }

        stats.ms_shade = timer.shade.elapsed_ms() as f32;
        timer.post = Stopwatch::start();
        if settings.debug_overlay {
            self.debug_draw_tiles(target);
        }
        stats.ms_post = timer.post.elapsed_ms() as f32;

        counts.faded = handles.iter().filter(|(_, a)| *a < 1.0).count();
        self.last_counts = counts;
        timer.finish(&mut stats);
        self.last_stats = stats;
        stats
    }
}

impl RasterRenderer {
    /// Renders the sun's depth map.
    fn render_shadow_pass(
        &mut self,
        scene: &Scene,
        handles: &[(InstanceHandle, f32)],
        stats: &mut RenderStats,
    ) {
        let light_vp = self.shadow_map.light_view_projection();
        for (handle, _) in handles {
            let Some(instance) = scene.instance(*handle) else {
                continue;
            };
            if !instance.visible || !instance.flags.cast_shadow {
                continue;
            }
            let Some(mesh) = scene.mesh(instance.mesh) else {
                continue;
            };
            let Some(material) = scene.material(instance.material) else {
                continue;
            };
            if !material.cast_shadow || mesh.is_empty() {
                continue;
            }
            let mvp = light_vp * instance.transform.to_mat4();
            for tri in 0..mesh.triangle_count() {
                let a = mvp.transform_point4(
                    mesh.vertices[mesh.indices[tri * 3] as usize]
                        .position
                        .extend(1.0),
                );
                let b = mvp.transform_point4(
                    mesh.vertices[mesh.indices[tri * 3 + 1] as usize]
                        .position
                        .extend(1.0),
                );
                let c = mvp.transform_point4(
                    mesh.vertices[mesh.indices[tri * 3 + 2] as usize]
                        .position
                        .extend(1.0),
                );
                for clipped in clip_near_positions(a, b, c) {
                    self.shadow_map
                        .raster_triangle(clipped[0], clipped[1], clipped[2]);
                }
            }
            stats.draw_calls += 1;
        }
    }
}

/// Bins `triangles` and rasterizes them, in parallel when a pool is set.
///
/// Returns the number of fragments shaded. The single-threaded and the parallel
/// path both run [`raster_tile_in_band`] over the same bands, so they cannot
/// diverge.
///
/// Free-standing rather than a method so the job pool and bins (borrowed
/// mutably) and the triangle list and material table (borrowed immutably by the
/// shading context) are disjoint field borrows.
#[allow(clippy::too_many_arguments)]
fn rasterize(
    jobs: &mut Option<JobPool>,
    bins: &mut TileBins,
    triangles: &[ScreenTriangle],
    materials: &[ShadeMaterial],
    shadow: &ShadowMap,
    scene: &Scene,
    camera: &CameraView,
    settings: &RenderSettings,
    target: &mut Framebuffer,
    depth_write: bool,
) -> (usize, usize) {
    {
        bins.build(target.width(), target.height(), TILE_SIZE, triangles);

        let (cols, rows) = bins.grid();
        let width = target.width();
        let ctx = ShadeContext {
            scene,
            camera,
            settings,
            materials,
            shadow,
            width,
        };

        // Bands are whole *tile* rows, so every band's pixel rows line up with
        // the tile grid. Without a pool a single band covers the frame: the same
        // code, just not split up.
        let height = target.height();
        let tile_rows_per_band = if jobs.is_some() { 4 } else { rows.max(1) };
        let band_count = rows.div_ceil(tile_rows_per_band).max(1);
        let mut bands: Vec<Band<'_>> = Vec::with_capacity(band_count as usize);
        {
            let (color, depth, ids) = target.split_mut();
            // Consume the buffers with `split_at_mut` rather than indexing, so
            // each band owns a disjoint slice and the borrow checker can see it.
            let mut color_rest: &mut [f32] = color;
            let mut depth_rest: &mut [f32] = depth;
            let mut ids_rest: &mut [u32] = ids;
            for band in 0..band_count {
                let tile_row_start = band * tile_rows_per_band;
                let tile_rows = tile_rows_per_band.min(rows - tile_row_start);
                let row_start = tile_row_start * TILE_SIZE;
                let pixel_rows = (tile_rows * TILE_SIZE).min(height.saturating_sub(row_start));
                let color_len = pixel_rows as usize * width as usize * 3;
                let depth_len = pixel_rows as usize * width as usize;
                if color_rest.len() < color_len
                    || depth_rest.len() < depth_len
                    || ids_rest.len() < depth_len
                {
                    break;
                }
                let (c, c_rest) = color_rest.split_at_mut(color_len);
                let (d, d_rest) = depth_rest.split_at_mut(depth_len);
                let (i, i_rest) = ids_rest.split_at_mut(depth_len);
                color_rest = c_rest;
                depth_rest = d_rest;
                ids_rest = i_rest;
                bands.push(Band {
                    color: c,
                    depth: d,
                    ids: i,
                    tile_row_start,
                    tile_rows,
                    row_start,
                    rows: pixel_rows,
                    stride: width,
                });
            }
        }

        // One flag per triangle so the "how many triangles actually drew
        // something" statistic stays exact even when a triangle spans several
        // bands processed by different threads.
        let drawn_flags: Vec<AtomicU8> = (0..triangles.len()).map(|_| AtomicU8::new(0)).collect();
        let shade = |band: &mut Band<'_>| -> (usize, usize) {
            let (mut pixels, mut new_triangles) = (0usize, 0usize);
            for local_tile_row in 0..band.tile_rows {
                let ty = band.tile_row_start + local_tile_row;
                for tx in 0..cols {
                    for &tri_index in bins.tile((ty * cols + tx) as usize) {
                        let Some(t) = triangles.get(tri_index as usize) else {
                            continue;
                        };
                        let shaded =
                            raster_tile_in_band(t, &ctx, band, tx, local_tile_row, depth_write);
                        if shaded > 0 {
                            pixels += shaded;
                            if let Some(flag) = drawn_flags.get(tri_index as usize) {
                                if flag.swap(1, Ordering::Relaxed) == 0 {
                                    new_triangles += 1;
                                }
                            }
                        }
                    }
                }
            }
            (pixels, new_triangles)
        };

        let (pixels, new_triangles) = if let Some(pool) = jobs.take() {
            let pixels = std::sync::atomic::AtomicUsize::new(0);
            let tris = std::sync::atomic::AtomicUsize::new(0);
            {
                let shade = &shade;
                pool.parallel_for(&mut bands, |_i, band| {
                    let (p, t) = shade(band);
                    pixels.fetch_add(p, Ordering::Relaxed);
                    tris.fetch_add(t, Ordering::Relaxed);
                });
            }
            *jobs = Some(pool);
            (pixels.load(Ordering::Relaxed), tris.load(Ordering::Relaxed))
        } else {
            let (mut pixels, mut tris) = (0usize, 0usize);
            for band in &mut bands {
                let (p, t) = shade(band);
                pixels += p;
                tris += t;
            }
            (pixels, tris)
        };
        (pixels, new_triangles)
    }
}

/// Rasterizes one triangle clipped to one tile, into a band of the framebuffer.
fn raster_tile_in_band(
    t: &ScreenTriangle,
    ctx: &ShadeContext<'_>,
    band: &mut Band<'_>,
    tx: u32,
    local_tile_row: u32,
    depth_write: bool,
) -> usize {
    let tile = TILE_SIZE as i32;
    let tri_min_x = t
        .screen
        .iter()
        .map(|v| v[0])
        .fold(f32::INFINITY, f32::min)
        .floor() as i32;
    let tri_max_x = t
        .screen
        .iter()
        .map(|v| v[0])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i32;
    let tri_min_y = t
        .screen
        .iter()
        .map(|v| v[1])
        .fold(f32::INFINITY, f32::min)
        .floor() as i32;
    let tri_max_y = t
        .screen
        .iter()
        .map(|v| v[1])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i32;

    let x0 = (tx as i32 * tile).max(tri_min_x);
    let x1 = ((tx as i32 + 1) * tile).min(tri_max_x);
    // Band-local pixel rows for this tile row.
    let y_start = (local_tile_row as i32 * tile).max(tri_min_y - band.row_start as i32);
    let y_end =
        (((local_tile_row as i32 + 1) * tile) as i32).min(tri_max_y - band.row_start as i32);
    let y_end = y_end.min(band.rows as i32);
    let x1 = x1.min(band.stride as i32);
    if x0 >= x1 || y_start >= y_end {
        return 0;
    }

    let area = t.area;
    if area.abs() < 1e-7 {
        return 0;
    }
    let inv_area = 1.0 / area;
    let Some(material) = ctx.materials.get(t.material as usize) else {
        return 0;
    };
    let texture = material_texture(ctx.scene, material.handle);
    let (p0, p1, p2) = (t.screen[0], t.screen[1], t.screen[2]);
    let mut shaded = 0usize;

    for local in y_start..y_end {
        let y = band.row_start as i32 + local;
        let py = y as f32 + 0.5;
        for x in x0..x1 {
            let px = x as f32 + 0.5;
            // A hair of bias lets the tie test below distinguish an exact zero
            // from a genuine float near-miss.
            let w0 = edge(p1, p2, px, py) * inv_area + TIE_EPSILON;
            let w1 = edge(p2, p0, px, py) * inv_area + TIE_EPSILON;
            let w2 = edge(p0, p1, px, py) * inv_area + TIE_EPSILON;
            let tied = |v: f32, a: [f32; 3], b: [f32; 3]| v < 2.0 * TIE_EPSILON && !tie_owner(a, b);
            if w0 < 0.0
                || w1 < 0.0
                || w2 < 0.0
                || tied(w0, p1, p2)
                || tied(w1, p2, p0)
                || tied(w2, p0, p1)
            {
                continue;
            }
            let depth = w0 * p0[2] + w1 * p1[2] + w2 * p2[2];
            let Some(pixel) = band.pixel_index(x, local) else {
                continue;
            };
            if depth > band.depth[pixel] {
                continue;
            }
            let inv_w = w0 * t.inv_w[0] + w1 * t.inv_w[1] + w2 * t.inv_w[2];
            if inv_w.abs() < 1e-9 {
                continue;
            }
            let rw = 1.0 / inv_w;
            let uv = Vec2::new(
                (w0 * t.uv[0].x * t.inv_w[0]
                    + w1 * t.uv[1].x * t.inv_w[1]
                    + w2 * t.uv[2].x * t.inv_w[2])
                    * rw,
                (w0 * t.uv[0].y * t.inv_w[0]
                    + w1 * t.uv[1].y * t.inv_w[1]
                    + w2 * t.uv[2].y * t.inv_w[2])
                    * rw,
            );
            let world = Vec3::new(
                (w0 * t.world[0].x * t.inv_w[0]
                    + w1 * t.world[1].x * t.inv_w[1]
                    + w2 * t.world[2].x * t.inv_w[2])
                    * rw,
                (w0 * t.world[0].y * t.inv_w[0]
                    + w1 * t.world[1].y * t.inv_w[1]
                    + w2 * t.world[2].y * t.inv_w[2])
                    * rw,
                (w0 * t.world[0].z * t.inv_w[0]
                    + w1 * t.world[1].z * t.inv_w[1]
                    + w2 * t.world[2].z * t.inv_w[2])
                    * rw,
            );
            let normal = Vec3::new(
                w0 * t.normal[0].x + w1 * t.normal[1].x + w2 * t.normal[2].x,
                w0 * t.normal[0].y + w1 * t.normal[1].y + w2 * t.normal[2].y,
                w0 * t.normal[0].z + w1 * t.normal[1].z + w2 * t.normal[2].z,
            )
            .normalize_or_zero();

            let (rgb, alpha) = shade_fragment(ctx, material, texture, uv, world, normal);
            if alpha <= 0.001 {
                continue;
            }
            let i3 = pixel * 3;
            if alpha >= 1.0 {
                band.color[i3] = rgb[0];
                band.color[i3 + 1] = rgb[1];
                band.color[i3 + 2] = rgb[2];
            } else {
                let inv = 1.0 - alpha;
                band.color[i3] = rgb[0] * alpha + band.color[i3] * inv;
                band.color[i3 + 1] = rgb[1] * alpha + band.color[i3 + 1] * inv;
                band.color[i3 + 2] = rgb[2] * alpha + band.color[i3 + 2] * inv;
            }
            // Transparent surfaces test depth but do not write it, which is what
            // lets a second transparent surface behind them still be seen.
            if depth_write {
                band.depth[pixel] = depth;
            }
            band.ids[pixel] = t.instance;
            shaded += 1;
        }
    }
    shaded
}

/// The texture a material points at, if any.
fn material_texture<'a>(scene: &'a Scene, handle: MaterialHandle) -> Option<&'a Texture> {
    let material = scene.material(handle)?;
    scene.texture(material.texture?)
}

/// The fragment shader, shared by the opaque and transparent passes.
#[must_use]
fn shade_fragment(
    ctx: &ShadeContext<'_>,
    material: &ShadeMaterial,
    texture: Option<&Texture>,
    uv: Vec2,
    world: Vec3,
    normal: Vec3,
) -> ([f32; 3], f32) {
    let mut base = material.base_color;
    let mut alpha = material.alpha;
    if let Some(tex) = texture {
        let texel = tex.sample_pixel_art(uv.x, uv.y);
        let linear = texel.to_linear();
        base = [base[0] * linear.r, base[1] * linear.g, base[2] * linear.b];
        alpha *= texel.a as f32 / 255.0;
    }
    match material.alpha_mode {
        AlphaMode::Opaque => alpha = 1.0,
        AlphaMode::Cutout { threshold } => {
            if alpha < threshold {
                return ([0.0; 3], 0.0);
            }
            alpha = 1.0;
        }
        AlphaMode::Blend => {}
        AlphaMode::Additive => alpha = alpha.min(1.0),
    }

    if material.unlit {
        // Unlit surfaces still take fog: atmospheric perspective is a property
        // of the scene, not of the shading model, and a hand-painted sprite
        // fading into the distance is exactly the pixel-art look.
        let mut out = [
            base[0] + material.emissive[0],
            base[1] + material.emissive[1],
            base[2] + material.emissive[2],
        ];
        if ctx.settings.fog {
            if let Some(fog) = ctx.scene.fog {
                out = fog.apply(out, fog.factor_at(world, ctx.camera.position));
            }
        }
        return (out, alpha);
    }

    let n = if normal.length_squared() > 1e-8 {
        normal
    } else {
        Vec3::Y
    };
    let mut light = ctx.scene.ambient.radiance_at(n);
    for l in &ctx.scene.lights {
        let contribution = l.radiance_at(world, n);
        if contribution == [0.0; 3] {
            continue;
        }
        let visibility = if ctx.settings.shadows
            && l.casts_shadow()
            && material.receive_shadow
            && matches!(l, crate::light::Light::Directional { .. })
        {
            // Offset a little along the normal before the lookup: surfaces
            // nearly parallel to the light would otherwise self-shadow.
            let biased = world + n * ctx.settings.shadow_bias.max(0.02) * 8.0;
            let clip = ctx
                .shadow
                .light_view_projection()
                .transform_point4(biased.extend(1.0));
            ctx.shadow.sample_pcf(clip, ctx.settings.shadow_bias)
        } else {
            1.0
        };
        light[0] += contribution[0] * visibility;
        light[1] += contribution[1] * visibility;
        light[2] += contribution[2] * visibility;
    }

    let mut out = [
        base[0] * light[0] + material.emissive[0],
        base[1] * light[1] + material.emissive[1],
        base[2] * light[2] + material.emissive[2],
    ];

    if ctx.settings.fog {
        if let Some(fog) = ctx.scene.fog {
            out = fog.apply(out, fog.factor_at(world, ctx.camera.position));
        }
    }
    (out, alpha)
}

/// Builds the per-instance shading summary.
fn shade_material(material: &Material, handle: MaterialHandle, alpha: f32) -> ShadeMaterial {
    let base = material.base_color;
    let emissive = material.emissive;
    ShadeMaterial {
        handle,
        base_color: [base.r, base.g, base.b],
        emissive: [emissive.r, emissive.g, emissive.b],
        alpha: base.a * alpha,
        unlit: material.unlit,
        double_sided: material.double_sided,
        alpha_mode: material.alpha_mode,
        receive_shadow: material.receive_shadow,
    }
}

/// Transforms a clip-space triangle into screen-space ones, clipping against
/// the near plane and culling back faces.
#[allow(clippy::too_many_arguments)]
fn emit_clipped(
    a: ClipVertex,
    b: ClipVertex,
    c: ClipVertex,
    material: u32,
    instance: u32,
    double_sided: bool,
    width: u32,
    height: u32,
    backface_culling: bool,
    out: &mut Vec<ScreenTriangle>,
) {
    for clipped in clip_near_vertices(a, b, c) {
        let Some(screen) = project(&clipped, width, height) else {
            continue;
        };
        let area = edge(screen[0], screen[1], screen[2][0], screen[2][1]);
        if area.abs() < 1e-7 {
            continue;
        }
        // `edge` below is the *negation* of the usual edge function, so with it
        // a front face (NDC counter-clockwise, which the viewport's Y flip turns
        // into screen-space clockwise) has a positive area. Back faces are
        // therefore the negative ones. `raster_pixels` relies on the same sign
        // convention to accept interior fragments.
        if backface_culling && !double_sided && area < 0.0 {
            continue;
        }
        out.push(ScreenTriangle {
            screen,
            inv_w: [clipped[0].inv_w, clipped[1].inv_w, clipped[2].inv_w],
            world: [clipped[0].world, clipped[1].world, clipped[2].world],
            uv: [clipped[0].uv, clipped[1].uv, clipped[2].uv],
            normal: [clipped[0].normal, clipped[1].normal, clipped[2].normal],
            material,
            instance,
            area,
        });
    }
}

/// Projects three clipped vertices to screen space, or `None` when any of them
/// is outside the depth range.
fn project(v: &[ClipVertex; 3], width: u32, height: u32) -> Option<[[f32; 3]; 3]> {
    let mut out = [[0.0f32; 3]; 3];
    for (i, vertex) in v.iter().enumerate() {
        let ndc = vertex.clip.perspective_divide()?;
        if !(0.0..=1.0).contains(&ndc.z) {
            return None;
        }
        out[i] = [
            (ndc.x * 0.5 + 0.5) * width as f32,
            (1.0 - (ndc.y * 0.5 + 0.5)) * height as f32,
            ndc.z,
        ];
    }
    Some(out)
}

/// Clips a clip-space triangle against the near plane (`w > epsilon`),
/// returning triangles.
///
/// A triangle with one vertex inside becomes a triangle; one with two inside
/// becomes a quad, which is fan-triangulated into two triangles.
fn clip_near_vertices(a: ClipVertex, b: ClipVertex, c: ClipVertex) -> Vec<[ClipVertex; 3]> {
    const EPS: f32 = 1e-5;
    let verts = [a, b, c];
    let inside = [a.clip.w > EPS, b.clip.w > EPS, c.clip.w > EPS];
    let count = inside.iter().filter(|x| **x).count();
    match count {
        3 => return vec![[a, b, c]],
        0 => return Vec::new(),
        _ => {}
    }
    let mut polygon: Vec<ClipVertex> = Vec::with_capacity(4);
    for i in 0..3 {
        let next = (i + 1) % 3;
        if inside[i] {
            polygon.push(verts[i]);
        }
        if inside[i] != inside[next] {
            polygon.push(near_intersection(verts[i], verts[next], EPS));
        }
    }
    fan_triangulate(&polygon)
}

/// Sutherland–Hodgman fan triangulation of a convex polygon.
fn fan_triangulate(polygon: &[ClipVertex]) -> Vec<[ClipVertex; 3]> {
    if polygon.len() < 3 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(polygon.len() - 2);
    for i in 1..polygon.len() - 1 {
        out.push([polygon[0], polygon[i], polygon[i + 1]]);
    }
    out
}

/// The point where the segment `a`–`b` crosses `w == EPS`.
fn near_intersection(a: ClipVertex, b: ClipVertex, eps: f32) -> ClipVertex {
    let denom = a.clip.w - b.clip.w;
    let t = if denom.abs() < 1e-9 {
        0.5
    } else {
        ((a.clip.w - eps) / denom).clamp(0.0, 1.0)
    };
    ClipVertex::lerp(&a, &b, t)
}

/// Clips a clip-space triangle against `w > 0` for the shadow pass, which needs
/// no interpolated attributes.
fn clip_near_positions(a: Vec4, b: Vec4, c: Vec4) -> Vec<[Vec4; 3]> {
    const EPS: f32 = 1e-5;
    let verts = [a, b, c];
    let inside = [a.w > EPS, b.w > EPS, c.w > EPS];
    let count = inside.iter().filter(|x| **x).count();
    match count {
        3 => return vec![[a, b, c]],
        0 => return Vec::new(),
        _ => {}
    }
    let mut polygon: Vec<Vec4> = Vec::with_capacity(4);
    for i in 0..3 {
        let next = (i + 1) % 3;
        if inside[i] {
            polygon.push(verts[i]);
        }
        if inside[i] != inside[next] {
            let (p, q) = (verts[i], verts[next]);
            let denom = p.w - q.w;
            let t = if denom.abs() < 1e-9 {
                0.5
            } else {
                ((p.w - EPS) / denom).clamp(0.0, 1.0)
            };
            polygon.push(p + (q - p) * t);
        }
    }
    if polygon.len() < 3 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(polygon.len() - 2);
    for i in 1..polygon.len() - 1 {
        out.push([polygon[0], polygon[i], polygon[i + 1]]);
    }
    out
}

/// Twice the signed area of the triangle `(a, b, p)` in screen space.
///
/// Note that this is the **negation** of the usual edge function; with it a
/// front-facing triangle has a positive area, and an interior point has all
/// three edge values positive. Both `emit_clipped` and `raster_pixels` depend on
/// that convention, so change them together if you change this.
#[inline]
fn edge(a: [f32; 3], b: [f32; 3], px: f32, py: f32) -> f32 {
    (px - a[0]) * (b[1] - a[1]) - (py - a[1]) * (b[0] - a[0])
}

/// How close to an edge a pixel centre must be for the edge to count as a tie.
const TIE_EPSILON: f32 = 1e-6;

/// Decides which of two triangles sharing an edge owns a pixel that lies exactly
/// on it.
///
/// Without a fill rule a tessellated surface double-shades every pixel along its
/// internal edges — the classic cause of a visible "quilt" pattern on a large
/// ground plane, and of semi-transparent geometry coming out too dark along the
/// diagonal.
///
/// Every closed, consistently wound surface traverses a shared edge in opposite
/// directions from its two triangles. Comparing the two endpoints gives a
/// single, orientation-independent owner, which is much easier to get right than
/// the usual top-left rule and needs no winding case analysis.
#[inline]
fn tie_owner(a: [f32; 3], b: [f32; 3]) -> bool {
    a[1] > b[1] || (a[1] == b[1] && a[0] > b[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light::{Ambient, Fog, Light};
    use crate::material::Material;
    use crate::mesh::Mesh;
    use noxel_core::math::{Color, Transform};

    fn camera() -> CameraView {
        CameraView::orthographic(
            Vec3::new(0.0, 20.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            12.0,
            1.0,
            1.0,
            100.0,
        )
    }

    fn scene_with(mesh: Mesh, material: Material, y: f32) -> Scene {
        let mut scene = Scene::new();
        scene.background = Color::BLACK;
        let m = scene.add_mesh(mesh);
        let mat = scene.add_material(material);
        scene.spawn(
            "obj",
            m,
            mat,
            Transform::from_translation(Vec3::new(0.0, y, 0.0)),
        );
        scene.update_all_bounds();
        scene
    }

    fn no_shadow_no_fog() -> RenderSettings {
        RenderSettings {
            shadows: false,
            fog: false,
            ..Default::default()
        }
    }

    #[test]
    fn renders_an_unlit_plane_faithfully() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let mut fb = Framebuffer::new(64, 64);
        let mut r = RasterRenderer::new(64, 64);
        let stats = r.render(&scene, &camera(), None, &mut fb, &no_shadow_no_fog());
        assert!(stats.triangles_drawn > 0, "{stats:?}");
        let c = fb.get(32, 32).unwrap();
        assert!(c[0] > 0.99 && c[1] > 0.99 && c[2] > 0.99, "{c:?}");
        assert_eq!(
            fb.get(0, 0).unwrap(),
            [0.0, 0.0, 0.0],
            "corner stays background"
        );
    }

    #[test]
    fn unlit_sprite_round_trips_to_the_authored_palette() {
        let authored = Color::from_srgb8(87, 143, 201, 255);
        let scene = scene_with(Mesh::plane(10.0, 1.0), Material::unlit("t", authored), 0.0);
        let mut fb = Framebuffer::new(32, 32);
        let mut r = RasterRenderer::new(32, 32);
        r.render(&scene, &camera(), None, &mut fb, &no_shadow_no_fog());
        let image = fb.resolve(&crate::framebuffer::ResolveSettings::default());
        assert_eq!(
            image.get(16, 16).unwrap().to_hex(),
            authored.to_srgb8().to_hex()
        );
    }

    #[test]
    fn depth_buffer_keeps_the_nearer_surface() {
        let mut scene = Scene::new();
        scene.background = Color::BLACK;
        let mesh = scene.add_mesh(Mesh::plane(10.0, 1.0));
        let far = scene.add_material(Material::unlit("far", Color::from_hex(0xFF00_0000)));
        let near = scene.add_material(Material::unlit("near", Color::from_hex(0xFFFF_0000)));
        scene.spawn("far", mesh, far, Transform::IDENTITY);
        scene.spawn(
            "near",
            mesh,
            near,
            Transform::from_translation(Vec3::new(0.0, 2.0, 0.0)),
        );
        scene.update_all_bounds();
        let mut fb = Framebuffer::new(32, 32);
        let mut r = RasterRenderer::new(32, 32);
        r.render(&scene, &camera(), None, &mut fb, &no_shadow_no_fog());
        let pixel = fb
            .resolve(&crate::framebuffer::ResolveSettings::default())
            .get(16, 16)
            .expect("in bounds");
        assert!(
            pixel.r > 200 && pixel.g < 60,
            "the near red plane must win: {pixel:?}"
        );
    }

    #[test]
    fn backface_culling_drops_the_underside() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let below = CameraView::orthographic(
            Vec3::new(0.0, -20.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            12.0,
            1.0,
            1.0,
            100.0,
        );
        let mut fb = Framebuffer::new(32, 32);
        let mut r = RasterRenderer::new(32, 32);
        let stats = r.render(&scene, &below, None, &mut fb, &no_shadow_no_fog());
        assert_eq!(
            stats.triangles_drawn, 0,
            "an upward face must be culled from below"
        );
    }

    #[test]
    fn double_sided_material_is_drawn_from_below() {
        let mut material = Material::unlit("ds", Color::WHITE);
        material.double_sided = true;
        let scene = scene_with(Mesh::plane(10.0, 1.0), material, 0.0);
        let below = CameraView::orthographic(
            Vec3::new(0.0, -20.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            12.0,
            1.0,
            1.0,
            100.0,
        );
        let mut fb = Framebuffer::new(32, 32);
        let mut r = RasterRenderer::new(32, 32);
        let stats = r.render(&scene, &below, None, &mut fb, &no_shadow_no_fog());
        assert_eq!(stats.triangles_drawn, 2);
    }

    #[test]
    fn cutout_material_discards_transparent_fragments() {
        let mut material = Material::unlit("cut", Color::rgba(1.0, 1.0, 1.0, 0.25));
        material.alpha_mode = AlphaMode::cutout(0.5);
        let scene = scene_with(Mesh::plane(10.0, 1.0), material, 0.0);
        let mut fb = Framebuffer::new(32, 32);
        let mut r = RasterRenderer::new(32, 32);
        r.render(&scene, &camera(), None, &mut fb, &no_shadow_no_fog());
        assert_eq!(
            fb.get(16, 16).unwrap(),
            [0.0, 0.0, 0.0],
            "alpha 0.25 < 0.5 is discarded"
        );
    }

    #[test]
    fn transparent_material_blends_with_what_is_behind() {
        let mut scene = Scene::new();
        scene.background = Color::BLACK;
        let mesh = scene.add_mesh(Mesh::plane(10.0, 1.0));
        let base = scene.add_material(Material::unlit("base", Color::WHITE));
        let mut glass_material = Material::unlit("glass", Color::rgba(0.0, 0.0, 0.0, 0.5));
        glass_material.alpha_mode = AlphaMode::Blend;
        let glass = scene.add_material(glass_material);
        scene.spawn("base", mesh, base, Transform::IDENTITY);
        scene.spawn(
            "glass",
            mesh,
            glass,
            Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)),
        );
        scene.update_all_bounds();
        let mut fb = Framebuffer::new(32, 32);
        let mut r = RasterRenderer::new(32, 32);
        r.render(&scene, &camera(), None, &mut fb, &no_shadow_no_fog());
        let c = fb.get(16, 16).unwrap();
        assert!(
            (c[0] - 0.5).abs() < 0.05,
            "50% black over white should be mid grey: {c:?}"
        );
    }

    #[test]
    fn transparent_geometry_does_not_hide_what_is_behind_it() {
        let mut scene = Scene::new();
        scene.background = Color::BLACK;
        let mesh = scene.add_mesh(Mesh::plane(10.0, 1.0));
        let base = scene.add_material(Material::unlit("base", Color::WHITE));
        let mut glass_material = Material::unlit("glass", Color::rgba(1.0, 0.0, 0.0, 0.5));
        glass_material.alpha_mode = AlphaMode::Blend;
        let glass = scene.add_material(glass_material);
        scene.spawn("base", mesh, base, Transform::IDENTITY);
        // The glass is *below* the base, but the transparent pass still draws it
        // on top because it does not depth-test against... it does depth-test,
        // so it must be drawn behind and blend through.
        scene.spawn(
            "glass",
            mesh,
            glass,
            Transform::from_translation(Vec3::new(0.0, -1.0, 0.0)),
        );
        scene.update_all_bounds();
        let mut fb = Framebuffer::new(32, 32);
        let mut r = RasterRenderer::new(32, 32);
        r.render(&scene, &camera(), None, &mut fb, &no_shadow_no_fog());
        let c = fb.get(16, 16).unwrap();
        assert!(
            c[0] > 0.9,
            "the opaque white plane must still be visible: {c:?}"
        );
    }

    #[test]
    fn fog_lifts_a_black_surface() {
        let mut scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::BLACK),
            0.0,
        );
        scene.fog = Some(Fog {
            color: Color::WHITE,
            start: 0.0,
            end: 30.0,
            height_falloff: 0.0,
        });
        let mut fb = Framebuffer::new(16, 16);
        let mut r = RasterRenderer::new(16, 16);
        let settings = RenderSettings {
            shadows: false,
            fog: true,
            ..Default::default()
        };
        r.render(&scene, &camera(), None, &mut fb, &settings);
        assert!(fb.get(8, 8).unwrap()[0] > 0.1, "{:?}", fb.get(8, 8));
    }

    #[test]
    fn lighting_modulates_a_lit_surface() {
        let mut scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::lit("g", Color::WHITE),
            0.0,
        );
        scene.ambient = Ambient {
            sky: Color::BLACK,
            ground: Color::BLACK,
            hemisphere: 0.0,
            intensity: 0.0,
        };
        scene.add_light(Light::sun());
        let mut fb = Framebuffer::new(16, 16);
        let mut r = RasterRenderer::new(16, 16);
        r.render(&scene, &camera(), None, &mut fb, &no_shadow_no_fog());
        let c = fb.get(8, 8).unwrap();
        assert!(
            c[0] > 0.0 && c[0] < 1.0,
            "the sun is not straight down: {c:?}"
        );
    }

    #[test]
    fn shadow_pass_writes_the_map() {
        let mut scene = Scene::new();
        let cube = scene.add_mesh(Mesh::cube(2.0));
        let mat = scene.add_material(Material::lit("box", Color::WHITE));
        scene.spawn(
            "box",
            cube,
            mat,
            Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)),
        );
        scene.add_light(Light::sun());
        scene.update_all_bounds();
        let mut fb = Framebuffer::new(32, 32);
        let mut r = RasterRenderer::new(32, 32);
        r.render(&scene, &camera(), None, &mut fb, &RenderSettings::default());
        let written = r.shadow_map().depth().iter().filter(|d| **d < 1.0).count();
        assert!(written > 0, "the cube must appear in the shadow map");
    }

    #[test]
    fn visible_set_limits_what_is_drawn() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let mut fb = Framebuffer::new(64, 64);
        let mut r = RasterRenderer::new(64, 64);
        let mut set = VisibleSet::default();
        set.counts.considered = 1;
        let stats = r.render(&scene, &camera(), Some(&set), &mut fb, &no_shadow_no_fog());
        assert_eq!(stats.triangles_drawn, 0);
        assert_eq!(fb.get(32, 32).unwrap(), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn tile_bins_place_triangles_in_the_right_tiles() {
        let mut bins = TileBins::new();
        let tris = vec![
            make_tri([[0.0, 0.0, 0.5], [10.0, 0.0, 0.5], [0.0, 10.0, 0.5]]),
            make_tri([
                [100.0, 100.0, 0.5],
                [120.0, 100.0, 0.5],
                [100.0, 120.0, 0.5],
            ]),
        ];
        bins.build(128, 128, 32, &tris);
        let (cols, rows) = bins.grid();
        assert_eq!((cols, rows), (4, 4));
        assert!(bins.tile(0).contains(&0), "triangle 0 belongs in tile 0");
        let bottom_right = (3 * cols + 3) as usize;
        assert!(
            bins.tile(bottom_right).contains(&1),
            "{:?}",
            bins.tile(bottom_right)
        );
        // Nothing was duplicated into a tile it does not touch.
        assert!(bins.tile(0).len() == 1);
    }

    #[test]
    fn tile_bins_cover_the_whole_screen_for_a_full_screen_triangle() {
        let mut bins = TileBins::new();
        let tris = vec![make_tri([
            [0.0, 0.0, 0.5],
            [128.0, 0.0, 0.5],
            [64.0, 128.0, 0.5],
        ])];
        bins.build(128, 128, 32, &tris);
        assert!(
            bins.len() > 1,
            "a big triangle must be binned into several tiles"
        );
    }

    fn make_tri(screen: [[f32; 3]; 3]) -> ScreenTriangle {
        let area = edge(screen[0], screen[1], screen[2][0], screen[2][1]);
        ScreenTriangle {
            screen,
            inv_w: [1.0; 3],
            world: [Vec3::ZERO; 3],
            uv: [Vec2::ZERO; 3],
            normal: [Vec3::Y; 3],
            material: 0,
            instance: 0,
            area,
        }
    }

    #[test]
    fn bins_ignore_degenerate_triangles() {
        let mut bins = TileBins::new();
        let tris = vec![make_tri([
            [5.0, 5.0, 0.5],
            [5.0, 5.0, 0.5],
            [5.0, 5.0, 0.5],
        ])];
        bins.build(64, 64, 32, &tris);
        assert!(bins.is_empty());
    }

    fn clip_vertex(w: f32) -> ClipVertex {
        ClipVertex {
            clip: Vec4::new(0.0, 0.0, 0.5 * w, w),
            world: Vec3::ZERO,
            uv: Vec2::ZERO,
            normal: Vec3::Y,
            inv_w: 1.0 / w,
        }
    }

    #[test]
    fn near_plane_clipping_produces_valid_triangles() {
        assert_eq!(
            clip_near_vertices(clip_vertex(1.0), clip_vertex(1.0), clip_vertex(1.0)).len(),
            1
        );
        assert!(
            clip_near_vertices(clip_vertex(-1.0), clip_vertex(-1.0), clip_vertex(-1.0)).is_empty()
        );

        // One vertex behind: a single triangle survives.
        // One vertex behind: the three edges cross the plane twice, so the
        // surviving polygon is a quad and fans into two triangles.
        let one = clip_near_vertices(clip_vertex(1.0), clip_vertex(-1.0), clip_vertex(1.0));
        assert_eq!(one.len(), 2);
        for tri in &one {
            for v in tri {
                assert!(v.clip.w > 0.0);
            }
        }

        // Two vertices behind: one crossing produces a single triangle.
        let two = clip_near_vertices(clip_vertex(1.0), clip_vertex(-1.0), clip_vertex(-1.0));
        assert_eq!(two.len(), 1);
        for v in &two[0] {
            assert!(
                v.clip.w > 0.0,
                "clipped vertices must be in front of the near plane"
            );
        }
    }

    #[test]
    fn clipping_interpolates_attributes() {
        let mut a = clip_vertex(2.0);
        a.uv = Vec2::new(0.0, 0.0);
        let mut b = clip_vertex(-2.0);
        b.uv = Vec2::new(1.0, 1.0);
        let clipped = clip_near_vertices(a, b, clip_vertex(1.0));
        let found = clipped
            .iter()
            .flatten()
            .find(|v| v.uv.x > 0.0 && v.uv.x < 1.0)
            .expect("an interpolated vertex must exist");
        assert!(
            (found.uv.x - found.uv.y).abs() < 1e-6,
            "attributes interpolate together"
        );
    }

    #[test]
    fn shadow_clip_fans_a_polygon() {
        let inside = Vec4::new(0.0, 0.0, 0.5, 1.0);
        let outside = Vec4::new(0.0, 0.0, 0.5, -1.0);
        assert_eq!(clip_near_positions(inside, inside, inside).len(), 1);
        assert!(clip_near_positions(outside, outside, outside).is_empty());
        // One vertex in front of the plane, two behind: a single triangle.
        let mixed = clip_near_positions(inside, outside, outside);
        assert_eq!(mixed.len(), 1);
        // Two in front: a quad, so two triangles. (The shadow pass needs
        // triangles only, hence the fan.)
        let quad = clip_near_positions(inside, outside, inside);
        assert_eq!(quad.len(), 2);
        for tri in &mixed {
            for v in tri {
                assert!(v.w > 0.0);
            }
        }
    }

    #[test]
    fn resize_is_picked_up_automatically() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let mut r = RasterRenderer::new(32, 32);
        let mut big = Framebuffer::new(96, 48);
        r.render(&scene, &camera(), None, &mut big, &no_shadow_no_fog());
        assert!(big.get(48, 24).unwrap()[0] > 0.5);
    }

    #[test]
    fn parallel_and_sequential_rendering_agree() {
        let mut scene = Scene::new();
        scene.background = Color::from_hex(0xFF11_2233);
        let mesh = scene.add_mesh(Mesh::plane(10.0, 1.0));
        let cube = scene.add_mesh(Mesh::cube(2.0));
        let plane_mat = scene.add_material(Material::unlit("p", Color::from_hex(0xFF88_4422)));
        let cube_mat = scene.add_material(Material::lit("c", Color::WHITE));
        scene.spawn("plane", mesh, plane_mat, Transform::IDENTITY);
        for i in 0..8 {
            scene.spawn(
                format!("cube{i}"),
                cube,
                cube_mat,
                Transform::from_translation(Vec3::new(i as f32 * 0.7 - 3.0, 1.0, 0.0)),
            );
        }
        scene.add_light(Light::sun());
        scene.update_all_bounds();

        let mut fb_a = Framebuffer::new(64, 64);
        let mut fb_b = Framebuffer::new(64, 64);
        let settings = RenderSettings {
            fog: false,
            ..Default::default()
        };
        let mut sequential = RasterRenderer::new(64, 64);
        let mut parallel = RasterRenderer::new(64, 64).with_jobs(JobPool::auto());
        let sa = sequential.render(&scene, &camera(), None, &mut fb_a, &settings);
        let sb = parallel.render(&scene, &camera(), None, &mut fb_b, &settings);
        assert_eq!(sa.triangles_drawn, sb.triangles_drawn);
        assert_eq!(
            fb_a.color_slice(),
            fb_b.color_slice(),
            "threading must not change pixels"
        );
        assert_eq!(fb_a.depth_slice(), fb_b.depth_slice());
    }

    #[test]
    fn parallel_rendering_uses_the_full_framebuffer() {
        // Guards against a band-splitting bug that silently leaves rows blank.
        let mut scene = Scene::new();
        // A plane comfortably larger than the camera's view, so every pixel of
        // the frame must be covered.
        let mesh = scene.add_mesh(Mesh::plane(80.0, 1.0));
        let mat = scene.add_material(Material::unlit("g", Color::WHITE));
        scene.spawn("g", mesh, mat, Transform::IDENTITY);
        scene.update_all_bounds();
        let cam = CameraView::orthographic(
            Vec3::new(0.0, 20.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            40.0,
            1.0,
            1.0,
            100.0,
        );
        let mut fb = Framebuffer::new(64, 64);
        let mut r = RasterRenderer::new(64, 64).with_jobs(JobPool::auto());
        r.render(&scene, &cam, None, &mut fb, &no_shadow_no_fog());
        for y in 0..64 {
            for x in 0..64 {
                assert!(
                    fb.get(x, y).unwrap()[0] > 0.9,
                    "pixel ({x},{y}) was not drawn"
                );
            }
        }
    }

    #[test]
    fn stats_are_populated() {
        let mut scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::lit("g", Color::WHITE),
            0.0,
        );
        scene.add_light(Light::sun());
        let mut fb = Framebuffer::new(64, 64);
        let mut r = RasterRenderer::new(64, 64);
        let settings = RenderSettings {
            shadows: true,
            fog: false,
            ..Default::default()
        };
        let stats = r.render(&scene, &camera(), None, &mut fb, &settings);
        assert_eq!(stats.mode, ShadingMode::Raster);
        assert_eq!(stats.instances, 1);
        assert_eq!(stats.triangles_submitted, 2);
        assert!(stats.ms_total >= 0.0);
        assert_eq!(stats.shadow_map_size, settings.shadow_map_size);
    }

    #[test]
    fn empty_scene_renders_the_background() {
        let scene = Scene::new();
        let mut fb = Framebuffer::new(16, 16);
        let mut r = RasterRenderer::new(16, 16);
        let stats = r.render(&scene, &camera(), None, &mut fb, &RenderSettings::default());
        assert_eq!(stats.triangles_drawn, 0);
        assert!((fb.get(8, 8).unwrap()[0] - scene.background.r).abs() < 1e-6);
    }

    #[test]
    fn culling_statistics_are_reported() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let mut set = VisibleSet::default();
        set.counts.considered = 10;
        set.counts.drawn = 1;
        set.counts.frustum = 9;
        let mut fb = Framebuffer::new(64, 64);
        let mut r = RasterRenderer::new(64, 64);
        r.render(&scene, &camera(), Some(&set), &mut fb, &no_shadow_no_fog());
        assert_eq!(r.last_counts().frustum, 9);
    }

    #[test]
    fn release_cached_keeps_the_renderer_usable() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let mut fb = Framebuffer::new(64, 64);
        let mut r = RasterRenderer::new(64, 64);
        r.render(&scene, &camera(), None, &mut fb, &no_shadow_no_fog());
        r.release_cached();
        let stats = r.render(&scene, &camera(), None, &mut fb, &no_shadow_no_fog());
        assert!(stats.triangles_drawn > 0);
    }

    #[test]
    fn debug_overlay_marks_occupied_tiles() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let mut fb = Framebuffer::new(64, 64);
        let mut r = RasterRenderer::new(64, 64);
        let settings = RenderSettings {
            shadows: false,
            fog: false,
            debug_overlay: true,
            ..Default::default()
        };
        r.render(&scene, &camera(), None, &mut fb, &settings);
        assert!(!r.tile_bins().is_empty());
    }

    #[test]
    fn describe_reports_the_pipeline() {
        let r = RasterRenderer::new(8, 8);
        assert!(r.describe().contains("software-raster"));
        assert_eq!(r.mode(), ShadingMode::Raster);
    }

    #[test]
    fn id_buffer_records_the_instance() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let mut fb = Framebuffer::new(32, 32);
        let mut r = RasterRenderer::new(32, 32);
        r.render(&scene, &camera(), None, &mut fb, &no_shadow_no_fog());
        assert_eq!(fb.id_at(16, 16), Some(0));
        assert_eq!(fb.id_at(0, 0), Some(Framebuffer::NO_ID));
    }
}
