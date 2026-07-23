//! The leash and the plant: two player verbs the objective slices will
//! build on. These are sandbox verbs — exercised here with no objective in
//! sight, only the mechanics themselves.

use murmur_core::actions::{ActionIntent, Command, RejectReason};
use murmur_core::ai::prepare_npc_actions;
use murmur_core::data::{GameData, Role};
use murmur_core::world::{ActorId, ItemId, ItemInstance, ItemLocation, Mood, World};

mod common;
use common::{free_run, place, quiet_all_npcs, setup, some_npc};

/// Chebyshev gap between two actors on the same floor.
fn gap(world: &World, a: ActorId, b: ActorId) -> i16 {
    world
        .actor(a)
        .pos
        .chebyshev(world.actor(b).pos)
        .expect("actors on the same floor")
}

/// Puts a plantable listening bug in an actor's pockets.
fn give_bug(world: &mut World, holder: ActorId) {
    let id = ItemId(world.items.len() as u32);
    world.items.push(ItemInstance {
        id,
        spec: "listening-bug".to_string(),
        location: ItemLocation::CarriedBy(holder),
        charges: 0,
    });
}

fn carries(world: &World, actor: ActorId, spec: &str) -> bool {
    world.carried_items(actor).any(|i| i.spec == spec)
}

fn following(world: &World, actor: ActorId) -> Option<ActorId> {
    world.actor(actor).ai.as_ref().and_then(|ai| ai.following)
}

// --- Lead ------------------------------------------------------------------

#[test]
fn a_led_person_closes_on_and_trails_the_player() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let civ = some_npc(driver.world(), Role::Civilian);
    let player = driver.world().player;
    let (start, dir) = free_run(driver.world(), 4);
    // Player one tile ahead of the civilian along a clear run.
    place(driver.world_mut(), player, start.step(dir), None);
    place(driver.world_mut(), civ, start, Some(dir));

    driver.submit(&data, &Command::Lead(civ)).unwrap();
    assert_eq!(
        following(driver.world(), civ),
        Some(player),
        "leading sets the follow assignment to the player"
    );

    // Walk two tiles further, opening a gap the leash must close.
    driver.submit(&data, &Command::Move(dir)).unwrap();
    driver.submit(&data, &Command::Move(dir)).unwrap();
    assert!(
        gap(driver.world(), player, civ) >= 2,
        "the walk opened a gap to close"
    );

    for _ in 0..16 {
        if driver.mission_over() {
            break;
        }
        driver.submit(&data, &Command::Wait).unwrap();
    }
    assert_eq!(
        gap(driver.world(), player, civ),
        1,
        "the follower closes and then trails one tile behind"
    );
    assert_eq!(
        following(driver.world(), civ),
        Some(player),
        "the leash persists while trailing"
    );
}

#[test]
fn releasing_a_led_person_stops_them() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let civ = some_npc(driver.world(), Role::Civilian);
    let player = driver.world().player;
    let (start, dir) = free_run(driver.world(), 4);
    place(driver.world_mut(), player, start.step(dir), None);
    place(driver.world_mut(), civ, start, Some(dir));

    driver.submit(&data, &Command::Lead(civ)).unwrap();
    // A second lead on the same person is a release.
    driver.submit(&data, &Command::Lead(civ)).unwrap();
    assert_eq!(
        following(driver.world(), civ),
        None,
        "leading again releases the leash"
    );

    let held = driver.world().actor(civ).pos;
    driver.submit(&data, &Command::Move(dir)).unwrap();
    driver.submit(&data, &Command::Move(dir)).unwrap();
    for _ in 0..8 {
        if driver.mission_over() {
            break;
        }
        driver.submit(&data, &Command::Wait).unwrap();
    }
    assert_eq!(
        driver.world().actor(civ).pos,
        held,
        "a released person no longer pursues the player"
    );
    assert_eq!(following(driver.world(), civ), None);
}

#[test]
fn a_frightened_follower_ignores_the_leash_until_it_calms() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let civ = some_npc(driver.world(), Role::Civilian);
    let player = driver.world().player;
    let (start, dir) = free_run(driver.world(), 4);
    place(driver.world_mut(), player, start.step(dir), None);
    place(driver.world_mut(), civ, start, Some(dir));

    driver.submit(&data, &Command::Lead(civ)).unwrap();
    // Open a gap so following would require a step.
    driver.submit(&data, &Command::Move(dir)).unwrap();
    driver.submit(&data, &Command::Move(dir)).unwrap();

    // Frighten the follower. Fear outranks the leash: its prepared action is
    // not a step toward the player, and the assignment survives untouched.
    driver.world_mut().actor_mut(civ).ai.as_mut().unwrap().mood = Mood::Suspicious;
    let intent = prepared_intent(driver.world_mut(), &data, civ);
    assert!(
        !matches!(intent, Some(ActionIntent::Step(_))),
        "a frightened follower does not walk the leash: {intent:?}"
    );
    assert_eq!(
        following(driver.world(), civ),
        Some(player),
        "fear does not clear the leash"
    );

    // Calm it, and on a turn it is scheduled to act, the leash resumes.
    murmur_core::perception::calm(driver.world_mut(), civ);
    align_to_cadence(driver.world_mut(), &data, civ);
    let intent = prepared_intent(driver.world_mut(), &data, civ);
    assert!(
        matches!(intent, Some(ActionIntent::Step(_))),
        "a calmed follower resumes trailing the player: {intent:?}"
    );
}

/// The intent this NPC would prepare for the next turn, if any.
fn prepared_intent(world: &mut World, data: &GameData, id: ActorId) -> Option<ActionIntent> {
    prepare_npc_actions(world, data)
        .into_iter()
        .find(|p| p.actor == id)
        .map(|p| p.intent)
}

/// Sets the turn counter so a relaxed NPC is scheduled to act this turn.
fn align_to_cadence(world: &mut World, data: &GameData, id: ActorId) {
    let cadence = u64::from(data.tuning.relaxed_cadence.max(1));
    world.turn = ((cadence - (u64::from(id.0) % cadence)) % cadence) as u32;
}

// --- Plant -----------------------------------------------------------------

#[test]
fn planting_on_a_person_moves_the_bug_into_their_pockets() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let civ = some_npc(driver.world(), Role::Civilian);
    let player = driver.world().player;
    give_bug(driver.world_mut(), player);
    let (start, dir) = free_run(driver.world(), 2);
    place(driver.world_mut(), player, start, None);
    place(driver.world_mut(), civ, start.step(dir), Some(dir));

    driver.submit(&data, &Command::Plant(Some(civ))).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    assert!(
        carries(driver.world(), civ, "listening-bug"),
        "the bug moved into the mark's pockets"
    );
    assert!(
        !carries(driver.world(), player, "listening-bug"),
        "the bug is no longer the player's"
    );
}

#[test]
fn planting_with_no_target_leaves_the_bug_on_the_players_tile() {
    let (data, mut driver) = setup(17);
    quiet_all_npcs(driver.world_mut());
    let player = driver.world().player;
    give_bug(driver.world_mut(), player);
    let (start, _) = free_run(driver.world(), 1);
    place(driver.world_mut(), player, start, None);

    driver.submit(&data, &Command::Plant(None)).unwrap();
    while driver.player_busy() {
        driver.continue_busy(&data);
    }
    let here = driver.world().player_actor().pos;
    let on_ground = driver
        .world()
        .items
        .iter()
        .any(|i| i.spec == "listening-bug" && i.location == ItemLocation::Ground(here));
    assert!(on_ground, "the bug was left on the player's own tile");
    assert!(
        !carries(driver.world(), player, "listening-bug"),
        "the bug is no longer carried"
    );
}

#[test]
fn planting_with_nothing_plantable_is_rejected() {
    let (data, mut driver) = setup(17);
    let civ = some_npc(driver.world(), Role::Civilian);
    // The default loadout (garrote, silenced pistol) carries nothing
    // plantable, so both forms of the plant are refused before any turn.
    assert!(matches!(
        driver.submit(&data, &Command::Plant(None)),
        Err(RejectReason::NothingToPlant)
    ));
    assert!(matches!(
        driver.submit(&data, &Command::Plant(Some(civ))),
        Err(RejectReason::NothingToPlant)
    ));
}
