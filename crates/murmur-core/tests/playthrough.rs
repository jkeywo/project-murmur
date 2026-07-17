//! Full-mission playthroughs and deterministic replay.
//!
//! A scripted assassin drives the game exactly the way any controller
//! does — one command per turn through the turn driver — proving the
//! mission is playable start to finish: infiltrate, locate the target,
//! kill, and extract. The accepted-command record then replays to a
//! bit-identical world.

use murmur_core::actions::Command;
use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::geom::Dir4;
use murmur_core::path::first_step_towards;
use murmur_core::replay::{MissionRecord, replay, world_fingerprint};
use murmur_core::turn::TurnDriver;
use murmur_core::world::{Hands, MissionOutcome, World};

/// One scripted decision: what the assassin submits this turn. The script
/// is deliberately careful about the drawn pistol: it is visible, illegal
/// equipment, so it comes out only close to the target and goes away right
/// after the kill.
fn decide(world: &World, data: &GameData) -> Command {
    let player = world.player;
    let target = world.target;
    let drawn = matches!(world.actor(player).hands, Hands::Drawn(_));

    if world.actor(target).alive() {
        let distance = world
            .actor(player)
            .pos
            .chebyshev(world.actor(target).pos)
            .unwrap_or(i16::MAX);
        let close = distance <= data.tuning.pistol_range;
        if close && !drawn {
            return Command::DrawOrHolster;
        }
        if !close && drawn {
            return Command::DrawOrHolster;
        }
        if let Some(dir) = first_step_towards(world, data, player, world.actor(target).pos) {
            return Command::Move(dir);
        }
        return Command::Wait;
    }

    // Target down: pocket the pistol and run for the nearest exit.
    if drawn {
        return Command::DrawOrHolster;
    }
    let mut exits = world.extraction_tiles.clone();
    let pos = world.actor(player).pos;
    exits.sort_by_key(|e| e.chebyshev(pos).map(i32::from).unwrap_or(i32::MAX / 2));
    for exit in exits {
        if pos == exit {
            return Command::Wait;
        }
        if let Some(dir) = first_step_towards(world, data, player, exit) {
            return Command::Move(dir);
        }
    }
    Command::Wait
}

/// Plays a full mission with the scripted assassin. Returns the record and
/// the final world.
fn run_mission(data: &GameData, seed: u64, max_turns: u32) -> (MissionRecord, World) {
    let world = generate(
        data,
        &murmur_core::contract::MissionConfig::new(seed, "nightclub"),
    )
    .unwrap();
    let mut driver = TurnDriver::new(world, data);

    while !driver.mission_over() && driver.world().turn < max_turns {
        if driver.player_busy() {
            driver.continue_busy(data);
            continue;
        }
        let mut command = decide(driver.world(), data);
        // A shot beats a step whenever the translator accepts it.
        if driver.world().actor(driver.world().target).alive()
            && matches!(driver.world().player_actor().hands, Hands::Drawn(_))
        {
            let shot = Command::Shoot(driver.world().target);
            if driver.submit(data, &shot).is_ok() {
                continue;
            }
        }
        if driver.submit(data, &command).is_err() {
            // Blocked (usually a crowd): sidestep deterministically.
            let mut accepted = false;
            for dir in Dir4::ALL {
                command = Command::Move(dir);
                if driver.submit(data, &command).is_ok() {
                    accepted = true;
                    break;
                }
            }
            if !accepted {
                driver.submit(data, &Command::Wait).unwrap();
            }
        }
    }

    let record = MissionRecord {
        config: murmur_core::contract::MissionConfig::new(seed, "nightclub"),
        commands: driver.accepted_commands().to_vec(),
    };
    (record, driver.into_world())
}

#[test]
fn missions_are_winnable_start_to_finish() {
    let data = GameData::embedded().unwrap();
    let mut outcomes = Vec::new();
    for seed in 0..10u64 {
        let (record, world) = run_mission(&data, seed, 600);
        assert!(
            !record.commands.is_empty(),
            "seed {seed}: the assassin submitted commands"
        );
        outcomes.push((seed, world.outcome.clone(), world.turn));
    }
    let extracted = outcomes
        .iter()
        .filter(|(_, o, _)| *o == Some(MissionOutcome::Extracted))
        .count();
    let ended = outcomes.iter().filter(|(_, o, _)| o.is_some()).count();
    assert!(
        extracted >= 3,
        "several seeds should be winnable by the simple assassin: {outcomes:?}"
    );
    assert!(
        ended >= 5,
        "most missions should reach an outcome: {outcomes:?}"
    );
}

#[test]
fn replaying_accepted_commands_reproduces_the_result() {
    let data = GameData::embedded().unwrap();
    // Use the first seed the assassin wins.
    let mut winning: Option<(MissionRecord, World)> = None;
    for seed in 0..10u64 {
        let (record, world) = run_mission(&data, seed, 600);
        if world.outcome == Some(MissionOutcome::Extracted) {
            winning = Some((record, world));
            break;
        }
    }
    let (record, live_world) = winning.expect("at least one winnable seed");

    let replayed = replay(&data, &record).expect("replay accepts every recorded command");
    assert_eq!(replayed.outcome, live_world.outcome);
    assert_eq!(replayed.turn, live_world.turn);
    assert_eq!(
        world_fingerprint(&replayed),
        world_fingerprint(&live_world),
        "replay must reproduce the identical world, turn by turn"
    );
}

#[test]
fn replay_is_stable_across_runs() {
    let data = GameData::embedded().unwrap();
    let (record, _) = run_mission(&data, 4, 300);
    let a = replay(&data, &record).unwrap();
    let b = replay(&data, &record).unwrap();
    assert_eq!(world_fingerprint(&a), world_fingerprint(&b));
}

#[test]
fn mission_records_round_trip_through_ron() {
    let data = GameData::embedded().unwrap();
    let (record, _) = run_mission(&data, 7, 200);
    let text = ron::to_string(&record).unwrap();
    let parsed: MissionRecord = ron::from_str(&text).unwrap();
    assert_eq!(parsed, record);
}
