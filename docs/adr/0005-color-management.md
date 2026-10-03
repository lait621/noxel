# ADR 0005 — Linear HDR working space, one sRGB resolve per frame

**Status:** accepted
**Applies to:** `noxel-render`, `noxel-asset`

## Context

Pixel art is authored in sRGB bytes. Lighting, blending and tone mapping are
correct in linear light. Doing arithmetic on sRGB values makes two 50%
transparencies composite to 75% instead of 50%, makes a light's falloff visibly
wrong, and makes bloom glow the wrong colour.

But converting back and forth per pixel, per operation, is both slow and
lossy — and lossy is fatal here, because the whole art direction depends on the
authored bytes surviving to the screen.

## Decision

**The framebuffer stores linear `f32` radiance. sRGB conversion happens exactly
once per frame, in `Framebuffer::resolve`, with a snap to the authored palette.**

```
author -> Color8 (sRGB bytes)
       -> Color::to_linear()          (once, at load)
       -> linear f32 arithmetic       (all shading, blending, lighting)
       -> tone map + exposure         (once, at resolve)
       -> linear_to_srgb()            (once, at resolve)
       -> Color8 (sRGB bytes)         (on screen)
```

The round trip is **exact**: `Color8 -> linear -> sRGB -> Color8` returns the same
bytes for every representable value. Three tests guarantee it:

- `unlit_surface_round_trips_its_colour` in the ray tracer and
  `unlit_sprite_round_trips_to_the_authored_palette` in the rasterizer.
- `palette_round_trips_exactly`, which walks 32 palette entries through the whole
  pipeline.
- `text_is_exactly_the_requested_colour`.

`Framebuffer::resolve` offers three tone curves — `None` (the raster default, so
the round trip is exact), `Reinhard` and `Aces` — plus exposure, dithering and
optional palette snapping for an authentic limited-palette look.

## Why HDR

An emissive material (a lamp, a spell effect) has a radiance above 1.0. Clamping
to 1.0 at the point of shading would make bloom impossible and would flatten
every highlight. Keeping `f32` radiance all the way to the resolve means the
bloom threshold has something to threshold.

## Consequences

- The framebuffer is 12 bytes per pixel for colour alone (`f32 * 3`). At 960x540
  that is 6 MB, which is fine; a 4K buffer would be 100 MB, which is why the
  engine targets an internal resolution and upscales.
- Dithering is applied in *linear* space before the sRGB conversion, so it must
  be small or it shifts the mean. The test asserts the mean moves by at most 4 of
  255.
- The ray-traced mode defaults to `ToneMap::Aces` because it produces genuine HDR
  radiance; switching it to `None` makes bright emissives clip.

## Alternatives rejected

- **sRGB framebuffer with per-operation conversion**: correct but slow, and
  repeated conversion is lossy in exactly the highlights the art cares about.
- **Linear `u8` framebuffer**: not enough precision; a linear 8-bit encoding
  bands visibly in the shadows.
- **No tone mapping at all**: fine for the raster path, but the ray tracer's
  reflections and emissives need a curve.
