//! The body-handling rulebook: carry, drop, and hide.
//!
//! Carrying states its preconditions once — [`carry_core`] — shared by
//! both sides. Drop and hide keep their two sides separate because the
//! in-turn checks deliberately differ: a drop no longer cares about a
//! bystander on the tile (they moved there mid-turn; the body lands
//! anyway), and hide distinguishes a vanished container from a refused
//! one.

use crate::geom::Dir4;
use crate::world::{ActorId, FurnitureId, FurnitureKind, Hands, World};

use super::{ActionIntent, Blockage, RejectReason, TurnEvents, complete, fail};

/// The carry preconditions: a visible, uncarried body in reach, and free
/// hands to lift it with.
fn carry_core(world: &World, actor: ActorId, target: ActorId) -> Result<(), RejectReason> {
    let actor_pos = world.actor(actor).pos;
    let target_ref = world.actor(target);
    if !target_ref.is_visible_body() || world.is_carried(target) {
        return Err(RejectReason::TargetGone);
    }
    if !actor_pos.is_adjacent(target_ref.pos) && actor_pos != target_ref.pos {
        return Err(RejectReason::NotAdjacent);
    }
    if world.actor(actor).hands != Hands::Free {
        return Err(RejectReason::HandsNotFree);
    }
    Ok(())
}

pub(super) fn validate_carry(world: &World, target: ActorId) -> Result<ActionIntent, RejectReason> {
    carry_core(world, world.player, target)?;
    Ok(ActionIntent::CarryBody(target))
}

pub(super) fn resolve_carry(
    world: &mut World,
    events: &mut TurnEvents,
    actor: ActorId,
    target: ActorId,
) {
    if carry_core(world, actor, target).is_err() {
        fail(world, events, actor, crate::tr!("fail.cannot_lift"));
        return;
    }
    let actor_pos = world.actor(actor).pos;
    world.actor_mut(actor).hands = Hands::CarryingBody(target);
    world.actor_mut(target).pos = actor_pos;
    events
        .messages
        .push(crate::tr!("log.body_lifted").to_string());
    complete(world, events, actor);
}

pub(super) fn validate_drop(
    world: &World,
    dir: Option<Dir4>,
) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    match player.hands {
        Hands::CarryingBody(_) => {
            let dest = match dir {
                Some(dir) => player.pos.step(dir),
                None => player.pos,
            };
            if !world.map.walkable(dest, |id| world.door(id).open) {
                return Err(RejectReason::PathBlocked(Blockage::Terrain));
            }
            if world.furniture_at(dest).is_some() {
                return Err(RejectReason::PathBlocked(Blockage::Furniture));
            }
            if world
                .standing_actor_at(dest)
                .is_some_and(|a| a.id != player.id)
            {
                return Err(RejectReason::OccupiedByActor);
            }
            if world.body_at(dest).is_some() {
                return Err(RejectReason::PathBlocked(Blockage::Body));
            }
            Ok(ActionIntent::DropBody(dir))
        }
        _ => Err(RejectReason::NotCarryingBody),
    }
}

pub(super) fn resolve_drop(
    world: &mut World,
    events: &mut TurnEvents,
    actor: ActorId,
    dir: Option<Dir4>,
) {
    let Hands::CarryingBody(body) = world.actor(actor).hands else {
        fail(world, events, actor, crate::tr!("fail.nothing_to_drop"));
        return;
    };
    let pos = world.actor(actor).pos;
    let dest = match dir {
        Some(dir) => pos.step(dir),
        None => pos,
    };
    if !world.map.walkable(dest, |id| world.door(id).open)
        || world.furniture_at(dest).is_some()
        || world.body_at(dest).is_some()
    {
        fail(world, events, actor, crate::tr!("fail.no_room_to_drop"));
        return;
    }
    world.actor_mut(actor).hands = Hands::Free;
    world.actor_mut(body).pos = dest;
    events
        .messages
        .push(crate::tr!("log.body_dropped").to_string());
    complete(world, events, actor);
}

pub(super) fn validate_hide(world: &World, id: FurnitureId) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    let Hands::CarryingBody(_) = player.hands else {
        return Err(RejectReason::NotCarryingBody);
    };
    let furniture = world
        .furniture
        .get(id.0 as usize)
        .ok_or(RejectReason::NotAContainer)?;
    if furniture.kind != FurnitureKind::Container {
        return Err(RejectReason::NotAContainer);
    }
    if !player.pos.is_adjacent(furniture.pos) {
        return Err(RejectReason::NotAdjacent);
    }
    if furniture.body.is_some() {
        return Err(RejectReason::ContainerOccupied);
    }
    Ok(ActionIntent::HideBody(id))
}

pub(super) fn resolve_hide(
    world: &mut World,
    events: &mut TurnEvents,
    actor: ActorId,
    id: FurnitureId,
) {
    let Hands::CarryingBody(body) = world.actor(actor).hands else {
        fail(world, events, actor, crate::tr!("fail.nothing_to_hide"));
        return;
    };
    let actor_pos = world.actor(actor).pos;
    let Some(furniture) = world.furniture.get(id.0 as usize) else {
        fail(world, events, actor, crate::tr!("fail.container_gone"));
        return;
    };
    if furniture.kind != FurnitureKind::Container
        || furniture.body.is_some()
        || !actor_pos.is_adjacent(furniture.pos)
    {
        fail(world, events, actor, crate::tr!("fail.container_refused"));
        return;
    }
    let furniture_pos = furniture.pos;
    world.actor_mut(actor).hands = Hands::Free;
    let body_mut = world.actor_mut(body);
    body_mut.hidden_in = Some(id);
    body_mut.pos = furniture_pos;
    world.furniture_mut(id).body = Some(body);
    events
        .messages
        .push(crate::tr!("log.body_hidden").to_string());
    complete(world, events, actor);
}
