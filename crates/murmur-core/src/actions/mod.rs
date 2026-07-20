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
//!
//! Each action family lives in its own module — movement, doors,
//! violence, bodies, theft, interact — holding both sides of its rules:
//! the pre-turn validator and the in-turn resolver. Where the two sides
//! check the same thing they share one precondition expression; where
//! they deliberately differ (an in-turn check is coarser, or skips a
//! queue-time gate that cannot change mid-turn) the two now sit side by
//! side instead of five hundred lines apart. [`translate`] and
//! [`resolve_turn`] are dispatchers only.

mod bodies;
mod doors;
mod interact;
mod movement;
mod theft;
mod violence;

use serde::{Deserialize, Serialize};

use crate::data::GameData;
use crate::geom::{Dir4, Pos};
use crate::map::{DoorId, TileKind};
use crate::world::{ActorId, BodyCondition, FurnitureId, Hands, ItemId, MissionOutcome, World};

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
    /// `None` drops on the carrier's tile; `Some(dir)` drops adjacent.
    DropBody(Option<Dir4>),
    HideBody(FurnitureId),
    DrawOrHolster,
    /// Pick a locked door open with lockpicks (slow; suspicious if seen).
    PickLock(DoorId),
    /// Throw a noisemaker charge at a visible tile to draw investigators.
    ThrowNoisemaker(Pos),
    /// Use an adjacent opportunity machine.
    Interact(FurnitureId),
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
    /// `None` drops on the carrier's tile; `Some(dir)` drops adjacent.
    DropBody(Option<Dir4>),
    HideBody(FurnitureId),
    DrawOrHolster,
    PickLock(DoorId),
    Throw(Pos),
    Interact(FurnitureId),
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
    PathBlocked(Blockage),
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
    NoGarrote,
    NoLockpicks,
    DoorNotLocked,
    NoNoisemaker,
    NothingToUse,
    MachineSpent,
    MissionOver,
}

impl RejectReason {
    pub fn message(&self) -> &'static str {
        match self {
            RejectReason::PathBlocked(what) => what.message(),
            RejectReason::OccupiedByActor => crate::tr!("reject.occupied"),
            RejectReason::DoorIsLocked => crate::tr!("reject.door_locked"),
            RejectReason::DoorAlreadyOpen => crate::tr!("reject.door_open"),
            RejectReason::DoorAlreadyClosed => crate::tr!("reject.door_closed"),
            RejectReason::DoorNotAdjacent => crate::tr!("reject.door_far"),
            RejectReason::DoorBlocked => crate::tr!("reject.door_blocked"),
            RejectReason::NotAdjacent => crate::tr!("reject.not_adjacent"),
            RejectReason::TargetGone => crate::tr!("reject.target_gone"),
            RejectReason::NotBehindTarget => crate::tr!("reject.not_behind"),
            RejectReason::HandsNotFree => crate::tr!("reject.hands_busy"),
            RejectReason::NotCarryingBody => crate::tr!("reject.not_carrying_body"),
            RejectReason::NotAContainer => crate::tr!("reject.not_container"),
            RejectReason::ContainerOccupied => crate::tr!("reject.container_full"),
            RejectReason::NoDisguiseThere => crate::tr!("reject.no_disguise"),
            RejectReason::TargetNotIncapacitated => crate::tr!("reject.target_conscious"),
            RejectReason::InventoryFull => crate::tr!("reject.inventory_full"),
            RejectReason::NothingToSteal => crate::tr!("reject.nothing_to_steal"),
            RejectReason::NoWeaponCarried => crate::tr!("reject.no_weapon"),
            RejectReason::NoAmmo => crate::tr!("reject.no_ammo"),
            RejectReason::WeaponNotDrawn => crate::tr!("reject.weapon_holstered"),
            RejectReason::TargetNotVisible => crate::tr!("reject.not_visible"),
            RejectReason::OutOfRange => crate::tr!("reject.out_of_range"),
            RejectReason::NoGarrote => crate::tr!("reject.no_garrote"),
            RejectReason::NoLockpicks => crate::tr!("reject.no_lockpicks"),
            RejectReason::DoorNotLocked => crate::tr!("reject.door_not_locked"),
            RejectReason::NoNoisemaker => crate::tr!("reject.no_noisemaker"),
            RejectReason::NothingToUse => crate::tr!("reject.nothing_to_use"),
            RejectReason::MachineSpent => crate::tr!("reject.machine_spent"),
            RejectReason::MissionOver => crate::tr!("reject.mission_over"),
        }
    }
}

/// What stopped a move or a throw. A typed reason rather than a message:
/// the words live in the catalogue, and callers that want to branch on the
/// cause can, which a `&'static str` never allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blockage {
    Wall,
    Terrain,
    Furniture,
    Body,
    /// A noisemaker throw with no landing tile.
    NoThrowTarget,
}

impl Blockage {
    pub fn message(self) -> &'static str {
        match self {
            Blockage::Wall => crate::tr!("reject.blocked_wall"),
            Blockage::Terrain => crate::tr!("reject.blocked_terrain"),
            Blockage::Furniture => crate::tr!("reject.blocked_furniture"),
            Blockage::Body => crate::tr!("reject.blocked_body"),
            Blockage::NoThrowTarget => crate::tr!("reject.no_throw_target"),
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
        ActionIntent::DropBody(_) => durations.drop_body,
        ActionIntent::HideBody(_) => durations.hide_body,
        ActionIntent::DrawOrHolster => durations.draw_holster,
        ActionIntent::PickLock(_) => durations.pick_lock,
        ActionIntent::Throw(_) => durations.throw,
        ActionIntent::Interact(id) => world
            .furniture
            .iter()
            .find(|f| f.id == *id)
            .and_then(|f| f.machine.as_deref())
            .and_then(|spec| data.opportunity(spec))
            .map(|spec| spec.interact_turns)
            .unwrap_or(1),
        ActionIntent::MeleeAttack(_) | ActionIntent::Arrest(_) => 1,
    }
}

/// The player's six general inventory slots.
pub const INVENTORY_SLOTS: usize = 6;

/// Pure, execution-time validation and translation of one player command
/// against the current world. On `Err`, nothing has changed anywhere.
/// Each arm delegates to its family's rulebook.
pub fn translate(
    world: &World,
    data: &GameData,
    command: &Command,
) -> Result<ActionIntent, RejectReason> {
    if world.outcome.is_some() {
        return Err(RejectReason::MissionOver);
    }
    match *command {
        Command::Wait => Ok(ActionIntent::Wait),
        Command::ToggleCrouch => Ok(ActionIntent::ToggleCrouch),
        Command::Move(dir) => movement::validate_move(world, data, dir),
        Command::OpenDoor(id) => doors::validate_open(world, data, id),
        Command::CloseDoor(id) => doors::validate_close(world, id),
        Command::Garrote(target) => violence::validate_garrote(world, data, target),
        Command::Shoot(target) => violence::validate_shoot(world, data, target),
        Command::Pickpocket(target) => theft::validate_pickpocket(world, data, target),
        Command::TakeDisguiseFromBody(target) => theft::validate_take_from_body(world, target),
        Command::TakeDisguiseFromWardrobe(id) => theft::validate_take_from_wardrobe(world, id),
        Command::CarryBody(target) => bodies::validate_carry(world, target),
        Command::DropBody(dir) => bodies::validate_drop(world, dir),
        Command::HideBody(id) => bodies::validate_hide(world, id),
        Command::DrawOrHolster => interact::validate_draw_or_holster(world, data),
        Command::PickLock(door) => doors::validate_pick_lock(world, data, door),
        Command::ThrowNoisemaker(pos) => interact::validate_throw(world, data, pos),
        Command::Interact(id) => interact::validate_interact(world, id),
    }
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
    /// The line announcing a contract breach, if one happened this turn.
    ///
    /// Separate from `messages` because presentation styles it differently
    /// (it is the one event that changes the run's payout) and used to pick
    /// it out by matching on the text. Once the words moved to the string
    /// catalogue that match became a translation the game silently depended
    /// on, so the distinction is carried structurally instead.
    pub breach: Option<String>,
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
                interact::resolve_draw_or_holster(world, data, &mut events, action.actor)
            }
            ActionIntent::Throw(pos) => {
                interact::resolve_throw(world, data, &mut events, action.actor, pos)
            }
            _ => {}
        }
    }

    // Phase 2: doors.
    for action in &applying {
        match action.intent {
            ActionIntent::OpenDoor(id) => {
                doors::resolve_open(world, data, &mut events, action.actor, id)
            }
            ActionIntent::CloseDoor(id) => {
                doors::resolve_close(world, &mut events, action.actor, id)
            }
            ActionIntent::Interact(id) => {
                interact::resolve_interact(world, data, &mut events, action.actor, id)
            }
            ActionIntent::PickLock(id) => {
                doors::resolve_pick_lock(world, &mut events, action.actor, id)
            }
            _ => {}
        }
    }

    // Phase 3: attacks.
    for action in &applying {
        match action.intent {
            ActionIntent::Garrote(target) => {
                violence::resolve_garrote(world, &mut events, action.actor, target)
            }
            ActionIntent::Shoot(target) => {
                violence::resolve_shoot(world, data, &mut events, action.actor, target)
            }
            ActionIntent::MeleeAttack(target) => {
                violence::resolve_melee(world, data, &mut events, action.actor, target)
            }
            ActionIntent::Arrest(target) => {
                violence::resolve_arrest(world, &mut events, action.actor, target)
            }
            _ => {}
        }
    }

    // Phase 4: bodies, disguises, and theft.
    for action in &applying {
        match action.intent {
            ActionIntent::Pickpocket(target) => {
                theft::resolve_pickpocket(world, data, &mut events, action.actor, target)
            }
            ActionIntent::TakeDisguise(source) => {
                theft::resolve_take_disguise(world, &mut events, action.actor, source)
            }
            ActionIntent::CarryBody(target) => {
                bodies::resolve_carry(world, &mut events, action.actor, target)
            }
            ActionIntent::DropBody(dir) => {
                bodies::resolve_drop(world, &mut events, action.actor, dir)
            }
            ActionIntent::HideBody(id) => {
                bodies::resolve_hide(world, &mut events, action.actor, id)
            }
            _ => {}
        }
    }

    // Phase 5: movement.
    movement::resolve_movement(world, data, &mut events, &applying);

    // Fleeing NPCs who reach an extraction exit leave the club for good;
    // otherwise crowds would cower on the exits and block extraction.
    let departures: Vec<ActorId> = world
        .actors
        .iter()
        .filter(|a| {
            !a.is_player()
                && a.alive()
                && !a.departed
                && a.ai
                    .as_ref()
                    .is_some_and(|ai| ai.mood == crate::world::Mood::Fleeing)
                && world.extraction_tiles.contains(&a.pos)
        })
        .map(|a| a.id)
        .collect();
    for id in departures {
        world.actor_mut(id).departed = true;
        let name = world.actor(id).name.clone();
        events
            .messages
            .push(crate::trf!("log.target_bolts", name = name));
    }

    // Post-phase bookkeeping: outcome checks and the turn counter.
    check_outcomes(world, &mut events);
    world.turn += 1;
    events
}

fn check_outcomes(world: &mut World, events: &mut TurnEvents) {
    if world.outcome.is_some() {
        return;
    }
    let player = world.player_actor();
    if player.condition == BodyCondition::Dead {
        world.outcome = Some(MissionOutcome::PlayerKilled);
        events
            .messages
            .push(crate::tr!("log.player_killed").to_string());
        return;
    }
    // The target escaping alive ends the mission: there is no completing
    // the contract once they are gone.
    let target = world.actor(world.target);
    if target.alive() && target.departed {
        world.outcome = Some(MissionOutcome::TargetEscaped);
        events
            .messages
            .push(crate::tr!("log.target_escaped").to_string());
        return;
    }
    let target_dead = world.actor(world.target).condition == BodyCondition::Dead;
    if target_dead && world.extraction_tiles.contains(&player.pos) {
        let player_pos = player.pos;
        if let Some(crate::contract::Constraint::SpecificExit { room_template }) =
            world.constraint.clone()
        {
            let via_required = world
                .room_at(player_pos)
                .is_some_and(|r| r.template == room_template);
            if !via_required {
                let exit_name = world
                    .rooms
                    .iter()
                    .find(|r| r.template == room_template)
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| room_template.clone());
                let reason = crate::trf!("contract.exit_via.breach", room = exit_name);
                breach_constraint(world, events, &reason);
            }
        }
        world.outcome = Some(MissionOutcome::Extracted);
        events
            .messages
            .push(crate::tr!("log.extracted").to_string());
    }
}

/// Marks the contract constraint broken (once), with the reason on the
/// turn's message log. The mission continues; the contract resolves
/// unclean.
pub(crate) fn breach_constraint(world: &mut World, events: &mut TurnEvents, reason: &str) {
    if world.constraint_breach.is_some() {
        return;
    }
    world.constraint_breach = Some(reason.to_string());
    events.breach = Some(crate::trf!("log.breached", reason = reason));
}

/// Kills `target` at the hand of `killer`: drops any carried body, records
/// constraint breaches the death causes, and leaves witnessable evidence.
pub(super) fn kill(world: &mut World, events: &mut TurnEvents, killer: ActorId, target: ActorId) {
    // Dying drops any carried body on the spot.
    if let Hands::CarryingBody(body) = world.actor(target).hands {
        let pos = world.actor(target).pos;
        world.actor_mut(body).pos = pos;
    }
    let pos = world.actor(target).pos;
    let is_target = world.actor(target).is_target;
    let victim_role = world.actor(target).role;
    let player_kill = killer == world.player;
    let target_mut = world.actor_mut(target);
    target_mut.condition = BodyCondition::Dead;
    target_mut.hp = 0;
    target_mut.hands = Hands::Free;
    target_mut.killed_by_player = player_kill;

    if player_kill {
        match &world.constraint {
            Some(crate::contract::Constraint::NoCivilianCasualties)
                if !is_target && victim_role != Some(crate::data::Role::Guard) =>
            {
                breach_constraint(world, events, crate::tr!("contract.no_collateral.breach"));
            }
            Some(crate::contract::Constraint::PrivateKill) if is_target => {
                let private = world
                    .room_at(pos)
                    .is_some_and(|r| r.zone == crate::data::Zone::Personal);
                if !private {
                    let where_ = world
                        .room_at(pos)
                        .map(|r| r.name.clone())
                        .unwrap_or_else(|| {
                            crate::tr!("contract.private_kill.open_floor").to_string()
                        });
                    let offices: Vec<String> = world
                        .rooms
                        .iter()
                        .filter(|r| r.zone == crate::data::Zone::Personal)
                        .map(|r| r.name.clone())
                        .collect();
                    let needed = if offices.is_empty() {
                        crate::tr!("contract.private_kill.fallback_room").to_string()
                    } else {
                        offices.join(" or ")
                    };
                    let reason = crate::loc::fmt(
                        "contract.private_kill.breach",
                        &[("where", &where_), ("needed", &needed)],
                    );
                    breach_constraint(world, events, &reason);
                }
            }
            _ => {}
        }
    }

    // A kill in the open is witnessable evidence this turn.
    world.incidents.push(crate::world::Incident {
        kind: crate::world::IncidentKind::Violence,
        pos,
        radius: 0,
        turn: world.turn,
    });
}

/// The firearm the actor carries, if any: its item id and charges. Four
/// separate sites (shoot and draw/holster, each side) restated this scan.
pub(super) fn carried_firearm(
    world: &World,
    data: &GameData,
    actor: ActorId,
) -> Option<(ItemId, u16)> {
    world
        .carried_items(actor)
        .find(|i| data.item(&i.spec).is_some_and(|s| s.firearm))
        .map(|i| (i.id, i.charges))
}

/// The tile holding door `id`, if it is adjacent to `from`.
pub(super) fn adjacent_door_pos(world: &World, from: Pos, id: DoorId) -> Option<Pos> {
    Dir4::ALL
        .into_iter()
        .map(|d| from.step(d))
        .find(|pos| matches!(world.map.tile(*pos), TileKind::Door(door) if door == id))
}

/// The tile holding door `id`, wherever it is.
pub(super) fn door_position(world: &World, id: DoorId) -> Option<Pos> {
    for floor in 0..world.map.floor_count() {
        for pos in world.map.floor_positions(floor) {
            if matches!(world.map.tile(pos), TileKind::Door(d) if d == id) {
                return Some(pos);
            }
        }
    }
    None
}

/// Records the player's action result; other actors' results are silent.
pub(super) fn record(world: &World, events: &mut TurnEvents, actor: ActorId, result: ActionResult) {
    if actor == world.player {
        events.player_result = Some(result);
    }
}

pub(super) fn complete(world: &World, events: &mut TurnEvents, actor: ActorId) {
    if actor == world.player {
        events.player_result = Some(ActionResult::Completed);
    }
}

/// An in-turn failure: unlike [`record`], this also narrates the reason on
/// the log, because a failed player action costs the turn.
pub(super) fn fail(world: &World, events: &mut TurnEvents, actor: ActorId, why: &'static str) {
    if actor == world.player {
        events.player_result = Some(ActionResult::Failed(why));
        events
            .messages
            .push(crate::trf!("log.failed", reason = why));
    }
}
