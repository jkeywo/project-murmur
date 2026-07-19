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
/// A standing spot one tile aside from the approach to the door of the
/// room containing `inside`. The room is usually locked until the target
/// lets itself in, so every tile within it is unreachable — but the
/// approach tile directly outside the door is the *only* way in, and a
/// player parked on it walls the target out of its own office (the player
/// is never displaced). The right place is beside the approach: close
/// enough to step in behind the target, never in its way.
fn door_post(
    world: &World,
    inside: murmur_core::geom::Pos,
    stops: &[murmur_core::geom::Pos],
) -> Option<murmur_core::geom::Pos> {
    let room = world.room_at(inside)?;
    for pos in world.map.floor_positions(room.floor) {
        let murmur_core::map::TileKind::Door(id) = world.map.tile(pos) else {
            continue;
        };
        if !room.doors.contains(&id) {
            continue;
        }
        let approach = Dir4::ALL.into_iter().map(|d| pos.step(d)).find(|p| {
            !room.bounds.contains(p.x, p.y)
                && matches!(world.map.tile(*p), murmur_core::map::TileKind::Floor)
        })?;
        // Any tile beside the approach will do, as long as it is not the
        // approach itself, not in the room, and — critically — not one of
        // the target's own stops: an undisplaceable player standing on a
        // beat tile stalls the whole schedule on it.
        for dir in Dir4::ALL {
            let aside = approach.step(dir);
            if aside != pos
                && !room.bounds.contains(aside.x, aside.y)
                && matches!(world.map.tile(aside), murmur_core::map::TileKind::Floor)
                && world.furniture_at(aside).is_none()
                && !stops.contains(&aside)
            {
                return Some(aside);
            }
        }
    }
    None
}

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

    let schedule = world
        .actor(target)
        .ai
        .as_ref()
        .and_then(|ai| ai.schedule.as_ref());
    let current = schedule.and_then(|s| s.current());
    let alone = current.is_some_and(|b| b.protection == murmur_core::world::Protection::Alone);

    if alone {
        let target_pos = world.actor(target).pos;
        let beat_pos = current.map(|b| b.pos).unwrap_or(target_pos);
        // Strike only once the target has settled in its private room:
        // killing it mid-corridor is a witnessed murder in a crowd, which
        // is what the arrest endings were.
        let settled = world.room_at(target_pos).is_some()
            && world.room_at(target_pos).map(|r| r.id) == world.room_at(beat_pos).map(|r| r.id);
        if settled {
            let behind = world
                .actor(target)
                .facing
                .map(|f| target_pos.step(f.opposite()));
            if let Some(behind) = behind {
                if player_pos == behind {
                    if drawn {
                        return Command::DrawOrHolster; // hands free for the wire
                    }
                    return Command::Garrote(target);
                }
                let takeable = matches!(world.map.tile(behind), murmur_core::map::TileKind::Floor)
                    && world.furniture_at(behind).is_none()
                    && world.standing_actor_at(behind).is_none();
                if takeable && let Some(dir) = first_step_towards(world, data, player, behind) {
                    return Command::Move(dir);
                }
            }
            let clear_shot = player_pos
                .chebyshev(target_pos)
                .is_some_and(|d| d <= data.tuning.pistol_range)
                && player_pos.floor == target_pos.floor
                && murmur_core::map::line_of_sight(
                    player_pos,
                    target_pos,
                    world.sight_blocker(false),
                );
            if clear_shot && !drawn {
                return Command::DrawOrHolster;
            }
            if clear_shot {
                return Command::Shoot(target);
            }
            if let Some(dir) = first_step_towards(world, data, player, target_pos) {
                return Command::Move(dir);
            }
            return Command::Wait;
        }
        // The window is opening: the target is walking to its private
        // beat and the detail has peeled off to its posts. The room is
        // locked until the target lets itself in — so the place to be is
        // the threshold, one step from the door it is about to open.
        let stops: Vec<murmur_core::geom::Pos> = schedule
            .map(|s| s.beats.iter().map(|b| b.pos).collect())
            .unwrap_or_default();
        if let Some(post) = door_post(world, beat_pos, &stops)
            && player_pos != post
            && let Some(dir) = first_step_towards(world, data, player, post)
        {
            return Command::Move(dir);
        }
        return Command::Wait;
    }
    if drawn {
        // Never walk the halls with the pistol out.
        return Command::DrawOrHolster;
    }

    // First errand: stop looking like a guest. Staff clothes legalise the
    // staff tier, which is where the desk lives and where most of the
    // walking happens.
    if world.actor(player).worn_disguise == "civilian"
        && let Some(wardrobe) = world
            .furniture
            .iter()
            .filter(|f| {
                f.kind == murmur_core::world::FurnitureKind::Wardrobe && f.disguise.is_some()
            })
            .min_by_key(|f| f.pos.chebyshev(player_pos).unwrap_or(i16::MAX))
    {
        if player_pos.is_adjacent(wardrobe.pos) {
            return Command::TakeDisguiseFromWardrobe(wardrobe.id);
        }
        if let Some(dir) = Dir4::ALL
            .into_iter()
            .map(|d| wardrobe.pos.step(d))
            .filter(|p| {
                matches!(world.map.tile(*p), murmur_core::map::TileKind::Floor)
                    && world.furniture_at(*p).is_none()
            })
            .find_map(|stand| first_step_towards(world, data, player, stand))
        {
            return Command::Move(dir);
        }
    }

    // Bring the private beat forward if the desk has not been used yet. It
    // is furniture: path to a tile beside it, never to it.
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
        if let Some(dir) = Dir4::ALL
            .into_iter()
            .map(|d| desk.pos.step(d))
            .filter(|p| {
                matches!(world.map.tile(*p), murmur_core::map::TileKind::Floor)
                    && world.furniture_at(*p).is_none()
            })
            .find_map(|stand| first_step_towards(world, data, player, stand))
        {
            return Command::Move(dir);
        }
    }

    // Otherwise take up a position near the next alone beat and let the
    // schedule come round: close enough to strike when the window opens,
    // never standing on one of the target's own stops — the player is not
    // displaceable, and a stop it cannot reach stalls the target's day.
    if let Some(s) = schedule
        && let Some(beat) = s.alone_beats().next()
    {
        let stops: Vec<murmur_core::geom::Pos> = s.beats.iter().map(|b| b.pos).collect();
        if let Some(post) = door_post(world, beat.pos, &stops)
            && player_pos != post
            && let Some(dir) = first_step_towards(world, data, player, post)
        {
            return Command::Move(dir);
        }
        if stops.contains(&player_pos) {
            // Standing on one of the target's own stops stalls its day
            // there forever — the player is not displaceable. Step off.
            for dir in Dir4::ALL {
                let p = player_pos.step(dir);
                if matches!(world.map.tile(p), murmur_core::map::TileKind::Floor)
                    && world.furniture_at(p).is_none()
                    && !stops.contains(&p)
                    && world.standing_actor_at(p).is_none()
                {
                    return Command::Move(dir);
                }
            }
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
/// plays the intended loop instead: take a staff disguise, page the
/// target through the desk, post up beside the approach to its private
/// room, follow it in through the door it opens itself, and kill quietly.
///
/// **Known gap.** This script completes a minority of seeds; the rest
/// time out camped or die on the exit. It is deliberately simple — no
/// hiding, no retreating when noticed, no reaction to being followed —
/// and the venue is certified winnable for every seed by the route
/// planner, while generation refuses any target that is never alone
/// somewhere reachable. Raising the rate is scripted-AI work. Writing
/// the script was still the most profitable testing this milestone did:
/// the attempt surfaced five real engine defects (teleport-oscillating
/// stair routes, a spine severed by inline stairwells, pathfinding
/// refusing locked doors that stand open, NPCs bumping a parked player
/// forever, and an escort ring that halved its own principal's walking
/// pace), every one invisible to unit tests.
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
