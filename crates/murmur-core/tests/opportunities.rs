//! Opportunity machine scenarios: placement follows the improvement
//! rules, and every effect changes routes concretely — lights die,
//! crowds evacuate, keys open everything, and the rigged hoist kills
//! deniably.

use murmur_core::actions::Command;
use murmur_core::contract::{Constraint, MissionConfig};
use murmur_core::data::{GameData, Lighting, OpportunityEffect, Role};
use murmur_core::generator::generate;
use murmur_core::geom::{Dir4, Pos};
use murmur_core::map::TileKind;
use murmur_core::turn::TurnDriver;
use murmur_core::world::{ActorId, FurnitureKind, Mood, World};

fn data() -> GameData {
    GameData::embedded().unwrap()
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

/// A free tile adjacent to `pos`, for standing beside machines.
fn stand_beside(world: &World, pos: Pos) -> Pos {
    Dir4::ALL
        .into_iter()
        .map(|d| pos.step(d))
        .find(|p| {
            matches!(world.map.tile(*p), TileKind::Floor)
                && world.standing_actor_at(*p).is_none()
                && world.furniture_at(*p).is_none()
        })
        .expect("machine has a free adjacent tile")
}

/// Parks every NPC but `keep` on the topmost storey. The hoist test is
/// about the crate and the tile under it, so nobody else may be close
/// enough to walk into the target and displace it off the drop tile
/// while the player is busy rigging the machine.
fn park_all_but(world: &mut World, keep: ActorId) {
    let ids: Vec<ActorId> = world
        .actors
        .iter()
        .filter(|a| !a.is_player() && a.id != keep)
        .map(|a| a.id)
        .collect();
    let mut spare: Vec<Pos> = world
        .map
        .floor_positions(world.map.floor_count() - 1)
        .filter(|p| matches!(world.map.tile(*p), TileKind::Floor))
        .collect();
    for id in ids {
        world.actor_mut(id).pos = spare.pop().expect("room on the top storey");
    }
}

fn find_machine(
    world: &World,
    data: &GameData,
    predicate: impl Fn(&OpportunityEffect) -> bool,
) -> Option<murmur_core::world::FurnitureId> {
    world
        .furniture
        .iter()
        .find(|f| {
            f.kind == FurnitureKind::Machine
                && f.machine
                    .as_deref()
                    .and_then(|id| data.opportunity(id))
                    .is_some_and(|s| predicate(&s.effect))
        })
        .map(|f| f.id)
}

#[test]
fn machines_are_placed_by_the_improvement_rules() {
    let data = data();
    let mut saw_hoist = false;
    let mut saw_alarm = false;
    for seed in 0..12u64 {
        let world = generate(&data, &MissionConfig::new(seed, "nightclub")).unwrap();
        for furniture in &world.furniture {
            let Some(spec) = furniture
                .machine
                .as_deref()
                .and_then(|s| data.opportunity(s))
            else {
                continue;
            };
            // Machines only stand in rooms of their allowed zones.
            let room = world.room_at(furniture.pos).expect("machine is in a room");
            assert!(
                spec.zones.contains(&room.zone),
                "seed {seed}: {} in a {:?} room",
                spec.id,
                room.zone
            );
            match &spec.effect {
                OpportunityEffect::AccidentDrop => {
                    saw_hoist = true;
                    // The drop tile is a stop on the target's schedule.
                    let drop = furniture.drop_tile.expect("hoist has a drop tile");
                    let target = world.actor(world.target);
                    let on_schedule = target.ai.as_ref().is_some_and(|ai| {
                        ai.routine.iter().any(|s| s.pos == drop) || target.pos == drop
                    });
                    assert!(on_schedule, "seed {seed}: hoist above nobody's path");
                }
                OpportunityEffect::Evacuate => saw_alarm = true,
                _ => {}
            }
        }
        // The uniform store never duplicates an existing staff wardrobe:
        // at most one wardrobe source per disguise from placement.
        let briefing_mentions = world.facts.opportunities.len();
        let machine_count = world
            .furniture
            .iter()
            .filter(|f| f.machine.is_some())
            .count();
        assert_eq!(briefing_mentions, machine_count, "seed {seed}");
    }
    assert!(saw_hoist, "the hoist should place on most seeds");
    assert!(saw_alarm, "the alarm should place on most seeds");
}

#[test]
fn the_fuse_box_cuts_the_lights_on_its_storey() {
    let data = data();
    for seed in 0..20u64 {
        let world = generate(&data, &MissionConfig::new(seed, "nightclub")).unwrap();
        let Some(id) = find_machine(&world, &data, |e| matches!(e, OpportunityEffect::CutLights))
        else {
            continue;
        };
        let mut driver = TurnDriver::new(world, &data);
        quiet_all_npcs(driver.world_mut());
        let player = driver.world().player;
        let machine_pos = driver
            .world()
            .furniture
            .iter()
            .find(|f| f.id == id)
            .unwrap()
            .pos;
        let stand = stand_beside(driver.world(), machine_pos);
        driver.world_mut().actor_mut(player).pos = stand;
        driver.submit(&data, &Command::Interact(id)).unwrap();
        while driver.player_busy() {
            driver.continue_busy(&data);
        }
        let world = driver.world();
        assert!(
            world
                .rooms
                .iter()
                .filter(|r| r.floor == machine_pos.floor)
                .all(|r| r.lighting == Lighting::Dim),
            "seed {seed}: every room on the storey goes dim"
        );
        assert!(
            world.furniture.iter().find(|f| f.id == id).unwrap().used,
            "one-shot"
        );
        return;
    }
    panic!("no seed placed a fuse box");
}

#[test]
fn the_rigged_hoist_kills_deniably_under_any_constraint() {
    let data = data();
    for seed in 0..20u64 {
        // Private-kill is the harshest test: an accident in public space
        // still keeps the contract clean, because accidents are deniable.
        let config = MissionConfig::new(seed, "nightclub").with_constraint(Constraint::PrivateKill);
        let Ok(world) = generate(&data, &config) else {
            continue;
        };
        let Some(id) = find_machine(&world, &data, |e| {
            matches!(e, OpportunityEffect::AccidentDrop)
        }) else {
            continue;
        };
        let mut driver = TurnDriver::new(world, &data);
        quiet_all_npcs(driver.world_mut());
        let player = driver.world().player;
        let target = driver.world().target;
        let (machine_pos, drop_tile) = {
            let f = driver
                .world()
                .furniture
                .iter()
                .find(|f| f.id == id)
                .unwrap();
            (f.pos, f.drop_tile.unwrap())
        };
        let stand = stand_beside(driver.world(), machine_pos);
        driver.world_mut().actor_mut(player).pos = stand;
        driver.world_mut().actor_mut(target).pos = drop_tile;
        park_all_but(driver.world_mut(), target);

        driver.submit(&data, &Command::Interact(id)).unwrap();
        while driver.player_busy() {
            driver.continue_busy(&data);
        }
        let world = driver.world();
        assert!(!world.actor(target).alive(), "the crate crushes the target");
        assert!(
            !world.actor(target).killed_by_player,
            "an accident is not attributed to the player"
        );
        assert!(
            world.constraint_breach.is_none(),
            "accidents are deniable: no constraint breach"
        );
        return;
    }
    panic!("no seed placed a usable hoist under a private-kill contract");
}

#[test]
fn the_fire_alarm_empties_the_club() {
    let data = data();
    for seed in 0..20u64 {
        let world = generate(&data, &MissionConfig::new(seed, "nightclub")).unwrap();
        let Some(id) = find_machine(&world, &data, |e| matches!(e, OpportunityEffect::Evacuate))
        else {
            continue;
        };
        let mut driver = TurnDriver::new(world, &data);
        quiet_all_npcs(driver.world_mut());
        let player = driver.world().player;
        let machine_pos = driver
            .world()
            .furniture
            .iter()
            .find(|f| f.id == id)
            .unwrap()
            .pos;
        let stand = stand_beside(driver.world(), machine_pos);
        driver.world_mut().actor_mut(player).pos = stand;
        driver.submit(&data, &Command::Interact(id)).unwrap();
        while driver.player_busy() {
            driver.continue_busy(&data);
        }
        let world = driver.world();
        let fleeing = world
            .actors
            .iter()
            .filter(|a| {
                !a.is_player()
                    && a.alive()
                    && a.role != Some(Role::Guard)
                    && a.ai.as_ref().is_some_and(|ai| ai.mood == Mood::Fleeing)
            })
            .count();
        assert!(fleeing >= 5, "seed {seed}: the club empties ({fleeing})");
        return;
    }
    panic!("no seed placed a fire alarm");
}

#[test]
fn the_key_cache_yields_a_master_key_that_opens_everything() {
    let data = data();
    for seed in 0..20u64 {
        let world = generate(&data, &MissionConfig::new(seed, "nightclub")).unwrap();
        let Some(id) = find_machine(&world, &data, |e| {
            matches!(e, OpportunityEffect::PlaceKey { .. })
        }) else {
            continue;
        };
        let mut driver = TurnDriver::new(world, &data);
        quiet_all_npcs(driver.world_mut());
        let player = driver.world().player;
        let machine_pos = driver
            .world()
            .furniture
            .iter()
            .find(|f| f.id == id)
            .unwrap()
            .pos;
        let stand = stand_beside(driver.world(), machine_pos);
        driver.world_mut().actor_mut(player).pos = stand;
        driver.submit(&data, &Command::Interact(id)).unwrap();
        while driver.player_busy() {
            driver.continue_busy(&data);
        }
        let world = driver.world();
        assert!(
            world
                .carried_items(player)
                .any(|i| i.spec == "service-master-key")
        );
        // Every locked door in the venue now yields.
        for (index, door) in world.doors.iter().enumerate() {
            if door.locked_by.is_some() {
                assert!(
                    murmur_core::access::can_pass_door(
                        world,
                        &data,
                        player,
                        murmur_core::map::DoorId(index as u16)
                    ),
                    "seed {seed}: master key opens door {index}"
                );
            }
        }
        return;
    }
    panic!("no seed placed a key cache");
}

#[test]
fn a_weaponless_loadout_generates_when_a_hoist_route_exists() {
    let data = data();
    // No weapon at all: only the rigged accident can certify the
    // loadout proof, so generation succeeds exactly when a hoist lands
    // on the target's path.
    let mut succeeded = false;
    for seed in 0..30u64 {
        let config =
            MissionConfig::new(seed, "nightclub").with_loadout(vec!["lockpicks".to_string()]);
        if let Ok(world) = generate(&data, &config) {
            succeeded = true;
            let proof = world.routes.loadout_proof.as_ref().unwrap();
            assert!(
                proof.steps.iter().any(|s| s.contains("rigged accident")),
                "the weaponless proof must lean on the accident: {:?}",
                proof.steps
            );
            break;
        }
    }
    assert!(
        succeeded,
        "some seed should certify a weaponless run through the hoist"
    );
}
