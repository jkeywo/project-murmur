//! The leash: leading a person and releasing them.
//!
//! A social verb, not a violent one. Leading toggles a standing
//! `following` assignment on the person's mind — orthogonal to mood, like a
//! bodyguard's detail — so fear always outranks the leash and a frightened
//! follower simply stops trailing until it calms, without the assignment
//! being lost. The AI acts on the assignment (see `ai.rs`); this module only
//! sets and clears it.

use crate::data::Role;
use crate::world::{ActorId, World};

use super::{ActionIntent, RejectReason, TurnEvents, complete, fail};

/// Whether `target` can be taken onto `leader`'s leash right now. Releasing
/// someone already following is always allowed; taking a new follower
/// requires a living, adjacent, non-hostile person who is not a guard.
pub(super) fn validate_lead(world: &World, target: ActorId) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    let target_ref = world.actor(target);
    if target_ref.is_player()
        || !target_ref.alive()
        || target_ref.hidden_in.is_some()
        || world.is_carried(target)
    {
        return Err(RejectReason::NotLeadable);
    }
    if !player.pos.is_adjacent(target_ref.pos) {
        return Err(RejectReason::NotAdjacent);
    }
    let already = target_ref.ai.as_ref().and_then(|ai| ai.following) == Some(world.player);
    if !already {
        let hostile = target_ref.role == Some(Role::Guard)
            || target_ref
                .ai
                .as_ref()
                .is_some_and(|ai| ai.knows_player_hostile);
        if hostile {
            return Err(RejectReason::NotLeadable);
        }
    }
    Ok(ActionIntent::Lead(target))
}

/// Toggles the leash: a person already following `leader` is released,
/// anyone else starts following. Pure state change; no RNG, no movement.
pub(super) fn resolve_lead(
    world: &mut World,
    events: &mut TurnEvents,
    leader: ActorId,
    target: ActorId,
) {
    let Some(following) = world.actor(target).ai.as_ref().map(|ai| ai.following) else {
        // A person with no mind cannot be led; defensive, since only living
        // NPCs are valid targets and they all have an AI.
        fail(world, events, leader, crate::tr!("fail.mark_slipped"));
        return;
    };
    let name = world.actor(target).name.clone();
    let ai = world
        .actor_mut(target)
        .ai
        .as_mut()
        .expect("target has an AI");
    if following == Some(leader) {
        ai.following = None;
        events
            .messages
            .push(crate::trf!("log.lead_release", name = name));
    } else {
        ai.following = Some(leader);
        events
            .messages
            .push(crate::trf!("log.lead_start", name = name));
    }
    complete(world, events, leader);
}
