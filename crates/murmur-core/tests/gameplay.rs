//! Scenario tests for the stealth toolkit: garrote, disguises, bodies,
//! containers, pickpocketing, detection, and arrest.
//!
//! Scenarios start from a generated world and teleport actors into
//! position via the driver's scenario-setup access; play then proceeds
//! through ordinary commands only.

use murmur_core::access::{AccessVerdict, verdict_for_pos};
use murmur_core::actions::{Command, RejectReason};
use murmur_core::data::{GameData, Role, Zone};
use murmur_core::generator::generate;
use murmur_core::geom::{Dir4, Pos};
use murmur_core::map::TileKind;
use murmur_core::perception::npc_sees;
use murmur_core::turn::TurnDriver;
use murmur_core::world::{
    ActorId, BodyCondition, FurnitureKind, Hands, IncidentKind, MissionOutcome, Mood, World,
};

fn setup(seed: u64) -> (GameData, TurnDriver) {
    let data = GameData::embedded().unwrap();
    let world = generate(
        &data,
        &murmur_core::contract::MissionConfig::new(seed, "nightclub"),
    )
    .unwrap();
    let driver = TurnDriver::new(world, &data);
    (data, driver)
}

/// A run of `count` free floor tiles in a straight line, so scenarios can
/// stage actors with guaranteed clear sight and adjacency.
fn free_run(world: &World, count: i16) -> (Pos, Dir4) {
    for floor in 0..world.map.floor_count() {
        for start in world.map.floor_positions(floor) {
            'dirs: for dir in [Dir4::East, Dir4::South] {
                for step in 0..count {
                    let mut pos = start;
                    for _ in 0..step {
                        pos = pos.step(dir);
                    }
                    let clear = matches!(world.map.tile(pos), TileKind::Floor)
                        && world.furniture_at(pos).is_none()
                        && world.standing_actor_at(pos).is_none()
                        && !world.extraction_tiles.contains(&pos);
                    if !clear {
                        continue 'dirs;
                    }
                }
                return (start, dir);
            }
        }
    }
    panic!("no free run of {count} tiles found");
}

fn place(world: &mut World, actor: ActorId, pos: Pos, facing: Option<Dir4>) {
    let actor = world.actor_mut(actor);
    actor.pos = pos;
    if actor.facing.is_some() || facing.is_some() {
        actor.facing = facing.or(actor.facing);
    }
}

fn some_npc(world: &World, role: Role) -> ActorId {
    world
        .actors
        .iter()
        .find(|a| a.role == Some(role) && a.alive() && !a.is_target)
        .map(|a| a.id)
        .unwrap_or_else(|| panic!("world has a {} NPC", role.name()))
}

fn quiet_all_npcs(world: &mut World) {
    // Park every NPC far from the scenario, facing a wall, so nobody
    // interferes with staged perception tests.
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

#[test]
fn movers_swap_places_with_civilians_but_not_guards() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let civilian = some_npc(driver.world(), Role::Civilian);
    let guard = some_npc(driver.world(), Role::Guard);
    let player = driver.world().player;
    let (start, dir) = free_run(driver.world(), 3);

    // A civilian in the way: moving into them swaps places.
    place(driver.world_mut(), player, start, None);
    place(driver.world_mut(), civilian, start.step(dir), Some(dir));
    let move_cmd = match dir {
        Dir4::East => Command::Move(Dir4::East),
        Dir4::South => Command::Move(Dir4::South),
        other => Command::Move(other),
    };
    driver.submit(&data, &move_cmd).unwrap();
    assert_eq!(driver.world().player_actor().pos, start.step(dir));
    assert_eq!(
        driver.world().actor(civilian).pos,
        start,
        "the civilian stepped aside into the mover's tile"
    );

    // A guard in the way still blocks outright.
    place(driver.world_mut(), player, start, None);
    place(
        driver.world_mut(),
        civilian,
        start.step(dir).step(dir),
        None,
    );
    place(driver.world_mut(), guard, start.step(dir), Some(dir));
    assert!(matches!(
        driver.submit(&data, &move_cmd),
        Err(RejectReason::OccupiedByActor)
    ));
}

#[test]
fn garrote_kills_silently_from_behind_only() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let victim = some_npc(driver.world(), Role::Civilian);
    let (start, dir) = free_run(driver.world(), 3);
    let player = driver.world().player;
    // Victim faces along the run; the player stands directly behind.
    place(driver.world_mut(), victim, start.step(dir), Some(dir));
    place(driver.world_mut(), player, start, None);

    // From the front it is rejected outright.
    let front = start.step(dir).step(dir);
    place(driver.world_mut(), player, front, None);
    assert!(matches!(
        driver.submit(&data, &Command::Garrote(victim)),
        Err(RejectReason::NotBehindTarget)
    ));

    // From behind it kills, with no gunshot incident.
    place(driver.world_mut(), player, start, None);
    let report = driver.submit(&data, &Command::Garrote(victim)).unwrap();
    assert!(!driver.world().actor(victim).alive());
    assert!(
        driver
            .world()
            .incidents
            .iter()
            .all(|i| i.kind != IncidentKind::Gunshot),
        "garrote must not produce a gunshot: {:?}",
        report.events.messages
    );
}

#[test]
fn staff_disguise_from_a_body_legitimises_staff_areas() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let victim = some_npc(driver.world(), Role::Bartender);
    let player = driver.world().player;
    let (start, dir) = free_run(driver.world(), 2);

    driver.world_mut().actor_mut(victim).condition = BodyCondition::Dead;
    place(driver.world_mut(), victim, start.step(dir), None);
    place(driver.world_mut(), player, start, None);

    let staff_room_probe = {
        let world = driver.world();
        let room = world
            .rooms
            .iter()
            .find(|r| r.zone == Zone::Staff)
            .expect("staff room exists");
        Pos::new(room.floor, room.bounds.x, room.bounds.y)
    };
    assert!(matches!(
        verdict_for_pos(driver.world(), &data, player, staff_room_probe),
        AccessVerdict::Illegal(Zone::Staff)
    ));

    driver
        .submit(&data, &Command::TakeDisguiseFromBody(victim))
        .unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    assert_eq!(driver.world().player_actor().worn_disguise, "staff");
    // Clothes were swapped, not duplicated.
    assert_eq!(driver.world().actor(victim).worn_disguise, "civilian");
    assert!(verdict_for_pos(driver.world(), &data, player, staff_room_probe).is_allowed());
}

#[test]
fn bodies_can_be_carried_and_hidden_in_containers() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let victim = some_npc(driver.world(), Role::Civilian);
    let player = driver.world().player;

    // Stage next to a container.
    let container = driver
        .world()
        .furniture
        .iter()
        .find(|f| f.kind == FurnitureKind::Container)
        .map(|f| (f.id, f.pos))
        .expect("containers exist");
    let stand = Dir4::ALL
        .into_iter()
        .map(|d| container.1.step(d))
        .find(|p| {
            matches!(driver.world().map.tile(*p), TileKind::Floor)
                && driver.world().furniture_at(*p).is_none()
                && driver.world().standing_actor_at(*p).is_none()
        })
        .expect("container has an open side");

    driver.world_mut().actor_mut(victim).condition = BodyCondition::Dead;
    place(driver.world_mut(), victim, stand, None);
    place(driver.world_mut(), player, stand, None);

    driver.submit(&data, &Command::CarryBody(victim)).unwrap();
    assert!(matches!(
        driver.world().player_actor().hands,
        Hands::CarryingBody(_)
    ));

    driver
        .submit(&data, &Command::HideBody(container.0))
        .unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    let world = driver.world();
    assert_eq!(world.player_actor().hands, Hands::Free);
    assert_eq!(world.actor(victim).hidden_in, Some(container.0));
    assert!(world.body_at(stand).is_none(), "hidden bodies leave sight");
    assert_eq!(world.furniture[container.0.0 as usize].body, Some(victim));

    // One body per container: a second body is rejected.
    let second = some_npc(driver.world(), Role::Technician);
    driver.world_mut().actor_mut(second).condition = BodyCondition::Dead;
    place(driver.world_mut(), second, stand, None);
    driver.submit(&data, &Command::CarryBody(second)).unwrap();
    assert!(matches!(
        driver.submit(&data, &Command::HideBody(container.0)),
        Err(RejectReason::ContainerOccupied)
    ));
}

#[test]
fn pickpocketed_invitation_grants_vip_access() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let player = driver.world().player;
    let vip = driver
        .world()
        .actors
        .iter()
        .find(|a| a.is_vip)
        .map(|a| a.id)
        .expect("VIP civilians exist");

    let vip_probe = {
        let world = driver.world();
        let room = world
            .rooms
            .iter()
            .find(|r| r.zone == Zone::Secure)
            .expect("VIP lounge exists");
        Pos::new(room.floor, room.bounds.x, room.bounds.y)
    };
    assert!(matches!(
        verdict_for_pos(driver.world(), &data, player, vip_probe),
        AccessVerdict::Illegal(Zone::Secure)
    ));

    let (start, dir) = free_run(driver.world(), 2);
    place(driver.world_mut(), vip, start.step(dir), Some(dir));
    place(driver.world_mut(), player, start, None);
    driver.submit(&data, &Command::Pickpocket(vip)).unwrap();

    assert!(
        driver
            .world()
            .carried_items(player)
            .any(|i| i.spec == "vip-invitation"),
        "the invitation moved into the player's pockets"
    );
    assert_eq!(
        verdict_for_pos(driver.world(), &data, player, vip_probe),
        AccessVerdict::AllowedByInvitation
    );
}

#[test]
fn guards_alert_on_bodies_and_propagate_to_guards_in_sight() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let victim = some_npc(driver.world(), Role::Civilian);
    let guard_a = some_npc(driver.world(), Role::Guard);
    let guard_b = {
        let world = driver.world();
        world
            .actors
            .iter()
            .find(|a| a.role == Some(Role::Guard) && a.id != guard_a)
            .map(|a| a.id)
            .expect("at least two guards")
    };
    let player = driver.world().player;

    let (start, dir) = free_run(driver.world(), 6);
    let at = |n: i16| {
        let mut pos = start;
        for _ in 0..n {
            pos = pos.step(dir);
        }
        pos
    };
    // body at 0; guard A at 2 facing the body; guard B at 4 facing guard A;
    // the player far away at 5 (unseen by facing).
    driver.world_mut().actor_mut(victim).condition = BodyCondition::Dead;
    place(driver.world_mut(), victim, at(0), None);
    place(driver.world_mut(), guard_a, at(2), Some(dir.opposite()));
    place(driver.world_mut(), guard_b, at(4), Some(dir.opposite()));
    place(driver.world_mut(), player, at(5), None);

    driver.submit(&data, &Command::Wait).unwrap();
    let mood_a = driver.world().actor(guard_a).ai.as_ref().unwrap().mood;
    assert_eq!(mood_a, Mood::Alerted, "guard A saw the body");

    driver.submit(&data, &Command::Wait).unwrap();
    let ai_b = driver.world().actor(guard_b).ai.as_ref().unwrap();
    assert_eq!(
        ai_b.mood,
        Mood::Alerted,
        "guard B caught the alert from guard A"
    );
    assert_eq!(
        ai_b.focus,
        Some(at(0)),
        "recipients receive the incident location, not the player position"
    );
}

#[test]
fn alerted_guard_arrests_the_nonviolent_player() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let guard = some_npc(driver.world(), Role::Guard);
    let player = driver.world().player;
    let (start, dir) = free_run(driver.world(), 2);
    place(driver.world_mut(), player, start, None);
    place(
        driver.world_mut(),
        guard,
        start.step(dir),
        Some(dir.opposite()),
    );
    {
        let ai = driver.world_mut().actor_mut(guard).ai.as_mut().unwrap();
        ai.mood = Mood::Alerted;
        ai.knows_player_hostile = true;
        ai.focus = Some(start);
    }
    // Turn 1: the guard prepares the arrest; turn 2 resolves it.
    driver.submit(&data, &Command::Wait).unwrap();
    if driver.world().outcome.is_none() {
        driver.submit(&data, &Command::Wait).unwrap();
    }
    assert_eq!(driver.world().outcome, Some(MissionOutcome::Arrested));
}

#[test]
fn crouching_behind_low_cover_breaks_line_of_sight() {
    // Whether a cover piece has two open colinear sides depends on the
    // generated furniture; search seeds until one does.
    let (data, mut driver, cover, axis) = (0..24u64)
        .find_map(|seed| {
            let (data, mut driver) = setup(seed);
            quiet_all_npcs(driver.world_mut());
            let world = driver.world();
            for furniture in &world.furniture {
                if furniture.kind != FurnitureKind::LowCover {
                    continue;
                }
                let cover = furniture.pos;
                let open = |p: Pos| {
                    matches!(world.map.tile(p), TileKind::Floor)
                        && world.furniture_at(p).is_none()
                        && world.standing_actor_at(p).is_none()
                };
                if let Some(axis) = Dir4::ALL
                    .into_iter()
                    .find(|d| open(cover.step(*d)) && open(cover.step(d.opposite())))
                {
                    return Some((data, driver, cover, axis));
                }
            }
            None
        })
        .expect("some seed offers cover with two open colinear sides");
    let guard = some_npc(driver.world(), Role::Guard);
    let player = driver.world().player;
    place(
        driver.world_mut(),
        guard,
        cover.step(axis),
        Some(axis.opposite()),
    );
    place(
        driver.world_mut(),
        player,
        cover.step(axis.opposite()),
        None,
    );

    let world = driver.world();
    assert!(
        npc_sees(world, &data, guard, world.actor(player).pos, false),
        "standing player is visible over low cover"
    );
    assert!(
        !npc_sees(world, &data, guard, world.actor(player).pos, true),
        "crouched player is hidden behind low cover"
    );
}

#[test]
fn drawn_weapon_alerts_guards_unless_the_disguise_legitimises_it() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let guard = some_npc(driver.world(), Role::Guard);
    let player = driver.world().player;
    let (start, dir) = free_run(driver.world(), 4);
    place(driver.world_mut(), player, start, None);
    place(
        driver.world_mut(),
        guard,
        start.step(dir).step(dir),
        Some(dir.opposite()),
    );

    driver.submit(&data, &Command::DrawOrHolster).unwrap();
    let mood = driver.world().actor(guard).ai.as_ref().unwrap().mood;
    assert_eq!(
        mood,
        Mood::Alerted,
        "a drawn pistol on a civilian alarms guards"
    );

    // Reset and try again wearing a guard uniform: legal equipment.
    {
        let world = driver.world_mut();
        world.actor_mut(player).worn_disguise = "guard".to_string();
        let ai = world.actor_mut(guard).ai.as_mut().unwrap();
        ai.mood = Mood::Relaxed;
        ai.suspicion = 0;
        ai.focus = None;
        ai.knows_player_hostile = false;
    }
    driver.submit(&data, &Command::Wait).unwrap();
    let mood = driver.world().actor(guard).ai.as_ref().unwrap().mood;
    assert_ne!(
        mood,
        Mood::Alerted,
        "a drawn pistol in a guard uniform is legal equipment"
    );
}
