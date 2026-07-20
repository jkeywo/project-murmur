//! The player's field of view, and the exploration memory built from it.
//!
//! Presentation-side sight: what the *player* is shown, as opposed to
//! `murmur_core::perception`, which decides what NPCs see. The widening
//! rule is deliberate and recorded — every sight-blocking tile bordering
//! a visible open tile is lit too, so standing against a wall shows the
//! whole wall face — and it can only ever add blocking tiles, never
//! actors or floor.

use std::collections::HashSet;

use murmur_core::data::GameData;
use murmur_core::geom::Pos;
use murmur_core::map::{GameMap, line_of_sight, tiles_visible_from};
use murmur_core::world::{ActorId, World};

/// The player's current field of view, widened so that every
/// sight-blocking tile bordering a visible open tile is lit too.
pub fn visible_tiles(world: &World, data: &GameData) -> Vec<Pos> {
    let player = world.player_actor();
    let base = tiles_visible_from(
        player.pos,
        data.tuning.player_vision_range,
        &world.map,
        world.sight_blocker(player.crouched),
    );
    let mut lit: HashSet<Pos> = base.iter().copied().collect();
    // Classify blockers without crouch effects: walls, closed doors,
    // and tall furniture, not low cover.
    let blocking = world.sight_blocker(false);
    for pos in &base {
        if blocking(*pos) {
            continue;
        }
        for dy in -1i16..=1 {
            for dx in -1i16..=1 {
                let neighbour = Pos::new(pos.floor, pos.x + dx, pos.y + dy);
                if world.map.in_bounds(neighbour) && blocking(neighbour) {
                    lit.insert(neighbour);
                }
            }
        }
    }
    lit.into_iter().collect()
}

/// Living NPCs the player can currently see, nearest first. The shooting
/// UI targets from this list and the threat panel reads it, so the two
/// cannot disagree about who is visible.
pub fn visible_actors(world: &World, data: &GameData) -> Vec<ActorId> {
    let player = world.player_actor();
    let mut ids: Vec<(i16, ActorId)> = world
        .actors
        .iter()
        .filter(|a| !a.is_player() && a.alive() && !a.departed && a.hidden_in.is_none())
        .filter(|a| {
            player
                .pos
                .chebyshev(a.pos)
                .is_some_and(|d| d <= data.tuning.player_vision_range)
                && line_of_sight(
                    player.pos,
                    a.pos,
                    world.sight_blocker(player.crouched || a.crouched),
                )
        })
        .map(|a| (player.pos.chebyshev(a.pos).unwrap_or(i16::MAX), a.id))
        .collect();
    ids.sort();
    ids.into_iter().map(|(_, id)| id).collect()
}

/// Which tiles the player has ever seen: the memory map. Grows only.
pub(crate) struct Explored {
    floors: Vec<Vec<bool>>,
}

impl Explored {
    pub(crate) fn new(map: &GameMap) -> Self {
        let floor_len = usize::from(map.width()) * usize::from(map.height());
        Self {
            floors: (0..map.floor_count())
                .map(|_| vec![false; floor_len])
                .collect(),
        }
    }

    pub(crate) fn contains(&self, map: &GameMap, pos: Pos) -> bool {
        if !map.in_bounds(pos) {
            return false;
        }
        let index = usize::try_from(pos.y).unwrap() * usize::from(map.width())
            + usize::try_from(pos.x).unwrap();
        self.floors[usize::from(pos.floor)][index]
    }

    /// Marks everything currently visible as seen.
    pub(crate) fn extend_visible(&mut self, world: &World, data: &GameData) {
        let width = usize::from(world.map.width());
        for pos in visible_tiles(world, data) {
            let index = usize::try_from(pos.y).unwrap() * width + usize::try_from(pos.x).unwrap();
            self.floors[usize::from(pos.floor)][index] = true;
        }
    }
}
