//! Police-heat scenarios: observed crimes accumulate mission heat,
//! tiers change the venue's response, and persistent district heat
//! hardens generation without locking an area out.

use murmur_core::actions::Command;
use murmur_core::contract::MissionConfig;
use murmur_core::data::{GameData, Role};
use murmur_core::generator::generate;
use murmur_core::geom::{Dir4, Pos};
use murmur_core::map::TileKind;
use murmur_core::turn::TurnDriver;
use murmur_core::world::{ActorId, Mood, World};

/// A driver whose NPCs are already inert. The quieting must happen before
/// the driver is built: construction primes each NPC's first action, so a
/// world calmed afterwards still has one turn of escorting and walking
/// queued up.
fn setup(seed: u64) -> (GameData, TurnDriver) {
    let data = GameData::embedded().unwrap();
    let mut world = generate(&data, &MissionConfig::new(seed, "nightclub")).unwrap();
    quiet_all_npcs(&mut world);
    (data.clone(), TurnDriver::new(world, &data))
}

fn quiet_all_npcs(world: &mut World) {
    let ids: Vec<ActorId> = world
        .actors
        .iter()
        .filter(|a| !a.is_player())
        .map(|a| a.id)
        .collect();
    for id in ids {
        if let Some(ai) = world.actor_mut(id).ai.as_mut() {
            ai.routine.clear();
            ai.mood = Mood::Relaxed;
            ai.suspicion = 0;
            ai.focus = None;
            // Including standing assignments: a bodyguard would walk off
            // towards its principal instead of standing where the scenario
            // put it.
            ai.detail = None;
        }
    }
}

/// Parks every NPC except `keep` far away on the topmost storey so
/// nothing is seen or heard. Derived from the map rather than pinned to
/// coordinates, so re-shaping a venue cannot silently park someone in
/// earshot and turn a heat assertion into a false pass.
fn park_npcs_far(world: &mut World, keep: &[ActorId]) {
    let ids: Vec<ActorId> = world
        .actors
        .iter()
        .filter(|a| !a.is_player() && !keep.contains(&a.id))
        .map(|a| a.id)
        .collect();
    let top = world.map.floor_count() - 1;
    let spots: Vec<Pos> = world
        .map
        .floor_positions(top)
        .filter(|p| matches!(world.map.tile(*p), TileKind::Floor))
        .collect();
    assert!(
        spots.len() >= ids.len(),
        "the top storey must hold every parked NPC"
    );
    for (pos, id) in spots.into_iter().zip(ids) {
        world.actor_mut(id).pos = pos;
        world.actor_mut(id).facing = Some(Dir4::North);
    }
}

/// A floor tile close enough to `spot` to witness what happens there,
/// but off the firing line itself.
fn witness_spot(world: &World, spot: Pos) -> Pos {
    let line = [
        spot,
        spot.step(Dir4::East),
        spot.step(Dir4::East).step(Dir4::East),
    ];
    for radius in 1..6i16 {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let pos = Pos::new(spot.floor, spot.x + dx, spot.y + dy);
                if matches!(world.map.tile(pos), TileKind::Floor) && !line.contains(&pos) {
                    return pos;
                }
            }
        }
    }
    panic!("no witness position near {spot:?}");
}

/// A ground-floor tile with two clear floor tiles to its east: room for a
/// shooter, a victim, and an unobstructed line between them.
fn firing_line(world: &World) -> Pos {
    world
        .map
        .floor_positions(0)
        .find(|p| {
            let clear = |q: Pos| matches!(world.map.tile(q), TileKind::Floor);
            clear(*p) && clear(p.step(Dir4::East)) && clear(p.step(Dir4::East).step(Dir4::East))
        })
        .expect("the ground floor has somewhere to stand three abreast")
}

#[test]
fn heard_gunshots_accumulate_heat_and_unheard_ones_do_not() {
    let (data, mut driver) = setup(21);
    let player = driver.world().player;
    let target = driver.world().target;

    // Nobody anywhere near: a shot adds no heat.
    park_npcs_far(driver.world_mut(), &[]);
    let spot = firing_line(driver.world());
    driver.world_mut().actor_mut(player).pos = spot;
    driver.world_mut().actor_mut(target).pos = spot.step(Dir4::East).step(Dir4::East);
    driver.submit(&data, &Command::DrawOrHolster).unwrap();
    driver.submit(&data, &Command::Shoot(target)).unwrap();
    assert_eq!(
        driver.world().mission_heat,
        0,
        "an unobserved shot is no crime anyone knows about"
    );

    // A guard within earshot on the second shot's turn: heat lands.
    let (data2, mut driver2) = setup(23);
    let player2 = driver2.world().player;
    let target2 = driver2.world().target;
    let guard = driver2
        .world()
        .actors
        .iter()
        .find(|a| a.role == Some(Role::Guard))
        .unwrap()
        .id;
    park_npcs_far(driver2.world_mut(), &[guard]);
    let spot2 = firing_line(driver2.world());
    let post = witness_spot(driver2.world(), spot2);
    driver2.world_mut().actor_mut(player2).pos = spot2;
    driver2.world_mut().actor_mut(target2).pos = spot2.step(Dir4::East).step(Dir4::East);
    driver2.world_mut().actor_mut(guard).pos = post;
    driver2.submit(&data2, &Command::DrawOrHolster).unwrap();
    driver2.submit(&data2, &Command::Shoot(target2)).unwrap();
    assert!(
        driver2.world().mission_heat >= data2.tuning.heat_gunshot,
        "a heard shot is a reported crime: heat {}",
        driver2.world().mission_heat
    );
}

#[test]
fn tier_two_heat_brings_reinforcements_through_the_door() {
    let (data, mut driver) = setup(25);
    let player = driver.world().player;
    let target = driver.world().target;
    let guard = driver
        .world()
        .actors
        .iter()
        .find(|a| a.role == Some(Role::Guard))
        .unwrap()
        .id;
    park_npcs_far(driver.world_mut(), &[guard]);

    let before = driver.world().actors.len();
    driver.world_mut().mission_heat = data.tuning.heat_tier2 - 1;

    let spot = firing_line(driver.world());
    driver.world_mut().actor_mut(player).pos = spot;
    driver.world_mut().actor_mut(target).pos = spot.step(Dir4::East).step(Dir4::East);
    let post = witness_spot(driver.world(), spot);
    driver.world_mut().actor_mut(guard).pos = post;
    driver.submit(&data, &Command::DrawOrHolster).unwrap();
    driver.submit(&data, &Command::Shoot(target)).unwrap();

    let world = driver.world();
    assert!(world.heat_tier >= 2, "tier two reached");
    assert_eq!(
        world.actors.len(),
        before + usize::from(data.tuning.heat_reinforcements),
        "backup guards spawned at the entrance"
    );
}

#[test]
fn district_heat_hardens_generation_with_a_cap() {
    let data = GameData::embedded().unwrap();
    let guards = |heat: u8| {
        let world = generate(&data, &MissionConfig::new(31, "nightclub").with_heat(heat)).unwrap();
        world
            .actors
            .iter()
            .filter(|a| a.role == Some(Role::Guard))
            .count()
    };
    let base = guards(0);
    assert_eq!(guards(3), base + 3, "each heat point adds a guard");
    assert_eq!(
        guards(9),
        base + usize::from(data.tuning.heat_extra_guard_cap),
        "the cap keeps hot districts playable"
    );
}
