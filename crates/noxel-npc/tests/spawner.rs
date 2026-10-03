//! Where new agents come from: walkable ground, a sensible kind, and the same
//! answer every time.

mod common;

use common::*;
use noxel_core::math::Vec3;
use noxel_npc::{NpcConfig, NpcKind, NpcSpawner, SpawnKind};
use noxel_world::town;

/// A town with a small built-up area, so the spawn ring reaches open country.
fn small_town(seed: u64) -> (noxel_world::WorldStreamer, Vec3) {
    let mut streamer = streamer_with(seed, Vec3::ZERO, |config| {
        config.town_radius_chunks = 1;
    });
    let centre = town::town_centre(
        streamer.generator().config(),
        town::nearest_town_cell(streamer.generator().config(), 0.0, 0.0),
    );
    streamer.update(centre);
    let centre = find_walkable(&streamer, centre, 96.0).unwrap_or(centre);
    (streamer, centre)
}

fn spawner() -> NpcSpawner {
    NpcSpawner::new(NpcConfig {
        seed: 7,
        ..NpcConfig::default()
    })
}

#[test]
fn choose_spawn_is_deterministic() {
    let (streamer, centre) = small_town(7);
    let first: Vec<Option<(NpcKind, [u32; 3])>> = {
        let mut spawner = spawner();
        (0..200u64)
            .map(|index| {
                spawner
                    .choose_spawn(&streamer, centre, index)
                    .map(|(kind, p)| (kind, [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]))
            })
            .collect()
    };
    let second: Vec<Option<(NpcKind, [u32; 3])>> = {
        let mut spawner = spawner();
        (0..200u64)
            .map(|index| {
                spawner
                    .choose_spawn(&streamer, centre, index)
                    .map(|(kind, p)| (kind, [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]))
            })
            .collect()
    };
    assert_eq!(
        first, second,
        "spawns must be a pure function of (seed, index)"
    );
    assert!(first.iter().filter(|entry| entry.is_some()).count() > 190);
}

#[test]
fn choose_spawn_returns_standable_ground() {
    let (streamer, centre) = small_town(11);
    let mut spawner = spawner();
    let mut interior = 0;
    let mut checked = 0;
    for index in 0..500u64 {
        let Some((_, position)) = spawner.choose_spawn(&streamer, centre, index) else {
            continue;
        };
        checked += 1;
        assert!(position.is_finite());
        assert!(
            is_loaded(&streamer, position),
            "spawn {position:?} is outside the streamed world"
        );
        assert!(
            streamer.is_walkable(position),
            "spawn {position:?} is water or a wall"
        );
        let kind = NpcSpawner::classify(&streamer, position);
        assert!(
            !is_water(&streamer, position),
            "spawn {position:?} is in water"
        );
        // Never inside solid geometry: not in a wall, not inside a prop.
        let region = noxel_core::math::Aabb::new(
            position + Vec3::new(-0.3, 0.1, -0.3),
            position + Vec3::new(0.3, 1.6, 0.3),
        );
        let mut colliders = Vec::new();
        streamer.colliders_in(region, &mut colliders);
        assert!(
            colliders.is_empty(),
            "spawn {position:?} is inside geometry: {colliders:?}"
        );
        if kind != SpawnKind::Interior {
            assert!(
                !inside_building(&streamer, position),
                "a {kind:?} spawn is under a roof at {position:?}"
            );
        } else {
            interior += 1;
            assert!(
                inside_building(&streamer, position),
                "an interior spawn is outdoors at {position:?}"
            );
        }
    }
    assert!(checked >= 450, "only {checked} of 500 samples spawned");
    // Building interiors are solid in this engine (one collider from the base
    // to the eaves), so a spawn under a roof would be a spawn inside a wall.
    // `classify` still reports them; `choose_spawn` must not use them.
    assert_eq!(interior, 0, "an agent was spawned inside a solid building");
    assert!(
        !interiors_are_solid(&streamer, centre),
        "buildings became enterable: interior spawns must work now"
    );
}

/// True when every walkable spot strictly inside a building is solid.
///
/// A generated house is one collider from its base to its eaves, so its floor
/// tiles are inside a wall. The check reads the building's own occluder boxes
/// rather than calling `colliders_in`, which would rescan every loaded chunk
/// for every sample.
fn interiors_are_solid(streamer: &noxel_world::WorldStreamer, centre: Vec3) -> bool {
    let inset = 0.6;
    for pos in streamer.loaded_positions() {
        let Some(chunk) = streamer.chunk(pos) else {
            continue;
        };
        for building in &chunk.buildings {
            let bounds = building.bounds;
            let mut z = bounds.min.z + inset;
            while z < bounds.max.z - inset {
                let mut x = bounds.min.x + inset;
                while x < bounds.max.x - inset {
                    let candidate = Vec3::new(x, 0.0, z);
                    x += 1.0;
                    let dx = candidate.x - centre.x;
                    let dz = candidate.z - centre.z;
                    if (dx * dx + dz * dz).sqrt() > 70.0 {
                        continue;
                    }
                    let Some(ground) = ground_at(streamer, candidate) else {
                        continue;
                    };
                    let head = ground + Vec3::Y * 0.9;
                    if !building
                        .occluders
                        .iter()
                        .any(|box_| box_.contains_point(head))
                    {
                        return false;
                    }
                }
                z += 1.0;
            }
        }
    }
    true
}

/// True when `p` lies under a building's roof.
fn inside_building(streamer: &noxel_world::WorldStreamer, p: Vec3) -> bool {
    streamer
        .chunk_at(p)
        .is_some_and(|chunk| chunk.buildings.iter().any(|b| b.contains_xz(p)))
}

#[test]
fn every_spawn_kind_appears_over_500_samples() {
    let (streamer, centre) = small_town(3);
    let mut spawner = spawner();
    let mut counts = [0usize; 4];
    for index in 0..500u64 {
        if let Some((_, position)) = spawner.choose_spawn(&streamer, centre, index) {
            let kind = NpcSpawner::classify(&streamer, position);
            let slot = SpawnKind::ALL
                .iter()
                .position(|candidate| *candidate == kind)
                .expect("a known kind");
            counts[slot] += 1;
        }
    }
    for (slot, count) in counts.iter().enumerate() {
        if SpawnKind::ALL[slot] == SpawnKind::Interior {
            // The generated town's houses are solid boxes: there is no indoor
            // ground an agent could stand on, so an interior spawn must never
            // appear. (The check below fails loudly if that ever changes.)
            assert_eq!(*count, 0, "an agent spawned inside a building");
            assert!(
                !interiors_are_solid(&streamer, centre),
                "buildings became enterable: interior spawns must work now"
            );
            continue;
        }
        assert!(
            *count > 0,
            "{} never appeared in 500 samples: {counts:?}",
            SpawnKind::ALL[slot].name()
        );
    }
    assert_eq!(counts.iter().sum::<usize>(), 500);
}

#[test]
fn spawns_stay_inside_the_spawn_radius() {
    let (streamer, centre) = small_town(5);
    let mut config = NpcConfig {
        seed: 5,
        spawn_radius: 45.0,
        ..NpcConfig::default()
    };
    config = config.sanitised();
    let mut spawner = NpcSpawner::new(config);
    for index in 0..200u64 {
        if let Some((_, position)) = spawner.choose_spawn(&streamer, centre, index) {
            let dx = position.x - centre.x;
            let dz = position.z - centre.z;
            assert!(
                (dx * dx + dz * dz).sqrt() <= 45.0 + 1e-3,
                "spawn {position:?} is outside the spawn radius"
            );
        }
    }
}

#[test]
fn choose_spawn_refuses_a_world_with_nothing_loaded() {
    let streamer = streamer(9);
    let mut spawner = spawner();
    assert!(
        spawner.choose_spawn(&streamer, Vec3::ZERO, 0).is_none(),
        "there is nowhere to stand in an empty streamer"
    );
    let (loaded, centre) = small_town(9);
    assert!(
        spawner
            .choose_spawn(&loaded, Vec3::new(f32::NAN, 0.0, 0.0), 1)
            .is_none(),
        "a non-finite centre is refused"
    );
    assert!(spawner.choose_spawn(&loaded, centre, 2).is_some());
}

#[test]
fn kind_for_suits_the_place() {
    let (streamer, centre) = small_town(13);
    let mut town_kinds = [0usize; 5];
    let mut road_kinds = [0usize; 5];
    let mut wild_kinds = [0usize; 5];
    let mut seen_town = 0;
    let mut seen_road = 0;
    let mut seen_wild = 0;
    for index in 0..900u64 {
        // Sample the world around the centre and classify what we find.
        let angle = index as f32 * 2.399_963;
        let radius = 4.0 + (index % 60) as f32;
        let candidate = centre + Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius);
        let Some(ground) = ground_at(&streamer, candidate) else {
            continue;
        };
        let kind = NpcSpawner::kind_for(&streamer, ground, index);
        let slot = NpcKind::ALL
            .iter()
            .position(|candidate| *candidate == kind)
            .expect("a known kind");
        match NpcSpawner::classify(&streamer, ground) {
            SpawnKind::Town | SpawnKind::Interior => {
                town_kinds[slot] += 1;
                seen_town += 1;
            }
            SpawnKind::Road => {
                road_kinds[slot] += 1;
                seen_road += 1;
            }
            SpawnKind::Wilderness => {
                wild_kinds[slot] += 1;
                seen_wild += 1;
            }
        }
    }
    assert!(seen_town > 20 && seen_road > 5 && seen_wild > 20);
    // A town has people, not livestock; open country has animals.
    assert_eq!(
        town_kinds[NpcKind::Animal.index()],
        0,
        "an animal in the middle of town"
    );
    assert_eq!(
        road_kinds[NpcKind::Animal.index()],
        0,
        "an animal walking the road"
    );
    assert!(
        wild_kinds[NpcKind::Animal.index()] * 2 > seen_wild,
        "the wilderness should be mostly animals: {wild_kinds:?}"
    );
    assert!(town_kinds[NpcKind::Villager.index()] > 0);
}

#[test]
fn classify_reports_the_world_it_sees() {
    let (streamer, centre) = small_town(17);
    let Some(water) = deep_water(&streamer) else {
        return;
    };
    assert_eq!(
        NpcSpawner::classify(&streamer, water),
        SpawnKind::Wilderness,
        "water is nobody's home"
    );
    assert_eq!(
        NpcSpawner::classify(&streamer, Vec3::new(f32::NAN, 0.0, 0.0)),
        SpawnKind::Wilderness
    );
    assert_eq!(
        NpcSpawner::classify(&streamer, Vec3::new(1.0e8, 0.0, 1.0e8)),
        SpawnKind::Wilderness,
        "an unloaded position claims the least"
    );
    assert_ne!(
        NpcSpawner::classify(&streamer, centre),
        SpawnKind::Wilderness
    );
}
