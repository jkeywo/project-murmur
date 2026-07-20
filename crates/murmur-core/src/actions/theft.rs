//! The theft rulebook: pickpocketing and disguise changes.
//!
//! Reaching the mark is stated once — [`pickpocket_reachable`] — and
//! shared. The remaining checks intentionally diverge: the validator
//! screens what is stealable before checking pocket space (so the player
//! hears the more useful refusal), while the resolver checks pockets
//! first because finding the item and taking it are one step there.

use crate::data::GameData;
use crate::world::{ActorId, FurnitureId, FurnitureKind, Hands, ItemLocation, World};

use super::{
    ActionIntent, DisguiseSource, INVENTORY_SLOTS, RejectReason, TurnEvents, complete, fail,
};

/// Whether the mark can be stolen from at all: present and in reach.
fn pickpocket_reachable(
    world: &World,
    actor: ActorId,
    target: ActorId,
) -> Result<(), RejectReason> {
    let actor_pos = world.actor(actor).pos;
    let target_ref = world.actor(target);
    if target_ref.hidden_in.is_some() || world.is_carried(target) {
        return Err(RejectReason::TargetGone);
    }
    if !actor_pos.is_adjacent(target_ref.pos) && actor_pos != target_ref.pos {
        return Err(RejectReason::NotAdjacent);
    }
    Ok(())
}

pub(super) fn validate_pickpocket(
    world: &World,
    data: &GameData,
    target: ActorId,
) -> Result<ActionIntent, RejectReason> {
    pickpocket_reachable(world, world.player, target)?;
    let target_ref = world.actor(target);
    let stealable = world.carried_items(target).any(|i| {
        data.item(&i.spec)
            .is_some_and(|s| s.pickpocketable || !target_ref.alive())
    });
    if !stealable {
        return Err(RejectReason::NothingToSteal);
    }
    if world.carried_items(world.player).count() >= INVENTORY_SLOTS {
        return Err(RejectReason::InventoryFull);
    }
    Ok(ActionIntent::Pickpocket(target))
}

pub(super) fn resolve_pickpocket(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
    target: ActorId,
) {
    if pickpocket_reachable(world, actor, target).is_err() {
        fail(world, events, actor, crate::tr!("fail.mark_slipped"));
        return;
    }
    if world.carried_items(actor).count() >= INVENTORY_SLOTS {
        fail(world, events, actor, crate::tr!("fail.pockets_full"));
        return;
    }
    let target_alive = world.actor(target).alive();
    let stolen = world
        .items
        .iter_mut()
        .find(|i| {
            i.location == ItemLocation::CarriedBy(target)
                && data
                    .item(&i.spec)
                    .is_some_and(|s| s.pickpocketable || !target_alive)
        })
        .map(|item| {
            item.location = ItemLocation::CarriedBy(actor);
            item.spec.clone()
        });
    match stolen {
        Some(spec) => {
            let name = data.item(&spec).map(|s| s.name.clone()).unwrap_or(spec);
            events
                .messages
                .push(crate::trf!("log.pickpocket", item = name));
            complete(world, events, actor);
        }
        None => fail(world, events, actor, crate::tr!("fail.nothing_to_steal")),
    }
}

pub(super) fn validate_take_from_body(
    world: &World,
    target: ActorId,
) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    let target_ref = world.actor(target);
    if target_ref.alive() {
        return Err(RejectReason::TargetNotIncapacitated);
    }
    if target_ref.hidden_in.is_some() || world.is_carried(target) {
        return Err(RejectReason::TargetGone);
    }
    if !player.pos.is_adjacent(target_ref.pos) && player.pos != target_ref.pos {
        return Err(RejectReason::NotAdjacent);
    }
    if player.hands != Hands::Free {
        return Err(RejectReason::HandsNotFree);
    }
    if target_ref.worn_disguise == player.worn_disguise {
        return Err(RejectReason::NoDisguiseThere);
    }
    Ok(ActionIntent::TakeDisguise(DisguiseSource::Body(target)))
}

pub(super) fn validate_take_from_wardrobe(
    world: &World,
    id: FurnitureId,
) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    let furniture = world
        .furniture
        .get(id.0 as usize)
        .ok_or(RejectReason::TargetGone)?;
    if furniture.kind != FurnitureKind::Wardrobe {
        return Err(RejectReason::NoDisguiseThere);
    }
    if !player.pos.is_adjacent(furniture.pos) {
        return Err(RejectReason::NotAdjacent);
    }
    if player.hands != Hands::Free {
        return Err(RejectReason::HandsNotFree);
    }
    match &furniture.disguise {
        Some(d) if *d != player.worn_disguise => {
            Ok(ActionIntent::TakeDisguise(DisguiseSource::Wardrobe(id)))
        }
        _ => Err(RejectReason::NoDisguiseThere),
    }
}

pub(super) fn resolve_take_disguise(
    world: &mut World,
    events: &mut TurnEvents,
    actor: ActorId,
    source: DisguiseSource,
) {
    let actor_pos = world.actor(actor).pos;
    let hands_free = world.actor(actor).hands == Hands::Free;
    if !hands_free {
        fail(world, events, actor, crate::tr!("fail.hands_busy"));
        return;
    }
    match source {
        DisguiseSource::Body(target) => {
            let target_ref = world.actor(target);
            let adjacent = actor_pos.is_adjacent(target_ref.pos) || actor_pos == target_ref.pos;
            if target_ref.alive() || target_ref.hidden_in.is_some() || !adjacent {
                fail(world, events, actor, crate::tr!("fail.clothes_far"));
                return;
            }
            let taken = world.actor(target).worn_disguise.clone();
            let own = world.actor(actor).worn_disguise.clone();
            world.actor_mut(target).worn_disguise = own;
            world.actor_mut(actor).worn_disguise = taken.clone();
            events
                .messages
                .push(crate::trf!("log.disguise_taken", disguise = taken));
            complete(world, events, actor);
        }
        DisguiseSource::Wardrobe(id) => {
            let Some(furniture) = world.furniture.get(id.0 as usize) else {
                fail(world, events, actor, crate::tr!("fail.wardrobe_gone"));
                return;
            };
            if furniture.kind != FurnitureKind::Wardrobe || !actor_pos.is_adjacent(furniture.pos) {
                fail(world, events, actor, crate::tr!("fail.wardrobe_far"));
                return;
            }
            let Some(taken) = furniture.disguise.clone() else {
                fail(world, events, actor, crate::tr!("fail.wardrobe_empty"));
                return;
            };
            let own = world.actor(actor).worn_disguise.clone();
            world.furniture_mut(id).disguise = Some(own);
            world.actor_mut(actor).worn_disguise = taken.clone();
            events
                .messages
                .push(crate::trf!("log.disguise_taken", disguise = taken));
            complete(world, events, actor);
        }
    }
}
