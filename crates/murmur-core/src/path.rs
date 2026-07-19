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

    // Dijkstra over landings, storing the first step of the cheapest
    // route to each tile. A stair transition costs several plain steps.
    //
    // The weight is not flavour — it is what keeps routes stable. Stair
    // tiles sit inline in corridors, so under uniform costs a walker
    // beside a stairwell often has two equal-length routes, one through
    // the stairs and one around, and per-turn re-planning with equal
    // costs can pick a different winner from each end of the same pair of
    // tiles. The observed result was an NPC teleport-oscillating between
    // two storeys forever. Pricing the transition breaks every such tie:
    // up-and-straight-back-down is now strictly worse than any flat
    // alternative, while a genuinely necessary climb is still found.
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
    let size = (width * height) as usize * floors as usize;
    let mut first_step: Vec<Option<Dir4>> = vec![None; size];
    let mut best: Vec<u32> = vec![u32::MAX; size];
    best[index(start)?] = 0;

    // Deterministic priority queue: cost first, then insertion order, so
    // equal-cost tiles expand in the order they were discovered and the
    // chosen route is a pure function of the world.
    type QueueEntry = std::cmp::Reverse<(u32, u32, i16, i16, u8)>;
    let mut heap: std::collections::BinaryHeap<QueueEntry> = std::collections::BinaryHeap::new();
    let mut seq: u32 = 0;
    heap.push(std::cmp::Reverse((0, seq, start.x, start.y, start.floor)));
    while let Some(std::cmp::Reverse((cost, _, x, y, floor))) = heap.pop() {
        let pos = Pos::new(floor, x, y);
        let pos_index = index(pos)?;
        if cost > best[pos_index] {
            continue; // a stale queue entry
        }
        if pos == goal {
            return first_step[pos_index];
        }
        for dir in Dir4::ALL {
            let ahead = pos.step(dir);
            if !passable(ahead) {
                continue;
            }
            let landing = world.map.resolve_step_destination(ahead);
            let Some(i) = index(landing) else { continue };
            let step_cost = if landing == ahead {
                STEP_COST
            } else {
                STAIR_COST
            };
            let next = cost + step_cost;
            if next >= best[i] {
                continue;
            }
            best[i] = next;
            first_step[i] = if pos == start {
                Some(dir)
            } else {
                first_step[pos_index]
            };
            seq += 1;
            heap.push(std::cmp::Reverse((
                next,
                seq,
                landing.x,
                landing.y,
                landing.floor,
            )));
        }
    }
    None
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
