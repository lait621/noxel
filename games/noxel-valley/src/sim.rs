//! The simulation: the clock, the weather, the inventory and the economy.
//!
//! None of this touches the renderer or the framebuffer, and it is all
//! deterministic functions of its own state. That is deliberate: the rules of a
//! farming game are the part worth testing, and a system that needs a `Scene` to
//! run is a system whose tests need a `Scene` too.
//!
//! # The day
//!
//! ```text
//!  06:00  wake up. Energy restored. Weather for today was decided last night.
//!    ...  hoe, plant, water, harvest, walk to the shop, sell
//!  02:00  forced to bed
//!         |
//!         v
//!     advance_day()
//!       1. the shipping bin is sold, and the gold arrives
//!       2. rain waters every tilled tile
//!       3. watered plants grow one day; out-of-season plants die
//!       4. today's watering is cleared
//!       5. tomorrow's weather is rolled
//!       6. the calendar advances
//! ```
//!
//! The order is the whole design. Rain before growth is what makes a rainy day
//! free; growth before clearing is what makes yesterday's watering count;
//! clearing last is what makes today start dry.

use noxel_core::math::Color8;

use crate::config::{
    CROPS, Crop, DAY_END_HOUR, DAY_START_HOUR, DAYS_PER_SEASON, FAST_FORWARD_MULTIPLIER,
    FORECAST_DAYS, GAME_MINUTES_PER_SECOND, HOTBAR_SLOTS, INVENTORY_SLOTS, MAX_ENERGY, MAX_STACK,
    Season, Tool, Weather,
};

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// The calendar and the time of day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    /// Minutes since [`DAY_START_HOUR`].
    minutes: f32,
    /// Day of the season, `0..DAYS_PER_SEASON`.
    day: u32,
    /// The season.
    season: Season,
    /// The year, starting at 1.
    year: u32,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            minutes: 0.0,
            day: 0,
            season: Season::Spring,
            year: 1,
        }
    }
}

impl Clock {
    /// The current hour as a float, so 6.5 is half past six.
    #[must_use]
    pub fn hour(&self) -> f32 {
        DAY_START_HOUR + self.minutes / 60.0
    }

    /// Day of the season, one-based for display.
    #[must_use]
    pub fn day_of_season(&self) -> u32 {
        self.day + 1
    }

    /// The season.
    #[must_use]
    pub fn season(&self) -> Season {
        self.season
    }

    /// The year.
    #[must_use]
    pub fn year(&self) -> u32 {
        self.year
    }

    /// How far through the working day it is, `0..=1`.
    #[must_use]
    pub fn fraction_elapsed(&self) -> f32 {
        let total = (DAY_END_HOUR - DAY_START_HOUR) * 60.0;
        (self.minutes / total).clamp(0.0, 1.0)
    }

    /// The time as `H:MM`.
    #[must_use]
    pub fn time_string(&self) -> String {
        let hour = self.hour();
        let h = hour.floor() as u32;
        let minutes = ((hour - hour.floor()) * 60.0) as u32;
        // The clock runs past midnight into a 24+ hour day, but the player reads
        // a wall clock: 25:30 is 1:30 in the morning.
        let display = h % 24;
        let suffix = if (12..24).contains(&h) { "PM" } else { "AM" };
        let twelve = match display % 12 {
            0 => 12,
            other => other,
        };
        format!("{twelve}:{minutes:02} {suffix}")
    }

    /// Whether it is late enough that the player should be heading to bed.
    #[must_use]
    pub fn is_late(&self) -> bool {
        self.hour() >= 24.0
    }

    /// Whether the day is over.
    #[must_use]
    pub fn is_over(&self) -> bool {
        self.hour() >= DAY_END_HOUR
    }

    /// Advances the clock, returning `true` when the day ended.
    ///
    /// `fast` multiplies the rate, for a tester who wants to see a season change
    /// without playing three hours.
    pub fn advance(&mut self, seconds: f32, fast: bool) -> bool {
        let rate = if fast {
            GAME_MINUTES_PER_SECOND * FAST_FORWARD_MULTIPLIER
        } else {
            GAME_MINUTES_PER_SECOND
        };
        self.minutes += seconds * rate;
        self.is_over()
    }

    /// Moves to the next morning.
    ///
    /// Called only by [`GameState::sleep`], never by [`Clock::advance`], so there
    /// is exactly one place that knows what a new day means.
    fn roll_over(&mut self) {
        self.minutes = 0.0;
        self.day += 1;
        if self.day >= DAYS_PER_SEASON {
            self.day = 0;
            self.season = self.season.next();
            if self.season == Season::Spring {
                self.year += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Weather
// ---------------------------------------------------------------------------

/// Today's weather and the forecast.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherSystem {
    today: Weather,
    forecast: [Weather; FORECAST_DAYS],
    /// Advanced once per day. Stored rather than derived from the date so the
    /// sequence is a real sequence; deriving it from `(year, day)` would make a
    /// reloaded save and a continued game agree, but it would also make the
    /// weather a function of the calendar alone, and two years would be
    /// identical.
    seed: u32,
}

impl WeatherSystem {
    /// Rolls the first day's weather for a season.
    #[must_use]
    pub fn new(season: Season, seed: u32) -> Self {
        let mut system = Self {
            today: Weather::Sunny,
            forecast: [Weather::Sunny; FORECAST_DAYS],
            seed,
        };
        for slot in 0..FORECAST_DAYS {
            system.forecast[slot] = system.roll(season);
        }
        system.today = system.forecast[0];
        system
    }

    /// Today's weather.
    #[must_use]
    pub fn today(&self) -> Weather {
        self.today
    }

    /// The forecast, today first.
    #[must_use]
    pub fn forecast(&self) -> &[Weather; FORECAST_DAYS] {
        &self.forecast
    }

    /// Slides the forecast forward one day and rolls a new last entry.
    ///
    /// Public because it is a legitimate thing for a game to do — a debug
    /// command, a cheat, a scripted story day — and because a test that wants to
    /// know what happens on a dry day should be able to arrange one rather than
    /// search for a seed that produces it.
    pub fn advance(&mut self, season: Season) {
        for index in 0..FORECAST_DAYS - 1 {
            self.forecast[index] = self.forecast[index + 1];
        }
        self.forecast[FORECAST_DAYS - 1] = self.roll(season);
        self.today = self.forecast[0];
    }

    /// The next value from the weather sequence.
    ///
    /// A xorshift rather than anything shared: the weather must be reproducible
    /// from the save file's seed, or a reloaded game has different weather from
    /// the one the player was playing.
    fn roll(&mut self, season: Season) -> Weather {
        let mut x = self.seed | 1;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        let roll = x % 100;
        // The seasonal table. Winter is the only season that snows, and summer
        // the only one with a real chance of a storm, which is what gives the
        // seasons a feel beyond a colour change.
        let (sunny, cloudy, rain, storm) = match season {
            Season::Spring => (40, 25, 29, 6),
            Season::Summer => (58, 15, 17, 10),
            Season::Fall => (38, 30, 26, 6),
            Season::Winter => (30, 35, 0, 0),
        };
        if roll < sunny {
            Weather::Sunny
        } else if roll < sunny + cloudy {
            Weather::Cloudy
        } else if roll < sunny + cloudy + rain {
            Weather::Rain
        } else if roll < sunny + cloudy + rain + storm {
            Weather::Storm
        } else if season == Season::Winter {
            Weather::Snow
        } else {
            Weather::Sunny
        }
    }
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

/// Something the player can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    /// A seed, which plants its crop.
    Seed(&'static Crop),
    /// A harvested crop, which sells.
    Produce(&'static Crop),
    /// A tool, which is never consumed and never stacks.
    Tool(Tool),
}

impl Item {
    /// The Chinese name.
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            Self::Seed(crop) => format!("{}种子", crop.name),
            Self::Produce(crop) => crop.name.to_string(),
            Self::Tool(tool) => tool.name().to_string(),
        }
    }

    /// The icon region name in the UI atlas.
    #[must_use]
    pub fn icon(&self) -> String {
        match self {
            Self::Seed(crop) => format!("seed_{}", crop.key),
            Self::Produce(crop) => format!("item_{}", crop.key),
            Self::Tool(tool) => tool.icon().to_string(),
        }
    }

    /// What one unit sells for. Seeds sell for half of what they cost.
    #[must_use]
    pub fn sell_price(&self) -> u32 {
        match self {
            Self::Seed(crop) => crop.seed_price / 2,
            Self::Produce(crop) => crop.sell_price,
            Self::Tool(_) => 0,
        }
    }

    /// How many fit in one slot.
    #[must_use]
    pub fn max_stack(&self) -> u32 {
        match self {
            Self::Tool(_) => 1,
            _ => MAX_STACK,
        }
    }

    /// The colour used to draw this item before the art atlas is loaded.
    #[must_use]
    pub fn color(&self) -> Color8 {
        match self {
            Self::Seed(crop) => crate::skin::desaturate(crop.color, 0.35),
            Self::Produce(crop) => crop.color,
            Self::Tool(_) => Color8::new(180, 186, 200, 255),
        }
    }
}

// ---------------------------------------------------------------------------
// Inventory
// ---------------------------------------------------------------------------

/// One inventory slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// What is in it.
    pub item: Item,
    /// How many.
    pub count: u32,
}

/// The player's bag and purse.
#[derive(Clone, Debug)]
pub struct Inventory {
    slots: Vec<Option<Slot>>,
    /// The player's money.
    pub gold: u32,
}

impl Inventory {
    /// An inventory holding the starting tools and `gold`.
    #[must_use]
    pub fn new(gold: u32) -> Self {
        let mut inventory = Self {
            slots: vec![None; INVENTORY_SLOTS],
            gold,
        };
        // The tools live in the hotbar and never move: a player who loses their
        // watering can to a full bag has lost the game.
        for (index, tool) in Tool::ALL.iter().enumerate() {
            if index < HOTBAR_SLOTS {
                inventory.slots[index] = Some(Slot {
                    item: Item::Tool(*tool),
                    count: 1,
                });
            }
        }
        inventory
    }

    /// Every slot, in order.
    #[must_use]
    pub fn slots(&self) -> &[Option<Slot>] {
        &self.slots
    }

    /// The tool in a hotbar slot, if any.
    #[must_use]
    pub fn tool_at(&self, index: usize) -> Option<Tool> {
        match self.slots.get(index)?.as_ref()?.item {
            Item::Tool(tool) => Some(tool),
            _ => None,
        }
    }

    /// How many slots are free.
    #[must_use]
    pub fn free_slots(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_none()).count()
    }

    /// Adds items, stacking into existing slots first.
    ///
    /// Returns however many did **not** fit. A caller that ignores the return
    /// value silently deletes items, which is why it is not `()`.
    pub fn add(&mut self, item: Item, mut count: u32) -> u32 {
        let max = item.max_stack();
        // Top up partial stacks before opening a new slot, so a bag does not
        // fill with five half-stacks of the same seed.
        for slot in self.slots.iter_mut().flatten() {
            if count == 0 {
                break;
            }
            if slot.item == item && slot.count < max {
                let room = max - slot.count;
                let moved = room.min(count);
                slot.count += moved;
                count -= moved;
            }
        }
        for slot in self.slots.iter_mut() {
            if count == 0 {
                break;
            }
            if slot.is_none() {
                let moved = max.min(count);
                *slot = Some(Slot { item, count: moved });
                count -= moved;
            }
        }
        count
    }

    /// Removes up to `count` of an item, returning how many were removed.
    pub fn remove(&mut self, item: Item, count: u32) -> u32 {
        let mut removed = 0;
        for slot in self.slots.iter_mut().flatten() {
            if removed >= count {
                break;
            }
            if slot.item != item {
                continue;
            }
            let taken = slot.count.min(count - removed);
            slot.count -= taken;
            removed += taken;
        }
        // Clear emptied slots after the loop, so the iterator stays valid.
        for slot in self.slots.iter_mut() {
            if slot.as_ref().is_some_and(|s| s.count == 0) {
                *slot = None;
            }
        }
        removed
    }

    /// How many of an item are held.
    #[must_use]
    pub fn count_of(&self, item: Item) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|slot| slot.item == item)
            .map(|slot| slot.count)
            .sum()
    }

    /// Removes one item from a specific slot, for drag-free UI interaction.
    pub fn take_one(&mut self, index: usize) -> Option<Item> {
        let slot = self.slots.get_mut(index)?.as_mut()?;
        if matches!(slot.item, Item::Tool(_)) {
            return None;
        }
        let item = slot.item;
        slot.count -= 1;
        if slot.count == 0 {
            self.slots[index] = None;
        }
        Some(item)
    }

    /// Adds `amount` gold.
    pub fn earn(&mut self, amount: u32) {
        self.gold = self.gold.saturating_add(amount);
    }

    /// Spends gold, returning whether it could be afforded.
    pub fn spend(&mut self, amount: u32) -> bool {
        if self.gold < amount {
            return false;
        }
        self.gold -= amount;
        true
    }
}

// ---------------------------------------------------------------------------
// The shipping bin and the shop
// ---------------------------------------------------------------------------

/// What the player will be paid in the morning.
///
/// The bin exists because the shop has opening hours and the farm does not: a
/// player who finishes harvesting at 1am should not have to lose the crop or the
/// night's sleep.
#[derive(Clone, Debug, Default)]
pub struct ShippingBin {
    contents: Vec<(Item, u32)>,
}

impl ShippingBin {
    /// Adds items to the bin, returning the value they will fetch.
    pub fn add(&mut self, item: Item, count: u32) -> u32 {
        if count == 0 || item.sell_price() == 0 {
            return 0;
        }
        if let Some(entry) = self
            .contents
            .iter_mut()
            .find(|(existing, _)| *existing == item)
        {
            entry.1 += count;
        } else {
            self.contents.push((item, count));
        }
        item.sell_price() * count
    }

    /// What is in the bin.
    #[must_use]
    pub fn contents(&self) -> &[(Item, u32)] {
        &self.contents
    }

    /// The total value waiting to be collected.
    #[must_use]
    pub fn value(&self) -> u32 {
        self.contents
            .iter()
            .map(|(item, count)| item.sell_price() * count)
            .sum()
    }

    /// Empties the bin, returning what it was worth.
    pub fn collect(&mut self) -> u32 {
        let value = self.value();
        self.contents.clear();
        value
    }
}

/// What yesterday was worth, for the morning report.
#[derive(Clone, Debug, Default)]
pub struct DaySummary {
    /// Gold from the shipping bin.
    pub shipped: u32,
    /// Crops that became ripe.
    pub ripened: u32,
    /// Crops lost to the season turning.
    pub died: u32,
    /// The day that just ended.
    pub day: u32,
    /// The season that just ended.
    pub season: Season,
}

// ---------------------------------------------------------------------------
// The whole game state
// ---------------------------------------------------------------------------

/// Everything the simulation owns.
#[derive(Clone, Debug)]
pub struct GameState {
    /// The calendar.
    pub clock: Clock,
    /// The sky.
    pub weather: WeatherSystem,
    /// The bag and the purse.
    pub inventory: Inventory,
    /// What is waiting to be sold.
    pub bin: ShippingBin,
    /// The player's energy.
    pub energy: f32,
    /// The hotbar selection.
    pub selected: usize,
    /// The last day's report, shown on waking.
    pub last_summary: Option<DaySummary>,
    /// Whether the player asked to sleep early.
    pub asleep: bool,
}

impl GameState {
    /// A new game.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self {
            clock: Clock::default(),
            weather: WeatherSystem::new(Season::Spring, seed),
            inventory: Inventory::new(crate::config::STARTING_GOLD),
            bin: ShippingBin::default(),
            energy: MAX_ENERGY,
            selected: 0,
            last_summary: None,
            asleep: false,
        }
    }

    /// The tool currently in hand.
    #[must_use]
    pub fn current_tool(&self) -> Tool {
        self.inventory.tool_at(self.selected).unwrap_or(Tool::Hand)
    }

    /// Spends energy, returning whether the action could be afforded.
    pub fn spend_energy(&mut self, amount: f32) -> bool {
        if self.energy < amount {
            return false;
        }
        self.energy = (self.energy - amount).max(0.0);
        true
    }

    /// Drains energy for walking.
    pub fn drain_walking_energy(&mut self, minutes: f32) {
        self.energy = (self.energy - minutes * crate::config::ENERGY_PER_MINUTE).max(0.0);
    }

    /// Whether the player is out of energy and moving at a crawl.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.energy <= 0.0
    }

    /// Runs the night, and returns the report for the morning.
    ///
    /// See the module documentation for why the order is what it is. `map` is
    /// advanced in place; the caller is responsible for rebuilding the mesh.
    pub fn sleep(&mut self, map: &mut crate::world::FarmMap) -> DaySummary {
        // 1. Sell what is in the bin.
        let shipped = self.bin.collect();
        self.inventory.earn(shipped);

        // 2. Rain waters everything, before growth is evaluated.
        if self.weather.today().waters_crops() {
            map.water_all();
        }

        // 3. Grow, and let the season end what it will.
        let report = map.advance_day(self.clock.season());

        // 4-6. Clear the watering, roll tomorrow, advance the calendar.
        self.weather.advance(self.clock.season().next());
        let summary = DaySummary {
            shipped,
            ripened: report.ripened,
            died: report.died,
            day: self.clock.day_of_season(),
            season: self.clock.season(),
        };
        self.clock.roll_over();
        self.energy = MAX_ENERGY;
        self.asleep = false;
        self.last_summary = Some(summary.clone());
        summary
    }

    /// A one-line status, for the log.
    #[must_use]
    pub fn status_line(&self) -> String {
        format!(
            "{} {} day {} {} | {} | {}g | energy {:.0}",
            self.clock.year(),
            self.clock.season().english(),
            self.clock.day_of_season(),
            self.clock.time_string(),
            self.weather.today().english(),
            self.inventory.gold,
            self.energy
        )
    }
}

/// Seeds the shop sells in a season.
#[must_use]
pub fn shop_stock(season: Season) -> Vec<&'static Crop> {
    CROPS.iter().filter(|crop| crop.season == season).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::crop_by_key;
    use crate::world::{Ground, Plant, build_farm};

    #[test]
    fn the_clock_starts_at_six_in_the_morning() {
        let clock = Clock::default();
        assert_eq!(clock.hour(), 6.0);
        assert_eq!(clock.day_of_season(), 1);
        assert_eq!(clock.season(), Season::Spring);
        assert_eq!(clock.year(), 1);
        assert!(clock.time_string().starts_with("6:00"));
    }

    #[test]
    fn the_displayed_time_reads_as_a_wall_clock() {
        // The day runs to 26:00, but a player at 25:30 is told it is 1:30 AM.
        let mut clock = Clock {
            minutes: (25.5 - DAY_START_HOUR) * 60.0,
            ..Clock::default()
        };
        assert!(
            clock.time_string().starts_with("1:30"),
            "got {}",
            clock.time_string()
        );
        assert!(clock.is_late());

        clock.minutes = (13.0 - DAY_START_HOUR) * 60.0;
        assert!(
            clock.time_string().starts_with("1:00 PM"),
            "got {}",
            clock.time_string()
        );
    }

    #[test]
    fn a_day_takes_about_ten_real_minutes() {
        let mut clock = Clock::default();
        let mut elapsed = 0.0;
        while !clock.advance(1.0, false) {
            elapsed += 1.0;
            assert!(elapsed < 3600.0, "the day never ended");
        }
        assert!(
            (540.0..660.0).contains(&elapsed),
            "a day took {elapsed} seconds"
        );
    }

    #[test]
    fn the_calendar_rolls_through_four_seasons_into_a_new_year() {
        let mut clock = Clock::default();
        for _ in 0..DAYS_PER_SEASON * 4 {
            clock.roll_over();
        }
        assert_eq!(clock.year(), 2);
        assert_eq!(clock.season(), Season::Spring);
        assert_eq!(clock.day_of_season(), 1);
    }

    #[test]
    fn weather_is_deterministic_for_a_seed() {
        // A reloaded save must not have different weather from the game that was
        // being played.
        let a = WeatherSystem::new(Season::Spring, 12345);
        let b = WeatherSystem::new(Season::Spring, 12345);
        assert_eq!(a.today(), b.today());
        assert_eq!(a.forecast(), b.forecast());
    }

    #[test]
    fn different_seeds_give_different_weather_sequences() {
        let mut a = WeatherSystem::new(Season::Spring, 1);
        let mut b = WeatherSystem::new(Season::Spring, 999);
        let a_seq: Vec<Weather> = (0..20)
            .map(|_| {
                a.advance(Season::Spring);
                a.today()
            })
            .collect();
        let b_seq: Vec<Weather> = (0..20)
            .map(|_| {
                b.advance(Season::Spring);
                b.today()
            })
            .collect();
        assert_ne!(a_seq, b_seq, "the weather does not depend on the seed");
    }

    #[test]
    fn it_never_rains_in_winter_and_it_snows_somewhere_in_it() {
        // Snow is winter's whole character; plain rain in winter would make the
        // season indistinguishable from autumn.
        let mut winter = WeatherSystem::new(Season::Winter, 7);
        let mut snowed = false;
        for _ in 0..400 {
            winter.advance(Season::Winter);
            assert_ne!(winter.today(), Weather::Rain, "it rained in winter");
            assert_ne!(winter.today(), Weather::Storm, "it stormed in winter");
            snowed |= winter.today() == Weather::Snow;
        }
        assert!(snowed, "it never snowed in a whole winter");
    }

    #[test]
    fn the_forecast_slides_rather_than_rerolling() {
        // Yesterday's "tomorrow" must be today's "today", or the forecast is a
        // lie the player will notice immediately.
        let mut system = WeatherSystem::new(Season::Summer, 42);
        let predicted = system.forecast()[1];
        system.advance(Season::Summer);
        assert_eq!(system.today(), predicted);
    }

    #[test]
    fn adding_items_stacks_before_opening_new_slots() {
        let mut inventory = Inventory::new(0);
        let seeds = Item::Seed(crop_by_key("parsnip").unwrap());
        let free_before = inventory.free_slots();
        inventory.add(seeds, 5);
        inventory.add(seeds, 7);
        assert_eq!(inventory.count_of(seeds), 12);
        assert_eq!(
            inventory.free_slots(),
            free_before - 1,
            "two adds must share one slot"
        );
    }

    #[test]
    fn a_full_inventory_reports_what_did_not_fit() {
        // Silently dropping the overflow deletes the player's crops.
        let mut inventory = Inventory::new(0);
        let seeds = Item::Seed(crop_by_key("parsnip").unwrap());
        // The capacity is derived, not assumed: the starting tools already
        // occupy slots, and hard-coding 24 slots of room is how this test was
        // wrong the first time.
        let capacity = inventory.free_slots() as u32 * MAX_STACK;
        let leftover = inventory.add(seeds, capacity + 40);
        assert_eq!(leftover, 40);
        assert_eq!(inventory.free_slots(), 0);
        assert_eq!(inventory.count_of(seeds), capacity);
    }

    #[test]
    fn tools_are_given_at_the_start_and_never_stack() {
        let inventory = Inventory::new(0);
        assert_eq!(inventory.tool_at(0), Some(Tool::ALL[0]));
        assert_eq!(inventory.count_of(Item::Tool(Tool::Hoe)), 1);
    }

    #[test]
    fn removing_clears_emptied_slots() {
        let mut inventory = Inventory::new(0);
        let seeds = Item::Seed(crop_by_key("parsnip").unwrap());
        inventory.add(seeds, 3);
        assert_eq!(inventory.remove(seeds, 2), 2);
        assert_eq!(inventory.count_of(seeds), 1);
        assert_eq!(
            inventory.remove(seeds, 5),
            1,
            "removing more than held takes what is there"
        );
        assert_eq!(
            inventory.free_slots(),
            INVENTORY_SLOTS - Tool::ALL.len().min(HOTBAR_SLOTS)
        );
    }

    #[test]
    fn spending_more_than_you_have_fails_without_going_negative() {
        let mut inventory = Inventory::new(100);
        assert!(!inventory.spend(101));
        assert_eq!(inventory.gold, 100);
        assert!(inventory.spend(100));
        assert_eq!(inventory.gold, 0);
    }

    #[test]
    fn the_shipping_bin_values_what_is_in_it_and_empties_once() {
        let mut bin = ShippingBin::default();
        let crop = crop_by_key("pumpkin").unwrap();
        let value = bin.add(Item::Produce(crop), 3);
        assert_eq!(value, crop.sell_price * 3);
        assert_eq!(bin.value(), value);
        assert_eq!(bin.collect(), value);
        assert_eq!(bin.collect(), 0, "collecting twice must not pay twice");
        assert!(bin.contents().is_empty());
    }

    #[test]
    fn sleeping_pays_for_the_bin_and_restores_energy() {
        let mut state = GameState::new(1);
        let mut map = build_farm();
        let crop = crop_by_key("parsnip").unwrap();
        state.inventory.gold = 0;
        state.bin.add(Item::Produce(crop), 2);
        state.energy = 3.0;

        let summary = state.sleep(&mut map);
        assert_eq!(summary.shipped, crop.sell_price * 2);
        assert_eq!(state.inventory.gold, crop.sell_price * 2);
        assert_eq!(state.energy, MAX_ENERGY);
        assert_eq!(state.clock.day_of_season(), 2);
    }

    #[test]
    fn sleeping_grows_only_what_was_watered() {
        let mut state = GameState::new(1);
        let mut map = build_farm();
        let crop = crop_by_key("parsnip").unwrap();
        for x in 30..33 {
            map.get_mut(x, 20).unwrap().ground = Ground::Tilled;
            map.get_mut(x, 20).unwrap().plant = Some(Plant::new(crop));
        }
        map.get_mut(30, 20).unwrap().plant.as_mut().unwrap().watered = true;

        // Force a sunny day so rain does not water everything for us.
        state.weather = WeatherSystem::new(Season::Spring, 0);
        while state.weather.today().waters_crops() {
            state.weather.advance(Season::Spring);
        }

        state.sleep(&mut map);
        assert_eq!(
            map.get(30, 20).unwrap().plant.unwrap().days,
            1,
            "the watered crop grew"
        );
        assert_eq!(
            map.get(31, 20).unwrap().plant.unwrap().days,
            0,
            "the dry crop did not"
        );
    }

    #[test]
    fn rain_waters_the_whole_farm_overnight() {
        let mut state = GameState::new(1);
        let mut map = build_farm();
        let crop = crop_by_key("parsnip").unwrap();
        map.get_mut(30, 20).unwrap().ground = Ground::Tilled;
        map.get_mut(30, 20).unwrap().plant = Some(Plant::new(crop));
        // Nothing was watered by hand.
        assert!(!map.get(30, 20).unwrap().watered);

        // Find a seed whose first day is rainy.
        let mut seed = 0;
        loop {
            let system = WeatherSystem::new(Season::Spring, seed);
            if system.today().waters_crops() {
                state.weather = system;
                break;
            }
            seed += 1;
            assert!(seed < 10_000, "no rainy seed found");
        }

        state.sleep(&mut map);
        assert_eq!(
            map.get(30, 20).unwrap().plant.unwrap().days,
            1,
            "rain should have grown it"
        );
    }

    #[test]
    fn the_shop_stocks_only_the_current_season() {
        for season in [Season::Spring, Season::Summer, Season::Fall, Season::Winter] {
            for crop in shop_stock(season) {
                assert_eq!(
                    crop.season,
                    season,
                    "{} is not a {} crop",
                    crop.english,
                    season.english()
                );
            }
        }
        assert_eq!(shop_stock(Season::Winter).len(), 0);
    }

    #[test]
    fn seeds_sell_for_less_than_they_cost() {
        // Otherwise buying and immediately selling is an infinite money loop.
        for crop in CROPS.iter() {
            let seed = Item::Seed(crop);
            assert!(
                seed.sell_price() < crop.seed_price,
                "{} seeds are free money",
                crop.english
            );
        }
    }

    #[test]
    fn the_current_tool_follows_the_hotbar_selection() {
        let mut state = GameState::new(1);
        state.selected = 1;
        assert_eq!(state.current_tool(), Tool::ALL[1]);
        // An out-of-range selection falls back to an empty hand rather than
        // panicking, which is what a stale save file would do.
        state.selected = 999;
        assert_eq!(state.current_tool(), Tool::Hand);
    }

    #[test]
    fn energy_runs_out_and_never_goes_negative() {
        let mut state = GameState::new(1);
        assert!(state.spend_energy(MAX_ENERGY));
        assert!(state.is_exhausted());
        assert!(!state.spend_energy(1.0), "an exhausted player cannot act");
        assert_eq!(state.energy, 0.0);
    }
}
