# ADR 0004 — One scene, two renderers

**Status:** accepted
**Applies to:** `noxel-render`

## Context

The requirement is to support both rasterization and ray tracing. The tempting
implementation is two pipelines with their own scene representations, because
each wants different data: a rasterizer wants screen-space triangles, a ray tracer
wants a world-space triangle soup with a BVH.

Two representations means two sets of bugs, two sets of material semantics, and a
scene that looks subtly different depending on which renderer drew it.

## Decision

**One scene, one camera view, one visible set, two `Renderer` implementations.**

```rust
pub trait Renderer {
    fn name(&self) -> &'static str;
    fn mode(&self) -> ShadingMode;
    fn resize(&mut self, width: u32, height: u32);
    fn release_cached(&mut self);
    fn render(&mut self, scene: &Scene, camera: &CameraView, visible: Option<&VisibleSet>,
              target: &mut Framebuffer, settings: &RenderSettings) -> RenderStats;
}
```

Both consume the same `Scene` (meshes, materials, instances, lights), the same
`CameraView`, the same `VisibleSet` from `noxel-visibility`, and the same
`RenderSettings`; both write into the same `Framebuffer` (linear HDR colour +
depth + instance id). Switching renderers is one field on `AppConfig`.

There are three modes, and the difference between them is *which rays are cast*:

| Mode | Primary visibility | Shadows | AO | Reflections |
|---|---|---|---|---|
| `Raster` | rasterizer | shadow map | — | — |
| `Hybrid` | rasterizer | ray-traced | ray-traced | — |
| `Raytrace` | ray tracer | ray-traced | ray-traced | ray-traced |

**Hybrid is the recommended quality mode.** A rasterized primary pass gives
exactly the crisp, pixel-aligned image pixel art needs; the expensive parts of ray
tracing are the *secondary* rays, which is precisely what a hybrid pass casts.
That is the trade every production renderer makes.

## Shared semantics

Both renderers read the same `Material`. In particular a material whose `unlit`
flag is set returns its base colour unchanged, in both paths, so a sprite drawn
with a ray tracer resolves to **the exact bytes the artist authored**. That is
asserted by test in both `raster` and `raytrace`:

```rust
let authored = Color::from_srgb8(87, 143, 201, 255);
// ... render ...
assert_eq!(image.get(8, 8).unwrap().to_hex(), authored.to_srgb8().to_hex());
```

This is why the raster path defaults to `ToneMap::None` while the ray-traced path
uses `ToneMap::Aces` (`RenderSettings::resolve`).

## Consequences

- The ray tracer keeps its own acceleration structure cache, keyed on
  `Scene::revision()`, so a static scene pays the BVH build once.
- The ray tracer in hybrid mode drives the rasterizer for its primary pass, which
  is why `RayTracer::raster_mut` exists.
- A new renderer (GPU, or a headless statistics-only renderer) is added by
  implementing one trait; every system above it is unchanged.

## Alternatives rejected

- **Separate scenes per renderer**: guarantees the two drift.
- **Ray tracing only**: an order of magnitude too slow at 960x540, and it throws
  away the pixel-exactness the art direction depends on.
