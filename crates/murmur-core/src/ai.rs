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
use crate::geom::Dir4;
use crate::path::first_step_towards;
use crate::world::{ActorId, Hands, Mood, World};

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
        let intent = match mood {
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
