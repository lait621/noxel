//! # Noxel render
//!
//! Two renderers over one scene: a tile-binned **software rasterizer** and a
//! **ray tracer** (with a hybrid mode that rasterizes primary visibility and
//! ray-traces the secondary effects). See `docs/adr/0004-dual-renderer.md`.
//!
//! | Module | Purpose |
//! |---|---|
//! | [`framebuffer`] | the linear HDR colour + depth + id target, and the sRGB resolve |
//! | [`mesh`] | vertices and procedural primitives (plane, grid, cuboid, sphere, cylinder, billboard) |
//! | [`material`] | surface appearance, alpha modes and the material library |
//! | [`light`] | directional/point/spot lights, ambient terms and fog |
//! | [`scene`] | meshes, materials, textures, instances and lights |
//! | [`renderer`] | the `Renderer` trait, `CameraView`, `VisibleSet`, settings and statistics |
//! | [`raster`] | the tile-binned software rasterizer, shadow maps and post-processing |
//! | [`raytrace`] | the triangle BVH, the integrator, sampling and the hybrid pipeline |
//! | [`overlay`] | the debug overlay: wireframes, occlusion rays, text |
//!
//! ## Why a software renderer
//!
//! Noxel targets pixel-art top-down RPGs at 320x180 to 960x540. At those
//! resolutions a well-written tiled software rasterizer is fast enough for a
//! full frame in a couple of milliseconds, and it buys three things a GPU path
//! cannot: **zero dependencies**, **bit-identical output on every platform**
//! (which is what makes golden-image testing possible), and a headless path that
//! runs in CI with no display server. See
//! `docs/adr/0003-software-renderer-first.md`.
//!
//! A GPU backend is an explicit extension point: implement [`renderer::Renderer`]
//! and follow `docs/guides/gpu-backend.md`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

pub mod framebuffer;
pub mod light;
pub mod material;
pub mod mesh;
pub mod overlay;
pub mod raster;
pub mod raytrace;
pub mod renderer;
pub mod scene;

pub use framebuffer::{Framebuffer, ResolveSettings, ToneMap};
pub use light::{Ambient, Falloff, Fog, Light};
pub use material::{
    AlphaMode, Material, MaterialHandle, MaterialLibrary, MeshHandle, TextureHandle,
};
pub use mesh::{Mesh, Vertex};
pub use noxel_core::math::{Color, Color8};
pub use overlay::{Font, Overlay};
pub use raster::RasterRenderer;
pub use raytrace::RayTracer;
pub use renderer::{
    CameraView, CullCounts, CullReason, RenderSettings, RenderStats, Renderer, ShadingMode,
    Viewport, VisibleItem, VisibleSet,
};
pub use scene::{Instance, InstanceFlags, InstanceHandle, Scene, SceneStats};

/// The types most render code touches.
pub mod prelude {
    pub use crate::framebuffer::{Framebuffer, ResolveSettings, ToneMap};
    pub use crate::light::{Ambient, Fog, Light};
    pub use crate::material::{AlphaMode, Material};
    pub use crate::mesh::Mesh;
    pub use crate::renderer::{CameraView, RenderSettings, RenderStats, Renderer, VisibleSet};
    pub use crate::scene::{InstanceFlags, Scene};
    pub use noxel_core::math::{Color, Color8, Mat4, Transform, Vec2, Vec3};
}

/// The crate version, from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
