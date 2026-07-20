//! Deterministic breadth-first pathfinding for NPC movement.
//!
//! NPCs path over terrain and furniture; closed doors count as passable
//! when unlocked (they bump them open) or when the walker carries the key.
//! Other actors are mostly ignored here — simultaneous resolution
//! arbitrates collisions — so paths stay stable while crowds shuffle.
//!
//! The one exception is the player. Every NPC either steps aside or can
//! be swapped with, so bumping resolves within a turn or two; the player
//! alone is never displaced, and an NPC whose shortest path crosses a
//! parked player would otherwise re-plan the identical blocked step and
//! bump into them forever. NPCs therefore route *around* the player's
//! tile — except when the player is the destination, which is how a
//! pursuing guard reaches an arrest. (Blocking every undisplaceable
//! actor was tried and freezes the venue: escort formations and patrol
//! crowds read as walls and whole corridors go dark.)

use crate::access;
use crate::data::GameData;
use crate::geom::{Dir4, Pos};
use crate::map::TileKind;
use crate::world::{ActorId, World};

/// The first step of a shortest path from the actor's position to `goal`,
/// or `None` when no path exists (or the actor is already there).
pub fn first_step_towards(
    world: &World,
    data: &GameData,
    actor: ActorId,
    goal: Pos,
) -> Option<Dir4> {
    first_step_priced(world, data, actor, goal, |_| 0)
}

/// As [`first_step_towards`], with an extra per-tile price.
///
/// Terrain and stairs are priced here; `surcharge` is whatever else the caller
/// would rather avoid but is not forbidden — a room it has no business in, a
/// corridor a guard is watching. Returning zero everywhere gives the plain
/// shortest path.
///
/// The search itself is `vellum-grid`. What stays here is the part that is
/// actually about this game: which tiles are passable for *this* actor, what a
/// stair transition costs, and the rule that NPCs route around the player.
pub fn first_step_priced(
    world: &World,
    data: &GameData,
    actor: ActorId,
    goal: Pos,
    surcharge: impl Fn(Pos) -> u32,
) -> Option<Dir4> {
    let start = world.actor(actor).pos;
    if start == goal {
        return None;
    }
    let player_block = (!world.actor(actor).is_player() && world.actor(world.player).pos != goal)
        .then(|| world.actor(world.player).pos);
    let passable = |pos: Pos| -> bool {
        if player_block == Some(pos) {
            return false;
        }
        match world.map.tile(pos) {
            TileKind::Wall | TileKind::Void => false,
            TileKind::Door(id) => {
                // A door that is standing open is walkable no matter who
                // locked it — movement already treats it that way, and a
                // pathfinder that refuses it strands anyone without the
                // key on the wrong side of a doorway the target itself
                // just walked through.
                world.furniture_at(pos).is_none()
                    && (world.door(id).open || access::can_pass_door(world, data, actor, id))
            }
            TileKind::Floor | TileKind::Stairs(_) => world.furniture_at(pos).is_none(),
        }
    };

    // A stair transition costs several plain steps. The weight is not flavour
    // — it is what keeps routes stable. Stair tiles sit inline in corridors,
    // so under uniform costs a walker beside a stairwell often has two
    // equal-length routes, one through the stairs and one around, and per-turn
    // re-planning with equal costs can pick a different winner from each end
    // of the same pair of tiles. The observed result was an NPC
    // teleport-oscillating between two storeys forever. Pricing the transition
    // breaks every such tie: up-and-straight-back-down is now strictly worse
    // than any flat alternative, while a genuinely necessary climb is still
    // found.
    const STEP_COST: u32 = 1;
    const STAIR_COST: u32 = 4;

    let width = world.map.width() as i16;
    let height = world.map.height() as i16;
    let floors = world.map.floor_count() as i16;
    let index = |pos: Pos| -> Option<usize> {
        (pos.x >= 0 && pos.y >= 0 && pos.x < width && pos.y < height && (pos.floor as i16) < floors)
            .then(|| {
                (usize::from(pos.floor) * height as usize + pos.y as usize) * width as usize
                    + pos.x as usize
            })
    };
    let from_index = |node: usize| -> Pos {
        let plane = (width as usize) * (height as usize);
        let floor = node / plane;
        let rest = node % plane;
        Pos::new(
            floor as u8,
            (rest % width as usize) as i16,
            (rest / width as usize) as i16,
        )
    };
    let size = (width * height) as usize * floors as usize;

    let move_index =
        vellum_grid::first_move_towards(size, index(start)?, index(goal)?, |node, out| {
            let pos = from_index(node);
            // Dir4::ALL order is what breaks equal-cost ties, so it is the
            // order neighbours are offered in.
            for (order, dir) in Dir4::ALL.into_iter().enumerate() {
                let ahead = pos.step(dir);
                if !passable(ahead) {
                    continue;
                }
                let landing = world.map.resolve_step_destination(ahead);
                let Some(node) = index(landing) else { continue };
                let base = if landing == ahead {
                    STEP_COST
                } else {
                    STAIR_COST
                };
                out.push(vellum_grid::Step {
                    node,
                    move_index: order as u8,
                    cost: base + surcharge(landing),
                });
            }
        })?;
    Some(Dir4::ALL[usize::from(move_index)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::GameData;
    use crate::generator::generate;

    #[test]
    fn every_npc_can_path_to_every_routine_stop() {
        let data = GameData::embedded().unwrap();
        let world = generate(&data, &crate::contract::MissionConfig::new(21, "nightclub")).unwrap();
        for actor in &world.actors {
            let Some(ai) = &actor.ai else { continue };
            for step in &ai.routine {
                if actor.pos == step.pos {
                    continue;
                }
                assert!(
                    first_step_towards(&world, &data, actor.id, step.pos).is_some(),
                    "{} cannot path from {:?} to routine stop {:?}",
                    actor.name,
                    actor.pos,
                    step.pos
                );
            }
        }
    }

    #[test]
    fn player_can_path_to_both_extraction_tiles() {
        let data = GameData::embedded().unwrap();
        let world = generate(&data, &crate::contract::MissionConfig::new(33, "nightclub")).unwrap();
        for exit in &world.extraction_tiles {
            if world.player_actor().pos == *exit {
                continue;
            }
            assert!(
                first_step_towards(&world, &data, world.player, *exit).is_some(),
                "no path from spawn to extraction tile {exit:?}"
            );
        }
    }
}
