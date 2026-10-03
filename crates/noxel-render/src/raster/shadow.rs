//! The shadow map: a depth-only render from the sun's point of view.
//!
//! # Why one map, and why it follows the camera
//!
//! A top-down world can be kilometres across, and a single orthographic shadow
//! volume covering all of it would give a few centimetres per texel — useless.
//! Instead the volume is fitted to a moving window around the camera
//! (`extent` metres across) and snapped to whole texels, so the shadow does not
//! crawl as the camera moves. Anything outside the window simply is not
//! shadowed, which is invisible in practice because it is off-screen.
//!
//! See `docs/guides/lighting.md`.

use noxel_core::math::{Aabb, Mat4, Vec3, Vec4};

/// A directional-light shadow map.
#[derive(Clone, Debug)]
pub struct ShadowMap {
    size: u32,
    /// Depth in `[0, 1]`.
    depth: Vec<f32>,
    /// Light view-projection used to produce the map.
    light_view_projection: Mat4,
    /// The world-space box the map covers, for the texel snap.
    light_bounds: Aabb,
}

impl ShadowMap {
    /// Creates a shadow map of `size × size` texels.
    #[must_use]
    pub fn new(size: u32) -> Self {
        let size = size.clamp(64, 8192);
        Self {
            size,
            depth: vec![1.0; (size as usize) * (size as usize)],
            light_view_projection: Mat4::IDENTITY,
            light_bounds: Aabb::EMPTY,
        }
    }

    /// Resizes the map, clearing it.
    pub fn resize(&mut self, size: u32) {
        let size = size.clamp(64, 8192);
        if size == self.size {
            return;
        }
        self.size = size;
        self.depth.clear();
        self.depth.resize((size as usize) * (size as usize), 1.0);
    }

    /// The map's edge length in texels.
    #[inline]
    #[must_use]
    pub fn size(&self) -> u32 {
        self.size
    }

    /// The raw depth buffer.
    #[inline]
    #[must_use]
    pub fn depth(&self) -> &[f32] {
        &self.depth
    }

    /// The light's view-projection matrix from the last build.
    #[inline]
    #[must_use]
    pub fn light_view_projection(&self) -> Mat4 {
        self.light_view_projection
    }

    /// The world box the last build covered.
    #[inline]
    #[must_use]
    pub fn light_bounds(&self) -> Aabb {
        self.light_bounds
    }

    /// Clears the map to the far plane.
    pub fn clear(&mut self) {
        self.depth.fill(1.0);
    }

    /// Fits the light frustum to a sphere around `focus` and prepares for a new
    /// pass.
    ///
    /// * `light_direction` — the direction light travels (points at the scene).
    /// * `extent` — half-width of the shadow volume in metres along the light's
    ///   right/up axes, and also its half-depth.
    /// * `texel_snap` — when true, the volume's centre is quantised to whole
    ///   texels so the shadow does not shimmer as the camera moves.
    pub fn begin(
        &mut self,
        light_direction: Vec3,
        focus: Vec3,
        extent: f32,
        height_range: f32,
        texel_snap: bool,
    ) {
        let dir = light_direction.try_normalize().unwrap_or(Vec3::DOWN);
        // Build an orthonormal basis around the light direction. The `up` hint
        // avoids the degenerate case where the light is exactly vertical.
        let up_hint = if dir.cross(Vec3::Y).length_squared() < 1e-4 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let right = dir.cross(up_hint).normalize_or_zero();
        let up = right.cross(dir).normalize_or_zero();

        let extent = extent.max(1.0);
        let half_height = height_range.max(1.0);
        let texel = (extent * 2.0) / self.size as f32;

        let center = if texel_snap {
            // Project the focus onto the light basis, round to texels, project
            // back. Without this the shadow edges crawl one texel at a time.
            let (x, y) = (focus.dot(right), focus.dot(up));
            let sx = (x / texel).round() * texel;
            let sy = (y / texel).round() * texel;
            right * sx + up * sy + dir * -focus.dot(dir)
        } else {
            focus
        };

        let eye = center - dir * half_height;
        let view = Mat4::look_at_rh(eye, center, up);
        let projection =
            Mat4::orthographic_rh(-extent, extent, -extent, extent, 0.0, half_height * 2.0);
        self.light_view_projection = projection * view;

        let corners = [
            center + right * extent + up * extent,
            center - right * extent + up * extent,
            center + right * extent - up * extent,
            center - right * extent - up * extent,
        ];
        let mut bounds = Aabb::EMPTY;
        for c in corners {
            bounds.grow(c - dir * half_height);
            bounds.grow(c + dir * half_height);
        }
        self.light_bounds = bounds;
        self.clear();
    }

    /// Writes a depth value at a clip-space position, keeping the nearest.
    ///
    /// Returns `false` when the position is outside the map.
    #[inline]
    pub fn write(&mut self, clip: Vec4) -> bool {
        let Some(ndc) = clip.perspective_divide() else {
            return false;
        };
        if !(-1.0..=1.0).contains(&ndc.x)
            || !(-1.0..=1.0).contains(&ndc.y)
            || !(0.0..=1.0).contains(&ndc.z)
        {
            return false;
        }
        let x = (((ndc.x * 0.5 + 0.5) * self.size as f32) as u32).min(self.size - 1);
        let y = (((1.0 - (ndc.y * 0.5 + 0.5)) * self.size as f32) as u32).min(self.size - 1);
        let i = (y as usize) * (self.size as usize) + (x as usize);
        if ndc.z < self.depth[i] {
            self.depth[i] = ndc.z;
            true
        } else {
            false
        }
    }

    /// Depth stored at a texel.
    #[inline]
    #[must_use]
    pub fn texel(&self, x: u32, y: u32) -> f32 {
        self.depth[(y.min(self.size - 1) as usize) * (self.size as usize)
            + (x.min(self.size - 1) as usize)]
    }

    /// Percentage-closer filtering: the fraction of the four nearest texels that
    /// are further away than `depth`.
    ///
    /// Returns `1.0` (lit) when the sample is outside the map — the region
    /// beyond the shadow volume is treated as unshadowed rather than black,
    /// which is what makes the moving window invisible.
    #[must_use]
    pub fn sample_pcf(&self, clip: Vec4, bias: f32) -> f32 {
        let Some(ndc) = clip.perspective_divide() else {
            return 1.0;
        };
        if !(-1.0..=1.0).contains(&ndc.x)
            || !(-1.0..=1.0).contains(&ndc.y)
            || !(0.0..=1.0).contains(&ndc.z)
        {
            return 1.0;
        }
        let u = ndc.x * 0.5 + 0.5;
        let v = 1.0 - (ndc.y * 0.5 + 0.5);
        let x = u * self.size as f32 - 0.5;
        let y = v * self.size as f32 - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let reference = ndc.z - bias;

        let mut lit = 0.0;
        for (dx, dy, weight) in [
            (0i32, 0i32, (1.0 - fx) * (1.0 - fy)),
            (1, 0, fx * (1.0 - fy)),
            (0, 1, (1.0 - fx) * fy),
            (1, 1, fx * fy),
        ] {
            if weight <= 0.0 {
                continue;
            }
            let sx = (x0 as i32 + dx).clamp(0, self.size as i32 - 1) as u32;
            let sy = (y0 as i32 + dy).clamp(0, self.size as i32 - 1) as u32;
            if reference <= self.texel(sx, sy) {
                lit += weight;
            }
        }
        lit
    }

    /// A hard 1-tap shadow test, cheaper than [`ShadowMap::sample_pcf`].
    #[must_use]
    pub fn sample_hard(&self, clip: Vec4, bias: f32) -> f32 {
        let Some(ndc) = clip.perspective_divide() else {
            return 1.0;
        };
        if !(-1.0..=1.0).contains(&ndc.x)
            || !(-1.0..=1.0).contains(&ndc.y)
            || !(0.0..=1.0).contains(&ndc.z)
        {
            return 1.0;
        }
        let x = (((ndc.x * 0.5 + 0.5) * self.size as f32) as u32).min(self.size - 1);
        let y = (((1.0 - (ndc.y * 0.5 + 0.5)) * self.size as f32) as u32).min(self.size - 1);
        if ndc.z - bias <= self.texel(x, y) {
            1.0
        } else {
            0.0
        }
    }

    /// Rasterises a depth-only triangle.
    ///
    /// A minimal clip-space triangle rasterizer, deliberately separate from the
    /// main one: the shadow pass needs no interpolation, no texture and no
    /// lighting, so sharing the full pipeline would only slow it down.
    pub fn raster_triangle(&mut self, a: Vec4, b: Vec4, c: Vec4) {
        let Some(pa) = clip_to_screen(a, self.size) else {
            return;
        };
        let Some(pb) = clip_to_screen(b, self.size) else {
            return;
        };
        let Some(pc) = clip_to_screen(c, self.size) else {
            return;
        };

        let area = edge(pa, pb, pc);
        if area.abs() < 1e-9 {
            return;
        }
        // Accept both windings: a closed mesh seen from the light has mixed
        // facing, and a single-sided shadow map would leak.
        let sign = if area > 0.0 { 1.0 } else { -1.0 };
        let (min_x, min_y, max_x, max_y) = bounds_of(pa, pb, pc, self.size);
        for y in min_y..max_y {
            for x in min_x..max_x {
                let p: ScreenPoint = [x as f32 + 0.5, y as f32 + 0.5, 0.0];
                let w0 = edge(pb, pc, p) * sign;
                let w1 = edge(pc, pa, p) * sign;
                let w2 = edge(pa, pb, p) * sign;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let inv_area = 1.0 / (area * sign);
                let depth = (w0 * pa[2] + w1 * pb[2] + w2 * pc[2]) * inv_area;
                let i = (y as usize) * (self.size as usize) + (x as usize);
                if depth < self.depth[i] {
                    self.depth[i] = depth;
                }
            }
        }
    }

    /// Bytes of heap used.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.depth.capacity() * 4
    }
}

/// A screen-space point `[x, y, depth]`.
type ScreenPoint = [f32; 3];

fn clip_to_screen(clip: Vec4, size: u32) -> Option<ScreenPoint> {
    let ndc = clip.perspective_divide()?;
    if ndc.z < 0.0 || ndc.z > 1.0 {
        return None;
    }
    Some([
        (ndc.x * 0.5 + 0.5) * size as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * size as f32,
        ndc.z,
    ])
}

#[inline]
fn edge(a: ScreenPoint, b: ScreenPoint, c: ScreenPoint) -> f32 {
    (c[0] - a[0]) * (b[1] - a[1]) - (c[1] - a[1]) * (b[0] - a[0])
}

fn bounds_of(a: ScreenPoint, b: ScreenPoint, c: ScreenPoint, size: u32) -> (u32, u32, u32, u32) {
    let min_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as u32;
    let min_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as u32;
    let max_x = (a[0].max(b[0]).max(c[0]).ceil() as i64).clamp(0, size as i64) as u32;
    let max_y = (a[1].max(b[1]).max(c[1]).ceil() as i64).clamp(0, size as i64) as u32;
    (min_x, min_y, max_x, max_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_map_is_empty() {
        let sm = ShadowMap::new(64);
        assert_eq!(sm.size(), 64);
        assert!(sm.depth().iter().all(|d| *d == 1.0));
    }

    #[test]
    fn size_is_clamped() {
        assert_eq!(ShadowMap::new(1).size(), 64);
        assert_eq!(ShadowMap::new(100_000).size(), 8192);
    }

    #[test]
    fn resize_reallocates() {
        let mut sm = ShadowMap::new(64);
        sm.resize(128);
        assert_eq!(sm.size(), 128);
        assert_eq!(sm.depth().len(), 128 * 128);
        // Same size is a no-op.
        sm.depth.fill(0.5);
        sm.resize(128);
        assert_eq!(sm.texel(0, 0), 0.5);
    }

    #[test]
    fn begin_builds_a_usable_light_matrix() {
        let mut sm = ShadowMap::new(256);
        sm.begin(Vec3::DOWN, Vec3::ZERO, 20.0, 40.0, true);
        assert!(sm.light_view_projection().is_finite());
        // A point at the focus must project inside the map.
        let clip = sm
            .light_view_projection()
            .transform_point4(Vec3::ZERO.extend(1.0));
        let ndc = clip.perspective_divide().unwrap();
        assert!(ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0, "{ndc:?}");
        assert!((0.0..=1.0).contains(&ndc.z), "{ndc:?}");
    }

    #[test]
    fn begin_handles_a_vertical_light() {
        let mut sm = ShadowMap::new(64);
        // Exactly straight down is the degenerate case for the up hint.
        sm.begin(
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(5.0, 0.0, 5.0),
            10.0,
            20.0,
            false,
        );
        assert!(sm.light_view_projection().is_finite());
        assert!(sm.light_bounds().is_finite());
    }

    #[test]
    fn texel_snapping_keeps_the_matrix_stable_under_small_moves() {
        let mut a = ShadowMap::new(64);
        let mut b = ShadowMap::new(64);
        a.begin(Vec3::DOWN, Vec3::new(10.0, 0.0, 10.0), 10.0, 20.0, true);
        // A sub-texel move must produce the same matrix.
        b.begin(Vec3::DOWN, Vec3::new(10.01, 0.0, 10.0), 10.0, 20.0, true);
        assert_eq!(
            a.light_view_projection().to_cols_array(),
            b.light_view_projection().to_cols_array(),
            "the shadow volume must not shimmer"
        );
    }

    #[test]
    fn write_keeps_the_nearest_depth() {
        let mut sm = ShadowMap::new(64);
        sm.begin(Vec3::DOWN, Vec3::ZERO, 10.0, 20.0, false);
        let proj = sm.light_view_projection();
        let near = proj.transform_point4(Vec3::new(0.0, 5.0, 0.0).extend(1.0));
        let far = proj.transform_point4(Vec3::new(0.0, -5.0, 0.0).extend(1.0));
        assert!(sm.write(far), "an empty map accepts any depth");
        assert!(sm.write(near), "a nearer fragment must overwrite");
        assert!(
            !sm.write(far),
            "a farther fragment must not overwrite a nearer one"
        );
        assert!(sm.depth().iter().any(|d| *d < 1.0));
    }

    #[test]
    fn outside_the_map_is_lit() {
        let sm = ShadowMap::new(64);
        // No begin(): the identity matrix maps the origin to ndc 0, and the
        // depth of 1.0 means fully shadowed... so use a definitely-outside point.
        let clip = Vec4::new(100.0, 100.0, 0.5, 1.0);
        assert_eq!(sm.sample_pcf(clip, 0.0), 1.0);
        assert_eq!(sm.sample_hard(clip, 0.0), 1.0);
    }

    #[test]
    fn rasterising_a_triangle_writes_depth() {
        let mut sm = ShadowMap::new(64);
        sm.begin(Vec3::DOWN, Vec3::ZERO, 10.0, 20.0, false);
        let proj = sm.light_view_projection();
        // A triangle in the middle of the light volume.
        let a = proj.transform_point4(Vec3::new(-4.0, 0.0, -4.0).extend(1.0));
        let b = proj.transform_point4(Vec3::new(4.0, 0.0, -4.0).extend(1.0));
        let c = proj.transform_point4(Vec3::new(0.0, 0.0, 4.0).extend(1.0));
        sm.raster_triangle(a, b, c);
        let covered = sm.depth().iter().filter(|d| **d < 1.0).count();
        assert!(covered > 100, "only {covered} texels written");
    }

    #[test]
    fn rasterising_a_degenerate_triangle_is_safe() {
        let mut sm = ShadowMap::new(64);
        let p = Vec4::new(0.0, 0.0, 0.5, 1.0);
        sm.raster_triangle(p, p, p);
        assert!(sm.depth().iter().all(|d| *d == 1.0));
    }

    #[test]
    fn pcf_returns_a_fraction() {
        let mut sm = ShadowMap::new(64);
        sm.begin(Vec3::DOWN, Vec3::ZERO, 10.0, 20.0, false);
        let proj = sm.light_view_projection();
        // Fill the whole map with the ground plane depth.
        let ground = proj.transform_point4(Vec3::ZERO.extend(1.0));
        let big = 100.0;
        sm.raster_triangle(
            proj.transform_point4(Vec3::new(-big, 0.0, -big).extend(1.0)),
            proj.transform_point4(Vec3::new(big, 0.0, -big).extend(1.0)),
            proj.transform_point4(Vec3::new(0.0, 0.0, big).extend(1.0)),
        );
        let lit = sm.sample_pcf(ground, 0.0);
        assert!((0.0..=1.0).contains(&lit));
    }

    #[test]
    fn clear_resets() {
        let mut sm = ShadowMap::new(64);
        sm.raster_triangle(
            Vec4::new(-0.5, -0.5, 0.5, 1.0),
            Vec4::new(0.5, -0.5, 0.5, 1.0),
            Vec4::new(0.0, 0.5, 0.5, 1.0),
        );
        sm.clear();
        assert!(sm.depth().iter().all(|d| *d == 1.0));
    }

    #[test]
    fn memory_is_reported() {
        assert!(ShadowMap::new(256).memory_bytes() >= 256 * 256 * 4);
    }
}
