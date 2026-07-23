//! The violence rulebook: garrote, shoot, melee, and arrest.
//!
//! Garrote and shoot each state their preconditions exactly once —
//! [`garrote_core`] and [`shoot_core`] — checked by the validator at
//! queue time (with the specific rejection) and again by the resolver
//! in-turn (mapped to a coarser failure line), because a prepared attack
//! can be invalidated by everything that resolves before it. The one
//! check the resolver deliberately skips is the garrote-in-inventory
//! gate: nothing in a resolving turn can take the wire away.

use crate::data::GameData;
use crate::map::line_of_sight;
use crate::world::{ActorId, Hands, MissionOutcome, World};

use super::{ActionIntent, RejectReason, TurnEvents, carried_firearm, complete, fail, kill};

/// Line of sight between two actors, through the crouch-aware blocker.
fn sees_actor(world: &World, actor: ActorId, target: ActorId) -> bool {
    let actor_ref = world.actor(actor);
    let target_ref = world.actor(target);
    let crouched = actor_ref.crouched || target_ref.crouched;
    line_of_sight(actor_ref.pos, target_ref.pos, world.sight_blocker(crouched))
}

/// The garrote's volatile preconditions: everything that can change
/// between queueing and resolving.
fn garrote_core(world: &World, actor: ActorId, target: ActorId) -> Result<(), RejectReason> {
    let target_ref = world.actor(target);
    if !target_ref.alive() || target_ref.hidden_in.is_some() {
        return Err(RejectReason::TargetGone);
    }
    if world.actor(actor).hands != Hands::Free {
        return Err(RejectReason::HandsNotFree);
    }
    let Some(facing) = target_ref.facing else {
        return Err(RejectReason::TargetGone);
    };
    if target_ref.pos.step(facing.opposite()) != world.actor(actor).pos {
        return Err(RejectReason::NotBehindTarget);
    }
    Ok(())
}

pub(super) fn validate_garrote(
    world: &World,
    data: &GameData,
    target: ActorId,
) -> Result<ActionIntent, RejectReason> {
    let carries_garrote = world
        .carried_items(world.player)
        .any(|i| data.item(&i.spec).is_some_and(|s| s.weapon && !s.firearm));
    if !carries_garrote {
        return Err(RejectReason::NoGarrote);
    }
    garrote_core(world, world.player, target)?;
    Ok(ActionIntent::Garrote(target))
}

pub(super) fn resolve_garrote(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
    target: ActorId,
) {
    if garrote_core(world, actor, target).is_err() {
        fail(world, events, actor, crate::tr!("fail.garrote_missed"));
        return;
    }
    kill(world, data, events, actor, target);
    let name = world.actor(target).name.clone();
    events
        .messages
        .push(crate::trf!("log.garrotted", name = name));
    complete(world, events, actor);
}

/// The shot's preconditions, in the order the validator reports them:
/// weapon, hands, ammunition, then the target's visibility.
fn shoot_core(
    world: &World,
    data: &GameData,
    actor: ActorId,
    target: ActorId,
) -> Result<(), RejectReason> {
    let (weapon_id, charges) =
        carried_firearm(world, data, actor).ok_or(RejectReason::NoWeaponCarried)?;
    match world.actor(actor).hands {
        Hands::Drawn(id) if id == weapon_id => {}
        Hands::CarryingBody(_) => return Err(RejectReason::HandsNotFree),
        _ => return Err(RejectReason::WeaponNotDrawn),
    }
    if charges == 0 {
        return Err(RejectReason::NoAmmo);
    }
    let target_ref = world.actor(target);
    if !target_ref.alive() || target_ref.hidden_in.is_some() {
        return Err(RejectReason::TargetGone);
    }
    match world.actor(actor).pos.chebyshev(target_ref.pos) {
        Some(d) if d <= data.tuning.pistol_range => {}
        _ => return Err(RejectReason::OutOfRange),
    }
    if !sees_actor(world, actor, target) {
        return Err(RejectReason::TargetNotVisible);
    }
    Ok(())
}

pub(super) fn validate_shoot(
    world: &World,
    data: &GameData,
    target: ActorId,
) -> Result<ActionIntent, RejectReason> {
    shoot_core(world, data, world.player, target)?;
    Ok(ActionIntent::Shoot(target))
}

pub(super) fn resolve_shoot(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
    target: ActorId,
) {
    if let Err(reason) = shoot_core(world, data, actor, target) {
        let why = match reason {
            RejectReason::NoWeaponCarried => crate::tr!("fail.no_weapon"),
            RejectReason::NoAmmo | RejectReason::WeaponNotDrawn | RejectReason::HandsNotFree => {
                crate::tr!("fail.cannot_fire")
            }
            _ => crate::tr!("fail.no_line"),
        };
        fail(world, events, actor, why);
        return;
    }
    let shooter_pos = world.actor(actor).pos;
    let target_pos = world.actor(target).pos;
    let (weapon_id, _) = carried_firearm(world, data, actor).expect("checked by shoot_core");
    let cheats_endless = world.cheats.endless_ammo;
    if !cheats_endless && let Some(item) = world.items.iter_mut().find(|i| i.id == weapon_id) {
        item.charges -= 1;
    }
    if actor == world.player
        && let Some(reason) = world.constraint.as_ref().and_then(|c| c.on_shot())
    {
        super::breach_constraint(world, events, &reason);
    }
    // Whoever is standing on the line takes the round. Actors do not block
    // sight — that is deliberate and symmetric, and adding it would rewrite
    // suspicion and alert propagation against every tuned number — but a
    // body absolutely stops a bullet. This is what makes a bodyguard
    // detail worth walking through: firing into it costs a round, a
    // gunshot, a witnessed death, and leaves the principal alive.
    let hit = interposed_actor(world, shooter_pos, target_pos, target).unwrap_or(target);

    kill(world, data, events, actor, hit);
    // Silenced, but still a local sound incident.
    world.incidents.push(crate::world::Incident {
        kind: crate::world::IncidentKind::Gunshot,
        pos: shooter_pos,
        radius: data.tuning.gunshot_sound_radius,
        turn: world.turn,
    });
    let name = world.actor(hit).name.clone();
    if hit == target {
        events.messages.push(crate::trf!("log.shot", name = name));
    } else {
        events
            .messages
            .push(crate::trf!("log.shot_interposed", name = name));
    }
    complete(world, events, actor);
}

/// The first standing actor strictly between shooter and target, if any.
/// Tiles are walked from the shooter outwards, so the nearest body is the
/// one that stops the round.
fn interposed_actor(
    world: &World,
    from: crate::geom::Pos,
    to: crate::geom::Pos,
    target: ActorId,
) -> Option<ActorId> {
    if from.floor != to.floor {
        return None;
    }
    // Anyone standing on the line stops the round.
    let on_the_line = crate::geom::supercover_between(from, to)
        .into_iter()
        .find_map(|pos| {
            world
                .standing_actor_at(pos)
                .filter(|a| !a.is_player())
                .map(|a| a.id)
        });
    if on_the_line.is_some() {
        return on_the_line;
    }

    // A detail covers its principal. Standing on the exact ray is far too
    // narrow a rule for what a bodyguard is *for*: with three guards on
    // four sides, most firing angles have a clean line, and an escorted
    // target could simply be shot from across the room — which would leave
    // the whole escorted/alone distinction with no teeth in actual play,
    // however carefully the route planner reasons about it.
    //
    // So a bodyguard standing beside its principal takes the shot instead.
    // The nearest to the shooter goes first, ties broken by ascending
    // actor id so the choice never depends on iteration order.
    let principal_pos = world.actor(target).pos;
    let mut covering: Vec<(i16, ActorId)> = world
        .actors
        .iter()
        .filter(|a| {
            a.alive()
                && !a.departed
                && a.pos.is_adjacent(principal_pos)
                && a.ai.as_ref().and_then(|ai| ai.detail.as_ref()).is_some_and(
                    |crate::world::DetailRole::Bodyguard { principal, .. }| *principal == target,
                )
        })
        .map(|a| (a.pos.chebyshev(from).unwrap_or(i16::MAX), a.id))
        .collect();
    covering.sort();
    covering.first().map(|(_, id)| *id)
}

pub(super) fn resolve_melee(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
    target: ActorId,
) {
    let attacker_pos = world.actor(actor).pos;
    let target_ref = world.actor(target);
    if !target_ref.alive() || !attacker_pos.is_adjacent(target_ref.pos) {
        fail(world, events, actor, crate::tr!("fail.blow_missed"));
        return;
    }
    let damage = data.tuning.guard_attack_damage;
    let target_mut = world.actor_mut(target);
    target_mut.hp = target_mut.hp.saturating_sub(damage);
    if target_mut.hp == 0 {
        kill(world, data, events, actor, target);
        let name = world.actor(target).name.clone();
        events
            .messages
            .push(crate::trf!("log.struck_down", name = name));
    } else {
        let name = world.actor(target).name.clone();
        events.messages.push(crate::trf!("log.struck", name = name));
    }
    complete(world, events, actor);
}

pub(super) fn resolve_arrest(
    world: &mut World,
    events: &mut TurnEvents,
    actor: ActorId,
    target: ActorId,
) {
    let arrester_pos = world.actor(actor).pos;
    let target_ref = world.actor(target);
    if target == world.player && target_ref.alive() && arrester_pos.is_adjacent(target_ref.pos) {
        world.outcome = Some(MissionOutcome::Arrested);
        events.messages.push(crate::tr!("log.arrest").to_string());
        complete(world, events, actor);
    } else {
        fail(world, events, actor, crate::tr!("fail.arrest_failed"));
    }
}
