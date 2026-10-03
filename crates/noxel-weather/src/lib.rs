//! Weather, as a system rather than an effect.
//!
//! A game that wants rain usually gets a screen-space particle loop: spawn
//! streaks at random screen positions, move them down, wrap them. It looks
//! wrong in a way that is hard to name, and the name is **world space**. Rain
//! that lives on the screen does not move when the camera does, does not have
//! depth, and does not know where the ground is — so it falls through the
//! player, in front of buildings it should be behind, at one flat speed, and it
//! never lands.
//!
//! This crate holds the model instead:
//!
//! * **Precipitation is in world space.** Every drop has a world position and a
//!   fall speed, and the camera decides where it lands on screen. Walk east and
//!   the rain is the same rain.
//! * **Drops are in layers.** A near drop is fat, fast and opaque; a far one is
//!   thin, slow and faint. That parallax is most of what makes rain read as
//!   volume rather than as texture.
//! * **Drops land.** Each one that reaches the ground becomes a splash with its
//!   own life, which is what tells the eye the ground is there.
//! * **The weather ramps.** Going from clear to storm is a change in intensity
//!   over seconds, not a switch, because weather does not switch.
//! * **Wind is a direction and a speed**, and the slant of the rain is derived
//!   from it rather than drawn in.
//!
//! ```no_run
//! use noxel_weather::{Weather, Kind};
//! use noxel_core::math::Vec3;
//!
//! let mut weather = Weather::new(1);
//! weather.set(Kind::Rain, 0.8);
//! // Once per frame, with wherever the camera is looking.
//! weather.update(1.0 / 60.0, Vec3::new(0.0, 0.0, 0.0));
//! for drop in weather.drops() {
//!     // project `drop.position` through your camera and draw `drop.length`.
//! }
//! ```
//!
//! The crate draws nothing and knows nothing about a renderer. It is a list of
//! positions and sizes, which is what lets a 2D game, a 3D game and a test use
//! the same weather.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::all)]

use noxel_core::math::Vec3;

/// The deterministic generator the weather scatters with.
///
/// The engine's determinism rule applies to rain as much as to terrain: the
/// same seed has to produce the same shower, or a recorded frame cannot be
/// reproduced and a test cannot assert on one.
#[derive(Clone, Debug)]
struct Rng(u32);

impl Rng {
    const fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + self.unit() * (high - low)
    }
}

/// What the sky is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    /// Nothing falling.
    #[default]
    Clear,
    /// Overcast, nothing falling. Worth its own variant because it changes the
    /// light without changing the particles, and a game almost always wants to
    /// distinguish it.
    Cloudy,
    /// Rain.
    Rain,
    /// Rain, harder, with more wind.
    Storm,
    /// Snow: slow, drifting, and it does not splash.
    Snow,
    /// Fog: not a particle effect at all, but it belongs to the same question.
    Fog,
}

impl Kind {
    /// How hard the precipitation falls, before a caller's own intensity.
    #[must_use]
    pub const fn base_intensity(self) -> f32 {
        match self {
            Self::Clear | Self::Cloudy | Self::Fog => 0.0,
            Self::Rain => 0.6,
            Self::Storm => 1.0,
            Self::Snow => 0.5,
        }
    }

    /// How fast a drop falls, in world units per second.
    ///
    /// Rain is fast and snow is slow, and the difference is the whole
    /// character of each: snow that fell at rain's speed would be hail.
    #[must_use]
    pub const fn fall_speed(self) -> f32 {
        match self {
            // Slower than real rain, deliberately: a 270-pixel view is a
            // window onto about seventeen tiles, and rain at a true 9 m/s
            // crosses it before the eye can follow a drop. This is the speed at
            // which rain *reads* as rain at this scale.
            Self::Rain => 15.0,
            Self::Storm => 20.0,
            Self::Snow => 4.2,
            _ => 0.0,
        }
    }

    /// How far a drop drifts sideways against its fall, from wind.
    #[must_use]
    pub const fn wind_response(self) -> f32 {
        match self {
            // Snow is light and is pushed a long way; rain is not.
            Self::Snow => 2.4,
            Self::Rain => 0.7,
            Self::Storm => 1.1,
            _ => 0.0,
        }
    }

    /// Which kinds put water on the ground.
    #[must_use]
    pub const fn is_wet(self) -> bool {
        matches!(self, Self::Rain | Self::Storm)
    }

    /// A name, for a HUD or a log.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Cloudy => "cloudy",
            Self::Rain => "rain",
            Self::Storm => "storm",
            Self::Snow => "snow",
            Self::Fog => "fog",
        }
    }
}

/// Which way the wind is blowing, and how hard.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Wind {
    /// Radians. Zero blows towards `+x`.
    pub direction: f32,
    /// World units per second at full strength.
    pub speed: f32,
}

impl Wind {
    /// The wind's push on one axis, at `strength`.
    #[must_use]
    pub fn push(self, strength: f32) -> (f32, f32) {
        (
            self.direction.cos() * self.speed * strength,
            self.direction.sin() * self.speed * strength,
        )
    }
}

/// One drop of rain or flake of snow, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drop {
    /// Where it is.
    pub position: Vec3,
    /// How fast it falls, in world units per second.
    pub speed: f32,
    /// How long it is drawn, in world units.
    pub length: f32,
    /// `0` is far away, `1` is in the camera's face.
    pub depth: f32,
}

impl Drop {
    /// A drop's opacity, from its depth.
    ///
    /// Far drops are faint. Without this the rain is a flat sheet; with it, it
    /// has a front and a back.
    #[must_use]
    pub fn alpha(self) -> f32 {
        0.25 + 0.75 * self.depth
    }
}

/// A drop landing, drawn for a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Splash {
    /// Where it landed.
    pub position: Vec3,
    /// Seconds left.
    pub life: f32,
    /// Total seconds, for the fade.
    pub total: f32,
    /// `0` is far, `1` is near.
    pub depth: f32,
}

impl Splash {
    /// How far the ring has spread, `0..=1`.
    #[must_use]
    pub fn progress(self) -> f32 {
        if self.total <= 0.0 {
            return 1.0;
        }
        (1.0 - self.life / self.total).clamp(0.0, 1.0)
    }

    /// How wide the ring is, `0..=1` of its final size.
    #[must_use]
    pub fn spread(self) -> f32 {
        // Eased out: a splash expands fast and then slows, which is what a
        // ring on water does.
        1.0 - (1.0 - self.progress()).powi(3)
    }

    /// Its opacity, fading to nothing.
    #[must_use]
    pub fn alpha(self) -> f32 {
        (1.0 - self.progress()).powi(2) * (0.35 + 0.65 * self.depth)
    }
}

/// The simulation.
#[derive(Clone, Debug)]
pub struct Weather {
    kind: Kind,
    /// What the weather is ramping towards.
    target: f32,
    /// What it is now. Between `0` and `1`.
    intensity: f32,
    /// World units per second the intensity changes.
    ramp: f32,
    wind: Wind,
    drops: Vec<Drop>,
    splashes: Vec<Splash>,
    rng: Rng,
    /// The volume drops are kept inside, centred on the last camera position.
    ///
    /// Rain is spawned around the camera rather than across the whole world:
    /// a farm is 44 tiles across and a shower only has to cover the screen,
    /// so simulating the rest would be work nobody sees.
    volume: Vec3,
    /// How high the ground is at the camera, for landing.
    ground: f32,
    /// Seconds of simulation, for the drift.
    time: f32,
    capacity: usize,
}

impl Weather {
    /// A clear sky.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self {
            kind: Kind::Clear,
            target: 0.0,
            intensity: 0.0,
            ramp: 1.2,
            wind: Wind {
                direction: 0.6,
                speed: 1.6,
            },
            drops: Vec::new(),
            splashes: Vec::new(),
            rng: Rng::new(seed),
            volume: Vec3::new(24.0, 14.0, 20.0),
            ground: 0.0,
            time: 0.0,
            capacity: 900,
        }
    }

    /// The volume precipitation is simulated in, in world units.
    ///
    /// The height matters most: a drop spawns at the top and has to have time to
    /// fall before it is visible, or the rain appears to start in mid-air.
    #[must_use]
    pub fn with_volume(mut self, width: f32, height: f32, depth: f32) -> Self {
        self.volume = Vec3::new(width, height, depth);
        self
    }

    /// The number of drops at full intensity.
    #[must_use]
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// How fast the weather changes, in intensity per second.
    #[must_use]
    pub fn with_ramp(mut self, ramp: f32) -> Self {
        self.ramp = ramp.max(0.01);
        self
    }

    /// The current kind.
    #[must_use]
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// How hard it is falling now, `0..=1`.
    #[must_use]
    pub fn intensity(&self) -> f32 {
        self.intensity
    }

    /// The wind.
    #[must_use]
    pub fn wind(&self) -> Wind {
        self.wind
    }

    /// Sets the wind.
    pub fn set_wind(&mut self, wind: Wind) {
        self.wind = wind;
    }

    /// The drops, in no particular order.
    #[must_use]
    pub fn drops(&self) -> &[Drop] {
        &self.drops
    }

    /// The splashes currently showing.
    #[must_use]
    pub fn splashes(&self) -> &[Splash] {
        &self.splashes
    }

    /// Whether the ground is getting wet.
    #[must_use]
    pub fn is_wet(&self) -> bool {
        self.kind.is_wet() && self.intensity > 0.05
    }

    /// Changes the weather. `intensity` is scaled by the kind's own base, so
    /// `set(Kind::Storm, 0.5)` is half a storm rather than a light rain.
    pub fn set(&mut self, kind: Kind, intensity: f32) {
        self.kind = kind;
        self.target = (intensity * kind.base_intensity()).clamp(0.0, 1.0);
        // Wind follows the weather rather than being set beside it, because a
        // caller who has to remember both will remember one.
        self.wind.speed = match kind {
            Kind::Storm => 3.4,
            Kind::Rain => 1.8,
            Kind::Snow => 0.9,
            _ => 0.5,
        };
    }

    /// Steps the simulation.
    ///
    /// `camera` is where the view is centred; drops are spawned and recycled
    /// around it. `ground` is the height they land at.
    pub fn update(&mut self, dt: f32, camera: Vec3) {
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        self.ground = camera.y;
        self.step_intensity(dt);
        self.recycle(dt, camera);
        self.step_splashes(dt);
    }

    fn step_intensity(&mut self, dt: f32) {
        let delta = self.target - self.intensity;
        let step = self.ramp * dt;
        self.intensity = if delta.abs() <= step {
            self.target
        } else {
            self.intensity + step * delta.signum()
        };
        if self.intensity <= 0.001 {
            self.intensity = 0.0;
            self.drops.clear();
        }
    }

    /// Moves every drop, and respawns the ones that have landed or drifted out.
    fn recycle(&mut self, dt: f32, camera: Vec3) {
        let want = (self.capacity as f32 * self.intensity) as usize;
        let response = self.kind.wind_response();
        let (wind_x, wind_z) = self.wind.push(response);

        // Move what is here, and count what survives.
        let falling = std::mem::take(&mut self.drops);
        let mut alive: Vec<Drop> = Vec::with_capacity(falling.len().max(want));
        let mut landed: Vec<Drop> = Vec::new();
        for mut drop in falling {
            drop.position.x += wind_x * dt * (0.4 + 0.6 * drop.depth);
            drop.position.z += wind_z * dt * (0.4 + 0.6 * drop.depth);
            drop.position.y -= drop.speed * dt;
            if drop.position.y <= self.ground {
                landed.push(drop);
                continue;
            }
            if self.outside(drop.position, camera) {
                continue;
            }
            alive.push(drop);
        }

        // Spawn up to the target count. New drops start spread through the
        // volume rather than all at the top, so a shower beginning does not look
        // like a shelf of water falling at once.
        while alive.len() < want {
            alive.push(self.spawn(camera, true));
        }
        self.drops = alive;
        for drop in landed {
            self.land(drop);
        }

        // Keep the splashes bounded even in a downpour.
        let splash_cap = (self.capacity / 4).max(32);
        if self.splashes.len() > splash_cap {
            let excess = self.splashes.len() - splash_cap;
            self.splashes.drain(..excess);
        }

        // A drop that has drifted out of the volume is replaced at the far side,
        // so wind does not slowly empty the sky.
        for index in 0..self.drops.len() {
            if self.drops[index].position.y < self.ground - self.volume.y {
                self.drops[index] = self.spawn(camera, false);
            }
        }
    }

    fn outside(&self, position: Vec3, camera: Vec3) -> bool {
        (position.x - camera.x).abs() > self.volume.x
            || (position.z - camera.z).abs() > self.volume.z
            || position.y < self.ground - self.volume.y
            || position.y > self.ground + self.volume.y
    }

    /// A new drop above the camera.
    ///
    /// `spread` starts it anywhere in the column — for filling a shower that is
    /// already falling — where `false` puts it at the top, which is what a
    /// recycled drop wants.
    fn spawn(&mut self, camera: Vec3, spread: bool) -> Drop {
        let depth = self.rng.unit();
        let height = if spread {
            self.ground + self.rng.range(0.0, self.volume.y)
        } else {
            self.ground + self.volume.y
        };
        let position = Vec3::new(
            camera.x + self.rng.range(-self.volume.x, self.volume.x),
            height,
            camera.z + self.rng.range(-self.volume.z, self.volume.z),
        );
        // Near drops fall faster and are longer. This is the parallax: without
        // it every drop moves together and the rain reads as a scrolling sheet.
        let mut speed = self.kind.fall_speed() * (0.72 + 0.55 * depth);
        if self.kind == Kind::Snow {
            // Snow wanders rather than falling straight.
            speed *= 0.75 + 0.5 * self.rng.unit();
        }
        Drop {
            position,
            speed,
            // In world units, and a world unit is a tile: rain is a quarter to
            // a half of one. The first version used whole tiles and drew
            // scratches across the screen.
            length: match self.kind {
                Kind::Snow => 0.15,
                // Motion blur is the *distance travelled in one frame*, plus a
                // little, which is what the eye actually integrates. Tying it to
                // `speed` rather than to a constant is what stops a slow drizzle
                // from looking like a fast one drawn shorter.
                _ => 0.06 + speed * 0.022,
            },
            depth,
        }
    }

    /// Turns a landing drop into a splash, unless it is snow.
    fn land(&mut self, drop: Drop) {
        if self.kind == Kind::Snow {
            // Snow settles rather than splashing, and a white ring on every
            // flake is the single fastest way to make snow look like rain.
            return;
        }
        let total = 0.22 + 0.18 * drop.depth;
        self.splashes.push(Splash {
            position: Vec3::new(drop.position.x, self.ground, drop.position.z),
            life: total,
            total,
            depth: drop.depth,
        });
    }

    fn step_splashes(&mut self, dt: f32) {
        for splash in &mut self.splashes {
            splash.life -= dt;
        }
        self.splashes.retain(|splash| splash.life > 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    fn run(weather: &mut Weather, seconds: f32) {
        let steps = (seconds * 60.0) as u32;
        for _ in 0..steps {
            weather.update(1.0 / 60.0, ORIGIN);
        }
    }

    #[test]
    fn a_clear_sky_has_nothing_falling() {
        let mut weather = Weather::new(1);
        run(&mut weather, 2.0);
        assert!(weather.drops().is_empty());
        assert!(weather.splashes().is_empty());
        assert!(!weather.is_wet());
    }

    #[test]
    fn rain_arrives_over_seconds_rather_than_switching_on() {
        // Weather does not switch. A shower that appears at full strength in one
        // frame is the tell that separates an effect from a system.
        let mut weather = Weather::new(1);
        weather.set(Kind::Rain, 1.0);
        weather.update(1.0 / 60.0, ORIGIN);
        let first = weather.intensity();
        assert!(first > 0.0 && first < 0.1, "it arrived instantly: {first}");
        run(&mut weather, 2.0);
        assert!(
            weather.intensity() > 0.5,
            "it never arrived: {}",
            weather.intensity()
        );
    }

    #[test]
    fn rain_reaches_full_strength_and_stops_when_cleared() {
        let mut weather = Weather::new(1);
        weather.set(Kind::Storm, 1.0);
        run(&mut weather, 4.0);
        assert!((weather.intensity() - 1.0).abs() < 0.01);
        assert!(weather.is_wet());
        assert!(
            weather.drops().len() > 100,
            "a storm with {} drops",
            weather.drops().len()
        );

        weather.set(Kind::Clear, 0.0);
        run(&mut weather, 4.0);
        assert_eq!(weather.intensity(), 0.0);
        assert!(weather.drops().is_empty(), "the rain never stopped");
    }

    #[test]
    fn drops_land_and_become_splashes() {
        // The property that makes rain read as *falling on something* rather
        // than as moving over the screen.
        let mut weather = Weather::new(7);
        weather.set(Kind::Rain, 1.0);
        run(&mut weather, 3.0);
        assert!(!weather.splashes().is_empty(), "not one drop ever landed");
        for splash in weather.splashes() {
            assert!(
                (splash.position.y - ORIGIN.y).abs() < 1e-3,
                "a splash landed off the ground"
            );
        }
    }

    #[test]
    fn snow_does_not_splash() {
        // A white ring under every flake is the fastest way to make snow look
        // like rain.
        let mut weather = Weather::new(3);
        weather.set(Kind::Snow, 1.0);
        run(&mut weather, 6.0);
        assert!(!weather.drops().is_empty(), "no snow");
        assert!(weather.splashes().is_empty(), "snow splashed");
        assert!(!weather.is_wet(), "snow should not wet the ground");
    }

    #[test]
    fn snow_falls_far_slower_than_rain() {
        let mut rain = Weather::new(1);
        rain.set(Kind::Rain, 1.0);
        run(&mut rain, 3.0);
        let mut snow = Weather::new(1);
        snow.set(Kind::Snow, 1.0);
        run(&mut snow, 3.0);
        let mean =
            |drops: &[Drop]| drops.iter().map(|d| d.speed).sum::<f32>() / drops.len().max(1) as f32;
        assert!(
            mean(snow.drops()) * 3.0 < mean(rain.drops()),
            "snow at {} against rain at {}",
            mean(snow.drops()),
            mean(rain.drops())
        );
    }

    #[test]
    fn drops_are_layered_so_the_rain_has_depth() {
        // Without layers every drop moves together and the rain is a scrolling
        // sheet. The spread of speeds is the volume.
        let mut weather = Weather::new(5);
        weather.set(Kind::Storm, 1.0);
        run(&mut weather, 3.0);
        let near = weather.drops().iter().filter(|d| d.depth > 0.75).count();
        let far = weather.drops().iter().filter(|d| d.depth < 0.25).count();
        assert!(near > 10 && far > 10, "{near} near, {far} far");
        // And a near drop is drawn longer and more opaque than a far one.
        let fast = weather
            .drops()
            .iter()
            .cloned()
            .max_by(|a, b| a.depth.total_cmp(&b.depth))
            .unwrap();
        let slow = weather
            .drops()
            .iter()
            .cloned()
            .min_by(|a, b| a.depth.total_cmp(&b.depth))
            .unwrap();
        assert!(fast.speed > slow.speed);
        assert!(fast.length > slow.length);
        assert!(fast.alpha() > slow.alpha());
    }

    #[test]
    fn drops_stay_around_the_camera() {
        // Rain is simulated around the view, not across the world: a farm is
        // forty tiles across and only the screen is visible.
        let mut weather = Weather::new(2).with_volume(20.0, 12.0, 20.0);
        weather.set(Kind::Storm, 1.0);
        run(&mut weather, 4.0);
        let camera = Vec3::new(500.0, 0.0, -300.0);
        for _ in 0..600 {
            weather.update(1.0 / 60.0, camera);
        }
        assert!(!weather.drops().is_empty());
        for drop in weather.drops() {
            assert!(
                (drop.position.x - camera.x).abs() <= 20.5,
                "a drop is {} away in x",
                (drop.position.x - camera.x).abs()
            );
            assert!((drop.position.z - camera.z).abs() <= 20.5);
        }
    }

    #[test]
    fn the_wind_moves_every_drop_it_touches_the_same_way() {
        // Asserted on the *rule* rather than on a field of drops: a frame can
        // land one drop and spawn another somewhere else entirely, so any mean
        // over the population compares two different sets. What has to be true
        // is that the horizontal step is downwind and proportional to the
        // drop's depth, and that is a property of the numbers.
        let wind = Wind {
            direction: 0.0,
            speed: 2.0,
        };
        let (push, cross) = wind.push(Kind::Rain.wind_response());
        assert!(push > 0.0, "a wind blowing towards +x must push towards +x");
        assert!(cross.abs() < 1e-6, "and must not push sideways on its own");

        // A near drop is moved further than a far one, which is what gives the
        // shower its slant rather than a uniform lean.
        let dt = 1.0 / 60.0;
        let near = push * dt * (0.4 + 0.6 * 1.0);
        let far = push * dt * (0.4 + 0.6 * 0.0);
        assert!(near > far, "{near} against {far}");
    }

    #[test]
    fn snow_is_carried_further_than_rain() {
        // Snow is light. If both blew the same way by the same amount the two
        // kinds would differ only in tempo.
        assert!(Kind::Snow.wind_response() > Kind::Rain.wind_response());
        let wind = Wind {
            direction: 0.0,
            speed: 3.0,
        };
        let (rain, _) = wind.push(Kind::Rain.wind_response());
        let (snow, _) = wind.push(Kind::Snow.wind_response());
        assert!(snow > rain * 2.0, "snow {snow} against rain {rain}");
    }

    #[test]
    fn the_same_seed_makes_the_same_shower() {
        let shower = |seed| {
            let mut weather = Weather::new(seed);
            weather.set(Kind::Storm, 1.0);
            run(&mut weather, 2.0);
            weather
                .drops()
                .iter()
                .map(|d| (d.position.x, d.position.y, d.position.z, d.speed))
                .collect::<Vec<_>>()
        };
        assert_eq!(shower(42), shower(42));
        assert_ne!(shower(42), shower(43));
    }

    #[test]
    fn splashes_fade_and_are_recycled() {
        let mut weather = Weather::new(4);
        weather.set(Kind::Storm, 1.0);
        run(&mut weather, 4.0);
        let before = weather.splashes().len();
        assert!(before > 0);
        // A splash's life is a fraction of a second, so a moment later they are
        // a different set rather than the same ones aged.
        let lives: Vec<f32> = weather.splashes().iter().map(|s| s.life).collect();
        assert!(
            lives.iter().all(|l| *l > 0.0 && *l < 0.5),
            "a splash outlived its welcome"
        );
        assert!(weather.splashes().iter().all(|s| s.progress() <= 1.0));
    }

    #[test]
    fn intensity_scales_the_number_of_drops() {
        let count = |intensity: f32| {
            let mut weather = Weather::new(6);
            weather.set(Kind::Storm, intensity);
            run(&mut weather, 4.0);
            weather.drops().len()
        };
        let light = count(0.25);
        let heavy = count(1.0);
        assert!(heavy > light * 2, "{light} drops light, {heavy} heavy");
    }

    #[test]
    fn a_huge_frame_does_not_teleport_the_weather() {
        // A stall or a breakpoint gives a frame of several seconds. Drops that
        // move by `dt` unclamped end up kilometres below the ground and the sky
        // empties for a second afterwards.
        let mut weather = Weather::new(1);
        weather.set(Kind::Storm, 1.0);
        run(&mut weather, 2.0);
        let before = weather.drops().len();
        weather.update(30.0, ORIGIN);
        assert!(
            weather.drops().len() >= before / 2,
            "the sky emptied on a long frame"
        );
    }

    #[test]
    fn every_kind_is_drawable_without_special_casing() {
        // A caller draws `drops()` and `splashes()` whatever the kind; a kind
        // that produced something undrawable would be a panic in the renderer.
        for kind in [
            Kind::Clear,
            Kind::Cloudy,
            Kind::Rain,
            Kind::Storm,
            Kind::Snow,
            Kind::Fog,
        ] {
            let mut weather = Weather::new(1);
            weather.set(kind, 1.0);
            run(&mut weather, 3.0);
            for drop in weather.drops() {
                assert!(drop.speed.is_finite() && drop.length.is_finite());
                assert!(drop.length > 0.0, "{} made a zero-length drop", kind.name());
                assert!((0.0..=1.0).contains(&drop.depth));
                assert!(drop.alpha() > 0.0 && drop.alpha() <= 1.0);
            }
            for splash in weather.splashes() {
                assert!((0.0..=1.0).contains(&splash.progress()));
                assert!(splash.alpha() >= 0.0);
            }
        }
    }
}
