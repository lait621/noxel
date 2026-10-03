//! Materials: how a surface responds to light.
//!
//! A deliberately small, flat model. Pixel-art rendering does not want a full
//! PBR stack: the art is hand-shaded, and the renderer's job is to multiply a
//! hand-picked colour by a light level and keep the result on-palette. See
//! `docs/adr/0005-color-management.md`.

use noxel_asset::texture::Texture;
use noxel_core::math::{Color, Vec2};
use noxel_core::pool::Handle;

/// A handle to a material in a [`Scene`](crate::Scene).
pub type MaterialHandle = Handle<Material>;

/// A handle to a texture in a [`Scene`](crate::Scene).
pub type TextureHandle = Handle<Texture>;

/// A handle to a mesh in a [`Scene`](crate::Scene).
pub type MeshHandle = Handle<crate::mesh::Mesh>;

/// How transparency is handled for a surface.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum AlphaMode {
    /// Fully opaque; written to the depth buffer.
    #[default]
    Opaque,
    /// Alpha-tested against a threshold; still depth-written, so it sorts with
    /// opaque geometry. The right choice for foliage and fences cut out of a
    /// texture.
    Cutout {
        /// Fragments with alpha below this are discarded.
        threshold: f32,
    },
    /// Alpha-blended; depth-tested but not depth-written, and drawn after all
    /// opaque geometry, sorted back to front.
    Blend,
    /// Added to the framebuffer. Glows, torches, magical effects.
    Additive,
}

impl AlphaMode {
    /// True when the surface must be drawn in the transparent pass.
    #[inline]
    #[must_use]
    pub fn is_transparent(&self) -> bool {
        matches!(self, Self::Blend | Self::Additive)
    }

    /// True when the surface writes depth.
    #[inline]
    #[must_use]
    pub fn writes_depth(&self) -> bool {
        !matches!(self, Self::Blend | Self::Additive)
    }

    /// Generates a cutout mode with the given threshold.
    #[must_use]
    pub fn cutout(threshold: f32) -> Self {
        Self::Cutout {
            threshold: threshold.clamp(0.0, 1.0),
        }
    }
}

/// A surface appearance.
#[derive(Clone, Debug)]
pub struct Material {
    /// Name, for debugging and for `Scene::material_by_name`.
    pub name: String,
    /// Base colour, linear. Multiplied by the texture (if any) and by lighting.
    pub base_color: Color,
    /// Optional albedo texture.
    pub texture: Option<TextureHandle>,
    /// Emitted light, linear. Added after lighting, so it survives darkness.
    pub emissive: Color,
    /// How rough the surface is, `0` (mirror) to `1` (matte). Drives the
    /// ray tracer's reflection blur and the rasterizer's specular strength.
    pub roughness: f32,
    /// How metallic the surface is, `0` to `1`.
    pub metallic: f32,
    /// Specular highlight strength for the rasterizer.
    pub specular: f32,
    /// Transparency handling.
    pub alpha_mode: AlphaMode,
    /// When true the surface ignores lighting entirely and shows `base_color`
    /// and the texture as authored.
    ///
    /// This is the mode to use for pixel-art sprites whose shading is painted
    /// in: 90% of a top-down pixel game's materials are unlit.
    pub unlit: bool,
    /// When true the surface receives shadows.
    pub receive_shadow: bool,
    /// When true the surface casts shadows.
    pub cast_shadow: bool,
    /// When true back faces are shaded instead of culled.
    pub double_sided: bool,
    /// UV tiling applied before sampling.
    pub uv_scale: Vec2,
    /// UV offset applied before sampling.
    pub uv_offset: Vec2,
    /// Index of the palette the renderer should snap this material to, if any.
    /// `None` means "do not snap".
    pub palette_snap: Option<u32>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: String::from("material"),
            base_color: Color::WHITE,
            texture: None,
            emissive: Color::BLACK,
            roughness: 0.9,
            metallic: 0.0,
            specular: 0.0,
            alpha_mode: AlphaMode::Opaque,
            // Default to unlit: this engine renders hand-authored pixel art, and
            // surprising an artist with lighting they did not ask for is worse
            // than lighting that has to be switched on deliberately.
            unlit: true,
            receive_shadow: true,
            cast_shadow: true,
            double_sided: false,
            uv_scale: Vec2::ONE,
            uv_offset: Vec2::ZERO,
            palette_snap: None,
        }
    }
}

impl Material {
    /// A flat unlit colour. The single most common material in a pixel game.
    #[must_use]
    pub fn unlit(name: impl Into<String>, color: Color) -> Self {
        Self {
            name: name.into(),
            base_color: color,
            unlit: true,
            ..Self::default()
        }
    }

    /// A textured unlit surface (a sprite or a ground tile).
    #[must_use]
    pub fn sprite(name: impl Into<String>, texture: TextureHandle) -> Self {
        Self {
            name: name.into(),
            texture: Some(texture),
            unlit: true,
            ..Self::default()
        }
    }

    /// A lit matte surface (terrain, masonry).
    #[must_use]
    pub fn lit(name: impl Into<String>, color: Color) -> Self {
        Self {
            name: name.into(),
            base_color: color,
            unlit: false,
            roughness: 0.95,
            ..Self::default()
        }
    }

    /// A cut-out surface that still writes depth: foliage, fences, grates.
    #[must_use]
    pub fn foliage(name: impl Into<String>, texture: TextureHandle, threshold: f32) -> Self {
        Self {
            name: name.into(),
            texture: Some(texture),
            alpha_mode: AlphaMode::cutout(threshold),
            unlit: true,
            double_sided: true,
            ..Self::default()
        }
    }

    /// A translucent surface: glass, water, a faded roof.
    #[must_use]
    pub fn transparent(name: impl Into<String>, color: Color) -> Self {
        Self {
            name: name.into(),
            base_color: color,
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..Self::default()
        }
    }

    /// An emissive surface: a lamp, a lava crack, a glowing rune.
    #[must_use]
    pub fn emissive(name: impl Into<String>, color: Color, strength: f32) -> Self {
        Self {
            name: name.into(),
            base_color: Color::BLACK,
            emissive: color.tint(strength),
            unlit: true,
            cast_shadow: false,
            ..Self::default()
        }
    }

    /// Enables UV tiling.
    #[must_use]
    pub fn with_uv_scale(mut self, scale: Vec2) -> Self {
        self.uv_scale = scale;
        self
    }

    /// Sets the alpha mode.
    #[must_use]
    pub fn with_alpha_mode(mut self, mode: AlphaMode) -> Self {
        self.alpha_mode = mode;
        self
    }

    /// Forces lighting on or off.
    #[must_use]
    pub fn with_lit(mut self, lit: bool) -> Self {
        self.unlit = !lit;
        self
    }

    /// Disables shadow casting for this surface.
    #[must_use]
    pub fn without_shadow_casting(mut self) -> Self {
        self.cast_shadow = false;
        self
    }

    /// Applies a UV transform to a coordinate.
    #[inline]
    #[must_use]
    pub fn transform_uv(&self, uv: Vec2) -> Vec2 {
        Vec2::new(
            uv.x * self.uv_scale.x + self.uv_offset.x,
            uv.y * self.uv_scale.y + self.uv_offset.y,
        )
    }

    /// A stable 32-bit key that groups materials needing the same pipeline
    /// state, used by the renderer to sort draws cheaply.
    #[must_use]
    pub fn sort_key(&self) -> u32 {
        let mut key = 0u32;
        if self.unlit {
            key |= 1;
        }
        if self.double_sided {
            key |= 2;
        }
        match self.alpha_mode {
            AlphaMode::Opaque => {}
            AlphaMode::Cutout { .. } => key |= 1 << 2,
            AlphaMode::Blend => key |= 2 << 2,
            AlphaMode::Additive => key |= 3 << 2,
        }
        if self.texture.is_some() {
            key |= 1 << 4;
        }
        key
    }
}

/// A named set of materials, so the world generator can look one up by name.
#[derive(Clone, Debug, Default)]
pub struct MaterialLibrary {
    entries: Vec<(String, MaterialHandle)>,
}

impl MaterialLibrary {
    /// An empty library.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Registers a material.
    pub fn insert(&mut self, name: impl Into<String>, handle: MaterialHandle) {
        let name = name.into();
        if let Some(slot) = self.entries.iter_mut().find(|(n, _)| *n == name) {
            slot.1 = handle;
        } else {
            self.entries.push((name, handle));
        }
    }

    /// Looks up a material by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<MaterialHandle> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| *h)
    }

    /// Number of registered names.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(n, _)| n.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_material_is_unlit_white() {
        let m = Material::default();
        assert!(m.unlit);
        assert_eq!(m.base_color, Color::WHITE);
        assert_eq!(m.alpha_mode, AlphaMode::Opaque);
        assert!(m.cast_shadow && m.receive_shadow);
        assert!(!m.double_sided);
    }

    #[test]
    fn alpha_modes_classify() {
        assert!(!AlphaMode::Opaque.is_transparent());
        assert!(AlphaMode::Opaque.writes_depth());
        assert!(!AlphaMode::Blend.is_transparent() == false);
        assert!(!AlphaMode::Blend.writes_depth());
        assert!(!AlphaMode::Additive.writes_depth());
        assert!(AlphaMode::Cutout { threshold: 0.5 }.writes_depth());
        assert!(!AlphaMode::Cutout { threshold: 0.5 }.is_transparent());
    }

    #[test]
    fn cutout_clamps_the_threshold() {
        assert_eq!(AlphaMode::cutout(2.0), AlphaMode::Cutout { threshold: 1.0 });
        assert_eq!(
            AlphaMode::cutout(-1.0),
            AlphaMode::Cutout { threshold: 0.0 }
        );
    }

    #[test]
    fn uv_transform() {
        let m = Material::default().with_uv_scale(Vec2::new(2.0, 3.0));
        assert_eq!(m.transform_uv(Vec2::new(0.5, 0.5)), Vec2::new(1.0, 1.5));
    }

    #[test]
    fn sort_key_groups_state() {
        let a = Material::sprite("a", Handle::INVALID);
        let b = Material::sprite("b", Handle::INVALID);
        assert_eq!(a.sort_key(), b.sort_key());
        let c = Material::default();
        assert_ne!(a.sort_key(), c.sort_key(), "unlit vs lit must differ");
    }

    #[test]
    fn builders_set_the_expected_flags() {
        assert!(Material::lit("m", Color::WHITE).unlit == false);
        assert!(Material::unlit("m", Color::WHITE).unlit);
        assert!(
            Material::transparent("m", Color::WHITE)
                .alpha_mode
                .is_transparent()
        );
        assert!(!Material::emissive("m", Color::WHITE, 2.0).cast_shadow);
        assert!(Material::foliage("m", Handle::INVALID, 0.4).double_sided);
    }

    #[test]
    fn library_roundtrip() {
        let mut lib = MaterialLibrary::new();
        assert!(lib.is_empty());
        let h = Handle::INVALID;
        lib.insert("grass", h);
        assert_eq!(lib.get("grass"), Some(h));
        assert_eq!(lib.get("nope"), None);
        assert_eq!(lib.len(), 1);
        lib.insert("grass", Handle::INVALID);
        assert_eq!(lib.len(), 1, "re-inserting a name must not duplicate it");
    }
}
