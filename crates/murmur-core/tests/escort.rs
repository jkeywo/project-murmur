//! Bodyguard details: the target is surrounded in public and alone in
//! private, and that difference is what a mission is about.

use murmur_core::actions::Command;
use murmur_core::contract::MissionConfig;
use murmur_core::data::GameData;
use murmur_core::generator::generate;
use murmur_core::geom::Dir4;
use murmur_core::turn::TurnDriver;
use murmur_core::world::{ActorId, DetailRole, Mood, Protection, World};

fn data() -> GameData {
    GameData::embedded().unwrap()
}

fn world(seed: u64, venue: &str) -> World {
    generate(&data(), &MissionConfig::new(seed, venue)).unwrap()
}

fn bodyguards(world: &World) -> Vec<ActorId> {
    world
        .actors
        .iter()
        .filter(|a| {
            a.ai.as_ref()
                .and_then(|ai| ai.detail.as_ref())
                .is_some_and(|d| matches!(d, DetailRole::Bodyguard { .. }))
        })
        .map(|a| a.id)
        .collect()
}

/// Runs the world forward, letting NPCs act. The player simply waits.
fn advance(driver: &mut TurnDriver, data: &GameData, turns: u32) {
    for _ in 0..turns {
        if driver.mission_over() {
            break;
        }
        if driver.submit(data, &Command::Wait).is_err() {
            break;
        }
        while driver.player_busy() {
            driver.continue_busy(data);
        }
    }
}

#[test]
fn every_target_walks_with_a_detail() {
    let data = data();
    for venue in ["nightclub", "warehouse", "grand-hotel", "embassy-villa"] {
        for seed in 0..10u64 {
            let world = world(seed, venue);
            let guards = bodyguards(&world);
            assert_eq!(
                guards.len(),
                usize::from(data.tuning.escort_slots),
                "{venue} seed {seed}: expected a full detail"
            );
            for id in guards {
                let Some(DetailRole::Bodyguard { principal, .. }) =
                    world.actor(id).ai.as_ref().unwrap().detail.clone()
                else {
                    panic!("not a bodyguard");
                };
                assert_eq!(
                    principal, world.target,
                    "{venue} seed {seed}: a detail guards the target"
                );
            }
        }
    }
}

/// Slots come from a fixed table by ascending actor id. If they were
/// chosen by proximity the assignment would depend on iteration order and
/// replay would drift.
#[test]
fn detail_slots_are_assigned_in_actor_order() {
    let world = world(4, "nightclub");
    let guards = bodyguards(&world);
    let mut previous: Option<(ActorId, u8)> = None;
    for id in guards {
        let Some(DetailRole::Bodyguard { slot, .. }) =
            world.actor(id).ai.as_ref().unwrap().detail.clone()
        else {
            panic!("not a bodyguard");
        };
        if let Some((prev_id, prev_slot)) = previous {
            assert!(prev_id < id, "guards are taken in ascending id");
            assert!(prev_slot < slot, "slots are handed out in order");
        }
        previous = Some((id, slot));
    }
}

/// The mechanic that makes an escorted target unattackable: a guard is not
/// displaceable, so the tiles the detail occupies are denied to the player
/// outright — including the tile directly behind the target that a garrote
/// requires.
#[test]
fn bodyguards_deny_the_tiles_beside_their_principal() {
    let world = world(4, "nightclub");
    for id in bodyguards(&world) {
        assert!(
            !world.is_displaceable(id),
            "a bodyguard the player can shove aside denies nothing"
        );
    }
}

/// The detail stands off while the principal takes a beat it does not
/// follow into — which is precisely what turns an alone beat into the
/// player's window.
#[test]
fn bodyguards_hold_back_from_a_no_follow_beat() {
    let data = data();
    let mut world = world(4, "nightclub");

    // Put the target on its alone beat and let the detail react.
    let schedule = world
        .actor(world.target)
        .ai
        .as_ref()
        .unwrap()
        .schedule
        .clone()
        .unwrap();
    let alone = schedule
        .beats
        .iter()
        .position(|b| b.protection == Protection::Alone)
        .expect("the target is alone somewhere");
    let beat = schedule.beats[alone].clone();
    {
        let target = world.target;
        world.actor_mut(target).pos = beat.pos;
        let ai = world.actor_mut(target).ai.as_mut().unwrap();
        ai.schedule.as_mut().unwrap().index = alone;
        ai.routine_index = alone;
        ai.mood = Mood::Relaxed;
        // Hold the target on the beat; otherwise it walks on to its next
        // public stop and there is no no-follow beat left to observe.
        ai.wait_remaining = u16::MAX;
    }
    let guards = bodyguards(&world);
    let mut driver = TurnDriver::new(world, &data);
    // Short of escort_search_turns: this is the window while it is open.
    // Past that clock the detail is *supposed* to come in and look, which
    // a_waiting_bodyguard_eventually_goes_in_to_look covers.
    advance(&mut driver, &data, 20);

    let world = driver.world();
    let target_pos = world.actor(world.target).pos;
    let room = world
        .room_at(target_pos)
        .map(|r| r.id)
        .expect("the alone beat is inside a room");
    for id in guards {
        // A guard that lost track of its principal may be anywhere, but it
        // must not be standing in the room with them.
        let guard_room = world.room_at(world.actor(id).pos).map(|r| r.id);
        let guard_room = match guard_room {
            Some(r) => r,
            None => continue, // in a corridor, which is outside by definition
        };
        if world.actor(id).ai.as_ref().unwrap().mood != Mood::Relaxed {
            continue; // perception outranks the escort; not this test's business
        }
        assert_ne!(
            guard_room, room,
            "a no-follow beat means the detail waits outside"
        );
    }
}

/// The regression guard for the decision not to make escorting a mood.
/// Every de-escalation path in the codebase returns an NPC to
/// `Mood::Relaxed` and nothing else, so an escort expressed as a mood
/// would be destroyed by the first noise a guard investigated. As an
/// orthogonal assignment it simply resumes.
#[test]
fn escort_survives_a_false_alarm() {
    let data = data();
    let mut world = world(4, "nightclub");
    let guard = bodyguards(&world)[0];
    // Leave one bodyguard on the detail. Guards are not displaceable, so a
    // full detail forms a queue behind its principal in a corridor, and
    // this test is about whether the assignment *survives*, not about how
    // three of them share four sides.
    for other in bodyguards(&world) {
        if other != guard {
            world.actor_mut(other).ai.as_mut().unwrap().detail = None;
        }
    }

    // Rattle the guard, then calm it the way perception does.
    let target_pos = world.actor(world.target).pos;
    let _ = target_pos;
    {
        let ai = world.actor_mut(guard).ai.as_mut().unwrap();
        ai.mood = Mood::Investigating;
        ai.focus = Some(target_pos);
    }
    assert!(
        world.actor(guard).ai.as_ref().unwrap().detail.is_some(),
        "an alarmed guard keeps its assignment"
    );
    {
        let ai = world.actor_mut(guard).ai.as_mut().unwrap();
        ai.mood = Mood::Relaxed;
        ai.focus = None;
    }

    // Park the guard well away, so "resumed escorting" has to mean it
    // actually closed the distance rather than happening to start nearby.
    let far = world
        .map
        .floor_positions(world.map.floor_count() - 1)
        .find(|p| matches!(world.map.tile(*p), murmur_core::map::TileKind::Floor))
        .unwrap();
    world.actor_mut(guard).pos = far;
    let before = far;

    // Relaxed NPCs act on a staggered cadence, so crossing the venue takes
    // roughly twice as many turns as tiles.
    let mut driver = TurnDriver::new(world, &data);
    advance(&mut driver, &data, 220);
    let world = driver.world();

    assert!(
        world.actor(guard).ai.as_ref().unwrap().detail.is_some(),
        "the assignment outlives the alarm"
    );
    // Escorting is the only behaviour that brings a guard back to its
    // principal's shoulder. A guard merely walking its own routine gets
    // no closer than chance allows, which is what makes this assertion a
    // real check on resumption rather than on wandering. Escort distance
    // is a small band, not exact adjacency: a detail trails a *walking*
    // principal in column, up to a slot's length behind.
    let now = world.actor(world.target).pos;
    let after = world.actor(guard).pos;
    assert_eq!(after.floor, now.floor, "the guard crossed the venue back");
    assert!(
        after.chebyshev(now).unwrap_or(i16::MAX) <= 4,
        "a calmed guard rejoins its principal: guard {after:?}, principal {now:?},          left at {before:?}"
    );
}

/// Firing into a detail costs the player a round, a gunshot, a witnessed
/// death and a live principal. Actors do not block *sight* — that is
/// deliberate and symmetric — but a body absolutely stops a bullet.
#[test]
fn a_bullet_at_an_escorted_target_hits_the_bodyguard() {
    let data = data();
    let mut world = world(4, "nightclub");
    let player = world.player;
    let target = world.target;
    let guard = bodyguards(&world)[0];

    // Line them up: player, guard, target, three in a row on open floor.
    let spot = world
        .map
        .floor_positions(0)
        .find(|p| {
            let clear = |q| matches!(world.map.tile(q), murmur_core::map::TileKind::Floor);
            clear(*p)
                && clear(p.step(Dir4::East))
                && clear(p.step(Dir4::East).step(Dir4::East))
                && world.furniture_at(*p).is_none()
        })
        .expect("three tiles in a row");
    world.actor_mut(player).pos = spot;
    world.actor_mut(guard).pos = spot.step(Dir4::East);
    world.actor_mut(target).pos = spot.step(Dir4::East).step(Dir4::East);
    // Nobody else near enough to wander onto the line.
    let strays: Vec<ActorId> = world
        .actors
        .iter()
        .filter(|a| a.id != player && a.id != guard && a.id != target)
        .map(|a| a.id)
        .collect();
    let mut spare: Vec<murmur_core::geom::Pos> = world
        .map
        .floor_positions(world.map.floor_count() - 1)
        .filter(|p| matches!(world.map.tile(*p), murmur_core::map::TileKind::Floor))
        .collect();
    for id in strays {
        world.actor_mut(id).pos = spare.pop().expect("room upstairs");
    }

    let mut driver = TurnDriver::new(world, &data);
    driver.submit(&data, &Command::DrawOrHolster).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    driver.submit(&data, &Command::Shoot(target)).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }

    let world = driver.world();
    assert!(
        !world.actor(guard).alive(),
        "the bodyguard on the line takes the round"
    );
    assert!(
        world.actor(target).alive(),
        "the principal survives a shot fired through its detail"
    );
}

/// A detail waits outside a no-follow beat, but not indefinitely. The
/// clock is what stops a private beat being an unlimited window and gives
/// the player a reason to hurry rather than simply to wait.
#[test]
fn a_waiting_bodyguard_eventually_goes_in_to_look() {
    let data = data();
    let mut world = world(4, "nightclub");
    let target = world.target;

    let schedule = world
        .actor(target)
        .ai
        .as_ref()
        .unwrap()
        .schedule
        .clone()
        .unwrap();
    let alone = schedule
        .beats
        .iter()
        .position(|b| b.protection == Protection::Alone)
        .expect("the target is alone somewhere");
    let beat_pos = schedule.beats[alone].pos;
    {
        world.actor_mut(target).pos = beat_pos;
        let ai = world.actor_mut(target).ai.as_mut().unwrap();
        ai.schedule.as_mut().unwrap().index = alone;
        ai.routine_index = alone;
        // Pin the target in place so the beat does not advance.
        ai.wait_remaining = u16::MAX;
    }
    let room = world.room_at(beat_pos).map(|r| r.id);
    let guards = bodyguards(&world);

    let mut driver = TurnDriver::new(world, &data);
    // Long enough to reach the post and then run the clock down.
    advance(&mut driver, &data, 400);

    let world = driver.world();
    let went_in = guards.iter().any(|id| {
        world.actor(*id).ai.as_ref().unwrap().mood == Mood::Relaxed
            && world.room_at(world.actor(*id).pos).map(|r| r.id) == room
    });
    let detail: Vec<String> = guards
        .iter()
        .map(|id| {
            let a = world.actor(*id);
            let d = a.ai.as_ref().unwrap().detail.clone();
            format!(
                "{:?} mood={:?} room={:?} {:?}",
                a.pos,
                a.ai.as_ref().unwrap().mood,
                world.room_at(a.pos).map(|r| r.id),
                d
            )
        })
        .collect();
    assert!(
        went_in,
        "no bodyguard ever checked on a principal alone behind a door.          target {:?} in {room:?}; guards: {detail:?}",
        world.actor(world.target).pos
    );
}

/// A principal with a detail on its feet does not run for the street like
/// a civilian: the detail walks it somewhere defensible and further in.
/// The fire alarm is deliberately double-edged — it empties the crowd and
/// hardens the target.
#[test]
fn a_panicking_principal_is_evacuated_inward_not_outward() {
    let data = data();
    let world = world(4, "nightclub");
    let target = world.target;
    let before = world.actor(target).pos;
    let exits = world.extraction_tiles.clone();

    let mut driver = TurnDriver::new(world, &data);
    // Hold the panic. A mood set once de-escalates within a few turns
    // because nothing is actually wrong, and the target simply resumes its
    // day — which measures the routine, not the evacuation.
    for _ in 0..200 {
        if driver.mission_over() {
            break;
        }
        let t = driver.world().target;
        driver.world_mut().actor_mut(t).ai.as_mut().unwrap().mood = Mood::Fleeing;
        if driver.submit(&data, &Command::Wait).is_err() {
            break;
        }
        while driver.player_busy() {
            driver.continue_busy(&data);
        }
    }

    let world = driver.world();
    let after = world.actor(world.target).pos;
    assert!(
        !world.actor(world.target).departed,
        "a principal with a detail on its feet does not walk out of the venue"
    );
    assert!(
        !exits.contains(&after),
        "a guarded principal does not run to an extraction tile: {before:?} -> {after:?}"
    );
}
