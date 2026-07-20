//! Palette availability: whether each keyed action has a valid potential
//! target right now, and if not, why.
//!
//! Availability is computed from the player's own tile plus its four
//! orthogonal neighbours and inventory, mirroring the command
//! translator's adjacency — but it stays deliberately separate from the
//! translator (recorded in keymap-is-one-table): this is a cheap probe
//! over live world state for greying out keys and explaining refusals,
//! not a validation pass. Non-targeted actions are always available.

use murmur_core::data::GameData;
use murmur_core::geom::{Dir4, Pos};
use murmur_core::map::TileKind;
use murmur_core::tr;
use murmur_core::world::{FurnitureKind, Hands, World};

/// The reason a targeted action cannot be attempted right now, or `None`
/// when it is available (or is a non-targeted action).
pub fn action_block(world: &World, data: &GameData, key: char) -> Option<&'static str> {
    let player = world.player_actor();
    let here = player.pos;
    // Own tile plus the four orthogonal neighbours — the reach of
    // every adjacency-based command.
    let near = |probe: &dyn Fn(Pos) -> bool| {
        probe(here) || Dir4::ALL.into_iter().any(|d| probe(here.step(d)))
    };
    let carries = |pred: &dyn Fn(&murmur_core::data::ItemSpec) -> bool| {
        world
            .carried_items(world.player)
            .any(|i| data.item(&i.spec).is_some_and(pred))
    };
    let living_npc = |p: Pos| {
        world
            .standing_actor_at(p)
            .is_some_and(|a| !a.is_player() && a.alive())
    };

    match key {
        'r' => (!carries(&|s| s.firearm)).then_some(tr!("mission.block.no_firearm_owned")),
        'g' => {
            if !carries(&|s| s.weapon && !s.firearm) {
                Some(tr!("mission.block.no_garrote"))
            } else if !near(&living_npc) {
                Some(tr!("mission.block.no_garrote_target"))
            } else {
                None
            }
        }
        'f' => {
            if !carries(&|s| s.firearm) {
                Some(tr!("mission.block.no_firearm"))
            } else if crate::fov::visible_actors(world, data).is_empty() {
                Some(tr!("mission.block.no_target"))
            } else {
                None
            }
        }
        'p' => {
            if world.carried_items(world.player).count() >= murmur_core::actions::INVENTORY_SLOTS {
                Some(tr!("mission.block.pockets_full"))
            } else if !near(&|p| {
                world.standing_actor_at(p).is_some_and(|a| !a.is_player())
                    || world.body_at(p).is_some()
            }) {
                Some(tr!("mission.block.no_mark"))
            } else {
                None
            }
        }
        'd' => {
            if player.hands != Hands::Free {
                Some(tr!("mission.block.hands_busy"))
            } else if !near(&|p| {
                world.body_at(p).is_some()
                    || world
                        .furniture_at(p)
                        .is_some_and(|f| f.kind == FurnitureKind::Wardrobe && f.disguise.is_some())
            }) {
                Some(tr!("mission.block.no_clothes"))
            } else {
                None
            }
        }
        'b' => {
            if matches!(player.hands, Hands::CarryingBody(_)) {
                None // drop is available
            } else if player.hands != Hands::Free {
                Some(tr!("mission.block.hands_busy"))
            } else if !near(&|p| world.body_at(p).is_some()) {
                Some(tr!("mission.block.no_body"))
            } else {
                None
            }
        }
        'h' => {
            if !matches!(player.hands, Hands::CarryingBody(_)) {
                Some(tr!("mission.block.not_carrying"))
            } else if !near(&|p| {
                world
                    .furniture_at(p)
                    .is_some_and(|f| f.kind == FurnitureKind::Container && f.body.is_none())
            }) {
                Some(tr!("mission.block.no_container"))
            } else {
                None
            }
        }
        'o' => {
            (!near(&|p| matches!(world.map.tile(p), TileKind::Door(id) if !world.door(id).open)))
                .then_some(tr!("mission.block.no_door_to_open"))
        }
        'k' => (!near(&|p| matches!(world.map.tile(p), TileKind::Door(id) if world.door(id).open)))
            .then_some(tr!("mission.block.no_door_to_close")),
        'l' => {
            if !carries(&|s| s.lockpick) {
                Some(tr!("mission.block.no_lockpicks"))
            } else if !near(
                &|p| matches!(world.map.tile(p), TileKind::Door(id) if world.door(id).locked_by.is_some()),
            ) {
                Some(tr!("mission.block.no_lock"))
            } else {
                None
            }
        }
        't' => {
            let ready = world
                .carried_items(world.player)
                .any(|i| data.item(&i.spec).is_some_and(|s| s.noisemaker) && i.charges > 0);
            (!ready).then_some(tr!("mission.block.no_charges"))
        }
        'u' => (!near(&|p| {
            world
                .furniture_at(p)
                .is_some_and(|f| f.kind == FurnitureKind::Machine && !f.used)
        }))
        .then_some(tr!("mission.block.nothing_to_use")),
        _ => None,
    }
}
