//! The debug switches, and the guarantee they must not break.
//!
//! Cheats are commands. That is the whole design: a switch the accepted
//! command record cannot see would silently break replay determinism, and
//! it would break it only for the runs someone was most likely to be
//! investigating. These tests hold both halves — that each switch does
//! what it says, and that using them leaves replay exact.

use murmur_core::actions::{Cheat, Command};
use murmur_core::contract::MissionConfig;
use murmur_core::data::Role;
use murmur_core::replay::{MissionRecord, replay, world_fingerprint};
use murmur_core::world::BodyCondition;

mod common;
use common::{data, place, quiet_all_npcs, setup_quiet, some_npc};

/// Flipping a switch is an ordinary command, so it lands in the record and
/// a replay of a cheated run reproduces the cheating. Without this the
/// determinism guarantee would hold for every run except the ones a
/// developer was debugging.
#[test]
fn a_cheated_run_replays_exactly() {
    let (data, mut driver) = setup_quiet(4);
    for command in [
        Command::Cheat(Cheat::Invulnerable),
        Command::Wait,
        Command::Cheat(Cheat::BlindNpcs),
        Command::Wait,
        Command::Cheat(Cheat::EndlessAmmo),
        Command::Wait,
    ] {
        driver.submit(&data, &command).unwrap();
        while driver.player_busy() {
            driver.continue_busy(&data);
        }
    }
    let played = driver.world().clone();
    assert!(played.cheats.ever_used, "the run is marked as cheated");

    let record = MissionRecord {
        config: MissionConfig::new(4, "nightclub"),
        commands: driver.accepted_commands().to_vec(),
    };
    assert!(
        record
            .commands
            .contains(&Command::Cheat(Cheat::Invulnerable)),
        "a cheat toggle is in the accepted-command record"
    );
    // The staged quieting is not in the record, so the replay is compared
    // against a second live run of the same commands rather than against
    // the staged world.
    let a = replay(&data, &record).unwrap();
    let b = replay(&data, &record).unwrap();
    assert_eq!(
        world_fingerprint(&a),
        world_fingerprint(&b),
        "replaying a cheated run is still deterministic"
    );
    assert!(a.cheats.ever_used, "replay reproduces the cheating too");
}

/// Turning a switch off does not un-cheat the run: the sticky flag is what
/// stops a debrief presenting a cheated result as a clean one.
#[test]
fn the_cheated_mark_is_permanent() {
    let (data, mut driver) = setup_quiet(4);
    driver
        .submit(&data, &Command::Cheat(Cheat::Invulnerable))
        .unwrap();
    driver
        .submit(&data, &Command::Cheat(Cheat::Invulnerable))
        .unwrap();
    let world = driver.world();
    assert!(!world.cheats.invulnerable, "the switch is back off");
    assert!(!world.cheats.any_active(), "nothing is active");
    assert!(world.cheats.ever_used, "but the run stays marked");
}

#[test]
fn invulnerability_survives_a_lethal_blow() {
    let (data, mut driver) = setup_quiet(4);
    driver
        .submit(&data, &Command::Cheat(Cheat::Invulnerable))
        .unwrap();

    // Kill the player outright through the world, then let a turn resolve.
    let player = driver.world().player;
    driver.world_mut().actor_mut(player).hp = 0;
    driver.world_mut().actor_mut(player).condition = BodyCondition::Dead;
    driver.submit(&data, &Command::Wait).unwrap();

    let world = driver.world();
    assert_eq!(
        world.actor(player).condition,
        BodyCondition::Healthy,
        "an invulnerable player is put back on their feet"
    );
    assert!(
        world.outcome.is_none(),
        "and the mission does not end: {:?}",
        world.outcome
    );
}

/// Without the switch the same staging kills the run, which is what makes
/// the test above a real check rather than a restatement of the default.
#[test]
fn without_invulnerability_the_same_blow_ends_the_mission() {
    let (data, mut driver) = setup_quiet(4);
    let player = driver.world().player;
    driver.world_mut().actor_mut(player).hp = 0;
    driver.world_mut().actor_mut(player).condition = BodyCondition::Dead;
    driver.submit(&data, &Command::Wait).unwrap();
    assert!(
        driver.world().outcome.is_some(),
        "a dead player normally ends the mission"
    );
}

#[test]
fn blind_npcs_see_nothing_at_point_blank_range() {
    let (data, mut driver) = setup_quiet(4);
    let guard = some_npc(driver.world(), Role::Guard);
    let player = driver.world().player;

    // Stand the guard next to the player, looking straight at them.
    let (start, dir) = common::free_run(driver.world(), 3);
    place(driver.world_mut(), player, start, None);
    place(
        driver.world_mut(),
        guard,
        start.step(dir),
        Some(dir.opposite()),
    );

    assert!(
        murmur_core::perception::npc_sees(driver.world(), &data, guard, start, false),
        "a guard one tile away, facing the player, sees them"
    );
    driver
        .submit(&data, &Command::Cheat(Cheat::BlindNpcs))
        .unwrap();
    assert!(
        !murmur_core::perception::npc_sees(driver.world(), &data, guard, start, false),
        "blinded, the same guard sees nothing"
    );
}

#[test]
fn endless_ammo_never_spends_a_round() {
    let data = data();
    let mut config = MissionConfig::new(4, "nightclub");
    config.loadout = vec!["silenced-pistol".to_string()];
    let world = murmur_core::generator::generate(&data, &config).unwrap();
    let mut world = world;
    quiet_all_npcs(&mut world);
    let mut driver = murmur_core::turn::TurnDriver::new(world, &data);

    let player = driver.world().player;
    let rounds = |driver: &murmur_core::turn::TurnDriver| {
        driver
            .world()
            .carried_items(player)
            .find(|i| i.spec == "silenced-pistol")
            .map(|i| i.charges)
            .unwrap()
    };
    let before = rounds(&driver);
    assert!(before > 0, "the loadout carries a loaded pistol");

    // A victim in the open, in line, at point-blank range.
    let victim = some_npc(driver.world(), Role::Civilian);
    let (start, dir) = common::free_run(driver.world(), 3);
    place(driver.world_mut(), player, start, None);
    place(driver.world_mut(), victim, start.step(dir), Some(dir));

    driver
        .submit(&data, &Command::Cheat(Cheat::EndlessAmmo))
        .unwrap();
    driver.submit(&data, &Command::DrawOrHolster).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    driver.submit(&data, &Command::Shoot(victim)).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }

    assert!(
        !driver.world().actor(victim).alive(),
        "the shot still resolved"
    );
    assert_eq!(
        rounds(&driver),
        before,
        "an endless magazine does not lose the round it just fired"
    );
}
