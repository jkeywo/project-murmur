//! End-to-end tests for the four non-assassination objectives. Each
//! generates a mission of that kind, drives the player through the intended
//! solution with real commands, and asserts the mission is won only when the
//! objective is complete and the player extracts.

use murmur_core::actions::{Command, RejectReason};
use murmur_core::contract::MissionConfig;
use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::replay::world_fingerprint;
use murmur_core::turn::TurnDriver;
use murmur_core::world::{
    ActorId, BodyCondition, ItemLocation, MissionOutcome, Objective, ObjectiveKind, PlantTarget,
};

mod common;
use common::{data, park_npcs_far, place, quiet_all_npcs, setup_config, stand_beside};

const VENUE: &str = "nightclub";
const SEED: u64 = 1;

fn mission(kind: ObjectiveKind) -> (GameData, TurnDriver) {
    setup_config(MissionConfig::new(SEED, VENUE).with_objective(kind))
}

/// Isolates the objective's cast: parks everyone else out of earshot on the
/// top storey and calms the ones we keep, so staged commands are not
/// disturbed by a patrol or a detail.
fn isolate(driver: &mut TurnDriver, keep: &[ActorId]) {
    park_npcs_far(driver.world_mut(), keep);
    quiet_all_npcs(driver.world_mut());
}

fn extract_and_expect(driver: &mut TurnDriver, data: &GameData, outcome: Option<MissionOutcome>) {
    let player = driver.world().player;
    let exit = driver.world().extraction_tiles[0];
    place(driver.world_mut(), player, exit, None);
    driver.submit(data, &Command::Wait).unwrap();
    assert_eq!(driver.world().outcome, outcome);
}

// --- Steal -----------------------------------------------------------------

#[test]
fn steal_is_won_by_lifting_the_item_and_extracting() {
    let (data, mut driver) = mission(ObjectiveKind::Steal);
    let Objective::Steal { item } = driver.world().objective.clone() else {
        panic!("a steal mission has a steal objective");
    };
    let ItemLocation::CarriedBy(holder) = driver.world().item(item).unwrap().location else {
        panic!("the ledger is carried by the mark");
    };
    let player = driver.world().player;
    isolate(&mut driver, &[player, holder]);

    let spot = stand_beside(driver.world(), driver.world().actor(holder).pos);
    place(driver.world_mut(), player, spot, None);
    // The mark may also carry keys; lift until the ledger is in hand.
    for _ in 0..6 {
        if driver.world().objective.is_complete(driver.world()) {
            break;
        }
        driver.submit(&data, &Command::Pickpocket(holder)).unwrap();
    }
    assert!(
        driver.world().objective.is_complete(driver.world()),
        "lifting the ledger completes the steal"
    );

    extract_and_expect(&mut driver, &data, Some(MissionOutcome::Extracted));
}

#[test]
fn extracting_without_the_item_does_not_win_a_steal() {
    let (data, mut driver) = mission(ObjectiveKind::Steal);
    let player = driver.world().player;
    isolate(&mut driver, &[player]);
    assert!(!driver.world().objective.is_complete(driver.world()));
    extract_and_expect(&mut driver, &data, None);
}

// --- Sabotage --------------------------------------------------------------

#[test]
fn sabotage_is_won_by_spending_the_machine_and_extracting() {
    let (data, mut driver) = mission(ObjectiveKind::Sabotage);
    let Objective::Sabotage { machine } = driver.world().objective.clone() else {
        panic!("a sabotage mission has a sabotage objective");
    };
    let player = driver.world().player;
    isolate(&mut driver, &[player]);

    let machine_pos = driver
        .world()
        .furniture
        .iter()
        .find(|f| f.id == machine)
        .unwrap()
        .pos;
    let spot = stand_beside(driver.world(), machine_pos);
    place(driver.world_mut(), player, spot, None);
    driver.submit(&data, &Command::Interact(machine)).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    assert!(
        driver.world().objective.is_complete(driver.world()),
        "spending the machine completes the sabotage"
    );

    extract_and_expect(&mut driver, &data, Some(MissionOutcome::Extracted));
}

// --- Rescue ----------------------------------------------------------------

#[test]
fn rescue_is_won_by_leading_the_captive_out_and_extracting() {
    let (data, mut driver) = mission(ObjectiveKind::Rescue);
    let Objective::Rescue { person } = driver.world().objective.clone() else {
        panic!("a rescue mission has a rescue objective");
    };
    let player = driver.world().player;
    isolate(&mut driver, &[player, person]);

    // Take the captive onto the leash while adjacent (a real command)...
    let beside = stand_beside(driver.world(), driver.world().actor(person).pos);
    place(driver.world_mut(), player, beside, None);
    driver.submit(&data, &Command::Lead(person)).unwrap();
    assert_eq!(
        driver.world().actor(person).ai.as_ref().unwrap().following,
        Some(player),
        "leading sets the captive on the leash"
    );

    // ...then out through the exits: captive alive on an extraction tile plus
    // the player extracting wins.
    let exits = driver.world().extraction_tiles.clone();
    let captive_exit = *exits.get(1).unwrap_or(&exits[0]);
    place(driver.world_mut(), person, captive_exit, None);
    place(driver.world_mut(), player, exits[0], None);
    driver.submit(&data, &Command::Wait).unwrap();
    assert_eq!(
        driver.world().outcome,
        Some(MissionOutcome::Extracted),
        "an alive captive on an exit plus the player extracting wins the rescue"
    );
}

#[test]
fn a_dead_rescue_subject_loses_the_mission() {
    let (data, mut driver) = mission(ObjectiveKind::Rescue);
    let Objective::Rescue { person } = driver.world().objective.clone() else {
        panic!("a rescue mission has a rescue objective");
    };
    let player = driver.world().player;
    isolate(&mut driver, &[player, person]);

    driver.world_mut().actor_mut(person).condition = BodyCondition::Dead;
    driver.submit(&data, &Command::Wait).unwrap();
    assert_eq!(
        driver.world().outcome,
        Some(MissionOutcome::TargetEscaped),
        "a dead captive can no longer be rescued: the job is lost"
    );
}

// --- Plant -----------------------------------------------------------------

#[test]
fn plant_is_won_by_bugging_the_mark_and_extracting() {
    let (data, mut driver) = mission(ObjectiveKind::Plant);
    let Objective::Plant {
        on: PlantTarget::Person(mark),
        ..
    } = driver.world().objective.clone()
    else {
        panic!("a plant mission plants on a person");
    };
    let player = driver.world().player;
    isolate(&mut driver, &[player, mark]);

    let spot = stand_beside(driver.world(), driver.world().actor(mark).pos);
    place(driver.world_mut(), player, spot, None);
    driver.submit(&data, &Command::Plant(Some(mark))).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    assert!(
        driver.world().objective.is_complete(driver.world()),
        "slipping the bug onto the mark completes the plant"
    );

    extract_and_expect(&mut driver, &data, Some(MissionOutcome::Extracted));
}

#[test]
fn extracting_without_planting_does_not_win() {
    let (data, mut driver) = mission(ObjectiveKind::Plant);
    let player = driver.world().player;
    isolate(&mut driver, &[player]);
    assert!(!driver.world().objective.is_complete(driver.world()));
    extract_and_expect(&mut driver, &data, None);
}

// --- Determinism -----------------------------------------------------------

#[test]
fn a_non_assassinate_objective_is_stable_across_regeneration() {
    let data = data();
    for kind in [
        ObjectiveKind::Steal,
        ObjectiveKind::Sabotage,
        ObjectiveKind::Rescue,
        ObjectiveKind::Plant,
    ] {
        let config = MissionConfig::new(SEED, VENUE).with_objective(kind);
        let a = generate(&data, &config).expect("generates");
        let b = generate(&data, &config).expect("generates");
        assert_eq!(
            a.objective, b.objective,
            "{kind:?} objective is deterministic"
        );
        assert_eq!(
            world_fingerprint(&a),
            world_fingerprint(&b),
            "{kind:?} world is byte-identical across regeneration"
        );
    }
}

/// The player still carries no plantable item unless a plant contract added
/// one, so the plant command is rejected on an ordinary mission.
#[test]
fn plant_is_rejected_without_the_bug_on_a_non_plant_mission() {
    let (data, mut driver) = setup_config(MissionConfig::new(SEED, VENUE));
    assert!(matches!(
        driver.submit(&data, &Command::Plant(None)),
        Err(RejectReason::NothingToPlant)
    ));
}
