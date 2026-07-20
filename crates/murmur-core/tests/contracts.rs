//! Constrained-contract scenarios: every constraint generates with a
//! certified compliant route, is tracked in-mission, and resolves the
//! contract unclean (never ending the mission) when broken.

use murmur_core::actions::Command;
use murmur_core::contract::{Constraint, MissionConfig};
use murmur_core::data::{GameData, Role, Zone};
use murmur_core::generator::generate;
use murmur_core::geom::{Dir4, Pos};
use murmur_core::map::TileKind;
use murmur_core::turn::TurnDriver;

mod common;
use common::{free_run, place, quiet_all_npcs, setup_config, some_npc};

fn setup(seed: u64, constraint: Constraint) -> (GameData, TurnDriver) {
    setup_config(MissionConfig::new(seed, "nightclub").with_constraint(constraint))
}

#[test]
fn private_kill_condition_text_names_the_actual_offices() {
    let data = GameData::embedded().unwrap();
    let text = Constraint::PrivateKill.describe(&data, "nightclub");
    // The condition names the real personal-tier rooms, not a vague
    // "away from the crowd" the player has to decode.
    assert!(text.contains("manager's office"), "{text}");
    assert!(text.contains("security office"), "{text}");
    // The single-office case reads naturally in the warehouse.
    let short = Constraint::PrivateKill.short(&data, "warehouse");
    assert!(short.contains("foreman"), "{short}");
}

#[test]
fn specific_exit_condition_uses_the_display_name_not_the_raw_id() {
    let data = GameData::embedded().unwrap();
    let c = Constraint::SpecificExit {
        room_template: "loading-bay".to_string(),
    };
    let text = c.describe(&data, "nightclub");
    assert!(text.contains("loading bay"), "{text}");
    assert!(!text.contains("loading-bay"), "raw id leaked: {text}");
}

#[test]
fn every_constraint_generates_with_a_compliant_route_proof() {
    let data = GameData::embedded().unwrap();
    let constraints = [
        Constraint::NoFirearms,
        Constraint::NoCivilianCasualties,
        Constraint::NoBodiesFound,
        Constraint::PrivateKill,
        Constraint::SpecificExit {
            room_template: "loading-bay".to_string(),
        },
    ];
    for constraint in constraints {
        for seed in 0..6u64 {
            let config = MissionConfig::new(seed, "nightclub").with_constraint(constraint.clone());
            let world = generate(&data, &config)
                .unwrap_or_else(|e| panic!("{constraint:?} seed {seed}: {e}"));
            let proof = world
                .routes
                .constraint_proof
                .as_ref()
                .unwrap_or_else(|| panic!("{constraint:?}: no constraint proof"));
            assert!(!proof.steps.is_empty());
            assert_eq!(world.constraint, Some(constraint.clone()));
            assert!(world.constraint_breach.is_none());
        }
    }
}

#[test]
fn private_kill_contracts_put_the_target_in_personal_space() {
    let data = GameData::embedded().unwrap();
    for seed in 0..8u64 {
        let config = MissionConfig::new(seed, "nightclub").with_constraint(Constraint::PrivateKill);
        let world = generate(&data, &config).unwrap();
        let target = world.actor(world.target);
        let visits_personal = target.ai.as_ref().is_some_and(|ai| {
            ai.routine.iter().any(|step| {
                world
                    .room_at(step.pos)
                    .is_some_and(|r| r.zone == Zone::Personal)
            })
        });
        assert!(
            visits_personal,
            "seed {seed}: private-kill target never visits personal space"
        );
    }
}

#[test]
fn firing_the_pistol_breaches_a_no_firearms_contract() {
    let (data, mut driver) = setup(11, Constraint::NoFirearms);
    quiet_all_npcs(driver.world_mut());
    let target = driver.world().target;
    let player = driver.world().player;
    let (start, dir) = free_run(driver.world(), 3);
    place(driver.world_mut(), player, start, None);
    place(
        driver.world_mut(),
        target,
        start.step(dir).step(dir),
        Some(dir),
    );

    driver.submit(&data, &Command::DrawOrHolster).unwrap();
    driver.submit(&data, &Command::Shoot(target)).unwrap();
    assert!(!driver.world().actor(target).alive());
    assert!(
        driver.world().constraint_breach.is_some(),
        "gunfire must breach the contract"
    );
    assert!(
        driver.world().outcome.is_none(),
        "a breach never ends the mission"
    );
}

#[test]
fn collateral_death_breaches_no_civilian_casualties_but_the_target_does_not() {
    let (data, mut driver) = setup(11, Constraint::NoCivilianCasualties);
    quiet_all_npcs(driver.world_mut());
    let player = driver.world().player;
    let target = driver.world().target;
    let civilian = some_npc(driver.world(), Role::Civilian);
    let (start, dir) = free_run(driver.world(), 3);

    // Killing the target is the job: no breach.
    place(driver.world_mut(), player, start, None);
    place(driver.world_mut(), target, start.step(dir), Some(dir));
    driver.submit(&data, &Command::Garrote(target)).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    assert!(driver.world().constraint_breach.is_none());

    // A bystander is collateral: breach.
    place(driver.world_mut(), civilian, start.step(dir), Some(dir));
    driver.submit(&data, &Command::Garrote(civilian)).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    assert!(driver.world().constraint_breach.is_some());
}

#[test]
fn killing_the_target_outside_personal_space_breaches_private_kill() {
    let (data, mut driver) = setup(13, Constraint::PrivateKill);
    quiet_all_npcs(driver.world_mut());
    let player = driver.world().player;
    let target = driver.world().target;
    let (start, dir) = free_run(driver.world(), 3);
    // The free run is public or staff space, never personal.
    place(driver.world_mut(), player, start, None);
    place(driver.world_mut(), target, start.step(dir), Some(dir));
    driver.submit(&data, &Command::Garrote(target)).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    assert!(driver.world().constraint_breach.is_some());
}

#[test]
fn killing_the_target_in_personal_space_satisfies_private_kill() {
    let (data, mut driver) = setup(13, Constraint::PrivateKill);
    quiet_all_npcs(driver.world_mut());
    let player = driver.world().player;
    let target = driver.world().target;

    // Stage inside a personal-tier room with space for both actors.
    let world = driver.world();
    let (spot, dir) = world
        .rooms
        .iter()
        .filter(|r| r.zone == Zone::Personal && r.bounds.w >= 3)
        .find_map(|r| {
            let a = Pos::new(r.floor, r.bounds.x, r.bounds.y);
            let b = a.step(Dir4::East);
            (world.furniture_at(a).is_none()
                && world.furniture_at(b).is_none()
                && world.standing_actor_at(a).is_none()
                && world.standing_actor_at(b).is_none())
            .then_some((a, Dir4::East))
        })
        .expect("a personal room with two free tiles");
    place(driver.world_mut(), player, spot, None);
    place(driver.world_mut(), target, spot.step(dir), Some(dir));
    driver.submit(&data, &Command::Garrote(target)).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    assert!(!driver.world().actor(target).alive());
    assert!(
        driver.world().constraint_breach.is_none(),
        "a private kill in personal space keeps the contract clean"
    );
}

#[test]
fn extracting_by_the_wrong_exit_breaches_specific_exit_but_still_extracts() {
    let (data, mut driver) = setup(
        17,
        Constraint::SpecificExit {
            room_template: "loading-bay".to_string(),
        },
    );
    quiet_all_npcs(driver.world_mut());
    let player = driver.world().player;
    let target = driver.world().target;

    // Kill the target somewhere quiet, then step onto the entrance exit.
    let (start, dir) = free_run(driver.world(), 3);
    place(driver.world_mut(), player, start, None);
    place(driver.world_mut(), target, start.step(dir), Some(dir));
    driver.submit(&data, &Command::Garrote(target)).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }

    let entrance_exit = driver.world().extraction_tiles[0];
    let world = driver.world_mut();
    let approach = entrance_exit.step(Dir4::North);
    let approach = if matches!(world.map.tile(approach), TileKind::Floor) {
        approach
    } else {
        entrance_exit.step(Dir4::South)
    };
    place(world, player, approach, None);
    let step_dir = Dir4::ALL
        .into_iter()
        .find(|d| approach.step(*d) == entrance_exit)
        .unwrap();
    driver.submit(&data, &Command::Move(step_dir)).unwrap();

    let world = driver.world();
    assert_eq!(
        world.outcome,
        Some(murmur_core::world::MissionOutcome::Extracted),
        "the wrong exit still extracts"
    );
    assert!(
        world.constraint_breach.is_some(),
        "but the contract resolves breached"
    );
}

#[test]
fn a_discovered_body_breaches_no_bodies_found() {
    let (data, mut driver) = setup(19, Constraint::NoBodiesFound);
    quiet_all_npcs(driver.world_mut());
    let player = driver.world().player;
    let civilian = some_npc(driver.world(), Role::Civilian);
    let guard = some_npc(driver.world(), Role::Guard);
    let (start, dir) = free_run(driver.world(), 4);

    // Kill a civilian, then let a guard walk in on the body.
    place(driver.world_mut(), player, start, None);
    place(driver.world_mut(), civilian, start.step(dir), Some(dir));
    driver.submit(&data, &Command::Garrote(civilian)).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    assert!(driver.world().constraint_breach.is_none());

    // Stage the guard two tiles away, facing the body.
    let facing = Dir4::ALL
        .into_iter()
        .find(|d| start.step(dir).step(*d).step(*d) == start.step(dir).step(dir).step(dir))
        .map(|_| dir.opposite())
        .unwrap_or(dir.opposite());
    place(
        driver.world_mut(),
        guard,
        start.step(dir).step(dir).step(dir),
        Some(facing),
    );
    driver.submit(&data, &Command::Wait).unwrap();
    assert!(
        driver.world().constraint_breach.is_some(),
        "a guard seeing the body breaches the contract"
    );
}
