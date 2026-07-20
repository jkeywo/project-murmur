//! The door rulebook: open, close, and pick-lock.
//!
//! The two sides deliberately differ: the validator rejects opening an
//! already-open door (a wasted queue entry), while the resolver treats it
//! as an idempotent success (the world got there first, the intent is
//! satisfied). Closing rechecks the doorway in-turn because an actor may
//! have stepped into it since the command was queued.

use crate::access;
use crate::data::GameData;
use crate::map::DoorId;
use crate::world::{ActorId, Hands, World};

use super::{
    ActionIntent, ActionResult, RejectReason, TurnEvents, adjacent_door_pos, door_position, record,
};

pub(super) fn validate_open(
    world: &World,
    data: &GameData,
    id: DoorId,
) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    adjacent_door_pos(world, player.pos, id).ok_or(RejectReason::DoorNotAdjacent)?;
    if world.door(id).open {
        return Err(RejectReason::DoorAlreadyOpen);
    }
    if !access::can_pass_door(world, data, world.player, id) {
        return Err(RejectReason::DoorIsLocked);
    }
    Ok(ActionIntent::OpenDoor(id))
}

pub(super) fn validate_close(world: &World, id: DoorId) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    let door_pos = adjacent_door_pos(world, player.pos, id).ok_or(RejectReason::DoorNotAdjacent)?;
    if !world.door(id).open {
        return Err(RejectReason::DoorAlreadyClosed);
    }
    if world.standing_actor_at(door_pos).is_some() {
        return Err(RejectReason::DoorBlocked);
    }
    Ok(ActionIntent::CloseDoor(id))
}

pub(super) fn validate_pick_lock(
    world: &World,
    data: &GameData,
    door: DoorId,
) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    let carries_picks = world
        .carried_items(world.player)
        .any(|i| data.item(&i.spec).is_some_and(|s| s.lockpick));
    if !carries_picks {
        return Err(RejectReason::NoLockpicks);
    }
    adjacent_door_pos(world, player.pos, door).ok_or(RejectReason::DoorNotAdjacent)?;
    if world.door(door).locked_by.is_none() {
        return Err(RejectReason::DoorNotLocked);
    }
    if player.hands != Hands::Free {
        return Err(RejectReason::HandsNotFree);
    }
    Ok(ActionIntent::PickLock(door))
}

pub(super) fn resolve_open(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
    id: DoorId,
) {
    if world.door(id).open {
        record(world, events, actor, ActionResult::Completed);
    } else if access::can_pass_door(world, data, actor, id) {
        world.door_mut(id).open = true;
        record(world, events, actor, ActionResult::Completed);
    } else {
        record(
            world,
            events,
            actor,
            ActionResult::Failed(crate::tr!("fail.door_locked")),
        );
    }
}

pub(super) fn resolve_close(
    world: &mut World,
    events: &mut TurnEvents,
    actor: ActorId,
    id: DoorId,
) {
    let door_pos = door_position(world, id);
    let blocked = door_pos.is_some_and(|pos| world.standing_actor_at(pos).is_some());
    if world.door(id).open && !blocked {
        world.door_mut(id).open = false;
        record(world, events, actor, ActionResult::Completed);
    } else {
        record(
            world,
            events,
            actor,
            ActionResult::Failed(crate::tr!("fail.doorway_blocked")),
        );
    }
}

/// Picking permanently defeats the lock and leaves the door open.
pub(super) fn resolve_pick_lock(
    world: &mut World,
    events: &mut TurnEvents,
    actor: ActorId,
    id: DoorId,
) {
    if world.door(id).locked_by.is_some() {
        let door = world.door_mut(id);
        door.locked_by = None;
        door.open = true;
        record(world, events, actor, ActionResult::Completed);
        if actor == world.player {
            events
                .messages
                .push(crate::tr!("log.lock_picked").to_string());
        }
    } else {
        record(
            world,
            events,
            actor,
            ActionResult::Failed(crate::tr!("fail.door_not_locked")),
        );
    }
}
