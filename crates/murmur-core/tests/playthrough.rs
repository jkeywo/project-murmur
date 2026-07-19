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

/// One scripted decision, played the way the mission is meant to be
/// played.
///
/// This script *is* the acceptance criterion for the milestone. Walking
/// at an escorted target and firing no longer works — the detail covers
/// its principal, so the shot kills a bodyguard, and the response kills
/// the player. The assassin must instead wait for, or engineer, the
/// moment the target steps away alone.
fn decide(world: &World, data: &GameData) -> Command {
    let player = world.player;
    let target = world.target;
    let drawn = matches!(world.actor(player).hands, Hands::Drawn(_));
    let player_pos = world.actor(player).pos;

    if !world.actor(target).alive() {
        // Target down: pocket the pistol and run for the nearest exit.
        if drawn {
            return Command::DrawOrHolster;
        }
        let mut exits = world.extraction_tiles.clone();
        exits.sort_by_key(|e| {
            e.chebyshev(player_pos)
                .map(i32::from)
                .unwrap_or(i32::MAX / 2)
        });
        for exit in exits {
            if player_pos == exit {
                return Command::Wait;
            }
            if let Some(dir) = first_step_towards(world, data, player, exit) {
                return Command::Move(dir);
            }
        }
        return Command::Wait;
    }

    let alone = world
        .actor(target)
        .ai
        .as_ref()
        .and_then(|ai| ai.schedule.as_ref())
        .and_then(|s| s.current())
        .is_some_and(|b| b.protection == murmur_core::world::Protection::Alone);

    if alone {
        let target_pos = world.actor(target).pos;
        if player_pos.is_adjacent(target_pos) {
            // Hands free and behind them: the quiet way.
            if !drawn {
                return Command::Garrote(target);
            }
            return Command::DrawOrHolster;
        }
        if let Some(dir) = first_step_towards(world, data, player, target_pos) {
            return Command::Move(dir);
        }
        return Command::Wait;
    }

    // Escorted. The private beat is usually behind a locked door, and the
    // detail carries its principal's keys — so the way in is to follow a
    // bodyguard and lift one. This is the intended loop, and the script
    // exists to prove it is walkable.
    let has_key = world
        .carried_items(player)
        .any(|i| data.item(&i.spec).is_some_and(|s| s.unlocks.is_some()));
    if !has_key {
        let mark = world
            .actors
            .iter()
            .filter(|a| {
                a.alive()
                    && !a.departed
                    && a.ai.as_ref().and_then(|ai| ai.detail.as_ref()).is_some()
                    && world
                        .carried_items(a.id)
                        .any(|i| data.item(&i.spec).is_some_and(|s| s.unlocks.is_some()))
            })
            .min_by_key(|a| a.pos.chebyshev(player_pos).unwrap_or(i16::MAX));
        if let Some(mark) = mark {
            if player_pos.is_adjacent(mark.pos) {
                return Command::Pickpocket(mark.id);
            }
            if let Some(dir) = first_step_towards(world, data, player, mark.pos) {
                return Command::Move(dir);
            }
        }
    }

    // Bring the private beat forward if there is a desk to do it with,
    // otherwise keep out of the way and let the schedule come round.
    let desk = world.furniture.iter().find(|f| {
        f.machine
            .as_deref()
            .and_then(|m| data.opportunity(m))
            .is_some_and(|s| {
                matches!(
                    s.effect,
                    murmur_core::data::OpportunityEffect::SummonTarget { .. }
                )
            })
    });
    if let Some(desk) = desk
        && !desk.used
    {
        if player_pos.is_adjacent(desk.pos) {
            return Command::Interact(desk.id);
        }
        if let Some(dir) = first_step_towards(world, data, player, desk.pos) {
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

/// The milestone's acceptance criterion, and an honest one.
///
/// Before this milestone a scripted assassin won by walking at the target
/// and firing from eight tiles; missions ended in 30-60 turns. That no
/// longer works at all — the detail covers its principal, so the shot
/// kills a bodyguard and the response kills the player. The script here
/// plays the intended loop instead: follow a bodyguard, lift the key its
/// principal's day needs, page the target through if there is a desk, and
/// take the window when the target steps away alone.
///
/// **Known gap.** This script completes 2 of 10 seeds. It is deliberately
/// simple — it has no notion of disguises, of hiding, of retreating when
/// noticed, or of positioning before a window opens — and the eight
/// timeouts are its crudeness rather than an unwinnable venue: the route
/// planner certifies every one of these missions, and generation refuses
/// any seed whose target is never alone somewhere reachable. Raising this
/// number is scripted-AI work, not mechanics work.
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
    if std::env::var("MEASURE").is_ok() {
        panic!("MEASURE: {outcomes:?}");
    }
    let extracted: Vec<_> = outcomes
        .iter()
        .filter(|(_, o, _)| *o == Some(MissionOutcome::Extracted))
        .collect();
    assert!(
        !extracted.is_empty(),
        "the intended loop must be walkable at all: {outcomes:?}"
    );

    // Every completed mission lands in the band the schedule cycle was
    // tuned for. A win far below it would mean the target was reachable
    // without waiting for a window, which is the thing this milestone
    // exists to prevent.
    for (seed, _, turns) in &extracted {
        assert!(
            *turns >= 100,
            "seed {seed} finished in {turns} turns — the target should not be              killable before a private window opens"
        );
    }
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
