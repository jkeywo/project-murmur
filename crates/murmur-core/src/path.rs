//! Deterministic breadth-first pathfinding for NPC movement.
//!
//! NPCs path over terrain and furniture; closed doors count as passable
//! when unlocked (they bump them open) or when the walker carries the key.
//! Other actors are ignored here — simultaneous resolution arbitrates
//! collisions — so paths stay stable while crowds shuffle.

use std::collections::VecDeque;

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
    let passable = |pos: Pos| -> bool {
        match world.map.tile(pos) {
            TileKind::Wall | TileKind::Void => false,
            TileKind::Door(id) => {
                world.furniture_at(pos).is_none() && access::can_pass_door(world, data, actor, id)
            }
            TileKind::Floor | TileKind::Stairs(_) => world.furniture_at(pos).is_none(),
        }
    };

    // BFS storing the first step that discovered each tile.
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
    let mut first_step: Vec<Option<Dir4>> = vec![None; (width * height) as usize * floors as usize];
    let mut visited = vec![false; first_step.len()];
    visited[index(start)?] = true;

    let mut frontier = VecDeque::new();
    frontier.push_back(start);
    while let Some(pos) = frontier.pop_front() {
        for dir in Dir4::ALL {
            let ahead = pos.step(dir);
            if !passable(ahead) {
                continue;
            }
            let landing = world.map.resolve_step_destination(ahead);
            let Some(i) = index(landing) else { continue };
            if visited[i] {
                continue;
            }
            visited[i] = true;
            let step = if pos == start {
                Some(dir)
            } else {
                first_step[index(pos).unwrap()]
            };
            first_step[i] = step;
            if landing == goal {
                return step;
            }
            frontier.push_back(landing);
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
