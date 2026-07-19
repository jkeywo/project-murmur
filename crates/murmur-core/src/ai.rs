//! NPC controllers.
//!
//! A controller-neutral AI slot: after each resolved turn, every eligible
//! NPC derives exactly one primitive [`ActionIntent`] for the *next* turn
//! from its routine, mood, memory, and the world. The intents go into the
//! same prepared-action store as the player's action; resolution cannot
//! tell them apart.
//!
//! Cadence: relaxed NPCs act on staggered alternating turns
//! (`(turn + actor id) % relaxed_cadence == 0`); suspicious,
//! investigating, alerted, escorting, fleeing, and combat NPCs prepare an
//! action every turn.

use crate::actions::{ActionIntent, PreparedAction, intent_duration};
use crate::data::GameData;
use crate::geom::{Dir4, Pos};
use crate::path::first_step_towards;
use crate::world::{ActorId, DetailRole, Hands, Mood, Protection, World};

/// Prepares one action per eligible NPC for the upcoming turn.
pub fn prepare_npc_actions(world: &mut World, data: &GameData) -> Vec<PreparedAction> {
    let cadence = u64::from(data.tuning.relaxed_cadence.max(1));
    let ids: Vec<ActorId> = world
        .actors
        .iter()
        .filter(|a| !a.is_player() && a.alive() && !a.departed && a.ai.is_some())
        .map(|a| a.id)
        .collect();

    let mut prepared = Vec::new();
    for id in ids {
        let mood = world.actor(id).ai.as_ref().map(|ai| ai.mood).unwrap();
        if mood == Mood::Relaxed && (u64::from(world.turn) + u64::from(id.0)) % cadence != 0 {
            continue;
        }
        // A standing detail assignment replaces only the *relaxed*
        // behaviour. Anything that raised this guard's mood — a noise, a
        // body, the player behaving oddly — outranks the escort, and when
        // perception calms them back to Relaxed they resume escorting with
        // no special handling. That is the whole reason the assignment is
        // orthogonal to mood rather than a mood of its own.
        let escorting = world.actor(id).ai.as_ref().and_then(|ai| ai.detail.clone());
        let intent = match mood {
            Mood::Relaxed if escorting.is_some() => {
                escort_intent(world, data, id, &escorting.unwrap())
            }
            Mood::Relaxed => routine_intent(world, data, id),
            Mood::Suspicious => watch_intent(world, id),
            Mood::Investigating => investigate_intent(world, data, id),
            Mood::Alerted => pursue_intent(world, data, id),
            Mood::Combat => combat_intent(world, data, id),
            Mood::Fleeing => flee_intent(world, data, id),
            Mood::Escorting => ActionIntent::Wait,
        };
        let remaining = intent_duration(data, world, id, &intent);
        prepared.push(PreparedAction {
            actor: id,
            intent,
            remaining,
        });
    }
    prepared
}

/// Follow the generated routine: walk to the current stop, linger for its
/// wait count, then advance to the next stop, forever.
fn routine_intent(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let (goal, arrived) = {
        let actor = world.actor(id);
        let ai = actor.ai.as_ref().unwrap();
        match ai.routine.get(ai.routine_index) {
            None => return ActionIntent::Wait,
            Some(step) => (step.pos, actor.pos == step.pos),
        }
    };
    if arrived {
        let actor = world.actor_mut(id);
        let ai = actor.ai.as_mut().unwrap();
        if ai.wait_remaining > 0 {
            ai.wait_remaining -= 1;
            return ActionIntent::Wait;
        }
        let step_wait = ai.routine[ai.routine_index].wait;
        ai.routine_index = (ai.routine_index + 1) % ai.routine.len();
        ai.wait_remaining = step_wait;
        return ActionIntent::Wait;
    }
    match first_step_towards(world, data, id, goal) {
        Some(dir) => ActionIntent::Step(dir),
        None => ActionIntent::Wait,
    }
}

/// Formation offsets, in a fixed order. Slots are handed out by ascending
/// actor id and read from this table, never chosen by proximity — "the
/// nearest free side" would depend on iteration order and quietly break
/// determinism.
const FORMATION: [Dir4; 4] = [Dir4::North, Dir4::South, Dir4::East, Dir4::West];

/// Where a guard waits while the principal is somewhere it does not
/// follow: just outside a door of the room the principal is in. Doors are
/// taken in their stored order and offset by the slot, so two guards on
/// the same room pick different doors when it has more than one.
fn no_follow_post(world: &World, principal: Pos, slot: u8) -> Option<Pos> {
    let room = world.room_at(principal)?;
    if room.doors.is_empty() {
        return None;
    }
    let door_index = usize::from(slot) % room.doors.len();
    let door_id = room.doors[door_index];
    // Find the door's tile, then step to the side of it outside the room.
    for pos in world.map.floor_positions(room.floor) {
        if world.map.tile(pos) != crate::map::TileKind::Door(door_id) {
            continue;
        }
        for dir in FORMATION {
            let outside = pos.step(dir);
            if !room.bounds.contains(outside.x, outside.y)
                && matches!(world.map.tile(outside), crate::map::TileKind::Floor)
            {
                return Some(outside);
            }
        }
    }
    None
}

/// The tile this guard should stand on beside its principal.
///
/// A slot is a *preference*, not an entitlement: in a corridor the north
/// and south slots are walls, and a guard that paths at a wall never
/// arrives and never gives up. Slots are therefore tried from the guard's
/// own index onwards, wrapping — a fixed order, so which guard ends up
/// where is still a function of actor id alone and replay is unaffected.
fn formation_tile(world: &World, id: ActorId, principal: Pos, slot: u8) -> Option<Pos> {
    for offset in 0..FORMATION.len() {
        let dir = FORMATION[(usize::from(slot) + offset) % FORMATION.len()];
        let tile = principal.step(dir);
        if !matches!(world.map.tile(tile), crate::map::TileKind::Floor) {
            continue;
        }
        if world.furniture_at(tile).is_some() {
            continue;
        }
        // Another guard already holding this side is fine to walk towards
        // only if it is this guard.
        match world.standing_actor_at(tile) {
            Some(other) if other.id != id => continue,
            _ => return Some(tile),
        }
    }
    None
}

/// Escort the principal: hold a formation slot beside them in public, and
/// stand off at a post while they take a beat guards do not follow.
///
/// Adjacency denial is not implemented here and needs no code: a guard is
/// not displaceable, so a guard standing on a tile beside the principal
/// denies that tile to the player outright. The garrote needs to stand
/// directly behind the target, and the formation occupies exactly those
/// tiles.
fn escort_intent(
    world: &mut World,
    data: &GameData,
    id: ActorId,
    detail: &DetailRole,
) -> ActionIntent {
    let DetailRole::Bodyguard {
        principal, slot, ..
    } = detail;
    let principal = *principal;
    let slot = *slot;
    if !world.actor(principal).alive() || world.actor(principal).departed {
        // Nothing left to guard; fall back to the guard's own routine.
        return routine_intent(world, data, id);
    }

    let principal_pos = world.actor(principal).pos;
    let no_follow = world
        .actor(principal)
        .ai
        .as_ref()
        .and_then(|ai| ai.schedule.as_ref())
        .and_then(|s| s.current())
        .is_some_and(|b| b.no_follow || b.protection == Protection::Alone);

    let goal = if no_follow {
        // Resolve the post once and remember it, so the detail does not
        // drift while the principal is out of sight.
        let cached = match world
            .actor(id)
            .ai
            .as_ref()
            .and_then(|ai| ai.detail.as_ref())
        {
            Some(DetailRole::Bodyguard { post, .. }) => *post,
            None => None,
        };
        match cached.or_else(|| no_follow_post(world, principal_pos, slot)) {
            Some(post) => {
                if let Some(ai) = world.actor_mut(id).ai.as_mut()
                    && let Some(DetailRole::Bodyguard { post: p, .. }) = ai.detail.as_mut()
                {
                    *p = Some(post);
                }
                post
            }
            None => return ActionIntent::Wait,
        }
    } else {
        // Back in formation; forget any post from the last private beat.
        if let Some(ai) = world.actor_mut(id).ai.as_mut()
            && let Some(DetailRole::Bodyguard { post: p, .. }) = ai.detail.as_mut()
        {
            *p = None;
        }
        match formation_tile(world, id, principal_pos, slot) {
            Some(tile) => tile,
            // Nowhere to stand: hold position rather than crowd. Walking
            // at an impossible tile is what a naive slot lookup does, and
            // it strands the guard several tiles out forever.
            None => return ActionIntent::Wait,
        }
    };

    if world.actor(id).pos == goal {
        return ActionIntent::Wait;
    }
    match first_step_towards(world, data, id, goal) {
        Some(dir) => ActionIntent::Step(dir),
        None => ActionIntent::Wait,
    }
}

/// Suspicious: stand and face the trouble spot, letting suspicion evolve.
fn watch_intent(world: &World, id: ActorId) -> ActionIntent {
    let actor = world.actor(id);
    let focus = actor.ai.as_ref().unwrap().focus;
    match focus.and_then(|f| Dir4::towards(actor.pos, f)) {
        Some(dir) if actor.facing != Some(dir) => ActionIntent::TurnFacing(dir),
        _ => ActionIntent::Wait,
    }
}

/// Investigating: walk to the remembered spot, look around, then relax.
fn investigate_intent(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let (focus, at_focus) = {
        let actor = world.actor(id);
        let ai = actor.ai.as_ref().unwrap();
        match ai.focus {
            None => {
                // Nothing left to check.
                let ai = world.actor_mut(id).ai.as_mut().unwrap();
                ai.mood = Mood::Relaxed;
                return ActionIntent::Wait;
            }
            Some(focus) => (focus, actor.pos == focus || actor.pos.is_adjacent(focus)),
        }
    };
    if at_focus {
        let linger_done = {
            let ai = world.actor_mut(id).ai.as_mut().unwrap();
            if ai.wait_remaining == 0 {
                ai.wait_remaining = data.tuning.investigate_linger;
            }
            ai.wait_remaining -= 1;
            ai.wait_remaining == 0
        };
        if linger_done {
            let ai = world.actor_mut(id).ai.as_mut().unwrap();
            ai.mood = Mood::Relaxed;
            ai.focus = None;
            ai.suspicion = 0;
            return ActionIntent::Wait;
        }
        // Look around while lingering: rotate the facing.
        let facing = world.actor(id).facing.unwrap_or(Dir4::North);
        return ActionIntent::TurnFacing(next_clockwise(facing));
    }
    match first_step_towards(world, data, id, focus) {
        Some(dir) => ActionIntent::Step(dir),
        None => {
            let ai = world.actor_mut(id).ai.as_mut().unwrap();
            ai.mood = Mood::Relaxed;
            ai.focus = None;
            ActionIntent::Wait
        }
    }
}

/// Alerted guards pursue and attempt arrest; other alerted actors flee.
fn pursue_intent(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let armed = world
        .actor(id)
        .role
        .and_then(|r| data.role_spec(r))
        .is_some_and(|s| s.armed);
    if !armed {
        return flee_intent(world, data, id);
    }
    let player_pos = world.player_actor().pos;
    let adjacent = world.actor(id).pos.is_adjacent(player_pos);
    if adjacent && world.player_actor().alive() {
        return if world.player_violence_witnessed {
            ActionIntent::MeleeAttack(world.player)
        } else {
            ActionIntent::Arrest(world.player)
        };
    }
    chase_focus(world, data, id)
}

/// Combat: armed actors close and strike lethally.
fn combat_intent(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let armed = world
        .actor(id)
        .role
        .and_then(|r| data.role_spec(r))
        .is_some_and(|s| s.armed);
    if !armed {
        return flee_intent(world, data, id);
    }
    let player_pos = world.player_actor().pos;
    if world.actor(id).pos.is_adjacent(player_pos) && world.player_actor().alive() {
        return ActionIntent::MeleeAttack(world.player);
    }
    chase_focus(world, data, id)
}

/// Walk toward the remembered focus; when it goes stale with nothing
/// found, fall back to the routine while staying on alert.
fn chase_focus(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let focus = world.actor(id).ai.as_ref().unwrap().focus;
    if let Some(focus) = focus {
        if world.actor(id).pos == focus {
            let ai = world.actor_mut(id).ai.as_mut().unwrap();
            ai.focus = None;
            let facing = world.actor(id).facing.unwrap_or(Dir4::North);
            return ActionIntent::TurnFacing(next_clockwise(facing));
        }
        if let Some(dir) = first_step_towards(world, data, id, focus) {
            return ActionIntent::Step(dir);
        }
    }
    // No focus: keep moving through the routine, still alert.
    routine_intent(world, data, id)
}

/// Fleeing actors run for the nearest extraction exit and cower there.
fn flee_intent(world: &mut World, data: &GameData, id: ActorId) -> ActionIntent {
    let pos = world.actor(id).pos;
    let mut exits = world.extraction_tiles.clone();
    exits.sort_by_key(|e| {
        // Storeys apart cost proportionally: with three or more floors a
        // flat penalty would rank the top floor and the next one down as
        // equally far away.
        e.chebyshev(pos).map(i32::from).unwrap_or(i32::MAX / 2)
            + i32::from(e.floor.abs_diff(pos.floor)) * 100
    });
    for exit in exits {
        if pos == exit {
            return ActionIntent::Wait;
        }
        if let Some(dir) = first_step_towards(world, data, id, exit) {
            return ActionIntent::Step(dir);
        }
    }
    ActionIntent::Wait
}

fn next_clockwise(dir: Dir4) -> Dir4 {
    match dir {
        Dir4::North => Dir4::East,
        Dir4::East => Dir4::South,
        Dir4::South => Dir4::West,
        Dir4::West => Dir4::North,
    }
}

/// True when an actor's hands are usable for a new weapon or body action.
pub fn hands_free(world: &World, id: ActorId) -> bool {
    world.actor(id).hands == Hands::Free
}
