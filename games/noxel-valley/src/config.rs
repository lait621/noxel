//! Every number and table the game is tuned by.
//!
//! Kept in one module because a farming game is mostly *data*: what a parsnip
//! costs, how long it takes, which season it grows in. Scattering those through
//! the systems that read them makes "make pumpkins more profitable" a
//! multi-file change and makes the economy impossible to reason about as a
//! whole.
//!
//! The tables are `const` rather than loaded from JSON because they are design
//! decisions, not content: a change to a price is a change to the game, it
//! should show up in review as one, and it should not be possible to ship a
//! build where the parsnip seed points at a crop that does not exist. A test at
//! the bottom of this file checks exactly that.

use noxel_core::math::Color8;

// ---------------------------------------------------------------------------
// Presentation
// ---------------------------------------------------------------------------

/// Internal render resolution.
///
/// 480x270 is 16:9 and exactly one quarter of 1920x1080, so the window's
/// whole-number upscale is 4x on a 1080p display and every authored pixel lands
/// on a 2x2 or 1x1 block of the panel. 320x180 would also be exact but shows
/// only 20 tiles across, which is too tight to see a farm.
pub const INTERNAL_WIDTH: u32 = 480;
/// Internal render height.
pub const INTERNAL_HEIGHT: u32 = 270;

/// One map tile, in screen pixels and in world units.
///
/// Tiles are square *in screen space*: the camera looks straight down an
/// orthographic axis, so one world unit is exactly [`TILE`] pixels in both
/// directions and a tile is never resampled. That is the whole reason the art
/// stays crisp.
pub const TILE: u32 = 16;

/// The orthographic view height that makes one world unit exactly [`TILE`] pixels.
pub const ORTHO_HEIGHT: f32 = INTERNAL_HEIGHT as f32 / TILE as f32;

/// The farm map's size in tiles.
pub const FARM_WIDTH: u32 = 44;
/// The farm map's height in tiles.
pub const FARM_HEIGHT: u32 = 34;

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// The hour the day starts.
pub const DAY_START_HOUR: f32 = 6.0;
/// The hour the day ends and the player is forced to bed.
pub const DAY_END_HOUR: f32 = 26.0;

/// In-game minutes advanced per real second.
///
/// A whole day is 1200 in-game minutes, so this is a ten-minute day. Faster and
/// the player cannot cross the farm before dusk; slower and testing the threeseason
/// economy takes an evening.
pub const GAME_MINUTES_PER_SECOND: f32 = 2.0;

/// Days in one season.
pub const DAYS_PER_SEASON: u32 = 28;
/// Seasons in one year.
pub const SEASONS_PER_YEAR: u32 = 4;

/// The four seasons, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Season {
    /// Planting weather, and the season the game starts in.
    #[default]
    Spring,
    /// The hot season.
    Summer,
    /// Harvest season.
    Fall,
    /// Nothing grows; the farm is snowed under.
    Winter,
}

impl Season {
    /// Every season, in calendar order.
    pub const ALL: [Self; 4] = [Self::Spring, Self::Summer, Self::Fall, Self::Winter];

    /// The next season.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Spring => Self::Summer,
            Self::Summer => Self::Fall,
            Self::Fall => Self::Winter,
            Self::Winter => Self::Spring,
        }
    }

    /// A short name for the HUD.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Spring => "春",
            Self::Summer => "夏",
            Self::Fall => "秋",
            Self::Winter => "冬",
        }
    }

    /// The English name, for the log and for `--help`.
    #[must_use]
    pub const fn english(self) -> &'static str {
        match self {
            Self::Spring => "Spring",
            Self::Summer => "Summer",
            Self::Fall => "Fall",
            Self::Winter => "Winter",
        }
    }

    /// The season's tint on the grass, so the world reads as seasonal without a
    /// second terrain tileset.
    #[must_use]
    pub const fn tint(self) -> Color8 {
        match self {
            Self::Spring => Color8::new(255, 255, 255, 255),
            Self::Summer => Color8::new(255, 248, 228, 255),
            Self::Fall => Color8::new(255, 226, 186, 255),
            Self::Winter => Color8::new(206, 224, 255, 255),
        }
    }
}

// ---------------------------------------------------------------------------
// Weather
// ---------------------------------------------------------------------------

/// What the sky is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weather {
    /// Clear and bright.
    Sunny,
    /// Overcast, with no rain.
    Cloudy,
    /// Rain: waters every crop on the farm.
    Rain,
    /// Heavy rain, with lightning, and darker than plain rain.
    Storm,
    /// Snow. Winter's answer to rain; it does not water anything, because
    /// nothing grows.
    Snow,
}

impl Weather {
    /// A short Chinese name for the HUD.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sunny => "晴天",
            Self::Cloudy => "多云",
            Self::Rain => "雨天",
            Self::Storm => "雷雨",
            Self::Snow => "下雪",
        }
    }

    /// The English name.
    #[must_use]
    pub const fn english(self) -> &'static str {
        match self {
            Self::Sunny => "Sunny",
            Self::Cloudy => "Cloudy",
            Self::Rain => "Rain",
            Self::Storm => "Storm",
            Self::Snow => "Snow",
        }
    }

    /// Whether this weather waters the crops for the player.
    ///
    /// The single most important rule in a farming game: rain is what makes
    /// skipping a day cost nothing, and a player who does not know it will
    /// over-water.
    #[must_use]
    pub const fn waters_crops(self) -> bool {
        matches!(self, Self::Rain | Self::Storm)
    }

    /// How much the weather dims the world, as a linear multiplier.
    #[must_use]
    pub const fn light(self) -> f32 {
        match self {
            Self::Sunny => 1.0,
            Self::Cloudy => 0.86,
            Self::Rain => 0.72,
            Self::Storm => 0.6,
            Self::Snow => 0.9,
        }
    }

    /// The name of this weather's icon in the UI atlas.
    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Sunny => "icon_sun",
            Self::Cloudy => "icon_cloud",
            Self::Rain => "icon_rain",
            Self::Storm => "icon_storm",
            Self::Snow => "icon_snow",
        }
    }
}

// ---------------------------------------------------------------------------
// Crops
// ---------------------------------------------------------------------------

/// A crop the player can grow.
///
/// One const table rather than a struct per crop, so the whole economy is
/// readable at once and a balance change is a one-line diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Crop {
    /// The key used in asset names (`pixel_0` .. `pixel_4`, `item_pixel`).
    pub key: &'static str,
    /// The Chinese display name.
    pub name: &'static str,
    /// The English name.
    pub english: &'static str,
    /// What a seed costs at the shop.
    pub seed_price: u32,
    /// What one harvested unit sells for.
    pub sell_price: u32,
    /// Days from planting to first harvest, when watered every day.
    pub growth_days: u32,
    /// Days to regrow after a harvest, or `0` for a crop that must be replanted.
    pub regrow_days: u32,
    /// The season this crop grows in. Crops die when their season ends.
    pub season: Season,
    /// The colour of the crop's icon background, used by the UI before the art
    /// atlas is loaded.
    pub color: Color8,
}

/// Every crop, in the order the shop lists them.
pub const CROPS: [Crop; 6] = [
    Crop {
        key: "parsnip",
        name: "防风草",
        english: "Parsnip",
        seed_price: 20,
        sell_price: 35,
        growth_days: 4,
        regrow_days: 0,
        season: Season::Spring,
        color: Color8::new(230, 214, 168, 255),
    },
    Crop {
        key: "cauliflower",
        name: "花椰菜",
        english: "Cauliflower",
        seed_price: 80,
        sell_price: 175,
        growth_days: 12,
        regrow_days: 0,
        season: Season::Spring,
        color: Color8::new(238, 236, 214, 255),
    },
    Crop {
        key: "potato",
        name: "土豆",
        english: "Potato",
        seed_price: 50,
        sell_price: 80,
        growth_days: 6,
        regrow_days: 0,
        season: Season::Spring,
        color: Color8::new(198, 158, 108, 255),
    },
    Crop {
        key: "tomato",
        name: "番茄",
        english: "Tomato",
        seed_price: 50,
        sell_price: 60,
        growth_days: 11,
        regrow_days: 4,
        season: Season::Summer,
        color: Color8::new(214, 74, 58, 255),
    },
    Crop {
        key: "corn",
        name: "玉米",
        english: "Corn",
        seed_price: 150,
        sell_price: 70,
        growth_days: 14,
        regrow_days: 4,
        season: Season::Summer,
        color: Color8::new(240, 206, 96, 255),
    },
    Crop {
        key: "pumpkin",
        name: "南瓜",
        english: "Pumpkin",
        seed_price: 100,
        sell_price: 320,
        growth_days: 13,
        regrow_days: 0,
        season: Season::Fall,
        color: Color8::new(226, 130, 44, 255),
    },
];

/// How many growth stages a crop sprite has: `_0` (sprout) to `_4` (ripe).
pub const CROP_STAGES: u32 = 5;

/// Looks a crop up by its asset key.
#[must_use]
pub fn crop_by_key(key: &str) -> Option<&'static Crop> {
    CROPS.iter().find(|crop| crop.key == key)
}

/// The crops that can be planted in a season.
pub fn crops_in_season(season: Season) -> impl Iterator<Item = &'static Crop> {
    CROPS.iter().filter(move |crop| crop.season == season)
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// A tool in the player's hotbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    /// Nothing in hand.
    Hand,
    /// Turns grass and dirt into tilled soil.
    Hoe,
    /// Waters tilled soil and the crop in it.
    Can,
    /// Removes a plant, a weed or a small prop.
    Axe,
    /// Breaks rock.
    Pickaxe,
    /// Harvests a ripe crop without destroying it. The primary way to collect.
    Scythe,
}

impl Tool {
    /// The tools, in hotbar order.
    pub const ALL: [Self; 6] = [
        Self::Hoe,
        Self::Can,
        Self::Axe,
        Self::Pickaxe,
        Self::Scythe,
        Self::Hand,
    ];

    /// The Chinese name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Hand => "手",
            Self::Hoe => "锄头",
            Self::Can => "水壶",
            Self::Axe => "斧头",
            Self::Pickaxe => "镐子",
            Self::Scythe => "镰刀",
        }
    }

    /// The English name.
    #[must_use]
    pub const fn english(self) -> &'static str {
        match self {
            Self::Hand => "Hand",
            Self::Hoe => "Hoe",
            Self::Can => "Watering Can",
            Self::Axe => "Axe",
            Self::Pickaxe => "Pickaxe",
            Self::Scythe => "Scythe",
        }
    }

    /// The icon region in the UI atlas.
    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Hand => "icon_bag",
            Self::Hoe => "icon_hoe",
            Self::Can => "icon_can",
            Self::Axe => "icon_axe",
            Self::Pickaxe => "icon_pickaxe",
            Self::Scythe => "icon_scythe",
        }
    }

    /// Energy spent on one use.
    #[must_use]
    pub const fn energy_cost(self) -> f32 {
        match self {
            Self::Hand => 0.0,
            Self::Hoe | Self::Can => 2.0,
            Self::Axe | Self::Pickaxe | Self::Scythe => 3.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Player
// ---------------------------------------------------------------------------

/// Full energy at the start of a day.
pub const MAX_ENERGY: f32 = 100.0;
/// Energy spent per in-game minute of walking, so crossing the farm costs
/// something without making movement feel punished.
pub const ENERGY_PER_MINUTE: f32 = 0.03;
/// Walking speed in tiles per second.
pub const WALK_SPEED: f32 = 4.4;
/// Running speed in tiles per second.
pub const RUN_SPEED: f32 = 7.4;
/// How long a tool swing takes, in seconds.
pub const TOOL_SWING_SECONDS: f32 = 0.32;

// ---------------------------------------------------------------------------
// Economy
// ---------------------------------------------------------------------------

/// Money the player starts with.
pub const STARTING_GOLD: u32 = 500;
/// Inventory slots.
pub const INVENTORY_SLOTS: usize = 24;
/// Hotbar slots — the first `HOTBAR_SLOTS` inventory slots, in hotbar order.
pub const HOTBAR_SLOTS: usize = 6;
/// The largest stack of one item a single slot can hold.
pub const MAX_STACK: u32 = 99;

/// The weather forecast is this many days long, today included.
pub const FORECAST_DAYS: usize = 3;

// ---------------------------------------------------------------------------
// Debug / testing
// ---------------------------------------------------------------------------

/// `--fast` multiplies the clock by this much, so a tester can reach winter in
/// a minute.
pub const FAST_FORWARD_MULTIPLIER: f32 = 24.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_crop_has_a_unique_key_and_a_positive_price() {
        let mut keys: Vec<&str> = CROPS.iter().map(|c| c.key).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "two crops share an asset key");

        for crop in CROPS {
            assert!(crop.seed_price > 0, "{} is free to plant", crop.english);
            assert!(crop.sell_price > 0, "{} is worthless", crop.english);
            assert!(
                crop.growth_days > 0,
                "{} is ripe the instant it is planted",
                crop.english
            );
        }
    }

    #[test]
    fn no_crop_is_a_free_money_loop_within_one_day() {
        // A crop that pays more than it costs without waiting is a design bug:
        // the player would plant and harvest in a loop and the economy ends.
        for crop in CROPS {
            assert!(
                crop.growth_days >= 4,
                "{} ripens in {} days, which is too fast to be a decision",
                crop.english,
                crop.growth_days
            );
        }
    }

    /// How many times a crop is harvested in one season.
    ///
    /// A crop that must be replanted is harvested `days / growth` times, each
    /// one costing another seed; a regrowing crop is harvested once at maturity
    /// and then every `regrow_days`. The difference is the whole reason both
    /// kinds exist, and getting it wrong is how a balance test ends up
    /// flattering the cheap crop.
    fn harvests_per_season(crop: &Crop) -> u32 {
        if crop.regrow_days == 0 {
            (DAYS_PER_SEASON / crop.growth_days.max(1)).max(1)
        } else {
            // `regrow_days` is non-zero in this branch, which the caller's
            // invariant test also asserts.
            1 + DAYS_PER_SEASON.saturating_sub(crop.growth_days) / crop.regrow_days.max(1)
        }
    }

    /// Net gold from one tile over one season, seed costs included.
    fn season_profit(crop: &Crop) -> i64 {
        let harvests = i64::from(harvests_per_season(crop));
        let seeds = if crop.regrow_days == 0 { harvests } else { 1 };
        harvests * i64::from(crop.sell_price) - seeds * i64::from(crop.seed_price)
    }

    #[test]
    fn every_crop_pays_for_itself_within_one_season() {
        // The invariant that actually matters for playability: a crop whose
        // whole season cannot repay its seed is a trap. The player plants it,
        // waits, and is poorer — and nothing in the code would tell them.
        //
        // An earlier version asserted that a regrowing crop's *first* harvest is
        // worth less than its seed. That is a much stronger claim and a wrong
        // one: it would have banned the tomato, which pays back over its
        // regrows and is a perfectly reasonable crop.
        for crop in CROPS.iter() {
            let profit = season_profit(crop);
            assert!(
                profit > 0,
                "{} returns {profit} gold over a {} day season for a {} gold seed; planting it is a loss",
                crop.english,
                DAYS_PER_SEASON,
                crop.seed_price
            );
        }
    }

    #[test]
    fn no_crop_is_a_runaway_money_printer() {
        // The other end: a crop that returns many times what it cost makes every
        // other choice on the farm pointless. The bound is loose on purpose —
        // where exactly the fall money crop sits is a design decision — but a
        // crop that returns twenty times its seed is a bug, not a decision.
        for crop in CROPS.iter() {
            let profit = season_profit(crop);
            let invested = i64::from(crop.seed_price) * i64::from(harvests_per_season(crop).max(1));
            assert!(
                profit <= invested * 8,
                "{} returns {profit} on {invested} invested, which crowds out every other crop",
                crop.english
            );
        }
    }

    #[test]
    fn every_season_has_something_to_plant_except_winter() {
        for season in [Season::Spring, Season::Summer, Season::Fall] {
            assert!(
                crops_in_season(season).count() > 0,
                "nothing can be planted in {}",
                season.english()
            );
        }
        // Winter is deliberately fallow: it is the season the player spends
        // mining, foraging and reorganising, and it gives the year a shape.
        assert_eq!(crops_in_season(Season::Winter).count(), 0);
    }

    #[test]
    fn crop_lookup_finds_every_crop_and_rejects_nonsense() {
        for crop in CROPS {
            assert_eq!(crop_by_key(crop.key).map(|c| c.key), Some(crop.key));
        }
        assert!(crop_by_key("turnip").is_none());
    }

    #[test]
    fn the_season_cycle_returns_to_spring_after_four_steps() {
        let mut season = Season::Spring;
        for _ in 0..SEASONS_PER_YEAR {
            season = season.next();
        }
        assert_eq!(season, Season::Spring);
    }

    #[test]
    fn only_rain_and_storm_water_the_crops() {
        // Rain is the mechanic that makes skipping a day free; snow deliberately
        // is not, because nothing grows in winter anyway.
        assert!(Weather::Rain.waters_crops());
        assert!(Weather::Storm.waters_crops());
        assert!(!Weather::Sunny.waters_crops());
        assert!(!Weather::Cloudy.waters_crops());
        assert!(!Weather::Snow.waters_crops());
    }

    #[test]
    fn the_geometry_puts_one_tile_on_exactly_sixteen_pixels() {
        // This is the crispness guarantee. If it drifts, every sprite in the
        // game is resampled and the art goes soft.
        let pixels_per_unit = INTERNAL_HEIGHT as f32 / ORTHO_HEIGHT;
        assert!(
            (pixels_per_unit - TILE as f32).abs() < 1e-4,
            "one world unit renders as {pixels_per_unit} pixels, not {TILE}"
        );
    }

    #[test]
    fn the_internal_resolution_is_an_exact_multiple_of_1080p() {
        // 480x270 at 4x is exactly 1920x1080, so the window's whole-number
        // upscale has no letterbox waste on the most common display.
        assert_eq!(INTERNAL_WIDTH * 4, 1920);
        assert_eq!(INTERNAL_HEIGHT * 4, 1080);
    }

    #[test]
    fn the_day_has_more_than_eight_hours_of_work_in_it() {
        let hours = DAY_END_HOUR - DAY_START_HOUR;
        assert!(hours >= 16.0, "a {hours} hour day is too short to farm");
        let real_seconds = hours * 60.0 / GAME_MINUTES_PER_SECOND;
        assert!(
            (300.0..900.0).contains(&real_seconds),
            "a day lasts {real_seconds} real seconds, which is outside the intended 5-15 minutes"
        );
    }

    #[test]
    fn the_farm_is_bigger_than_one_screen() {
        // A farm that fits on one screen has no reason for a camera.
        let screen_tiles_x = INTERNAL_WIDTH / TILE;
        let screen_tiles_y = INTERNAL_HEIGHT / TILE;
        assert!(
            FARM_WIDTH > screen_tiles_x,
            "the farm is no wider than the view"
        );
        assert!(
            FARM_HEIGHT > screen_tiles_y,
            "the farm is no taller than the view"
        );
    }

    #[test]
    fn every_tool_has_a_distinct_icon_and_hand_is_free() {
        let mut icons: Vec<&str> = Tool::ALL.iter().map(|t| t.icon()).collect();
        icons.sort_unstable();
        let count = icons.len();
        icons.dedup();
        assert_eq!(icons.len(), count, "two tools share an icon");
        assert_eq!(Tool::Hand.energy_cost(), 0.0);
    }
}
