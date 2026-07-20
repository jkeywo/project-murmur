//! The movement rulebook: stepping, bump-open doors, displacement swaps,
//! and simultaneous resolution with deterministic tie-breaks.
//!
//! Displacement lives here whole: the pre-turn allowance (a queued step
//! into a displaceable bystander is legal) and the resolution swap that
//! realises it. The bump-open rule likewise appears on both sides — the
//! validator decides a step into a closed door is legal, the resolver
//! turns that step into the door opening.

use crate::access;
use crate::data::GameData;
use crate::geom::{Dir4, Pos};
use crate::map::TileKind;
use crate::world::{ActorId, Hands, World};

use super::{ActionIntent, Blockage, PreparedAction, RejectReason, TurnEvents, complete, fail};

/// Pre-turn validation of a step, including the bump-open door rule.
pub(super) fn validate_move(
    world: &World,
    data: &GameData,
    dir: Dir4,
) -> Result<ActionIntent, RejectReason> {
    let player = world.player_actor();
    let dest = player.pos.step(dir);
    match world.map.tile(dest) {
        TileKind::Wall | TileKind::Void => Err(RejectReason::PathBlocked(Blockage::Wall)),
        TileKind::Door(id) => {
            // Bump-open: stepping into a closed door opens it when
            // unlocked or when the player holds the key (recorded
            // decision); the step itself lands next turn.
            if !world.door(id).open && !access::can_pass_door(world, data, world.player, id) {
                return Err(RejectReason::DoorIsLocked);
            }
            if world.door(id).open {
                validate_step_destination(world, dest, world.player)?;
            }
            Ok(ActionIntent::Step(dir))
        }
        TileKind::Floor | TileKind::Stairs(_) => {
            validate_step_destination(world, dest, world.player)?;
            Ok(ActionIntent::Step(dir))
        }
    }
}

fn validate_step_destination(world: &World, dest: Pos, mover: ActorId) -> Result<(), RejectReason> {
    let landing = world.map.resolve_step_destination(dest);
    if world.furniture_at(dest).is_some() {
        return Err(RejectReason::PathBlocked(Blockage::Furniture));
    }
    if let Some(occupant) = world.standing_actor_at(landing) {
        // Civilians and staff step aside (the mover swaps places with
        // them at resolution) — but not across a stairs transition, where
        // a swap would teleport the bystander between storeys.
        if landing != dest || !world.is_displaceable_by(occupant.id, mover) {
            return Err(RejectReason::OccupiedByActor);
        }
    }
    Ok(())
}

/// Simultaneous movement with deterministic tie-breaks: contested tiles go
/// to one shuffled winner; chains settle iteratively; unresolvable moves
/// fail in-turn.
pub(super) fn resolve_movement(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    applying: &[PreparedAction],
) {
    struct Move {
        actor: ActorId,
        dir: Dir4,
        dest: Pos,
    }
    let mut moves: Vec<Move> = Vec::new();
    for action in applying {
        if let ActionIntent::Step(dir) = action.intent {
            let actor = world.actor(action.actor);
            if !actor.alive() {
                continue;
            }
            let ahead = actor.pos.step(dir);
            // Bump-open: a step into a closed door opens it (with the key
            // if locked) and completes as a door action.
            if let TileKind::Door(id) = world.map.tile(ahead)
                && !world.door(id).open
            {
                if access::can_pass_door(world, data, action.actor, id) {
                    world.door_mut(id).open = true;
                    world.actor_mut(action.actor).facing = Some(dir);
                    complete(world, events, action.actor);
                } else {
                    fail(world, events, action.actor, crate::tr!("fail.door_locked"));
                }
                continue;
            }
            let terrain_open = matches!(
                world.map.tile(ahead),
                TileKind::Floor | TileKind::Stairs(_) | TileKind::Door(_)
            ) && world.furniture_at(ahead).is_none();
            if !terrain_open {
                fail(
                    world,
                    events,
                    action.actor,
                    crate::tr!("fail.blocked_furniture"),
                );
                continue;
            }
            moves.push(Move {
                actor: action.actor,
                dir,
                dest: world.map.resolve_step_destination(ahead),
            });
        }
    }

    // Contested destinations: shuffle deterministically, first claim wins.
    let mut order: Vec<usize> = (0..moves.len()).collect();
    world.resolution_rng.shuffle(&mut order);
    let mut claimed: Vec<Pos> = Vec::new();
    let mut winners: Vec<usize> = Vec::new();
    for index in order {
        let dest = moves[index].dest;
        if claimed.contains(&dest) {
            fail(
                world,
                events,
                moves[index].actor,
                crate::tr!("fail.someone_first"),
            );
        } else {
            claimed.push(dest);
            winners.push(index);
        }
    }
    winners.sort_unstable();

    // Settle: apply moves whose destination is free, repeatedly, so chains
    // (A follows B) resolve. When nothing frees up, mutual swaps apply
    // simultaneously (two actors squeezing past each other); moves that
    // still cannot land fail in-turn.
    fn apply_move(
        world: &mut World,
        events: &mut TurnEvents,
        actor_id: ActorId,
        dir: Dir4,
        dest: Pos,
    ) {
        {
            let actor = world.actor_mut(actor_id);
            actor.pos = dest;
            if actor.facing.is_some() {
                actor.facing = Some(dir);
            }
        }
        // A carried body travels with its carrier.
        if let Hands::CarryingBody(body) = world.actor(actor_id).hands {
            world.actor_mut(body).pos = dest;
        }
        complete(world, events, actor_id);
    }

    let mut pending: Vec<usize> = winners;
    loop {
        let mut progressed = false;
        let mut still_pending: Vec<usize> = Vec::new();
        for index in pending.iter().copied() {
            let dest = moves[index].dest;
            if let Some(occupant) = world.standing_actor_at(dest).map(|a| a.id) {
                // Bystanders step aside: a mover swaps places with a
                // civilian or staff member who is not moving this turn.
                let occupant_is_moving = pending
                    .iter()
                    .any(|&other| other != index && moves[other].actor == occupant);
                if !occupant_is_moving && world.is_displaceable_by(occupant, moves[index].actor) {
                    let origin = world.actor(moves[index].actor).pos;
                    apply_move(world, events, moves[index].actor, moves[index].dir, dest);
                    world.actor_mut(occupant).pos = origin;
                    progressed = true;
                } else {
                    still_pending.push(index);
                }
                continue;
            }
            apply_move(world, events, moves[index].actor, moves[index].dir, dest);
            progressed = true;
        }
        if still_pending.is_empty() {
            break;
        }
        if !progressed {
            // Mutual swaps: both actors step into each other's tile at once.
            let mut consumed: Vec<usize> = Vec::new();
            for (slot, &i) in still_pending.iter().enumerate() {
                if consumed.contains(&i) {
                    continue;
                }
                for &j in &still_pending[slot + 1..] {
                    if consumed.contains(&j) {
                        continue;
                    }
                    let a = moves[i].actor;
                    let b = moves[j].actor;
                    if moves[i].dest == world.actor(b).pos && moves[j].dest == world.actor(a).pos {
                        apply_move(world, events, a, moves[i].dir, moves[i].dest);
                        apply_move(world, events, b, moves[j].dir, moves[j].dest);
                        consumed.push(i);
                        consumed.push(j);
                        progressed = true;
                        break;
                    }
                }
            }
            still_pending.retain(|index| !consumed.contains(index));
        }
        if !progressed {
            for index in still_pending {
                fail(
                    world,
                    events,
                    moves[index].actor,
                    crate::tr!("fail.blocked_person"),
                );
            }
            break;
        }
        pending = still_pending;
    }
}
