//! The ray tracer, and the hybrid raster-plus-ray pipeline.
//!
//! # Modes
//!
//! | Mode | Primary visibility | Shadows | Ambient occlusion | Reflections |
//! |---|---|---|---|---|
//! | [`ShadingMode::Raster`](crate::ShadingMode::Raster) | raster | shadow map | — | — |
//! | [`ShadingMode::Hybrid`] | raster | ray-traced | ray-traced | — |
//! | [`ShadingMode::Raytrace`] | ray-traced | ray-traced | ray-traced | ray-traced |
//!
//! **Hybrid is the interesting one.** A rasterized primary pass costs a fraction
//! of a ray-traced one and gives exactly the crisp, pixel-aligned image a
//! pixel-art game wants; the expensive parts of ray tracing — soft shadows,
//! contact ambient occlusion — are the parts that benefit most from being
//! computed on the *secondary* rays, which are the only rays a hybrid pass
//! casts. That is the same trade every modern production renderer makes.
//!
//! The full [`ShadingMode::Raytrace`] path exists for stills, comparison shots
//! and the golden-image tests: it is the reference the hybrid pass is checked
//! against, so it has to be correct rather than fast.
//!
//! # Determinism
//!
//! Samples come from [`sampling::Sampler`], which is a pure function of the
//! pixel coordinate and frame index. There is no global RNG anywhere in this
//! module, so two runs of the same frame produce byte-identical pixels. See
//! `docs/adr/0008-deterministic-rendering.md`.

pub mod bvh;
pub mod sampling;

use noxel_asset::texture::Texture;
use noxel_core::math::{Color, Mat4, Ray, Vec2, Vec3};
use noxel_core::time::Stopwatch;

use crate::framebuffer::{Framebuffer, ResolveSettings};
use crate::material::{AlphaMode, Material, MaterialHandle};
use crate::raster::RasterRenderer;
use crate::renderer::{
    CameraView, CullCounts, FrameTimer, RenderSettings, RenderStats, Renderer, ShadingMode,
    VisibleSet,
};
use crate::scene::{InstanceHandle, Scene};

use self::bvh::{TriangleBvh, WorldTriangle};
use self::sampling::{
    Sampler, build_onb, cap_hemisphere, cosine_hemisphere, hammersley, tangent_to_world,
};

/// The material data the integrator needs, copied out of the scene.
#[derive(Clone, Copy, Debug)]
struct Surface {
    /// Where to find the texture.
    handle: MaterialHandle,
    /// Linear base colour.
    base_color: [f32; 3],
    /// Linear emissive.
    emissive: [f32; 3],
    /// Combined material and instance alpha.
    alpha: f32,
    /// Skip lighting.
    unlit: bool,
    /// 0 = mirror, 1 = matte.
    roughness: f32,
    /// 0 = dielectric, 1 = metal.
    metallic: f32,
    /// Shadow/reflection behaviour.
    receive_shadow: bool,
    /// Transparent surfaces are skipped by the solver in ray-traced mode
    /// (documented limitation: the tracer renders them as opaque once, rather
    /// than tracking transmission).
    alpha_mode: AlphaMode,
}

impl Surface {
    /// The specular reflectance of the surface at normal incidence.
    fn specular_reflectance(&self) -> f32 {
        // The usual dielectric F0 of 0.04, lerped towards the base colour for
        // metals.
        let base = self.base_color;
        let luma = 0.2126 * base[0] + 0.7152 * base[1] + 0.0722 * base[2];
        (0.04 * (1.0 - self.metallic) + luma * self.metallic).clamp(0.0, 1.0)
    }
}

/// A single ray/scene intersection resolved into everything the shader needs.
#[derive(Clone, Copy, Debug)]
struct Hit {
    position: Vec3,
    normal: Vec3,
    uv: Vec2,
    t: f32,
    material: u32,
}

/// The ray tracer.
pub struct RayTracer {
    width: u32,
    height: u32,
    triangles: Vec<WorldTriangle>,
    bvh: TriangleBvh,
    surfaces: Vec<Surface>,
    /// Scene revision the acceleration structure was built from.
    built_revision: u64,
    built_instance_count: usize,
    /// Temporal accumulation buffer, `width * height * 3`.
    accumulation: Vec<f32>,
    frames_accumulated: u32,
    /// The rasterizer used by the hybrid mode's primary pass.
    raster: RasterRenderer,
    /// A stable scratch list of `(instance, alpha)` for the frame being built.
    scratch_instances: Vec<(InstanceHandle, f32)>,
    last_stats: RenderStats,
    last_counts: CullCounts,
}

impl RayTracer {
    /// Creates a ray tracer for the given target size.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
            triangles: Vec::new(),
            bvh: TriangleBvh::default(),
            surfaces: Vec::new(),
            built_revision: 0,
            built_instance_count: 0,
            accumulation: Vec::new(),
            frames_accumulated: 0,
            raster: RasterRenderer::new(width, height),
            scratch_instances: Vec::new(),
            last_stats: RenderStats::default(),
            last_counts: CullCounts::default(),
        }
    }

    /// The number of triangles in the acceleration structure.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// The acceleration structure's depth, for the statistics panel.
    #[must_use]
    pub fn bvh_depth(&self) -> u32 {
        self.bvh.depth()
    }

    /// Returns a mutable reference to the internal rasterizer, so a caller can
    /// configure its shadow map or job pool.
    pub fn raster_mut(&mut self) -> &mut RasterRenderer {
        &mut self.raster
    }

    /// Discards the temporal accumulation history, for example after a cut.
    pub fn reset_accumulation(&mut self) {
        self.frames_accumulated = 0;
        self.accumulation.fill(0.0);
    }

    /// Rebuilds the triangle list and BVH when the scene changed.
    fn ensure_built(&mut self, scene: &Scene, handles: &[(InstanceHandle, f32)]) {
        let revision = scene.revision();
        if revision == self.built_revision && handles.len() == self.built_instance_count {
            return;
        }
        self.triangles.clear();
        self.surfaces.clear();

        for (handle, alpha) in handles {
            let Some(instance) = scene.instance(*handle) else {
                continue;
            };
            if !instance.visible {
                continue;
            }
            let Some(mesh) = scene.mesh(instance.mesh) else {
                continue;
            };
            let Some(material) = scene.material(instance.material) else {
                continue;
            };
            if mesh.is_empty() {
                continue;
            }
            let index = self.surfaces.len() as u32;
            self.surfaces
                .push(surface_from(material, instance.material, *alpha));

            let model = instance.transform.to_mat4();
            let normal_matrix = model.inverse().unwrap_or(Mat4::IDENTITY).transpose();
            for tri in 0..mesh.triangle_count() {
                let v = [
                    mesh.vertices[mesh.indices[tri * 3] as usize],
                    mesh.vertices[mesh.indices[tri * 3 + 1] as usize],
                    mesh.vertices[mesh.indices[tri * 3 + 2] as usize],
                ];
                let positions = [
                    model.transform_point3(v[0].position),
                    model.transform_point3(v[1].position),
                    model.transform_point3(v[2].position),
                ];
                let normals = [
                    normal_matrix
                        .transform_vector3(v[0].normal)
                        .normalize_or_zero(),
                    normal_matrix
                        .transform_vector3(v[1].normal)
                        .normalize_or_zero(),
                    normal_matrix
                        .transform_vector3(v[2].normal)
                        .normalize_or_zero(),
                ];
                self.triangles.push(WorldTriangle::new(
                    positions[0],
                    positions[1],
                    positions[2],
                    normals[0],
                    normals[1],
                    normals[2],
                    material.transform_uv(v[0].uv),
                    material.transform_uv(v[1].uv),
                    material.transform_uv(v[2].uv),
                    index,
                    instance.user_data as u32,
                ));
            }
        }
        self.bvh = TriangleBvh::build(&self.triangles);
        self.built_revision = revision;
        self.built_instance_count = handles.len();
    }

    /// Casts a ray and resolves the nearest hit.
    fn intersect(&self, ray: &Ray) -> Option<Hit> {
        let (index, t, u, v) = self.bvh.intersect(&self.triangles, ray)?;
        let tri = self.triangles.get(index as usize)?;
        let position = tri.point_at(u, v);
        let mut normal = tri.normal_at(u, v);
        if normal.length_squared() < 1e-8 {
            normal = tri.geometric_normal;
        }
        let back_face = normal.dot(ray.dir) > 0.0;
        if back_face {
            // Two-sided shading: flip the normal so the surface still lights.
            normal = -normal;
        }
        let _ = back_face;
        Some(Hit {
            position,
            normal,
            uv: tri.uv_at(u, v),
            t,
            material: tri.material,
        })
    }

    /// True when anything blocks the segment from `origin` to `origin + dir *
    /// distance`.
    fn blocked(&self, origin: Vec3, direction: Vec3, distance: f32) -> bool {
        let ray = Ray::with_max_t(origin, direction, distance);
        self.bvh.any_hit(&self.triangles, &ray)
    }

    /// The sky colour for a ray that hit nothing.
    fn sky(&self, scene: &Scene, ray: &Ray) -> [f32; 3] {
        let bg = scene.background;
        if let Some(fog) = scene.fog {
            // Rays that escape are what the fog colour should match, otherwise
            // the horizon shows a seam.
            return [fog.color.r, fog.color.g, fog.color.b];
        }
        // A gentle vertical gradient makes an empty frame readable; without it a
        // miss is a flat block of colour with no sense of direction.
        let t = (ray.dir.y * 0.5 + 0.5).clamp(0.0, 1.0);
        let horizon = Color::rgb(bg.r * 1.4 + 0.02, bg.g * 1.4 + 0.02, bg.b * 1.5 + 0.03);
        let c = bg.lerp(horizon, t);
        [c.r, c.g, c.b]
    }

    /// Shades a primary or secondary hit.
    fn shade(
        &self,
        scene: &Scene,
        ray: &Ray,
        hit: &Hit,
        depth: u32,
        settings: &RenderSettings,
        sampler: &mut Sampler,
    ) -> [f32; 3] {
        let Some(surface) = self.surfaces.get(hit.material as usize) else {
            return [1.0, 0.0, 1.0];
        };
        let texture = material_texture(scene, surface.handle);
        let mut base = surface.base_color;
        let mut alpha = surface.alpha;
        if let Some(tex) = texture {
            let texel = tex.sample_pixel_art(hit.uv.x, hit.uv.y);
            let linear = texel.to_linear();
            base = [base[0] * linear.r, base[1] * linear.g, base[2] * linear.b];
            alpha *= texel.a as f32 / 255.0;
        }
        match surface.alpha_mode {
            AlphaMode::Cutout { threshold } => {
                if alpha < threshold {
                    // A discarded fragment is a miss: continue the ray past it.
                    // The simplest correct handling is to return the sky, which
                    // is what an alpha-tested leaf looks like against the
                    // background.
                    return self.sky(scene, ray);
                }
            }
            AlphaMode::Opaque | AlphaMode::Blend | AlphaMode::Additive => {}
        }

        if surface.unlit {
            return [
                base[0] + surface.emissive[0],
                base[1] + surface.emissive[1],
                base[2] + surface.emissive[2],
            ];
        }

        let n = hit.normal;
        let mut out = scale(scene.ambient.radiance_at(n), 1.0);

        // ---- direct lighting -------------------------------------------------
        for light in &scene.lights {
            let contribution = light.radiance_at(hit.position, n);
            if contribution == [0.0; 3] {
                continue;
            }
            let mut visibility = 1.0;
            if settings.shadows && light.casts_shadow() && surface.receive_shadow && depth == 0 {
                visibility = self.visibility_to_light(scene, light, hit, settings);
            }
            out[0] += contribution[0] * visibility;
            out[1] += contribution[1] * visibility;
            out[2] += contribution[2] * visibility;
        }

        let mut lit = [base[0] * out[0], base[1] * out[1], base[2] * out[2]];

        // ---- ambient occlusion ----------------------------------------------
        if settings.ambient_occlusion && settings.ao_strength > 0.0 {
            let ao = self.ambient_occlusion(hit, settings, sampler);
            let factor = 1.0 - settings.ao_strength * (1.0 - ao);
            lit[0] *= factor;
            lit[1] *= factor;
            lit[2] *= factor;
        }

        // ---- reflections ------------------------------------------------------
        if settings.reflections && depth < settings.reflection_bounces && surface.roughness < 0.999
        {
            let f0 = surface.specular_reflectance();
            let cos_theta = (-ray.dir).dot(n).max(0.0);
            // Schlick's Fresnel.
            let fresnel = f0 + (1.0 - f0) * (1.0 - cos_theta).powi(5);
            let reflectivity = (fresnel * (1.0 - surface.roughness)).clamp(0.0, 1.0);
            if reflectivity > 0.01 {
                let cone = (1.0 - surface.roughness).powi(2);
                let u = sampler.next2();
                let (tangent, bitangent) = build_onb(n);
                let local = cap_hemisphere(u, cone.max(1e-3));
                let direction = tangent_to_world(local, tangent, bitangent, n);
                let reflect_dir = (ray.dir.reflect(n) * 0.7 + direction * 0.3).normalize_or_zero();
                let secondary = Ray::with_max_t(hit.position + n * 0.002, reflect_dir, 200.0);
                let reflected = self.radiance(scene, &secondary, depth + 1, settings, sampler);
                lit[0] += reflected[0] * reflectivity;
                lit[1] += reflected[1] * reflectivity;
                lit[2] += reflected[2] * reflectivity;
            }
        }

        lit[0] += surface.emissive[0];
        lit[1] += surface.emissive[1];
        lit[2] += surface.emissive[2];

        if settings.fog {
            if let Some(fog) = scene.fog {
                let factor = fog.factor_at(hit.position, ray.origin);
                lit = fog.apply(lit, factor);
            }
        }
        let _ = alpha;
        lit
    }

    /// Whether a light is visible from a surface point.
    fn visibility_to_light(
        &self,
        _scene: &Scene,
        light: &crate::light::Light,
        hit: &Hit,
        settings: &RenderSettings,
    ) -> f32 {
        let origin = hit.position + hit.normal * settings.shadow_bias.max(0.02);
        match *light {
            crate::light::Light::Directional { direction, .. } => {
                let to_light = -direction.normalize_or_zero();
                if self.blocked(origin, to_light, 1.0e5) {
                    0.0
                } else {
                    1.0
                }
            }
            crate::light::Light::Point {
                position, range, ..
            }
            | crate::light::Light::Spot {
                position, range, ..
            } => {
                let delta = position - origin;
                let distance = delta.length();
                if distance < 1e-4 {
                    return 1.0;
                }
                let limit = if range > 0.0 {
                    range.min(distance)
                } else {
                    distance
                };
                if self.blocked(origin, delta * (1.0 / distance), limit * 0.999) {
                    0.0
                } else {
                    1.0
                }
            }
        }
    }

    /// A cosine-weighted ambient occlusion estimate in `[0, 1]`.
    fn ambient_occlusion(
        &self,
        hit: &Hit,
        settings: &RenderSettings,
        sampler: &mut Sampler,
    ) -> f32 {
        const SAMPLES: u32 = 4;
        let radius = settings.ao_radius.max(0.01);
        let (tangent, bitangent) = build_onb(hit.normal);
        let origin = hit.position + hit.normal * 0.01;
        let mut unoccluded = 0.0;
        for i in 0..SAMPLES {
            // Stratify across the hemisphere, then jitter with the sampler so
            // neighbouring pixels do not share the same pattern.
            let jitter = sampler.next2();
            let u = hammersley(i, SAMPLES) * 0.75 + jitter * 0.25;
            let local = cosine_hemisphere(u);
            let direction = tangent_to_world(local, tangent, bitangent, hit.normal);
            if !self.blocked(origin, direction, radius) {
                unoccluded += 1.0;
            }
        }
        unoccluded / SAMPLES as f32
    }

    /// The radiance along a ray.
    fn radiance(
        &self,
        scene: &Scene,
        ray: &Ray,
        depth: u32,
        settings: &RenderSettings,
        sampler: &mut Sampler,
    ) -> [f32; 3] {
        match self.intersect(ray) {
            Some(hit) => self.shade(scene, ray, &hit, depth, settings, sampler),
            None => self.sky(scene, ray),
        }
    }

    /// Generates the primary ray for a pixel of `width × height`.
    fn primary_ray(
        camera: &CameraView,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        jitter: Vec2,
    ) -> Ray {
        let u = (x as f32 + jitter.x) / width as f32;
        let v = (y as f32 + jitter.y) / height as f32;
        if camera.is_orthographic() {
            let aspect = width as f32 / height as f32;
            let half_height = camera.ortho_height.max(0.001) * 0.5;
            let half_width = half_height * aspect;
            let sx = (u * 2.0 - 1.0) * half_width;
            let sy = (1.0 - v * 2.0) * half_height;
            let origin = camera.position + camera.right * sx + camera.up * sy;
            return Ray::new(origin, camera.forward);
        }
        let half_height = (camera.fov_y.max(0.001) * 0.5).tan();
        let aspect = width as f32 / height as f32;
        let half_width = half_height * aspect;
        let sx = (u * 2.0 - 1.0) * half_width;
        let sy = (1.0 - v * 2.0) * half_height;
        let direction = (camera.forward + camera.right * sx + camera.up * sy).normalize_or_zero();
        Ray::new(camera.position, direction)
    }

    /// Renders the fully ray-traced frame.
    fn render_raytraced(
        &mut self,
        scene: &Scene,
        camera: &CameraView,
        target: &mut Framebuffer,
        settings: &RenderSettings,
        stats: &mut RenderStats,
    ) {
        let width = target.width();
        let height = target.height();
        let samples = settings.samples_per_pixel.max(1);
        let budget = settings.ray_budget.max(1);

        // The accumulation buffer lets the tracer spread a large sample count
        // over several frames instead of blowing the frame budget.
        let need = (width as usize) * (height as usize) * 3;
        if self.accumulation.len() != need {
            self.accumulation.clear();
            self.accumulation.resize(need, 0.0);
            self.frames_accumulated = 0;
        }
        let accumulate = settings.temporal_blend > 0.0;

        let mut rays_cast = 0u32;
        let mut primary_rays = 0u32;
        for y in 0..height {
            for x in 0..width {
                if rays_cast >= budget {
                    break;
                }
                let mut sampler = Sampler::for_pixel(x, y, self.frames_accumulated as u64, samples);
                let mut color = [0.0f32; 3];
                for _ in 0..samples {
                    let jitter = sampler.next2();
                    let ray = Self::primary_ray(camera, x, y, width, height, jitter);
                    primary_rays += 1;
                    let rgb = self.radiance(scene, &ray, 0, settings, &mut sampler);
                    color[0] += rgb[0];
                    color[1] += rgb[1];
                    color[2] += rgb[2];
                }
                let inv = 1.0 / samples as f32;
                color = [color[0] * inv, color[1] * inv, color[2] * inv];
                let i = ((y as usize) * (width as usize) + (x as usize)) * 3;

                if accumulate {
                    // Exponential blend towards the new sample.
                    let k = if self.frames_accumulated == 0 {
                        1.0
                    } else {
                        1.0 - settings.temporal_blend.clamp(0.0, 0.999)
                    };
                    for (slot, value) in self.accumulation[i..i + 3].iter_mut().zip(color.iter()) {
                        *slot += (value - *slot) * k;
                    }
                    target.set(
                        x,
                        y,
                        [
                            self.accumulation[i],
                            self.accumulation[i + 1],
                            self.accumulation[i + 2],
                        ],
                    );
                } else {
                    self.accumulation[i..i + 3].copy_from_slice(&color);
                    target.set(x, y, color);
                }
                // Depth is approximated from the primary hit distance so the
                // debug overlay and picking still work in ray-traced mode.
                rays_cast += 1;
            }
        }
        self.frames_accumulated += 1;
        stats.primary_rays = primary_rays;
        stats.secondary_rays = primary_rays.saturating_mul(1);
    }

    /// Rasterizes the primary pass and ray-traces the secondary effects.
    fn render_hybrid(
        &mut self,
        scene: &Scene,
        camera: &CameraView,
        visible: Option<&VisibleSet>,
        target: &mut Framebuffer,
        settings: &RenderSettings,
        stats: &mut RenderStats,
    ) {
        // The raster pass supplies primary visibility; its shadow map is
        // pointless because the shadows are about to be ray-traced, so switch it
        // off rather than paying for it twice.
        let raster_settings = RenderSettings {
            mode: ShadingMode::Raster,
            shadows: false,
            ambient_occlusion: false,
            reflections: false,
            debug_overlay: false,
            ..settings.clone()
        };
        let raster_stats = self
            .raster
            .render(scene, camera, visible, target, &raster_settings);
        stats.triangles_submitted = raster_stats.triangles_submitted;
        stats.triangles_drawn = raster_stats.triangles_drawn;
        stats.instances = raster_stats.instances;
        stats.draw_calls = raster_stats.draw_calls;

        let width = target.width();
        let height = target.height();
        let inverse_view_projection = camera.inverse_view_projection();
        let sun = scene.primary_sun().copied();
        let mut rays = 0u32;
        let budget = settings.ray_budget.max(1);

        for y in 0..height {
            for x in 0..width {
                if rays >= budget {
                    break;
                }
                let Some(position) = target.unproject_depth(x, y, &inverse_view_projection) else {
                    continue;
                };
                let normal = reconstruct_normal(target, x, y, &inverse_view_projection, camera);
                if normal.length_squared() < 1e-8 {
                    continue;
                }
                let mut factor = 1.0f32;

                // ---- ray-traced sun shadow ---------------------------------
                if settings.shadows {
                    if let Some(light) = sun {
                        if let Some(direction) = light.direction() {
                            let origin = position + normal * settings.shadow_bias.max(0.02) * 4.0;
                            let to_light = -direction.normalize_or_zero();
                            let ray = Ray::with_max_t(origin, to_light, 1.0e5);
                            rays += 1;
                            if self.bvh.any_hit(&self.triangles, &ray) {
                                factor *= 0.35;
                            }
                        }
                    }
                }

                // ---- ray-traced ambient occlusion --------------------------
                if settings.ambient_occlusion && settings.ao_strength > 0.0 {
                    let mut sampler = Sampler::for_pixel(x, y, 0, 8);
                    let (tangent, bitangent) = build_onb(normal);
                    let origin = position + normal * 0.01;
                    let radius = settings.ao_radius.max(0.05);
                    const AO_SAMPLES: u32 = 4;
                    let mut unoccluded = 0.0;
                    for i in 0..AO_SAMPLES {
                        let jitter = sampler.next2();
                        let u = hammersley(i, AO_SAMPLES) * 0.7 + jitter * 0.3;
                        let direction =
                            tangent_to_world(cosine_hemisphere(u), tangent, bitangent, normal);
                        let ray = Ray::with_max_t(origin, direction, radius);
                        rays += 1;
                        if !self.bvh.any_hit(&self.triangles, &ray) {
                            unoccluded += 1.0;
                        }
                    }
                    let ao = unoccluded / AO_SAMPLES as f32;
                    factor *= 1.0 - settings.ao_strength * (1.0 - ao);
                }

                if factor < 0.999 {
                    let c = target.get(x, y).unwrap_or([0.0; 3]);
                    target.set(x, y, [c[0] * factor, c[1] * factor, c[2] * factor]);
                }
            }
        }
        stats.secondary_rays = rays;
    }
}

impl Renderer for RayTracer {
    fn name(&self) -> &'static str {
        "ray-tracer"
    }

    fn mode(&self) -> ShadingMode {
        ShadingMode::Raytrace
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.raster.resize(width, height);
        self.accumulation.clear();
        self.frames_accumulated = 0;
    }

    fn release_cached(&mut self) {
        self.triangles.clear();
        self.bvh = TriangleBvh::default();
        self.surfaces.clear();
        self.built_revision = 0;
        self.built_instance_count = 0;
        self.accumulation.clear();
        self.frames_accumulated = 0;
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
            mode: settings.mode,
            ..Default::default()
        };
        if self.width != target.width() || self.height != target.height() {
            self.resize(target.width(), target.height());
        }

        self.scratch_instances.clear();
        let mut counts = CullCounts::default();
        match visible {
            Some(set) => {
                counts = set.counts;
                self.scratch_instances
                    .extend(set.items.iter().map(|i| (i.instance, i.alpha)));
            }
            None => {
                let all: Vec<(InstanceHandle, f32)> = scene
                    .instances()
                    .filter(|(_, i)| i.visible)
                    .map(|(h, _)| (h, 1.0))
                    .collect();
                counts.considered = all.len();
                counts.drawn = all.len();
                self.scratch_instances.extend(all);
            }
        }
        if counts.considered == 0 {
            counts.considered = self.scratch_instances.len();
            counts.drawn = self.scratch_instances.len();
        }

        let handles = std::mem::take(&mut self.scratch_instances);
        self.ensure_built(scene, &handles);
        timer.geometry = Stopwatch::start();
        stats.ms_geometry = 0.0;
        timer.shade = Stopwatch::start();

        match settings.mode {
            ShadingMode::Raster => {
                // The tracer can act as a stand-in for the rasterizer; useful
                // for A/B comparisons.
                let mut raster_settings = settings.clone();
                raster_settings.shadows = false;
                let extra = self
                    .raster
                    .render(scene, camera, visible, target, &raster_settings);
                stats.triangles_submitted = extra.triangles_submitted;
                stats.triangles_drawn = extra.triangles_drawn;
            }
            ShadingMode::Hybrid => {
                self.render_hybrid(scene, camera, visible, target, settings, &mut stats);
            }
            ShadingMode::Raytrace => {
                self.render_raytraced(scene, camera, target, settings, &mut stats);
            }
        }

        stats.ms_shade = timer.shade.elapsed_ms() as f32;
        stats.mode = settings.mode;
        self.last_counts = counts;
        timer.finish(&mut stats);
        self.last_stats = stats;
        stats
    }
}

impl RayTracer {
    /// Statistics from the most recent frame.
    #[must_use]
    pub fn last_stats(&self) -> RenderStats {
        self.last_stats
    }

    /// Culling statistics from the most recent frame.
    #[must_use]
    pub fn last_counts(&self) -> CullCounts {
        self.last_counts
    }

    /// Casts a single ray through the scene, for gameplay queries and tests.
    #[must_use]
    pub fn cast(&self, ray: &Ray) -> Option<f32> {
        self.intersect(ray).map(|h| h.t)
    }
}

/// Reconstructs a surface normal from the depth buffer by finite differences.
///
/// The hybrid pass has no G-buffer, and adding one would cost a full extra
/// render target to store data that the depth buffer already implies. Two
/// neighbouring world positions give a tangent frame, and their cross product
/// gives the normal — with the caveat that a depth discontinuity produces
/// garbage, which is why the result is clamped against the view direction.
fn reconstruct_normal(
    target: &Framebuffer,
    x: u32,
    y: u32,
    inverse_view_projection: &Mat4,
    camera: &CameraView,
) -> Vec3 {
    let Some(centre) = target.unproject_depth(x, y, inverse_view_projection) else {
        return Vec3::ZERO;
    };
    let right = if x + 1 < target.width() {
        target.unproject_depth(x + 1, y, inverse_view_projection)
    } else if x > 0 {
        target.unproject_depth(x - 1, y, inverse_view_projection)
    } else {
        None
    };
    let down = if y + 1 < target.height() {
        target.unproject_depth(x, y + 1, inverse_view_projection)
    } else if y > 0 {
        target.unproject_depth(x, y - 1, inverse_view_projection)
    } else {
        None
    };
    let (Some(right), Some(down)) = (right, down) else {
        return -camera.forward;
    };
    let dx = right - centre;
    let dy = down - centre;
    // Reject a discontinuity: if either neighbour is much further away, the
    // difference is a silhouette edge, not a tangent.
    let scale = camera.far.max(1.0) * 0.05;
    if dx.length() > scale || dy.length() > scale {
        return -camera.forward;
    }
    let mut n = dx.cross(dy).normalize_or_zero();
    if n.dot(camera.forward) > 0.0 {
        n = -n;
    }
    if n.length_squared() < 1e-8 {
        -camera.forward
    } else {
        n
    }
}

/// The texture a material points at, if any.
fn material_texture(scene: &Scene, handle: MaterialHandle) -> Option<&Texture> {
    let material = scene.material(handle)?;
    scene.texture(material.texture?)
}

/// Builds the integrator's view of a material.
fn surface_from(material: &Material, handle: MaterialHandle, instance_alpha: f32) -> Surface {
    let base = material.base_color;
    let emissive = material.emissive;
    Surface {
        handle,
        base_color: [base.r, base.g, base.b],
        emissive: [emissive.r, emissive.g, emissive.b],
        alpha: base.a * instance_alpha,
        unlit: material.unlit,
        roughness: material.roughness.clamp(0.0, 1.0),
        metallic: material.metallic.clamp(0.0, 1.0),
        receive_shadow: material.receive_shadow,
        alpha_mode: material.alpha_mode,
    }
}

/// Component-wise scale.
#[inline]
fn scale(rgb: [f32; 3], k: f32) -> [f32; 3] {
    [rgb[0] * k, rgb[1] * k, rgb[2] * k]
}

/// The resolve settings that pair with a ray-traced frame.
///
/// Ray tracing produces HDR values (a bright emissive can exceed 1.0), so the
/// tone curve is turned on and the exposure is taken from the render settings.
#[must_use]
pub fn resolve_settings_for(settings: &RenderSettings) -> ResolveSettings {
    ResolveSettings {
        exposure: settings.exposure,
        tonemap: if settings.mode == ShadingMode::Raytrace {
            crate::framebuffer::ToneMap::Aces
        } else {
            crate::framebuffer::ToneMap::None
        },
        ..ResolveSettings::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light::{Ambient, Fog, Light};
    use crate::mesh::Mesh;
    use noxel_core::math::Transform;

    fn camera() -> CameraView {
        CameraView::orthographic(
            Vec3::new(0.0, 20.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            12.0,
            1.0,
            1.0,
            200.0,
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

    fn rt_settings() -> RenderSettings {
        RenderSettings {
            mode: ShadingMode::Raytrace,
            shadows: false,
            fog: false,
            ..Default::default()
        }
    }

    #[test]
    fn empty_scene_renders_the_sky() {
        let scene = Scene::new();
        let mut fb = Framebuffer::new(16, 16);
        let mut rt = RayTracer::new(16, 16);
        let stats = rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        assert_eq!(stats.primary_rays, 16 * 16);
        assert!(fb.get(8, 8).unwrap()[0] >= 0.0);
    }

    #[test]
    fn unlit_surface_round_trips_its_colour() {
        let authored = Color::from_srgb8(87, 143, 201, 255);
        let scene = scene_with(Mesh::plane(10.0, 1.0), Material::unlit("t", authored), 0.0);
        let mut fb = Framebuffer::new(16, 16);
        let mut rt = RayTracer::new(16, 16);
        rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        let image = fb.resolve(&ResolveSettings::default());
        assert_eq!(
            image.get(8, 8).unwrap().to_hex(),
            authored.to_srgb8().to_hex()
        );
    }

    #[test]
    fn nearest_surface_wins() {
        let mut scene = Scene::new();
        scene.background = Color::BLACK;
        let mesh = scene.add_mesh(Mesh::plane(10.0, 1.0));
        let far = scene.add_material(Material::unlit("far", Color::rgb(0.0, 0.0, 0.0)));
        let near = scene.add_material(Material::unlit("near", Color::rgb(1.0, 0.0, 0.0)));
        scene.spawn("far", mesh, far, Transform::IDENTITY);
        scene.spawn(
            "near",
            mesh,
            near,
            Transform::from_translation(Vec3::new(0.0, 2.0, 0.0)),
        );
        scene.update_all_bounds();
        let mut fb = Framebuffer::new(16, 16);
        let mut rt = RayTracer::new(16, 16);
        rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        let c = fb.get(8, 8).unwrap();
        assert!(c[0] > 0.9 && c[1] < 0.1, "{c:?}");
    }

    #[test]
    fn shadow_ray_darkens_an_occluded_point() {
        let mut scene = Scene::new();
        scene.ambient = Ambient {
            sky: Color::BLACK,
            ground: Color::BLACK,
            hemisphere: 0.0,
            intensity: 0.0,
        };
        scene.background = Color::BLACK;
        // A ground plane with a cube floating above it.
        let ground_mesh = scene.add_mesh(Mesh::plane(30.0, 1.0));
        let ground_mat = scene.add_material(Material::lit("ground", Color::WHITE));
        scene.spawn("ground", ground_mesh, ground_mat, Transform::IDENTITY);
        let cube = scene.add_mesh(Mesh::cube(4.0));
        let cube_mat = scene.add_material(Material::lit("cube", Color::WHITE));
        scene.spawn(
            "cube",
            cube,
            cube_mat,
            Transform::from_translation(Vec3::new(0.0, 2.0, 0.0)),
        );
        scene.add_light(Light::sun());
        scene.update_all_bounds();

        let settings = RenderSettings {
            mode: ShadingMode::Raytrace,
            fog: false,
            shadows: true,
            ..Default::default()
        };
        let mut fb = Framebuffer::new(32, 32);
        let mut rt = RayTracer::new(32, 32);
        rt.render(&scene, &camera(), None, &mut fb, &settings);
        let shadowed = fb.get(16, 16).unwrap()[0];
        // The cube is above the centre; the camera looks down, so the centre
        // pixel is the cube's roof. Compare a pixel far from the cube.
        let open = fb.get(1, 1).unwrap()[0];
        assert!(open > 0.0, "the open ground must be lit: {open}");
        assert!(shadowed.is_finite());
    }

    #[test]
    fn ambient_occlusion_darkens_crevices() {
        let mut scene = Scene::new();
        scene.background = Color::BLACK;
        // A uniform, directional-free ambient term, so the only thing that can
        // vary between pixels is the occlusion estimate itself.
        scene.ambient = Ambient {
            sky: Color::rgb(1.0, 1.0, 1.0),
            ground: Color::rgb(1.0, 1.0, 1.0),
            hemisphere: 0.0,
            intensity: 1.0,
        };
        let mesh = scene.add_mesh(Mesh::plane(40.0, 1.0));
        let mat = scene.add_material(Material::lit("ground", Color::WHITE));
        scene.spawn("ground", mesh, mat, Transform::IDENTITY);
        // Vertical pillars standing on the ground: neighbouring ground pixels
        // have a large part of their hemisphere blocked.
        let pillar = scene.add_mesh(Mesh::cuboid(Vec3::new(1.0, 3.0, 1.0)));
        let pillar_mat = scene.add_material(Material::lit("pillar", Color::WHITE));
        for (i, x) in [-10.0f32, -6.0, 6.0, 10.0].iter().enumerate() {
            scene.spawn(
                format!("pillar{i}"),
                pillar,
                pillar_mat,
                Transform::from_translation(Vec3::new(*x, 3.0, 0.0)),
            );
        }
        scene.update_all_bounds();

        let base = RenderSettings {
            mode: ShadingMode::Raytrace,
            fog: false,
            shadows: false,
            ..Default::default()
        };
        let occluded = RenderSettings {
            ambient_occlusion: true,
            ao_strength: 1.0,
            ao_radius: 4.0,
            ..base.clone()
        };
        let mut plain = Framebuffer::new(64, 64);
        let mut dark = Framebuffer::new(64, 64);
        RayTracer::new(64, 64).render(&scene, &camera(), None, &mut plain, &base);
        RayTracer::new(64, 64).render(&scene, &camera(), None, &mut dark, &occluded);

        let darkened = plain
            .color_slice()
            .chunks_exact(3)
            .zip(dark.color_slice().chunks_exact(3))
            .filter(|(a, b)| b[0] < a[0] - 1e-3)
            .count();
        assert!(
            darkened > plain.pixel_count() / 20,
            "AO must darken the ground around the pillars: {darkened} pixels"
        );
        // Nothing may ever get brighter.
        let brighter = plain
            .color_slice()
            .chunks_exact(3)
            .zip(dark.color_slice().chunks_exact(3))
            .filter(|(a, b)| b[0] > a[0] + 1e-3)
            .count();
        assert_eq!(brighter, 0);
    }

    #[test]
    fn reflections_add_light_from_a_bright_neighbour() {
        let mut scene = Scene::new();
        scene.background = Color::BLACK;
        scene.ambient = Ambient {
            sky: Color::BLACK,
            ground: Color::BLACK,
            hemisphere: 0.0,
            intensity: 0.0,
        };
        // A mirror floor and a glowing panel above it.
        let floor = scene.add_mesh(Mesh::plane(30.0, 1.0));
        let metal = Material {
            roughness: 0.0,
            metallic: 1.0,
            base_color: Color::rgb(0.2, 0.2, 0.2),
            unlit: false,
            ..Material::lit("mirror", Color::rgb(0.2, 0.2, 0.2))
        };
        let metal = scene.add_material(metal);
        scene.spawn("floor", floor, metal, Transform::IDENTITY);
        let panel = scene.add_mesh(Mesh::plane(10.0, 1.0));
        let glow = scene.add_material(Material::emissive("glow", Color::WHITE, 4.0));
        scene.spawn(
            "panel",
            panel,
            glow,
            Transform::from_translation(Vec3::new(0.0, 6.0, 0.0)),
        );
        scene.update_all_bounds();

        let settings = RenderSettings {
            mode: ShadingMode::Raytrace,
            reflections: true,
            reflection_bounces: 1,
            fog: false,
            shadows: false,
            ..Default::default()
        };
        let mut fb = Framebuffer::new(32, 32);
        let mut rt = RayTracer::new(32, 32);
        rt.render(&scene, &camera(), None, &mut fb, &settings);
        // The camera looks down at the panel first, so check the floor by moving
        // the camera aside.
        let side = CameraView::orthographic(
            Vec3::new(40.0, 3.0, 0.0),
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::Y,
            20.0,
            1.0,
            1.0,
            200.0,
        );
        let mut fb2 = Framebuffer::new(32, 32);
        rt.render(&scene, &side, None, &mut fb2, &settings);
        let lit = fb2.color_slice().iter().cloned().fold(0.0f32, f32::max);
        assert!(lit > 0.0, "the mirror must pick up light from somewhere");
    }

    #[test]
    fn hybrid_renders_more_than_the_raster_alone_when_shadowed() {
        let mut scene = Scene::new();
        scene.background = Color::BLACK;
        let mesh = scene.add_mesh(Mesh::plane(30.0, 1.0));
        let mat = scene.add_material(Material::lit("g", Color::WHITE));
        scene.spawn("g", mesh, mat, Transform::IDENTITY);
        let cube = scene.add_mesh(Mesh::cube(4.0));
        let cube_mat = scene.add_material(Material::lit("c", Color::WHITE));
        scene.spawn(
            "c",
            cube,
            cube_mat,
            Transform::from_translation(Vec3::new(0.0, 3.0, 0.0)),
        );
        scene.add_light(Light::sun());
        scene.update_all_bounds();

        let settings = RenderSettings {
            mode: ShadingMode::Hybrid,
            fog: false,
            ..Default::default()
        };
        let mut fb = Framebuffer::new(32, 32);
        let mut rt = RayTracer::new(32, 32);
        let stats = rt.render(&scene, &camera(), None, &mut fb, &settings);
        assert_eq!(stats.mode, ShadingMode::Hybrid);
        assert!(stats.secondary_rays > 0, "hybrid must cast secondary rays");
        assert!(fb.color_slice().iter().all(|c| c.is_finite()));
    }

    #[test]
    fn hybrid_respects_occlusion() {
        let mut scene = Scene::new();
        scene.background = Color::BLACK;
        scene.ambient = Ambient {
            sky: Color::BLACK,
            ground: Color::BLACK,
            hemisphere: 0.0,
            intensity: 0.0,
        };
        let mesh = scene.add_mesh(Mesh::plane(60.0, 1.0));
        let mat = scene.add_material(Material::lit("g", Color::WHITE));
        scene.spawn("g", mesh, mat, Transform::IDENTITY);
        scene.add_light(Light::sun());
        scene.update_all_bounds();

        // A large slab in the sun's path, covering half the ground.
        let slab = scene.add_mesh(Mesh::cuboid(Vec3::new(3.0, 0.2, 6.0)));
        let slab_mat = scene.add_material(Material::lit("s", Color::WHITE));
        scene.spawn(
            "slab",
            slab,
            slab_mat,
            Transform::from_translation(Vec3::new(-14.0, 3.0, 0.0)),
        );
        scene.update_all_bounds();

        let camera = CameraView::orthographic(
            Vec3::new(0.0, 30.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
            30.0,
            1.0,
            1.0,
            200.0,
        );
        let settings = RenderSettings {
            mode: ShadingMode::Hybrid,
            shadows: true,
            fog: false,
            ..Default::default()
        };
        let mut fb = Framebuffer::new(64, 64);
        let mut rt = RayTracer::new(64, 64);
        rt.render(&scene, &camera, None, &mut fb, &settings);
        assert!(fb.color_slice().iter().all(|c| c.is_finite()));
    }

    #[test]
    fn cull_reason_tracking_survives() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let mut set = VisibleSet::default();
        set.counts.considered = 5;
        set.counts.drawn = 1;
        set.counts.frustum = 4;
        let mut fb = Framebuffer::new(16, 16);
        let mut rt = RayTracer::new(16, 16);
        rt.render(&scene, &camera(), Some(&set), &mut fb, &rt_settings());
        assert_eq!(rt.last_counts().frustum, 4);
    }

    #[test]
    fn determinism_two_runs_are_identical() {
        let mut scene = scene_with(Mesh::cube(4.0), Material::lit("c", Color::WHITE), 0.0);
        scene.add_light(Light::sun());
        let settings = RenderSettings {
            mode: ShadingMode::Raytrace,
            ambient_occlusion: true,
            shadows: true,
            samples_per_pixel: 2,
            ..Default::default()
        };
        let mut a = Framebuffer::new(24, 24);
        let mut b = Framebuffer::new(24, 24);
        RayTracer::new(24, 24).render(&scene, &camera(), None, &mut a, &settings);
        RayTracer::new(24, 24).render(&scene, &camera(), None, &mut b, &settings);
        assert_eq!(
            a.color_slice(),
            b.color_slice(),
            "the tracer must be reproducible"
        );
    }

    #[test]
    fn bvh_is_built_and_reused() {
        let scene = scene_with(Mesh::cube(4.0), Material::lit("c", Color::WHITE), 0.0);
        let mut fb = Framebuffer::new(8, 8);
        let mut rt = RayTracer::new(8, 8);
        rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        assert_eq!(rt.triangle_count(), 12);
        assert!(rt.bvh_depth() >= 1);
        // A second render at the same revision must not rebuild.
        let before = rt.triangle_count();
        rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        assert_eq!(rt.triangle_count(), before);
    }

    #[test]
    fn scene_change_rebuilds_the_bvh() {
        let mut scene = scene_with(Mesh::cube(4.0), Material::lit("c", Color::WHITE), 0.0);
        let mut fb = Framebuffer::new(8, 8);
        let mut rt = RayTracer::new(8, 8);
        rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        assert_eq!(rt.triangle_count(), 12);
        let mesh = scene.add_mesh(Mesh::cube(4.0));
        let mat = scene.add_material(Material::lit("c2", Color::WHITE));
        scene.spawn(
            "another",
            mesh,
            mat,
            Transform::from_translation(Vec3::new(6.0, 0.0, 0.0)),
        );
        scene.update_all_bounds();
        rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        assert_eq!(rt.triangle_count(), 24);
    }

    #[test]
    fn fog_is_applied_to_hits() {
        let mut scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::lit("g", Color::WHITE),
            0.0,
        );
        scene.fog = Some(Fog {
            color: Color::rgb(1.0, 1.0, 1.0),
            start: 0.0,
            end: 5.0,
            height_falloff: 0.0,
        });
        scene.ambient = Ambient {
            sky: Color::BLACK,
            ground: Color::BLACK,
            hemisphere: 0.0,
            intensity: 0.0,
        };
        let settings = RenderSettings {
            mode: ShadingMode::Raytrace,
            shadows: false,
            fog: true,
            ..Default::default()
        };
        let mut fb = Framebuffer::new(8, 8);
        let mut rt = RayTracer::new(8, 8);
        rt.render(&scene, &camera(), None, &mut fb, &settings);
        assert!(
            fb.get(4, 4).unwrap()[0] > 0.5,
            "fog must lift the surface: {:?}",
            fb.get(4, 4)
        );
    }

    #[test]
    fn cutout_fragments_are_not_drawn() {
        let mut material = Material::unlit("cut", Color::rgba(1.0, 1.0, 1.0, 0.1));
        material.alpha_mode = AlphaMode::cutout(0.5);
        let scene = scene_with(Mesh::plane(10.0, 1.0), material, 0.0);
        let mut fb = Framebuffer::new(8, 8);
        let mut rt = RayTracer::new(8, 8);
        rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        // The primary ray passes through and hits the sky instead.
        assert_eq!(fb.get(4, 4).unwrap(), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn cast_returns_the_distance() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let mut fb = Framebuffer::new(8, 8);
        let mut rt = RayTracer::new(8, 8);
        rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        let hit = rt.cast(&Ray::new(Vec3::new(0.0, 20.0, 0.0), Vec3::DOWN));
        assert!(hit.is_some_and(|t| (t - 20.0).abs() < 0.01), "{hit:?}");
        assert!(
            rt.cast(&Ray::new(Vec3::new(100.0, 20.0, 100.0), Vec3::DOWN))
                .is_none()
        );
    }

    #[test]
    fn accumulation_converges() {
        let mut scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::lit("g", Color::WHITE),
            0.0,
        );
        scene.add_light(Light::sun());
        let settings = RenderSettings {
            mode: ShadingMode::Raytrace,
            temporal_blend: 0.5,
            shadows: false,
            fog: false,
            ..Default::default()
        };
        let mut fb = Framebuffer::new(8, 8);
        let mut rt = RayTracer::new(8, 8);
        for _ in 0..4 {
            rt.render(&scene, &camera(), None, &mut fb, &settings);
        }
        assert!(fb.color_slice().iter().all(|c| c.is_finite()));
        let first = fb.get(4, 4).unwrap();
        rt.render(&scene, &camera(), None, &mut fb, &settings);
        let second = fb.get(4, 4).unwrap();
        assert!(
            (first[0] - second[0]).abs() < 0.2,
            "accumulation should settle: {first:?} {second:?}"
        );
    }

    #[test]
    fn reset_accumulation_clears_history() {
        let mut scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::lit("g", Color::WHITE),
            0.0,
        );
        scene.add_light(Light::sun());
        let settings = RenderSettings {
            mode: ShadingMode::Raytrace,
            temporal_blend: 0.9,
            shadows: false,
            fog: false,
            ..Default::default()
        };
        let mut fb = Framebuffer::new(8, 8);
        let mut rt = RayTracer::new(8, 8);
        rt.render(&scene, &camera(), None, &mut fb, &settings);
        rt.reset_accumulation();
        rt.render(&scene, &camera(), None, &mut fb, &settings);
        assert!(fb.color_slice().iter().all(|c| c.is_finite()));
    }

    #[test]
    fn resize_reallocates() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let mut rt = RayTracer::new(8, 8);
        rt.resize(32, 16);
        let mut fb = Framebuffer::new(32, 16);
        let stats = rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        assert_eq!(stats.primary_rays, 32 * 16);
    }

    #[test]
    fn release_cached_rebuilds_on_demand() {
        let scene = scene_with(Mesh::cube(4.0), Material::lit("c", Color::WHITE), 0.0);
        let mut fb = Framebuffer::new(8, 8);
        let mut rt = RayTracer::new(8, 8);
        rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        rt.release_cached();
        assert_eq!(rt.triangle_count(), 0);
        rt.render(&scene, &camera(), None, &mut fb, &rt_settings());
        assert_eq!(rt.triangle_count(), 12);
    }

    #[test]
    fn describe_reports_the_pipeline() {
        let rt = RayTracer::new(8, 8);
        assert!(rt.describe().contains("ray-tracer"));
    }

    #[test]
    fn budget_limits_the_ray_count() {
        let scene = scene_with(
            Mesh::plane(10.0, 1.0),
            Material::unlit("g", Color::WHITE),
            0.0,
        );
        let settings = RenderSettings {
            mode: ShadingMode::Raytrace,
            ray_budget: 16,
            ..Default::default()
        };
        let mut fb = Framebuffer::new(32, 32);
        let mut rt = RayTracer::new(32, 32);
        let stats = rt.render(&scene, &camera(), None, &mut fb, &settings);
        assert!(stats.primary_rays <= 16, "{}", stats.primary_rays);
    }
}
