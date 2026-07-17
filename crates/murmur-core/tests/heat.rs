//! Police-heat scenarios: observed crimes accumulate mission heat,
//! tiers change the venue's response, and persistent district heat
//! hardens generation without locking an area out.

use murmur_core::actions::Command;
use murmur_core::contract::MissionConfig;
use murmur_core::data::{GameData, Role};
use murmur_core::generator::generate;
use murmur_core::geom::{Dir4, Pos};
use murmur_core::turn::TurnDriver;
use murmur_core::world::{ActorId, Mood, World};

fn setup(seed: u64) -> (GameData, TurnDriver) {
    let data = GameData::embedded().unwrap();
    let world = generate(&data, &MissionConfig::new(seed, "nightclub")).unwrap();
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
        }
    }
}

/// Parks every NPC except `keep` far away on the other storey so nothing
/// is seen or heard.
fn park_npcs_far(world: &mut World, keep: &[ActorId]) {
    let ids: Vec<ActorId> = world
        .actors
        .iter()
        .filter(|a| !a.is_player() && !keep.contains(&a.id))
        .map(|a| a.id)
        .collect();
    // The upper service corridor's west end is a quiet, distant spot.
    for (x, id) in (2i16..).zip(ids) {
        world.actor_mut(id).pos = Pos::new(1, x.min(30), 1);
        world.actor_mut(id).facing = Some(Dir4::North);
    }
}

#[test]
fn heard_gunshots_accumulate_heat_and_unheard_ones_do_not() {
    let (data, mut driver) = setup(21);
    quiet_all_npcs(driver.world_mut());
    let player = driver.world().player;
    let target = driver.world().target;

    // Nobody anywhere near: a shot adds no heat.
    park_npcs_far(driver.world_mut(), &[]);
    let spot = Pos::new(0, 8, 14); // main corridor, ground floor
    driver.world_mut().actor_mut(player).pos = spot;
    driver.world_mut().actor_mut(target).pos = spot.step(Dir4::East).step(Dir4::East);
    driver.world_mut().actor_mut(target).pos.floor = 0;
    driver.submit(&data, &Command::DrawOrHolster).unwrap();
    driver.submit(&data, &Command::Shoot(target)).unwrap();
    assert_eq!(
        driver.world().mission_heat,
        0,
        "an unobserved shot is no crime anyone knows about"
    );

    // A guard within earshot on the second shot's turn: heat lands.
    let (data2, mut driver2) = setup(23);
    quiet_all_npcs(driver2.world_mut());
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
    let spot2 = Pos::new(0, 8, 14);
    driver2.world_mut().actor_mut(player2).pos = spot2;
    driver2.world_mut().actor_mut(target2).pos = spot2.step(Dir4::East).step(Dir4::East);
    driver2.world_mut().actor_mut(guard).pos = Pos::new(0, 14, 14);
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
    quiet_all_npcs(driver.world_mut());
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

    let spot = Pos::new(0, 8, 14);
    driver.world_mut().actor_mut(player).pos = spot;
    driver.world_mut().actor_mut(target).pos = spot.step(Dir4::East).step(Dir4::East);
    driver.world_mut().actor_mut(guard).pos = Pos::new(0, 14, 14);
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
