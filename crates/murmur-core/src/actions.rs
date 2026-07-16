//! Primitive actions: commands, intents, translation, and resolution.
//!
//! Three layers, per the foundation:
//!
//! * [`Command`] — a queued player intention. Never prevalidated; carries
//!   stable domain IDs for targets, resolved only at execution time.
//! * [`ActionIntent`] — a controller-neutral primitive action for one
//!   specific turn. Human, AI, replay, and test controllers all submit the
//!   same intents; once a batch is frozen, resolution never branches on
//!   the source of an action.
//! * Resolution — applies one frozen batch simultaneously against the
//!   authoritative world, distinguishing pre-turn command *rejection*
//!   (pure, no time passes) from in-turn action *failure* (time passes).

use serde::{Deserialize, Serialize};

use crate::access;
use crate::data::GameData;
use crate::geom::{Dir4, Pos};
use crate::map::{DoorId, TileKind, line_of_sight};
use crate::world::{
    ActorId, BodyCondition, FurnitureId, FurnitureKind, Hands, ItemLocation, MissionOutcome, World,
};

/// A queued player intention. Targets are stable domain IDs captured when
/// the command was written, validated against the live world only when the
/// command reaches the queue head.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    Move(Dir4),
    Wait,
    ToggleCrouch,
    OpenDoor(DoorId),
    CloseDoor(DoorId),
    Garrote(ActorId),
    Shoot(ActorId),
    /// Steals from the living; loots the dead and unconscious.
    Pickpocket(ActorId),
    TakeDisguiseFromBody(ActorId),
    TakeDisguiseFromWardrobe(FurnitureId),
    CarryBody(ActorId),
    DropBody,
    HideBody(FurnitureId),
    DrawOrHolster,
}

/// Where a disguise change sources its clothes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisguiseSource {
    Body(ActorId),
    Wardrobe(FurnitureId),
}

/// A controller-neutral primitive action prepared for one specific turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionIntent {
    Wait,
    Step(Dir4),
    ToggleCrouch,
    OpenDoor(DoorId),
    CloseDoor(DoorId),
    Garrote(ActorId),
    Shoot(ActorId),
    Pickpocket(ActorId),
    TakeDisguise(DisguiseSource),
    CarryBody(ActorId),
    DropBody,
    HideBody(FurnitureId),
    DrawOrHolster,
    /// NPC-only in practice: turn on the spot to face a direction.
    TurnFacing(Dir4),
    /// NPC-only in practice: adjacent lethal melee.
    MeleeAttack(ActorId),
    /// NPC-only in practice: adjacent arrest ending the mission.
    Arrest(ActorId),
}

/// Why a command could not be submitted from the pre-turn state.
/// Rejection is pure: no turn passes, no state mutates, no randomness is
/// consumed, and prepared AI actions stay fixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectReason {
    PathBlocked,
    OccupiedByActor,
    DoorIsLocked,
    DoorAlreadyOpen,
    DoorAlreadyClosed,
    DoorNotAdjacent,
    DoorBlocked,
    NotAdjacent,
    TargetGone,
    NotBehindTarget,
    HandsNotFree,
    NotCarryingBody,
    NotAContainer,
    ContainerOccupied,
    NoDisguiseThere,
    TargetNotIncapacitated,
    InventoryFull,
    NothingToSteal,
    NoWeaponCarried,
    NoAmmo,
    WeaponNotDrawn,
    TargetNotVisible,
    OutOfRange,
    MissionOver,
}

impl RejectReason {
    pub fn message(&self) -> &'static str {
        match self {
            RejectReason::PathBlocked => "the way is blocked",
            RejectReason::OccupiedByActor => "someone is standing there",
            RejectReason::DoorIsLocked => "the door is locked",
            RejectReason::DoorAlreadyOpen => "the door is already open",
            RejectReason::DoorAlreadyClosed => "the door is already closed",
            RejectReason::DoorNotAdjacent => "no such door within reach",
            RejectReason::DoorBlocked => "the doorway is blocked",
            RejectReason::NotAdjacent => "not close enough",
            RejectReason::TargetGone => "they are no longer there",
            RejectReason::NotBehindTarget => "you must be directly behind them",
            RejectReason::HandsNotFree => "your hands are not free",
            RejectReason::NotCarryingBody => "you are not carrying a body",
            RejectReason::NotAContainer => "that cannot hold a body",
            RejectReason::ContainerOccupied => "it is already occupied",
            RejectReason::NoDisguiseThere => "no usable clothing there",
            RejectReason::TargetNotIncapacitated => "they are in no state to undress",
            RejectReason::InventoryFull => "your pockets are full",
            RejectReason::NothingToSteal => "nothing worth taking",
            RejectReason::NoWeaponCarried => "you carry no weapon",
            RejectReason::NoAmmo => "the pistol is empty",
            RejectReason::WeaponNotDrawn => "your weapon is holstered",
            RejectReason::TargetNotVisible => "no line of sight",
            RejectReason::OutOfRange => "out of range",
            RejectReason::MissionOver => "the mission is over",
        }
    }
}

/// How long an intent takes, in turns, from authored data.
pub fn intent_duration(
    data: &GameData,
    world: &World,
    actor: ActorId,
    intent: &ActionIntent,
) -> u16 {
    let durations = &data.tuning.durations;
    match intent {
        ActionIntent::Step(_) => {
            if matches!(world.actor(actor).hands, Hands::CarryingBody(_)) {
                durations.carry_step
            } else {
                durations.step
            }
        }
        ActionIntent::Wait | ActionIntent::ToggleCrouch | ActionIntent::TurnFacing(_) => 1,
        ActionIntent::OpenDoor(_) | ActionIntent::CloseDoor(_) => durations.door,
        ActionIntent::Garrote(_) => durations.garrote,
        ActionIntent::Shoot(_) => durations.shoot,
        ActionIntent::Pickpocket(_) => durations.pickpocket,
        ActionIntent::TakeDisguise(_) => durations.change_disguise,
        ActionIntent::CarryBody(_) => durations.carry_body,
        ActionIntent::DropBody => durations.drop_body,
        ActionIntent::HideBody(_) => durations.hide_body,
        ActionIntent::DrawOrHolster => durations.draw_holster,
        ActionIntent::MeleeAttack(_) | ActionIntent::Arrest(_) => 1,
    }
}

/// The player's six general inventory slots.
pub const INVENTORY_SLOTS: usize = 6;

fn player_sees_actor(world: &World, target: ActorId) -> bool {
    let player = world.player_actor();
    let target_ref = world.actor(target);
    let crouched = player.crouched || target_ref.crouched;
    line_of_sight(player.pos, target_ref.pos, world.sight_blocker(crouched))
}

/// Pure, execution-time validation and translation of one player command
/// against the current world. On `Err`, nothing has changed anywhere.
pub fn translate(
    world: &World,
    data: &GameData,
    command: &Command,
) -> Result<ActionIntent, RejectReason> {
    if world.outcome.is_some() {
        return Err(RejectReason::MissionOver);
    }
    let player = world.player_actor();
    match *command {
        Command::Wait => Ok(ActionIntent::Wait),
        Command::ToggleCrouch => Ok(ActionIntent::ToggleCrouch),
        Command::Move(dir) => {
            let dest = player.pos.step(dir);
            match world.map.tile(dest) {
                TileKind::Wall | TileKind::Void => Err(RejectReason::PathBlocked),
                TileKind::Door(id) => {
                    // Bump-open: stepping into a closed door opens it when
                    // unlocked or when the player holds the key (recorded
                    // decision); the step itself lands next turn.
                    if !world.door(id).open && !access::can_pass_door(world, data, world.player, id)
                    {
                        return Err(RejectReason::DoorIsLocked);
                    }
                    if world.door(id).open {
                        validate_step_destination(world, dest)?;
                    }
                    Ok(ActionIntent::Step(dir))
                }
                TileKind::Floor | TileKind::Stairs => {
                    validate_step_destination(world, dest)?;
                    Ok(ActionIntent::Step(dir))
                }
            }
        }
        Command::OpenDoor(id) => {
            let door_pos =
                adjacent_door_pos(world, player.pos, id).ok_or(RejectReason::DoorNotAdjacent)?;
            let _ = door_pos;
            if world.door(id).open {
                return Err(RejectReason::DoorAlreadyOpen);
            }
            if !access::can_pass_door(world, data, world.player, id) {
                return Err(RejectReason::DoorIsLocked);
            }
            Ok(ActionIntent::OpenDoor(id))
        }
        Command::CloseDoor(id) => {
            let door_pos =
                adjacent_door_pos(world, player.pos, id).ok_or(RejectReason::DoorNotAdjacent)?;
            if !world.door(id).open {
                return Err(RejectReason::DoorAlreadyClosed);
            }
            if world.standing_actor_at(door_pos).is_some() {
                return Err(RejectReason::DoorBlocked);
            }
            Ok(ActionIntent::CloseDoor(id))
        }
        Command::Garrote(target) => {
            let target_ref = world.actor(target);
            if !target_ref.alive() || target_ref.hidden_in.is_some() {
                return Err(RejectReason::TargetGone);
            }
            if player.hands != Hands::Free {
                return Err(RejectReason::HandsNotFree);
            }
            let Some(facing) = target_ref.facing else {
                return Err(RejectReason::TargetGone);
            };
            if target_ref.pos.step(facing.opposite()) != player.pos {
                return Err(RejectReason::NotBehindTarget);
            }
            Ok(ActionIntent::Garrote(target))
        }
        Command::Shoot(target) => {
            let pistol = world
                .carried_items(world.player)
                .find(|i| data.item(&i.spec).is_some_and(|s| s.weapon))
                .ok_or(RejectReason::NoWeaponCarried)?;
            match player.hands {
                Hands::Drawn(id) if id == pistol.id => {}
                Hands::CarryingBody(_) => return Err(RejectReason::HandsNotFree),
                _ => return Err(RejectReason::WeaponNotDrawn),
            }
            if pistol.charges == 0 {
                return Err(RejectReason::NoAmmo);
            }
            let target_ref = world.actor(target);
            if !target_ref.alive() || target_ref.hidden_in.is_some() {
                return Err(RejectReason::TargetGone);
            }
            match player.pos.chebyshev(target_ref.pos) {
                Some(d) if d <= data.tuning.pistol_range => {}
                _ => return Err(RejectReason::OutOfRange),
            }
            if !player_sees_actor(world, target) {
                return Err(RejectReason::TargetNotVisible);
            }
            Ok(ActionIntent::Shoot(target))
        }
        Command::Pickpocket(target) => {
            let target_ref = world.actor(target);
            if target_ref.hidden_in.is_some() || world.is_carried(target) {
                return Err(RejectReason::TargetGone);
            }
            if !player.pos.is_adjacent(target_ref.pos) && player.pos != target_ref.pos {
                return Err(RejectReason::NotAdjacent);
            }
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
        Command::TakeDisguiseFromBody(target) => {
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
        Command::TakeDisguiseFromWardrobe(id) => {
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
        Command::CarryBody(target) => {
            let target_ref = world.actor(target);
            if !target_ref.is_visible_body() || world.is_carried(target) {
                return Err(RejectReason::TargetGone);
            }
            if !player.pos.is_adjacent(target_ref.pos) && player.pos != target_ref.pos {
                return Err(RejectReason::NotAdjacent);
            }
            if player.hands != Hands::Free {
                return Err(RejectReason::HandsNotFree);
            }
            Ok(ActionIntent::CarryBody(target))
        }
        Command::DropBody => match player.hands {
            Hands::CarryingBody(_) => {
                if world.body_at(player.pos).is_some() {
                    return Err(RejectReason::PathBlocked);
                }
                Ok(ActionIntent::DropBody)
            }
            _ => Err(RejectReason::NotCarryingBody),
        },
        Command::HideBody(id) => {
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
        Command::DrawOrHolster => {
            let pistol = world
                .carried_items(world.player)
                .find(|i| data.item(&i.spec).is_some_and(|s| s.weapon))
                .ok_or(RejectReason::NoWeaponCarried)?;
            match player.hands {
                Hands::Free => Ok(ActionIntent::DrawOrHolster),
                Hands::Drawn(id) if id == pistol.id => Ok(ActionIntent::DrawOrHolster),
                _ => Err(RejectReason::HandsNotFree),
            }
        }
    }
}

fn validate_step_destination(world: &World, dest: Pos) -> Result<(), RejectReason> {
    let landing = world.map.resolve_step_destination(dest);
    if world.furniture_at(dest).is_some() {
        return Err(RejectReason::PathBlocked);
    }
    if world.standing_actor_at(landing).is_some() {
        return Err(RejectReason::OccupiedByActor);
    }
    Ok(())
}

fn adjacent_door_pos(world: &World, from: Pos, id: DoorId) -> Option<Pos> {
    Dir4::ALL
        .into_iter()
        .map(|d| from.step(d))
        .find(|pos| matches!(world.map.tile(*pos), TileKind::Door(door) if door == id))
}

/// One prepared, frozen action for the upcoming turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedAction {
    pub actor: ActorId,
    pub intent: ActionIntent,
    /// Turns until the effect applies. 1 means "this turn".
    pub remaining: u16,
}

/// How one actor's action ended this turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionResult {
    Completed,
    InProgress,
    Failed(&'static str),
}

/// Everything presentation needs to narrate one resolved turn.
#[derive(Clone, Debug, Default)]
pub struct TurnEvents {
    pub messages: Vec<String>,
    /// The player's action outcome this turn, if the player acted.
    pub player_result: Option<ActionResult>,
}

/// Resolves one frozen batch simultaneously against the world.
///
/// Order inside a turn is: instant state changes, doors, attacks,
/// object/body interactions, then movement with deterministic tie-breaks
/// from the resolution RNG stream. All completing actions see the world as
/// it stood at the start of the phase list; failures within a phase are
/// in-turn failures, never rejections.
pub fn resolve_turn(
    world: &mut World,
    data: &GameData,
    batch: &mut Vec<PreparedAction>,
) -> TurnEvents {
    let mut events = TurnEvents::default();
    world.incidents.clear();

    // Multi-turn actions tick down; only those reaching zero apply.
    let mut applying: Vec<PreparedAction> = Vec::new();
    for prepared in batch.iter_mut() {
        prepared.remaining -= 1;
        if prepared.remaining == 0 {
            applying.push(*prepared);
        } else if prepared.actor == world.player {
            events.player_result = Some(ActionResult::InProgress);
        }
    }
    batch.retain(|p| p.remaining > 0);

    let record = |world: &World, events: &mut TurnEvents, actor: ActorId, result: ActionResult| {
        if actor == world.player {
            events.player_result = Some(result);
        }
    };

    // Phase 1: instant state changes.
    for action in &applying {
        match action.intent {
            ActionIntent::Wait => record(world, &mut events, action.actor, ActionResult::Completed),
            ActionIntent::ToggleCrouch => {
                let actor = world.actor_mut(action.actor);
                actor.crouched = !actor.crouched;
                record(world, &mut events, action.actor, ActionResult::Completed);
            }
            ActionIntent::TurnFacing(dir) => {
                world.actor_mut(action.actor).facing = Some(dir);
                record(world, &mut events, action.actor, ActionResult::Completed);
            }
            ActionIntent::DrawOrHolster => {
                let weapon = world
                    .carried_items(action.actor)
                    .find(|i| data.item(&i.spec).is_some_and(|s| s.weapon))
                    .map(|i| i.id);
                let actor = world.actor_mut(action.actor);
                match (actor.hands, weapon) {
                    (Hands::Free, Some(id)) => {
                        actor.hands = Hands::Drawn(id);
                        record(world, &mut events, action.actor, ActionResult::Completed);
                        if action.actor == world.player {
                            events.messages.push("you draw the pistol".to_string());
                        }
                    }
                    (Hands::Drawn(_), _) => {
                        actor.hands = Hands::Free;
                        record(world, &mut events, action.actor, ActionResult::Completed);
                        if action.actor == world.player {
                            events.messages.push("you holster the pistol".to_string());
                        }
                    }
                    _ => record(
                        world,
                        &mut events,
                        action.actor,
                        ActionResult::Failed("your hands are not free"),
                    ),
                }
            }
            _ => {}
        }
    }

    // Phase 2: doors.
    for action in &applying {
        match action.intent {
            ActionIntent::OpenDoor(id) => {
                if world.door(id).open {
                    record(world, &mut events, action.actor, ActionResult::Completed);
                } else if access::can_pass_door(world, data, action.actor, id) {
                    world.door_mut(id).open = true;
                    record(world, &mut events, action.actor, ActionResult::Completed);
                } else {
                    record(
                        world,
                        &mut events,
                        action.actor,
                        ActionResult::Failed("the door is locked"),
                    );
                }
            }
            ActionIntent::CloseDoor(id) => {
                let door_pos = door_position(world, id);
                let blocked = door_pos.is_some_and(|pos| world.standing_actor_at(pos).is_some());
                if world.door(id).open && !blocked {
                    world.door_mut(id).open = false;
                    record(world, &mut events, action.actor, ActionResult::Completed);
                } else {
                    record(
                        world,
                        &mut events,
                        action.actor,
                        ActionResult::Failed("the doorway is blocked"),
                    );
                }
            }
            _ => {}
        }
    }

    // Phase 3: attacks.
    for action in &applying {
        match action.intent {
            ActionIntent::Garrote(target) => {
                resolve_garrote(world, data, &mut events, action.actor, target)
            }
            ActionIntent::Shoot(target) => {
                resolve_shoot(world, data, &mut events, action.actor, target)
            }
            ActionIntent::MeleeAttack(target) => {
                resolve_melee(world, data, &mut events, action.actor, target)
            }
            ActionIntent::Arrest(target) => {
                resolve_arrest(world, &mut events, action.actor, target)
            }
            _ => {}
        }
    }

    // Phase 4: bodies, disguises, and theft.
    for action in &applying {
        match action.intent {
            ActionIntent::Pickpocket(target) => {
                resolve_pickpocket(world, data, &mut events, action.actor, target)
            }
            ActionIntent::TakeDisguise(source) => {
                resolve_take_disguise(world, &mut events, action.actor, source)
            }
            ActionIntent::CarryBody(target) => {
                resolve_carry(world, &mut events, action.actor, target)
            }
            ActionIntent::DropBody => resolve_drop(world, &mut events, action.actor),
            ActionIntent::HideBody(id) => resolve_hide(world, &mut events, action.actor, id),
            _ => {}
        }
    }

    // Phase 5: movement.
    resolve_movement(world, data, &mut events, &applying);

    // Post-phase bookkeeping: outcome checks and the turn counter.
    check_outcomes(world, &mut events);
    world.turn += 1;
    events
}

fn door_position(world: &World, id: DoorId) -> Option<Pos> {
    for floor in 0..world.map.floor_count() {
        for pos in world.map.floor_positions(floor) {
            if matches!(world.map.tile(pos), TileKind::Door(d) if d == id) {
                return Some(pos);
            }
        }
    }
    None
}

fn resolve_garrote(
    world: &mut World,
    _data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
    target: ActorId,
) {
    let attacker_pos = world.actor(actor).pos;
    let target_ref = world.actor(target);
    let valid = target_ref.alive()
        && target_ref.hidden_in.is_none()
        && target_ref
            .facing
            .is_some_and(|f| target_ref.pos.step(f.opposite()) == attacker_pos)
        && world.actor(actor).hands == Hands::Free;
    if !valid {
        fail(world, events, actor, "the garrote finds no purchase");
        return;
    }
    kill(world, target);
    let name = world.actor(target).name.clone();
    events
        .messages
        .push(format!("{name} is garrotted silently"));
    complete(world, events, actor);
}

fn resolve_shoot(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
    target: ActorId,
) {
    let shooter_pos = world.actor(actor).pos;
    let target_ref = world.actor(target);
    let target_pos = target_ref.pos;
    let crouched = world.actor(actor).crouched || target_ref.crouched;
    let weapon = world
        .carried_items(actor)
        .find(|i| data.item(&i.spec).is_some_and(|s| s.weapon))
        .map(|i| (i.id, i.charges));

    let visible = target_ref.alive()
        && target_ref.hidden_in.is_none()
        && shooter_pos
            .chebyshev(target_pos)
            .is_some_and(|d| d <= data.tuning.pistol_range)
        && line_of_sight(shooter_pos, target_pos, world.sight_blocker(crouched));
    let Some((weapon_id, charges)) = weapon else {
        fail(world, events, actor, "no weapon to fire");
        return;
    };
    if charges == 0 || world.actor(actor).hands != Hands::Drawn(weapon_id) {
        fail(world, events, actor, "the pistol cannot fire");
        return;
    }
    if !visible {
        fail(world, events, actor, "the shot has no clear line");
        return;
    }
    if let Some(item) = world.items.iter_mut().find(|i| i.id == weapon_id) {
        item.charges -= 1;
    }
    kill(world, target);
    // Silenced, but still a local sound incident.
    world.incidents.push(crate::world::Incident {
        kind: crate::world::IncidentKind::Gunshot,
        pos: shooter_pos,
        radius: data.tuning.gunshot_sound_radius,
        turn: world.turn,
    });
    let name = world.actor(target).name.clone();
    events
        .messages
        .push(format!("the pistol coughs; {name} drops"));
    complete(world, events, actor);
}

fn resolve_melee(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
    target: ActorId,
) {
    let attacker_pos = world.actor(actor).pos;
    let target_ref = world.actor(target);
    if !target_ref.alive() || !attacker_pos.is_adjacent(target_ref.pos) {
        fail(world, events, actor, "the blow misses");
        return;
    }
    let damage = data.tuning.guard_attack_damage;
    let target_mut = world.actor_mut(target);
    target_mut.hp = target_mut.hp.saturating_sub(damage);
    if target_mut.hp == 0 {
        kill(world, target);
        let name = world.actor(target).name.clone();
        events.messages.push(format!("{name} is struck down"));
    } else {
        let name = world.actor(target).name.clone();
        events.messages.push(format!("{name} is struck"));
    }
    complete(world, events, actor);
}

fn resolve_arrest(world: &mut World, events: &mut TurnEvents, actor: ActorId, target: ActorId) {
    let arrester_pos = world.actor(actor).pos;
    let target_ref = world.actor(target);
    if target == world.player && target_ref.alive() && arrester_pos.is_adjacent(target_ref.pos) {
        world.outcome = Some(MissionOutcome::Arrested);
        events
            .messages
            .push("a hand clamps your shoulder: you are under arrest".to_string());
        complete(world, events, actor);
    } else {
        fail(world, events, actor, "the arrest fails");
    }
}

fn resolve_pickpocket(
    world: &mut World,
    data: &GameData,
    events: &mut TurnEvents,
    actor: ActorId,
    target: ActorId,
) {
    let actor_pos = world.actor(actor).pos;
    let target_ref = world.actor(target);
    let adjacent = actor_pos.is_adjacent(target_ref.pos) || actor_pos == target_ref.pos;
    if target_ref.hidden_in.is_some() || world.is_carried(target) || !adjacent {
        fail(world, events, actor, "the mark slipped away");
        return;
    }
    if world.carried_items(actor).count() >= INVENTORY_SLOTS {
        fail(world, events, actor, "your pockets are full");
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
            events.messages.push(format!("you palm the {name}"));
            complete(world, events, actor);
        }
        None => fail(world, events, actor, "nothing worth taking"),
    }
}

fn resolve_take_disguise(
    world: &mut World,
    events: &mut TurnEvents,
    actor: ActorId,
    source: DisguiseSource,
) {
    let actor_pos = world.actor(actor).pos;
    let hands_free = world.actor(actor).hands == Hands::Free;
    if !hands_free {
        fail(world, events, actor, "your hands are not free");
        return;
    }
    match source {
        DisguiseSource::Body(target) => {
            let target_ref = world.actor(target);
            let adjacent = actor_pos.is_adjacent(target_ref.pos) || actor_pos == target_ref.pos;
            if target_ref.alive() || target_ref.hidden_in.is_some() || !adjacent {
                fail(world, events, actor, "the clothes are out of reach");
                return;
            }
            let taken = world.actor(target).worn_disguise.clone();
            let own = world.actor(actor).worn_disguise.clone();
            world.actor_mut(target).worn_disguise = own;
            world.actor_mut(actor).worn_disguise = taken.clone();
            events.messages.push(format!("you change into the {taken}"));
            complete(world, events, actor);
        }
        DisguiseSource::Wardrobe(id) => {
            let Some(furniture) = world.furniture.get(id.0 as usize) else {
                fail(world, events, actor, "the wardrobe is gone");
                return;
            };
            if furniture.kind != FurnitureKind::Wardrobe || !actor_pos.is_adjacent(furniture.pos) {
                fail(world, events, actor, "the wardrobe is out of reach");
                return;
            }
            let Some(taken) = furniture.disguise.clone() else {
                fail(world, events, actor, "the wardrobe is empty");
                return;
            };
            let own = world.actor(actor).worn_disguise.clone();
            world.furniture_mut(id).disguise = Some(own);
            world.actor_mut(actor).worn_disguise = taken.clone();
            events.messages.push(format!("you change into the {taken}"));
            complete(world, events, actor);
        }
    }
}

fn resolve_carry(world: &mut World, events: &mut TurnEvents, actor: ActorId, target: ActorId) {
    let actor_pos = world.actor(actor).pos;
    let target_ref = world.actor(target);
    let adjacent = actor_pos.is_adjacent(target_ref.pos) || actor_pos == target_ref.pos;
    if !target_ref.is_visible_body()
        || world.is_carried(target)
        || !adjacent
        || world.actor(actor).hands != Hands::Free
    {
        fail(world, events, actor, "you cannot lift the body");
        return;
    }
    world.actor_mut(actor).hands = Hands::CarryingBody(target);
    world.actor_mut(target).pos = actor_pos;
    events.messages.push("you heave the body up".to_string());
    complete(world, events, actor);
}

fn resolve_drop(world: &mut World, events: &mut TurnEvents, actor: ActorId) {
    let Hands::CarryingBody(body) = world.actor(actor).hands else {
        fail(world, events, actor, "nothing to drop");
        return;
    };
    let pos = world.actor(actor).pos;
    world.actor_mut(actor).hands = Hands::Free;
    world.actor_mut(body).pos = pos;
    events.messages.push("you set the body down".to_string());
    complete(world, events, actor);
}

fn resolve_hide(world: &mut World, events: &mut TurnEvents, actor: ActorId, id: FurnitureId) {
    let Hands::CarryingBody(body) = world.actor(actor).hands else {
        fail(world, events, actor, "nothing to hide");
        return;
    };
    let actor_pos = world.actor(actor).pos;
    let Some(furniture) = world.furniture.get(id.0 as usize) else {
        fail(world, events, actor, "the container is gone");
        return;
    };
    if furniture.kind != FurnitureKind::Container
        || furniture.body.is_some()
        || !actor_pos.is_adjacent(furniture.pos)
    {
        fail(world, events, actor, "the container will not take it");
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
        .push("the body disappears from sight".to_string());
    complete(world, events, actor);
}

/// Simultaneous movement with deterministic tie-breaks: contested tiles go
/// to one shuffled winner; chains settle iteratively; unresolvable moves
/// fail in-turn.
fn resolve_movement(
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
                    fail(world, events, action.actor, "the door is locked");
                }
                continue;
            }
            let terrain_open = matches!(
                world.map.tile(ahead),
                TileKind::Floor | TileKind::Stairs | TileKind::Door(_)
            ) && world.furniture_at(ahead).is_none();
            if !terrain_open {
                fail(world, events, action.actor, "the way is blocked");
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
            fail(world, events, moves[index].actor, "someone got there first");
        } else {
            claimed.push(dest);
            winners.push(index);
        }
    }
    winners.sort_unstable();

    // Settle: apply moves whose destination is free, repeatedly, so chains
    // (A follows B) resolve; swaps and cycles fail deterministically.
    let mut pending: Vec<usize> = winners;
    loop {
        let mut progressed = false;
        let mut still_pending: Vec<usize> = Vec::new();
        for index in pending.iter().copied() {
            let dest = moves[index].dest;
            if world.standing_actor_at(dest).is_some() {
                still_pending.push(index);
                continue;
            }
            let actor_id = moves[index].actor;
            let dir = moves[index].dir;
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
            progressed = true;
        }
        if still_pending.is_empty() {
            break;
        }
        if !progressed {
            for index in still_pending {
                fail(world, events, moves[index].actor, "the way is blocked");
            }
            break;
        }
        pending = still_pending;
    }
}

fn check_outcomes(world: &mut World, events: &mut TurnEvents) {
    if world.outcome.is_some() {
        return;
    }
    let player = world.player_actor();
    if player.condition == BodyCondition::Dead {
        world.outcome = Some(MissionOutcome::PlayerKilled);
        events.messages.push("everything goes dark".to_string());
        return;
    }
    let target_dead = world.actor(world.target).condition == BodyCondition::Dead;
    if target_dead && world.extraction_tiles.contains(&player.pos) {
        world.outcome = Some(MissionOutcome::Extracted);
        events
            .messages
            .push("you slip out into the night; the job is done".to_string());
    }
}

fn kill(world: &mut World, target: ActorId) {
    // Dying drops any carried body on the spot.
    if let Hands::CarryingBody(body) = world.actor(target).hands {
        let pos = world.actor(target).pos;
        world.actor_mut(body).pos = pos;
    }
    let pos = world.actor(target).pos;
    let target_mut = world.actor_mut(target);
    target_mut.condition = BodyCondition::Dead;
    target_mut.hp = 0;
    target_mut.hands = Hands::Free;
    // A kill in the open is witnessable evidence this turn.
    world.incidents.push(crate::world::Incident {
        kind: crate::world::IncidentKind::Violence,
        pos,
        radius: 0,
        turn: world.turn,
    });
}

fn complete(world: &World, events: &mut TurnEvents, actor: ActorId) {
    if actor == world.player {
        events.player_result = Some(ActionResult::Completed);
    }
}

fn fail(world: &World, events: &mut TurnEvents, actor: ActorId, why: &'static str) {
    if actor == world.player {
        events.player_result = Some(ActionResult::Failed(why));
        events.messages.push(format!("failed: {why}"));
    }
}
