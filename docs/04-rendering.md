# Rendering

`noxel-render` ships two renderers over one scene — a tile-binned software
rasterizer and a CPU ray tracer — plus a hybrid that rasterizes primary
visibility and ray-traces the secondary effects. Design record:
`docs/adr/0003-software-renderer-first.md`, `0004-dual-renderer.md`,
`0005-color-management.md`.

## The three shading modes

`ShadingMode` selects which rays are cast, not which scene is drawn. All three
consume the same `Scene`, `CameraView`, `VisibleSet` and `RenderSettings`, and
all three write into the same `Framebuffer`.

| Mode | Primary | Shadows | AO | Reflections | Tone curve |
|---|---|---|---|---|---|
| `Raster` | rasterizer | shadow map | — | — | `None` |
| `Hybrid` | rasterizer | ray-traced | ray-traced | — (settings exist, this pass does not cast them) | `None` |
| `Raytrace` | ray tracer | ray-traced | ray-traced | ray-traced | `Aces` |

```rust
use noxel_render::renderer::{RenderSettings, ShadingMode};

let fast = RenderSettings::fast();     // Raster, 512 shadow map, no AO
let still = RenderSettings::quality(); // Hybrid, 2048 shadow map, AO, 4 spp
let _ = (fast.mode, still.mode);
```

| Situation | Use | Why |
|---|---|---|
| Gameplay at 60 Hz | `Raster` | one pass over the tiles plus a depth-only shadow pass |
| Screenshots, a lit 3D-looking scene | `Hybrid` | the primary image stays pixel-aligned; the expensive rays are all secondary |
| Golden images, comparison shots | `Raytrace` | the reference the other two are checked against |

No benchmark numbers are committed, so treat cost as a work model. At the default
320x180 (57 600 pixels):

| Mode | Work per frame | What dominates |
|---|---|---|
| `Raster` | 57 600 primary pixels + one 1024² depth-only pass | fragment shading, then shadow-map rasterization |
| `Hybrid` | that raster pass (shadow map switched off) + up to `ray_budget` secondary rays | the ray budget, not the pixel count |
| `Raytrace` | `samples_per_pixel` primary rays per pixel, each recursing into shadow/AO/reflection rays | primary rays; `ray_budget` defaults to 400 000 |

All three are deterministic — no pass reads a clock, a thread id or a global RNG
(`docs/adr/0008-deterministic-rendering.md`).

## The rasterizer

```text
scene -> vertex transform -> near-plane clip -> backface cull -> screen triangles
      -> tile binning -> per-tile raster -> shading (lights + shadow + fog + texture)
      -> transparent pass (back to front) -> post -> resolve
```

**32x32 tiles.** `TILE_SIZE` is 32, so a 320-wide frame is exactly ten tiles
across and a tile's triangle list stays in L1. `TileBins` is a counting sort (one
count pass, a prefix sum, one scatter), so each tile's triangles are contiguous
and the inner loop never walks the whole scene.

**Band parallelisation.** The framebuffer splits into row bands that are whole
*tile* rows — four tile rows per band with a job pool, one band for the whole
frame without. `Framebuffer::split_mut()` hands out colour, depth and id slices
at once and `split_at_mut` carves disjoint per-band slices, so the parallel path
needs no lock or per-pixel atomic. Both paths run the same
`raster_tile_in_band`, and `parallel_and_sequential_rendering_agree` asserts they
produce identical pixels.

```rust
use noxel_core::jobs::JobPool;
use noxel_render::raster::RasterRenderer;

let mut r = RasterRenderer::new(320, 180).with_jobs(JobPool::new(4));
r.set_shadow_map_size(1024);
assert_eq!(r.name(), "software-raster");
```

**The fill rule.** Without one, every pixel exactly on a shared triangle edge is
shaded twice — a dark quilt along a tessellated floor's seams and a darker
diagonal on transparent quads. `tie_owner(a, b)` accepts a pixel within
`TIE_EPSILON` of an edge only if the edge's first endpoint is greater (screen Y,
then X). Every closed, consistently wound surface traverses a shared edge in
opposite directions from its two triangles, so exactly one claims the pixel, with
no winding case analysis.

**Depth and clipping.** Depth is the projection's native `[0, 1]`, so the test is
`depth > stored => reject` — nearer or equal wins, with no remap. Triangles are
clipped against `w > 1e-5` *before* projection: one vertex inside becomes a
triangle, two inside becomes a quad that is fan-triangulated. The `edge` function
is the **negation** of the usual one, so a front face has positive area and both
`emit_clipped` and the pixel loop depend on that sign — change them together.
Interpolation is perspective-correct (`1/w` per vertex, divided back out). The
instance id lands in the id buffer; untouched pixels keep `Framebuffer::NO_ID`.

## The shadow map

One directional map, fitted to a window that follows the camera: a top-down world
is kilometres wide, so a single volume covering it would be centimetres per texel.

| Step | What happens |
|---|---|
| Fit | `ShadowMap::begin(direction, camera.position, extent, range, true)` builds a basis around the light, projects the camera onto it, **rounds that projection to whole texels**, and projects back |
| Why the snap | without it the volume slides sub-texel and every shadow edge crawls |
| Extent | `Light::Directional::shadow_extent` (default 40 m), depth range `extent * 4` |
| Pass | a minimal clip-space depth-only rasterizer that accepts both windings — a closed mesh seen from the light has mixed facing, and a single-sided map leaks |
| Filtering | `sample_pcf`: four bilinear taps; outside the volume returns `1.0` (lit), which hides the moving window |
| Bias | `shadow_bias` (0.0015) plus a normal offset of `max(0.02) * 8` world units |

Only `Light::Directional` uses the map; point and spot shadows come from the
ray-traced paths.

## The transparent pass

`AlphaMode::is_transparent()` (`Blend`, `Additive`) sends triangles to a second
pass that sorts **instances** back to front by camera distance to their bounds
centre (per-triangle sorting would be more accurate and would destroy the
batching), then draws each instance's triangles as one subset. It depth-tests but
does not depth-**write**, which is what lets a second transparent surface behind
the first still be seen. `Opaque` and `Cutout` stay in the opaque pass; `Cutout`
discards below its threshold and treats the rest as opaque, so foliage sorts with
the world.

`Additive` genuinely **adds**: the fragment's radiance is multiplied by its alpha
and summed into the target, rather than composited source-over. That is the whole
point of the mode — a fireball, a lamp glow or a magic sparkle should *light* what
is behind it, and compositing source-over would darken the background instead,
which is exactly backwards. At a 320x180 internal resolution, where an effect is
the entire pixel, the difference is not subtle.

Because linear radiance is unbounded, additive output can exceed 1.0; the tone
curve in `Framebuffer::resolve` is what brings it back (see the colour section).

## The ray tracer

- **Acceleration.** `TriangleBvh` is built from the transformed triangle soup and
  cached against `Scene::revision()`, so a static scene pays once.
  `triangle_count()`/`bvh_depth()` expose it; `release_cached()` drops it.
- **Sampling.** Rays come from `sampling::Sampler`, a pure function of
  `(x, y, frame, sample)` — Hammersley points plus a per-pixel jitter, no global
  RNG.
- **Primary rays.** Orthographic cameras generate parallel rays across
  `ortho_height`; perspective cameras build from `fov_y`; both jitter within the
  pixel.
- **Budget.** `ray_budget` (400 000) is a hard stop: quality degrades rather than
  the frame blowing. `samples_per_pixel` (1) is the quality knob inside it.
- **AO.** Four cosine-weighted hemisphere samples stratified by `hammersley(i, 4)`
  and jittered, cast up to `ao_radius` (1.5 m). `ao_strength` (0.6) blends
  unoccluded to occluded. Needs `ambient_occlusion: true`.
- **Reflections.** Schlick Fresnel against F0 = 0.04 (lerped to base colour by
  `metallic`), attenuated by `1 - roughness`, only when `roughness < 0.999`; the
  direction is a cone sample biased towards the mirror; recursion stops at
  `reflection_bounces` (1).
- **Temporal accumulation.** With `temporal_blend > 0` each frame blends into an
  accumulation buffer (the first frame takes the sample whole);
  `reset_accumulation()` discards the history after a cut. At `0` the buffer is
  scratch only, so post has no temporal component.
- **Misses.** Fog colour when the scene has fog (no horizon seam), else the
  background blended towards a brighter horizon by `ray.dir.y`.
- **Two-sided shading.** A hit whose interpolated normal faces away from the ray
  is flipped so the surface still lights.
- **Documented limitation.** Transparent materials are not transmitted or
  refracted; the tracer renders them as opaque once.

```rust,no_run
use noxel_render::Framebuffer;
use noxel_render::raytrace::RayTracer;
use noxel_render::renderer::{CameraView, RenderSettings, ShadingMode};
use noxel_render::scene::Scene;

let mut tracer = RayTracer::new(320, 180);
let mut scene = Scene::new();
let camera = CameraView::default();
let mut target = Framebuffer::new(320, 180);
let settings = RenderSettings {
    mode: ShadingMode::Raytrace,
    samples_per_pixel: 4,
    ..RenderSettings::default()
};
let stats = tracer.render(&scene, &camera, None, &mut target, &settings);
let _ = (&mut scene, stats);
```

## The hybrid pipeline

Hybrid runs the rasterizer with its shadow map **off** — the shadows are about to
be ray-traced — then walks every pixel that has geometry:

1. `Framebuffer::unproject_depth(x, y, inv_view_projection)` turns stored depth
   into a world position; depth `>= 1.0` is background and is skipped.
2. `reconstruct_normal` rebuilds a normal from the right and down neighbours'
   depth. If either neighbour is much further than `camera.far * 0.05` the
   difference is a silhouette edge, not a tangent, and the fallback
   `-camera.forward` is used — which is why no normal buffer is needed.
3. A sun-visibility ray multiplies a blocked pixel by **0.35**, not 0, so a
   hybrid shadow is a soft darkening.
4. AO is four stratified cosine rays inside `ao_radius`; the pixel is multiplied
   by `1 - ao_strength * (1 - ao)`.
5. Only pixels whose factor is below 0.999 are written back.

Both ray loops count against `ray_budget`.

## Post-processing

`PostProcess::apply` runs on the **linear HDR** buffer before the resolve, so an
effect can push above 1.0 and let the tone curve bring it back. Scratch buffers
are owned by the `PostProcess` instance, so the chain does not allocate per frame.

| Setting | Default | Effect |
|---|---|---|
| `gain`, `lift` | 1.0, 0.0 | `rgb = rgb * gain + lift`, clamped at zero |
| `desaturate` | 0.0 | blends towards luminance |
| `bloom_threshold`, `bloom_intensity`, `bloom_radius` | 0, 0, 4 | bright pass, two separable box blurs (radius, then 2×), added back |
| `vignette`, `vignette_radius` | 0.0, 0.6 | squared falloff from the corner distance |
| `scanline_period`, `scanline_strength` | 0, 0.0 | darkens every `period`th row |

`PostSettings::default()` is all-off — a pixel-art engine should not tint a
palette unless asked; `cinematic()` and `retro()` are the presets, and
`is_identity()` short-circuits the whole chain. Order inside `apply`:
gain/lift, desaturate, bloom, vignette, scanlines.

Dithering is the exception: it belongs to quantisation, so it lives in
`Framebuffer::resolve` as `dither_strength`, a 4x4 Bayer offset applied in linear
space before the sRGB conversion.

## The framebuffer and the resolve

| Buffer | Type | Layout |
|---|---|---|
| colour | `f32 * 3` per pixel | **linear** HDR RGB |
| depth | `f32` per pixel | `[0, 1]`, `1.0` is the far plane |
| ids | `u32` per pixel | instance `user_data`; `Framebuffer::NO_ID` where nothing drew |

`resolve(&ResolveSettings)` is the only place the engine leaves linear space:
exposure, tone curve (`None`, `Reinhard`, `Aces`), optional dithering, then
`linear_to_srgb()`. With the defaults — `ToneMap::None`, exposure 1.0, no dither
— an unlit surface whose colour came from a palette round-trips to exactly the
authored byte (`unlit_sprite_round_trips_to_the_authored_palette`,
`palette_round_trips_exactly`).

```rust
use noxel_render::framebuffer::{Framebuffer, ResolveSettings, ToneMap};

let mut fb = Framebuffer::new(320, 180);
let exact = fb.resolve(&ResolveSettings::default());
let graded = fb.resolve(&ResolveSettings { tonemap: ToneMap::Aces, ..ResolveSettings::default() });
let _ = (exact, graded);
```

`resolve_palette(&settings, &palette)` is the same resolve with every pixel
snapped to the nearest palette entry, in sRGB (where the artist chose it).

**The tone curve follows the renderer, automatically.** `RenderSettings::resolve()`
picks `Aces` when its `mode` field is `Raytrace`, and `App::resolve()` derives the
mode from `config.mode` — the mode the frame was actually rendered with — rather
than trusting whatever `config.render.mode` happens to hold. Switching renderers
with `AppConfig::with_mode()` is therefore enough on its own: a ray-traced frame
resolves with a curve, and a raster frame still resolves byte-exactly with none.

## Pixel-art rules

1. **Internal resolution is fixed.** `AppConfig::internal` defaults to 320x180;
   the scene renders there and is upscaled. A window resize must not change the
   number of scene pixels, or sprites shimmer.
2. **Upscale by whole integers.** `Viewport::pixel_art(w, h, iw, ih)` gives
   `integer_scale()` (rounded *down*, so no pixel is repeated twice),
   `scaled_size()`, `letterbox_offset()` and `internal_aspect()`.
3. **Snap the camera, not the sprite.** `PixelPerfect::at(320, 180)` sets
   `snap_focus` and `snap_sprites`. `snap_to_pixels(world_size, units_per_pixel)`
   rounds a *size* while leaving position continuous — the standard fix for a
   sprite alternating between 15 and 16 px as it moves.
4. **Nearest sampling with a half-texel inset.** `Texture::sample_pixel_art`
   snaps to the texel centre, so a `u` on an atlas cell boundary resolves to the
   left/top texel and `u == 1.0` resolves to the last texel instead of wrapping.
5. **Unlit materials round-trip exactly.** `Material::default()` sets
   `unlit: true` deliberately: the engine renders hand-authored pixel art, and
   surprising an artist with lighting they did not ask for is worse than lighting
   that must be switched on.

## Materials

```rust,no_run
use noxel_core::math::Color;
use noxel_render::material::Material;

// `texture_handle` is a `TextureHandle` from `Scene::add_texture`.
let flat    = Material::unlit("flat", Color::from_hex(0xFF00_FF00));
let sprite  = Material::sprite("sprite", texture_handle);
let terrain = Material::lit("terrain", Color::WHITE);
let leaves  = Material::foliage("leaves", texture_handle, 0.5);
let glass   = Material::transparent("glass", Color::rgba(0.6, 0.7, 0.8, 0.4));
let lamp    = Material::emissive("lamp", Color::from_hex(0xFFFF_E0A0), 2.0);
let tiled   = flat.with_uv_scale(noxel_core::math::Vec2::new(4.0, 4.0));
let _ = (sprite, terrain, leaves, glass, lamp, tiled);
```

| Builder | `unlit` | Alpha mode | Notes |
|---|---|---|---|
| `unlit(name, color)` | true | `Opaque` | the most common material in a pixel game |
| `sprite(name, texture)` | true | `Opaque` | textured unlit |
| `lit(name, color)` | false | `Opaque` | roughness 0.95 |
| `foliage(name, texture, threshold)` | true | `Cutout` | double-sided |
| `transparent(name, color)` | true | `Blend` | |
| `emissive(name, color, strength)` | true | `Opaque` | black base, `cast_shadow = false` |

Modifiers: `with_texture`, `with_uv_scale`, `with_alpha_mode`, `with_lit`,
`without_shadow_casting`. Other fields: `base_color` (linear), `emissive`,
`roughness` (0 mirror → 1 matte), `metallic`, `specular`, `receive_shadow`,
`cast_shadow`, `double_sided`, `uv_scale`/`uv_offset`, `palette_snap`.
`sort_key()` packs the state-grouping flags into one `u32`.

| `AlphaMode` | Depth-written | Pass | Use for |
|---|---|---|---|
| `Opaque` | yes | opaque | everything solid |
| `Cutout { threshold }` | yes | opaque | foliage, fences, grates |
| `Blend` | no | transparent, sorted | glass, water, a faded roof |
| `Additive` | no | transparent, sorted | glow, fire, magic — genuinely adds light |

## Lights, ambient and fog

`Scene::lights` is a public `Vec<Light>`; `add_light` appends;
`primary_sun()` returns the first `Light::Directional`, which is what the shadow
map and the hybrid shadow pass use.

| Constructor | Variant | Defaults |
|---|---|---|
| `Light::sun()` | `Directional` | direction `(-0.35, -1.0, -0.25)` normalised, warm white, intensity 1.0, `cast_shadow: true`, `shadow_bias: 0.0015`, `normal_bias: 0.02`, `shadow_extent: 40.0` |
| `Light::point(pos, color, intensity, range)` | `Point` | `Falloff::SmoothRange`, no shadow |
| `Light::torch(pos, dir, color, intensity, range)` | `Spot` | `cos_inner: 0.85`, `cos_outer: 0.55`, casts a shadow |

`Light` is an enum with public fields and **no `with_*` builders**: construct the
variant, or match on it. `Falloff` is `InverseSquare` (`1/(1+d²)`, the tracer's
default), `SmoothRange` (`1 - (d/range)²`, the rasterizer's, so a light can be
excluded by a range query) or `None`.

`Scene::ambient` is an `Ambient` { `sky`, `ground`, `hemisphere`, `intensity` }:
`default()` is a cool sky/warm ground daylight pair, `night()` and `interior()`
are presets, `radiance_at(normal)` is the per-normal term, `flat()` the
directionless one.

`Scene::fog` is an `Option<Fog>` { `color`, `start` (40 m), `end` (120 m),
`height_falloff` (0 disables height fog) }; `factor_at(position, camera_position)`
is distance-based and `apply` blends towards the colour. Fog applies to unlit
surfaces too — atmospheric perspective is a property of the scene, and a
hand-painted sprite fading into the distance is the pixel-art look.

## The debug overlay

`noxel-render::overlay` is drawing primitives, not a UI toolkit
(`docs/adr/0009-no-ui.md`). It writes into the **linear** framebuffer using the
exact linear value of the requested sRGB colour, so `#FF0000` resolves back to
`#FF0000`.

| Method | Draws |
|---|---|
| `text` / `text_scaled` | the built-in 3x5 font (`WIDTH` 3, `HEIGHT` 5, one column of spacing) |
| `line`, `screen_line`, `segment` | world and screen lines |
| `aabb`, `ray`, `highlight_bounds` | world-space box wireframes and camera rays |
| `cross`, `crosshair` | markers |
| `screen_rect`, `panel`, `stats_panel` | panels |

`set_depth_test(false)` draws over everything, which is what a HUD wants.
`RasterRenderer::debug_draw_tiles` outlines every non-empty tile bin — the
fastest way to see whether the binner is working. The font is uppercase,
case-insensitive, and a character with no glyph (any non-ASCII codepoint) is
skipped and its cell left blank rather than truncated or panicked; `'\n'` is one
blank cell and does not break the line, so call `text_scaled` once per line.

## Common mistakes

- **"My sprite's colour is wrong."** Something is lit: `Material::default()` is
  unlit, but a struct literal or `Material::lit` is not.
- **"Transparent objects vanish behind other transparent objects."** The pass
  sorts by *instance centre*, so overlapping translucency inside one instance
  cannot interleave; split them into separate instances.
- **"Shadows crawl, jitter, or cost too much."** Texel snapping is off, or
  `shadow_extent` is far larger than the visible area, so each texel covers a lot
  of world. Acne is a too-small `shadow_bias`.
- **"Hybrid shadows are black."** They should be a 0.35 multiplier; a black
  shadow means the raster shadow map produced it, so `RenderSettings::mode` is
  probably not `Hybrid`.
- **"Nothing is visible in ray-traced mode."** `ray_budget` ran out before the
  pixel loop reached that part of the screen (it walks in row order and stops).
  Raise the budget, or lower `samples_per_pixel`.
- **"A ray-traced frame is washed out or clipped."** `App::resolve` derives the
  tone curve from the mode the frame was rendered with, so this is usually
  `RenderSettings::exposure` being wrong for the scene rather than the curve
  being missing. A frame rendered with `ShadingMode::Raytrace` resolves with
  `ToneMap::Aces`; switching the mode at runtime is enough, no extra
  configuration is needed.
- **"Bloom is missing."** `bloom_threshold` and `bloom_intensity` both default to
  0, and a threshold of 0 blooms the whole image rather than the bright parts.
- **"`Framebuffer::new` panicked."** A zero-sized target is always a bug (usually
  a window minimised to 0x0); the constructor asserts rather than silently
  producing an empty frame.
