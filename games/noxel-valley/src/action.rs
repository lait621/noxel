//! What using a tool on a tile does.
//!
//! This is the game's core verb, and it lives here rather than in `main.rs`
//! because it is a **rule**, not a piece of presentation. A rule buried in a
//! binary cannot be tested without a window, and "hoe a tile, plant a seed,
//! water it every day, harvest it, sell it" is the one sequence a farming game
//! had better get right.
//!
//! The function is pure in the sense that matters: it takes the map and the game
//! state and mutates them, and it returns what happened so the caller can say so.
//! It does not touch the renderer, the camera or the interface.

use crate::config::Tool;
use crate::sim::{GameState, Item};
use crate::world::{FarmMap, Ground, Plant};

/// What a use of a tool did.
///
/// Returned rather than logged, so the caller decides how to say it: a toast on
/// screen, a line in a test, nothing at all when the action was a no-op.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing happened, and nothing was spent.
    Nothing,
    /// The tile was hoed into soil.
    Tilled,
    /// The tile was watered.
    Watered,
    /// A seed was planted.
    Planted(&'static crate::config::Crop),
    /// A ripe crop was harvested.
    Harvested(&'static crate::config::Crop),
    /// A prop was cleared away.
    Cleared(&'static str),
    /// The tool could not be used because there was nothing to use it on.
    Refused(Refusal),
}

/// Why a tool did nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The player has no energy left.
    Exhausted,
    /// The bag has no room for the harvest.
    BagFull,
    /// The tile is not something this tool works on.
    WrongTool,
    /// There is nothing in the selected slot to plant.
    NoSeed,
}

impl Outcome {
    /// Whether anything actually happened.
    #[must_use]
    pub const fn changed_something(self) -> bool {
        !matches!(self, Self::Nothing | Self::Refused(_))
    }

    /// A line for the player, or `None` when there is nothing to say.
    #[must_use]
    pub fn message(self) -> Option<String> {
        match self {
            Self::Nothing => None,
            Self::Tilled => Some("翻好了土".to_string()),
            Self::Watered => Some("浇了水".to_string()),
            Self::Planted(crop) => Some(format!("种下 {}", crop.name)),
            Self::Harvested(crop) => Some(format!("收获 {}", crop.name)),
            Self::Cleared(prop) => Some(format!("清理了 {prop}")),
            Self::Refused(Refusal::Exhausted) => Some("太累了，去睡一觉吧".to_string()),
            Self::Refused(Refusal::BagFull) => Some("背包满了".to_string()),
            Self::Refused(_) => None,
        }
    }
}

/// Props a clearing tool can remove.
///
/// Rock and bush are deliberately absent from the pickaxe's list and present in
/// the axe's, and vice versa: a tool that removes everything is a tool with no
/// reason to exist, and the player has five of them.
const CHOPPABLE: [&str; 4] = ["weed", "bush", "log", "tree_stump"];
const MINABLE: [&str; 2] = ["rock_small", "rock_large"];

/// Uses `tool` on `aim`, and plants the selected seed if the tile is ready.
///
/// The order is tool first, then planting. That is what lets a player hold a
/// seed and press once on tilled soil: hoeing is skipped because the ground is
/// already tilled, and the seed goes in. It is also what stops the hoe from
/// being used on a tile that already has a plant in it — uprooting a crop the
/// player spent twelve days growing is never what they meant.
pub fn use_tool(
    map: &mut FarmMap,
    state: &mut GameState,
    tool: Tool,
    selected: Option<Item>,
    aim: (i32, i32),
) -> Outcome {
    if state.is_exhausted() && tool.energy_cost() > 0.0 {
        return Outcome::Refused(Refusal::Exhausted);
    }
    let Some(tile) = map.get(aim.0, aim.1).copied() else {
        return Outcome::Nothing;
    };

    let outcome = match tool {
        Tool::Hoe => {
            if tile.ground.is_hoeable()
                && tile.plant.is_none()
                && state.spend_energy(tool.energy_cost())
            {
                if let Some(tile) = map.get_mut(aim.0, aim.1) {
                    tile.ground = Ground::Tilled;
                }
                Outcome::Tilled
            } else {
                Outcome::Nothing
            }
        }
        Tool::Can => {
            if tile.ground == Ground::Tilled
                && !tile.watered
                && state.spend_energy(tool.energy_cost())
            {
                if let Some(tile) = map.get_mut(aim.0, aim.1) {
                    tile.watered = true;
                    if let Some(plant) = tile.plant.as_mut() {
                        plant.watered = true;
                    }
                }
                Outcome::Watered
            } else {
                Outcome::Nothing
            }
        }
        Tool::Scythe | Tool::Hand => harvest(map, state, aim, tool.energy_cost()),
        Tool::Axe => clear(map, state, aim, tool.energy_cost(), &CHOPPABLE),
        Tool::Pickaxe => clear(map, state, aim, tool.energy_cost(), &MINABLE),
    };

    if outcome.changed_something() {
        return outcome;
    }

    // A seed in hand on prepared soil is the second thing a press can mean.
    let Some(Item::Seed(crop)) = selected else {
        return outcome;
    };
    // Re-read the tile: a hoe may have just tilled it.
    let Some(tile) = map.get(aim.0, aim.1).copied() else {
        return outcome;
    };
    if tile.ground != Ground::Tilled || tile.plant.is_some() {
        return outcome;
    }
    if state.inventory.count_of(Item::Seed(crop)) == 0 {
        return Outcome::Refused(Refusal::NoSeed);
    }
    state.inventory.remove(Item::Seed(crop), 1);
    if let Some(tile) = map.get_mut(aim.0, aim.1) {
        tile.plant = Some(Plant::new(crop));
    }
    Outcome::Planted(crop)
}

fn harvest(map: &mut FarmMap, state: &mut GameState, aim: (i32, i32), cost: f32) -> Outcome {
    let Some(plant) = map
        .get(aim.0, aim.1)
        .and_then(|t| t.plant)
        .filter(|p| p.is_ripe())
    else {
        return Outcome::Nothing;
    };
    if state.inventory.count_of(Item::Produce(plant.crop)) >= crate::config::MAX_STACK
        && state.inventory.free_slots() == 0
    {
        return Outcome::Refused(Refusal::BagFull);
    }
    if state.inventory.add(Item::Produce(plant.crop), 1) > 0 {
        return Outcome::Refused(Refusal::BagFull);
    }
    let _ = state.spend_energy(cost);

    if let Some(tile) = map.get_mut(aim.0, aim.1) {
        if plant.crop.regrow_days > 0 {
            // A regrowing crop is cut back rather than dug up, so it is ready
            // again in `regrow_days` instead of needing a new seed.
            if let Some(plant) = tile.plant.as_mut() {
                plant.days = plant
                    .crop
                    .growth_days
                    .saturating_sub(plant.crop.regrow_days);
                plant.watered = false;
            }
        } else {
            tile.plant = None;
            tile.ground = Ground::Dirt;
        }
    }
    Outcome::Harvested(plant.crop)
}

fn clear(
    map: &mut FarmMap,
    state: &mut GameState,
    aim: (i32, i32),
    cost: f32,
    allowed: &[&str],
) -> Outcome {
    let Some(name) = map.get(aim.0, aim.1).and_then(|t| t.prop) else {
        return Outcome::Nothing;
    };
    if !allowed.contains(&name) {
        return Outcome::Refused(Refusal::WrongTool);
    }
    if !state.spend_energy(cost) {
        return Outcome::Refused(Refusal::Exhausted);
    }
    if let Some(tile) = map.get_mut(aim.0, aim.1) {
        tile.prop = None;
    }
    Outcome::Cleared(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MAX_ENERGY, Season, crop_by_key};
    use crate::sim::WeatherSystem;
    use crate::world::build_farm;

    fn field() -> (FarmMap, GameState, (i32, i32)) {
        let mut map = build_farm();
        let state = GameState::new(1);
        // The middle of the field: dirt, no props, and open on every side.
        let tile = (25, 20);
        map.get_mut(tile.0, tile.1).unwrap().ground = Ground::Dirt;
        (map, state, tile)
    }

    #[test]
    fn hoeing_turns_ground_into_soil_and_costs_energy() {
        let (mut map, mut state, tile) = field();
        let before = state.energy;
        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Hoe, None, tile),
            Outcome::Tilled
        );
        assert_eq!(map.get(tile.0, tile.1).unwrap().ground, Ground::Tilled);
        assert!(state.energy < before);
    }

    #[test]
    fn hoeing_a_tile_with_a_crop_in_it_does_nothing() {
        // Uprooting twelve days of growth is never what the player meant.
        let (mut map, mut state, tile) = field();
        map.get_mut(tile.0, tile.1).unwrap().ground = Ground::Tilled;
        map.get_mut(tile.0, tile.1).unwrap().plant =
            Some(Plant::new(crop_by_key("cauliflower").unwrap()));
        let before = state.energy;
        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Hoe, None, tile),
            Outcome::Nothing
        );
        assert!(map.get(tile.0, tile.1).unwrap().plant.is_some());
        assert_eq!(
            state.energy, before,
            "a refused action must not cost energy"
        );
    }

    #[test]
    fn watering_a_tile_twice_in_a_day_is_a_no_op_the_second_time() {
        let (mut map, mut state, tile) = field();
        use_tool(&mut map, &mut state, Tool::Hoe, None, tile);
        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Can, None, tile),
            Outcome::Watered
        );
        let energy = state.energy;
        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Can, None, tile),
            Outcome::Nothing
        );
        assert_eq!(state.energy, energy);
    }

    #[test]
    fn a_seed_in_hand_plants_on_prepared_soil_in_one_press() {
        // The press that tills and the press that plants are the same button:
        // once the ground is tilled, the next press puts the seed in.
        let (mut map, mut state, tile) = field();
        let crop = crop_by_key("parsnip").unwrap();
        state.inventory.add(Item::Seed(crop), 3);

        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Hoe, None, tile),
            Outcome::Tilled
        );
        assert_eq!(
            use_tool(
                &mut map,
                &mut state,
                Tool::Hoe,
                Some(Item::Seed(crop)),
                tile
            ),
            Outcome::Planted(crop)
        );
        assert!(map.get(tile.0, tile.1).unwrap().plant.is_some());
        assert_eq!(
            state.inventory.count_of(Item::Seed(crop)),
            2,
            "planting spends one seed"
        );
    }

    #[test]
    fn holding_a_seed_does_not_let_the_hoe_skip_the_tilling() {
        // One press is one action. A press on untilled ground hoes it and stops;
        // the next press sows. Doing both at once would mean the first press on
        // a new field always plants, which is not what a player standing in
        // front of a patch of grass with a hoe is asking for.
        let (mut map, mut state, tile) = field();
        let crop = crop_by_key("parsnip").unwrap();
        state.inventory.add(Item::Seed(crop), 1);

        assert_eq!(
            use_tool(
                &mut map,
                &mut state,
                Tool::Hoe,
                Some(Item::Seed(crop)),
                tile
            ),
            Outcome::Tilled
        );
        assert!(
            map.get(tile.0, tile.1).unwrap().plant.is_none(),
            "nothing was sown yet"
        );
        assert_eq!(
            state.inventory.count_of(Item::Seed(crop)),
            1,
            "and no seed was spent"
        );

        // The second press is the one that sows.
        assert_eq!(
            use_tool(
                &mut map,
                &mut state,
                Tool::Hoe,
                Some(Item::Seed(crop)),
                tile
            ),
            Outcome::Planted(crop)
        );
    }

    #[test]
    fn harvesting_a_ripe_crop_yields_produce_and_clears_the_tile() {
        let (mut map, mut state, tile) = field();
        let crop = crop_by_key("parsnip").unwrap();
        map.get_mut(tile.0, tile.1).unwrap().ground = Ground::Tilled;
        let mut plant = Plant::new(crop);
        plant.days = crop.growth_days;
        map.get_mut(tile.0, tile.1).unwrap().plant = Some(plant);

        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Scythe, None, tile),
            Outcome::Harvested(crop)
        );
        assert_eq!(state.inventory.count_of(Item::Produce(crop)), 1);
        assert!(map.get(tile.0, tile.1).unwrap().plant.is_none());
        assert_eq!(map.get(tile.0, tile.1).unwrap().ground, Ground::Dirt);
    }

    #[test]
    fn an_unripe_crop_is_not_harvested() {
        let (mut map, mut state, tile) = field();
        let crop = crop_by_key("cauliflower").unwrap();
        map.get_mut(tile.0, tile.1).unwrap().ground = Ground::Tilled;
        map.get_mut(tile.0, tile.1).unwrap().plant = Some(Plant::new(crop));
        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Scythe, None, tile),
            Outcome::Nothing
        );
        assert_eq!(state.inventory.count_of(Item::Produce(crop)), 0);
    }

    #[test]
    fn a_regrowing_crop_stays_in_the_ground() {
        let (mut map, mut state, tile) = field();
        let crop = crop_by_key("tomato").unwrap();
        assert!(crop.regrow_days > 0);
        map.get_mut(tile.0, tile.1).unwrap().ground = Ground::Tilled;
        let mut plant = Plant::new(crop);
        plant.days = crop.growth_days;
        map.get_mut(tile.0, tile.1).unwrap().plant = Some(plant);

        use_tool(&mut map, &mut state, Tool::Scythe, None, tile);
        let plant = map
            .get(tile.0, tile.1)
            .unwrap()
            .plant
            .expect("a regrowing crop survives");
        assert!(!plant.is_ripe(), "it has to grow back");
        assert_eq!(plant.days, crop.growth_days - crop.regrow_days);
    }

    #[test]
    fn the_axe_takes_weeds_and_the_pickaxe_does_not() {
        // A tool that clears everything is a tool with no reason to exist.
        let (mut map, mut state, tile) = field();
        map.get_mut(tile.0, tile.1).unwrap().prop = Some("weed");
        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Pickaxe, None, tile),
            Outcome::Refused(Refusal::WrongTool)
        );
        assert!(map.get(tile.0, tile.1).unwrap().prop.is_some());
        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Axe, None, tile),
            Outcome::Cleared("weed")
        );
        assert!(map.get(tile.0, tile.1).unwrap().prop.is_none());
    }

    #[test]
    fn an_exhausted_player_cannot_work() {
        let (mut map, mut state, tile) = field();
        state.energy = 0.0;
        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Hoe, None, tile),
            Outcome::Refused(Refusal::Exhausted)
        );
        assert_eq!(map.get(tile.0, tile.1).unwrap().ground, Ground::Dirt);
    }

    #[test]
    fn a_harvest_with_no_room_is_refused_rather_than_lost() {
        // The crop stays in the ground and the player is told. Silently dropping
        // a harvest is the worst possible outcome for a farming game.
        let (mut map, mut state, tile) = field();
        let crop = crop_by_key("pumpkin").unwrap();
        map.get_mut(tile.0, tile.1).unwrap().ground = Ground::Tilled;
        let mut plant = Plant::new(crop);
        plant.days = crop.growth_days;
        map.get_mut(tile.0, tile.1).unwrap().plant = Some(plant);

        // Fill every free slot with a different item.
        for index in 0..crate::config::INVENTORY_SLOTS {
            if state.inventory.slots()[index].is_none() {
                state
                    .inventory
                    .add(Item::Produce(crop_by_key("parsnip").unwrap()), 99);
            }
        }
        assert_eq!(state.inventory.free_slots(), 0);

        let outcome = use_tool(&mut map, &mut state, Tool::Scythe, None, tile);
        assert_eq!(outcome, Outcome::Refused(Refusal::BagFull));
        assert!(
            map.get(tile.0, tile.1).unwrap().plant.is_some(),
            "the crop must still be there"
        );
    }

    /// The whole loop, in one test: this is what the game *is*.
    #[test]
    fn a_seed_becomes_gold_over_a_season() {
        let mut map = build_farm();
        let mut state = GameState::new(1);
        let tile = (25, 20);
        let crop = crop_by_key("parsnip").unwrap();
        state.inventory.add(Item::Seed(crop), 1);

        // Find a seed whose first day is dry, so the watering is the test's
        // doing and not the weather's.
        let mut seed = 0;
        loop {
            let system = WeatherSystem::new(Season::Spring, seed);
            if !system.today().waters_crops() {
                state.weather = system;
                break;
            }
            seed += 1;
        }

        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Hoe, None, tile),
            Outcome::Tilled
        );
        assert_eq!(
            use_tool(
                &mut map,
                &mut state,
                Tool::Hoe,
                Some(Item::Seed(crop)),
                tile
            ),
            Outcome::Planted(crop)
        );

        // Water it every morning and sleep, until it is ripe.
        for _ in 0..crop.growth_days {
            assert_eq!(
                use_tool(&mut map, &mut state, Tool::Can, None, tile),
                Outcome::Watered
            );
            // Keep the weather dry so each day's watering is what grows it.
            while state.weather.today().waters_crops() {
                state.weather.advance(Season::Spring);
            }
            state.sleep(&mut map);
        }

        let plant = map
            .get(tile.0, tile.1)
            .unwrap()
            .plant
            .expect("the crop is still planted");
        assert!(
            plant.is_ripe(),
            "{} days of watering did not ripen it",
            crop.growth_days
        );

        assert_eq!(
            use_tool(&mut map, &mut state, Tool::Scythe, None, tile),
            Outcome::Harvested(crop)
        );
        assert_eq!(state.inventory.count_of(Item::Produce(crop)), 1);

        // Ship it and sleep: the gold arrives in the morning.
        let gold_before = state.inventory.gold;
        state.bin.add(Item::Produce(crop), 1);
        state.sleep(&mut map);
        assert_eq!(state.inventory.gold, gold_before + crop.sell_price);
    }

    #[test]
    fn a_full_season_of_hoeing_costs_more_energy_than_a_day_has() {
        // Sanity on the energy budget: the player cannot till the whole field in
        // one day, which is what makes the first week a series of choices.
        let tilled = MAX_ENERGY / Tool::Hoe.energy_cost();
        let field_tiles = 20 * 14;
        assert!(
            (tilled as i32) < field_tiles,
            "one day of energy tills {tilled} tiles of a {field_tiles} tile field"
        );
    }
}
