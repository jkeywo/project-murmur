//! Shared scenario vocabulary for the integration suites.
//!
//! Every suite starts the same way — generate a world, neutralise the
//! crowd, teleport the cast into position via the driver's
//! scenario-setup access — then plays through ordinary commands only.
//! That staging language lives here once.
//!
//! Each suite compiles this module independently (`mod common;`) and no
//! suite uses every helper, so dead-code warnings are silenced for the
//! module as a whole.
#![allow(dead_code)]

use murmur_core::contract::MissionConfig;
use murmur_core::data::{GameData, Role};
use murmur_core::generator::generate;
use murmur_core::geom::{Dir4, Pos};
use murmur_core::map::TileKind;
use murmur_core::turn::TurnDriver;
use murmur_core::world::{ActorId, Mood, World};

pub fn data() -> GameData {
    GameData::embedded().unwrap()
}

/// A plain nightclub mission: the default scenario stage.
pub fn setup(seed: u64) -> (GameData, TurnDriver) {
    setup_config(MissionConfig::new(seed, "nightclub"))
}

/// A mission from any config — constraint, loadout, venue, or heat.
pub fn setup_config(config: MissionConfig) -> (GameData, TurnDriver) {
    let data = data();
    let world = generate(&data, &config).unwrap();
    let driver = TurnDriver::new(world, &data);
    (data, driver)
}

/// Like [`setup`], but the crowd is quieted before the driver prepares
/// its first turn, so no opening action was chosen from a live routine.
pub fn setup_quiet(seed: u64) -> (GameData, TurnDriver) {
    let data = data();
    let mut world = generate(&data, &MissionConfig::new(seed, "nightclub")).unwrap();
    quiet_all_npcs(&mut world);
    let driver = TurnDriver::new(world, &data);
    (data, driver)
}

/// Strips every NPC of routine, mood, suspicion, focus, and standing
/// assignment, so nobody interferes with staged perception tests. A
/// bodyguard keeping its detail would walk off towards its principal
/// instead of standing where the scenario put it.
pub fn quiet_all_npcs(world: &mut World) {
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
            ai.detail = None;
        }
    }
}

/// A run of `count` free floor tiles in a straight line, so scenarios
/// can stage actors with guaranteed clear sight and adjacency.
pub fn free_run(world: &World, count: i16) -> (Pos, Dir4) {
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

/// Teleports an actor into position. `None` leaves facing untouched;
/// actors with no facing (the player) never gain one.
pub fn place(world: &mut World, actor: ActorId, pos: Pos, facing: Option<Dir4>) {
    let actor = world.actor_mut(actor);
    actor.pos = pos;
    if actor.facing.is_some() || facing.is_some() {
        actor.facing = facing.or(actor.facing);
    }
}

/// Any living, non-target NPC of the given role.
pub fn some_npc(world: &World, role: Role) -> ActorId {
    world
        .actors
        .iter()
        .find(|a| a.role == Some(role) && a.alive() && !a.is_target)
        .map(|a| a.id)
        .unwrap_or_else(|| panic!("world has a {} NPC", role.name()))
}

/// A free tile adjacent to `pos`, for standing beside machines.
pub fn stand_beside(world: &World, pos: Pos) -> Pos {
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

/// Parks every NPC except `keep` far away on the topmost storey so
/// nothing is seen or heard. Derived from the map rather than pinned to
/// coordinates, so re-shaping a venue cannot silently park someone in
/// earshot and turn an assertion into a false pass.
pub fn park_npcs_far(world: &mut World, keep: &[ActorId]) {
    let ids: Vec<ActorId> = world
        .actors
        .iter()
        .filter(|a| !a.is_player() && !keep.contains(&a.id))
        .map(|a| a.id)
        .collect();
    let top = world.map.floor_count() - 1;
    let spots: Vec<Pos> = world
        .map
        .floor_positions(top)
        .filter(|p| matches!(world.map.tile(*p), TileKind::Floor))
        .collect();
    assert!(
        spots.len() >= ids.len(),
        "the top storey must hold every parked NPC"
    );
    for (pos, id) in spots.into_iter().zip(ids) {
        world.actor_mut(id).pos = pos;
        world.actor_mut(id).facing = Some(Dir4::North);
    }
}
