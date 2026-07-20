//! The equipment-and-machines rulebook: draw/holster, noisemaker throws,
//! and opportunity machine effects.
//!
//! The machine resolver deliberately does not recheck adjacency or free
//! hands: interaction began when the action was queued beside the
//! machine, and a multi-turn interaction finishes even if the crowd
//! shifted around it. What it does recheck is the machine itself —
//! present, recognised, and unspent.

use crate::data::GameData;
use crate::geom::Pos;
use crate::map::{TileKind, line_of_sight};
use crate::world::{
    ActorId, BodyCondition, FurnitureId, FurnitureKind, Hands, ItemLocation, World,
};

use super::{
    ActionIntent, ActionResult, Blockage, INVENTORY_SLOTS, RejectReason, TurnEvents,
    carried_firearm, complete, fail, record,
};

pub(super) fn validate_draw_or_holster(
    world: &World,
    data: &GameData,
) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    let (weapon_id, _) =
        carried_firearm(world, data, world.player).ok_or(RejectReason::NoWeaponCarried)?;
    match player.hands {
        Hands::Free => Ok(ActionIntent::DrawOrHolster),
        Hands::Drawn(id) if id == weapon_id => Ok(ActionIntent::DrawOrHolster),
        _ => Err(RejectReason::HandsNotFree),
    }
}

pub(super) fn resolve_draw_or_holster(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
) {
    let weapon = carried_firearm(world, data, actor).map(|(id, _)| id);
    let actor_mut = world.actor_mut(actor);
    match (actor_mut.hands, weapon) {
        (Hands::Free, Some(id)) => {
            actor_mut.hands = Hands::Drawn(id);
            record(world, events, actor, ActionResult::Completed);
            if actor == world.player {
                events.messages.push(crate::tr!("log.draw").to_string());
            }
        }
        (Hands::Drawn(_), _) => {
            actor_mut.hands = Hands::Free;
            record(world, events, actor, ActionResult::Completed);
            if actor == world.player {
                events.messages.push(crate::tr!("log.holster").to_string());
            }
        }
        _ => record(
            world,
            events,
            actor,
            ActionResult::Failed(crate::tr!("fail.hands_busy")),
        ),
    }
}

pub(super) fn validate_throw(
    world: &World,
    data: &GameData,
    pos: Pos,
) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    let charge = world
        .carried_items(world.player)
        .find(|i| data.item(&i.spec).is_some_and(|s| s.noisemaker));
    match charge {
        Some(item) if item.charges > 0 => {}
        _ => return Err(RejectReason::NoNoisemaker),
    }
    match player.pos.chebyshev(pos) {
        Some(d) if d <= data.tuning.noisemaker_range => {}
        _ => return Err(RejectReason::OutOfRange),
    }
    if !matches!(
        world.map.tile(pos),
        TileKind::Floor | TileKind::Stairs(_) | TileKind::Door(_)
    ) {
        return Err(RejectReason::PathBlocked(Blockage::NoThrowTarget));
    }
    if !line_of_sight(player.pos, pos, world.sight_blocker(player.crouched)) {
        return Err(RejectReason::TargetNotVisible);
    }
    Ok(ActionIntent::Throw(pos))
}

pub(super) fn resolve_throw(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
    pos: Pos,
) {
    let charge = world
        .carried_items(actor)
        .find(|i| data.item(&i.spec).is_some_and(|s| s.noisemaker) && i.charges > 0)
        .map(|i| i.id);
    match charge {
        Some(id) => {
            if let Some(item) = world.items.iter_mut().find(|i| i.id == id) {
                item.charges -= 1;
            }
            world.incidents.push(crate::world::Incident {
                kind: crate::world::IncidentKind::Noise,
                pos,
                radius: data.tuning.noise_radius,
                turn: world.turn,
            });
            record(world, events, actor, ActionResult::Completed);
            if actor == world.player {
                events
                    .messages
                    .push(crate::tr!("log.noisemaker").to_string());
            }
        }
        None => record(
            world,
            events,
            actor,
            ActionResult::Failed(crate::tr!("fail.no_charges")),
        ),
    }
}

pub(super) fn validate_interact(
    world: &World,
    id: FurnitureId,
) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    let furniture = world
        .furniture
        .iter()
        .find(|f| f.id == id && f.kind == FurnitureKind::Machine)
        .ok_or(RejectReason::NothingToUse)?;
    if furniture.machine.is_none() {
        return Err(RejectReason::NothingToUse);
    }
    if furniture.used {
        return Err(RejectReason::MachineSpent);
    }
    if !player.pos.is_adjacent(furniture.pos) {
        return Err(RejectReason::NotAdjacent);
    }
    if player.hands != Hands::Free {
        return Err(RejectReason::HandsNotFree);
    }
    Ok(ActionIntent::Interact(id))
}

/// Applies one opportunity machine's effect.
pub(super) fn resolve_interact(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
    id: FurnitureId,
) {
    let Some(furniture) = world.furniture.iter().find(|f| f.id == id) else {
        fail(world, events, actor, crate::tr!("fail.nothing_to_use"));
        return;
    };
    let (spec_id, machine_pos, drop_tile, used) = (
        furniture.machine.clone(),
        furniture.pos,
        furniture.drop_tile,
        furniture.used,
    );
    let Some(spec) = spec_id
        .as_deref()
        .and_then(|s| data.opportunity(s))
        .cloned()
    else {
        fail(world, events, actor, crate::tr!("fail.nothing_to_use"));
        return;
    };
    if used {
        fail(world, events, actor, crate::tr!("fail.machine_spent"));
        return;
    }

    match &spec.effect {
        crate::data::OpportunityEffect::CutLights => {
            for room in &mut world.rooms {
                if room.floor == machine_pos.floor {
                    room.lighting = crate::data::Lighting::Dim;
                }
            }
            events
                .messages
                .push(crate::tr!("log.lights_cut").to_string());
        }
        crate::data::OpportunityEffect::AccidentDrop => {
            let victim = drop_tile.and_then(|tile| world.standing_actor_at(tile).map(|a| a.id));
            match victim {
                Some(victim) => {
                    // A deniable accident: no murder evidence, no
                    // constraint breach, no player attribution.
                    if let Hands::CarryingBody(body) = world.actor(victim).hands {
                        let pos = world.actor(victim).pos;
                        world.actor_mut(body).pos = pos;
                    }
                    let name = world.actor(victim).name.clone();
                    let victim_mut = world.actor_mut(victim);
                    victim_mut.condition = BodyCondition::Dead;
                    victim_mut.hp = 0;
                    victim_mut.hands = Hands::Free;
                    world.incidents.push(crate::world::Incident {
                        kind: crate::world::IncidentKind::Noise,
                        pos: drop_tile.unwrap_or(machine_pos),
                        radius: 8,
                        turn: world.turn,
                    });
                    events
                        .messages
                        .push(crate::trf!("log.hoist_hit", name = name));
                }
                None => {
                    events
                        .messages
                        .push(crate::tr!("log.hoist_miss").to_string());
                }
            }
        }
        crate::data::OpportunityEffect::Evacuate => {
            crate::perception::evacuate(world, machine_pos);
            events
                .messages
                .push(crate::tr!("log.fire_alarm").to_string());
        }
        crate::data::OpportunityEffect::SummonTarget { tag } => {
            // Page the target to a named beat. Because that beat is an
            // alone beat with no_follow set, the detail peels off to its
            // posts by the rules already written — there is no escort code
            // here at all. This is the answer to a target that is
            // unattackable in public being *act*, rather than *wait*.
            let target = world.target;
            let jumped = {
                let Some(ai) = world.actor_mut(target).ai.as_mut() else {
                    fail(world, events, actor, crate::tr!("fail.nothing_happens"));
                    return;
                };
                let Some(schedule) = ai.schedule.as_mut() else {
                    fail(world, events, actor, crate::tr!("fail.nothing_happens"));
                    return;
                };
                match schedule.beats.iter().position(|b| b.tag == *tag) {
                    Some(index) if index != schedule.index => {
                        // Remember where the day was, so the cycle resumes
                        // rather than restarting: the schedule guarantee
                        // rests on every sequential beat still coming round.
                        schedule.resume_index = Some(schedule.index);
                        schedule.index = index;
                        schedule.dwell_remaining = schedule.beats[index].dwell;
                        ai.routine_index = index;
                        ai.wait_remaining = 0;
                        true
                    }
                    _ => false,
                }
            };
            if !jumped {
                fail(world, events, actor, crate::tr!("fail.nothing_happens"));
                return;
            }
            let name = world.actor(target).name.clone();
            events.messages.push(crate::trf!("log.paged", name = name));
        }
        crate::data::OpportunityEffect::PlaceKey { item } => {
            if world.carried_items(actor).count() >= INVENTORY_SLOTS {
                // Same words as the pickpocket refusal today, but a
                // separate id: one is about a mark's pockets and the other
                // about a machine, and a translator may not phrase them
                // alike.
                fail(world, events, actor, crate::tr!("fail.pockets_full_item"));
                return;
            }
            let spec_item = data.item(item).expect("validated at load");
            let new_id = crate::world::ItemId(world.items.len() as u32);
            world.items.push(crate::world::ItemInstance {
                id: new_id,
                spec: spec_item.id.clone(),
                location: ItemLocation::CarriedBy(actor),
                charges: spec_item.charges,
            });
            events
                .messages
                .push(crate::trf!("log.key_taken", item = spec_item.name));
        }
        crate::data::OpportunityEffect::StockWardrobe { .. } => {
            fail(world, events, actor, crate::tr!("fail.nothing_to_use"));
            return;
        }
    }

    if let Some(furniture) = world.furniture.iter_mut().find(|f| f.id == id) {
        furniture.used = true;
    }
    complete(world, events, actor);
}
