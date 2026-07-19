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
                TileKind::Wall | TileKind::Void => Err(RejectReason::PathBlocked(Blockage::Wall)),
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
                TileKind::Floor | TileKind::Stairs(_) => {
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
            let carries_garrote = world
                .carried_items(world.player)
                .any(|i| data.item(&i.spec).is_some_and(|s| s.weapon && !s.firearm));
            if !carries_garrote {
                return Err(RejectReason::NoGarrote);
            }
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
                .find(|i| data.item(&i.spec).is_some_and(|s| s.firearm))
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
        Command::DropBody(dir) => match player.hands {
            Hands::CarryingBody(_) => {
                let dest = match dir {
                    Some(dir) => player.pos.step(dir),
                    None => player.pos,
                };
                validate_drop_destination(world, dest, player.id)?;
                Ok(ActionIntent::DropBody(dir))
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
                .find(|i| data.item(&i.spec).is_some_and(|s| s.firearm))
                .ok_or(RejectReason::NoWeaponCarried)?;
            match player.hands {
                Hands::Free => Ok(ActionIntent::DrawOrHolster),
                Hands::Drawn(id) if id == pistol.id => Ok(ActionIntent::DrawOrHolster),
                _ => Err(RejectReason::HandsNotFree),
            }
        }
        Command::PickLock(door) => {
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
        Command::ThrowNoisemaker(pos) => {
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
        Command::Interact(id) => {
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
    }
}

fn validate_drop_destination(
    world: &World,
    dest: Pos,
    player_id: ActorId,
) -> Result<(), RejectReason> {
    if !world.map.walkable(dest, |id| world.door(id).open) {
        return Err(RejectReason::PathBlocked(Blockage::Terrain));
    }
    if world.furniture_at(dest).is_some() {
        return Err(RejectReason::PathBlocked(Blockage::Furniture));
    }
    if world
        .standing_actor_at(dest)
        .is_some_and(|a| a.id != player_id)
    {
        return Err(RejectReason::OccupiedByActor);
    }
    if world.body_at(dest).is_some() {
        return Err(RejectReason::PathBlocked(Blockage::Body));
    }
    Ok(())
}

fn validate_step_destination(world: &World, dest: Pos) -> Result<(), RejectReason> {
    let landing = world.map.resolve_step_destination(dest);
    if world.furniture_at(dest).is_some() {
        return Err(RejectReason::PathBlocked(Blockage::Furniture));
    }
    if let Some(occupant) = world.standing_actor_at(landing) {
        // Civilians and staff step aside (the mover swaps places with
        // them at resolution) — but not across a stairs transition, where
        // a swap would teleport the bystander between storeys.
        if landing != dest || !world.is_displaceable(occupant.id) {
            return Err(RejectReason::OccupiedByActor);
        }
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
                    .find(|i| data.item(&i.spec).is_some_and(|s| s.firearm))
                    .map(|i| i.id);
                let actor = world.actor_mut(action.actor);
                match (actor.hands, weapon) {
                    (Hands::Free, Some(id)) => {
                        actor.hands = Hands::Drawn(id);
                        record(world, &mut events, action.actor, ActionResult::Completed);
                        if action.actor == world.player {
                            events.messages.push(crate::tr!("log.draw").to_string());
                        }
                    }
                    (Hands::Drawn(_), _) => {
                        actor.hands = Hands::Free;
                        record(world, &mut events, action.actor, ActionResult::Completed);
                        if action.actor == world.player {
                            events.messages.push(crate::tr!("log.holster").to_string());
                        }
                    }
                    _ => record(
                        world,
                        &mut events,
                        action.actor,
                        ActionResult::Failed(crate::tr!("fail.hands_busy")),
                    ),
                }
            }
            ActionIntent::Throw(pos) => {
                let charge = world
                    .carried_items(action.actor)
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
                        record(world, &mut events, action.actor, ActionResult::Completed);
                        if action.actor == world.player {
                            events
                                .messages
                                .push(crate::tr!("log.noisemaker").to_string());
                        }
                    }
                    None => record(
                        world,
                        &mut events,
                        action.actor,
                        ActionResult::Failed(crate::tr!("fail.no_charges")),
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
                        ActionResult::Failed(crate::tr!("fail.door_locked")),
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
                        ActionResult::Failed(crate::tr!("fail.doorway_blocked")),
                    );
                }
            }
            ActionIntent::Interact(id) => {
                resolve_interact(world, data, &mut events, action.actor, id)
            }
            ActionIntent::PickLock(id) => {
                if world.door(id).locked_by.is_some() {
                    let door = world.door_mut(id);
                    door.locked_by = None;
                    door.open = true;
                    record(world, &mut events, action.actor, ActionResult::Completed);
                    if action.actor == world.player {
                        events
                            .messages
                            .push(crate::tr!("log.lock_picked").to_string());
                    }
                } else {
                    record(
                        world,
                        &mut events,
                        action.actor,
                        ActionResult::Failed(crate::tr!("fail.door_not_locked")),
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
            ActionIntent::DropBody(dir) => resolve_drop(world, &mut events, action.actor, dir),
            ActionIntent::HideBody(id) => resolve_hide(world, &mut events, action.actor, id),
            _ => {}
        }
    }

    // Phase 5: movement.
    resolve_movement(world, data, &mut events, &applying);

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
        fail(world, events, actor, crate::tr!("fail.garrote_missed"));
        return;
    }
    kill(world, events, actor, target);
    let name = world.actor(target).name.clone();
    events
        .messages
        .push(crate::trf!("log.garrotted", name = name));
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
        .find(|i| data.item(&i.spec).is_some_and(|s| s.firearm))
        .map(|i| (i.id, i.charges));

    let visible = target_ref.alive()
        && target_ref.hidden_in.is_none()
        && shooter_pos
            .chebyshev(target_pos)
            .is_some_and(|d| d <= data.tuning.pistol_range)
        && line_of_sight(shooter_pos, target_pos, world.sight_blocker(crouched));
    let Some((weapon_id, charges)) = weapon else {
        fail(world, events, actor, crate::tr!("fail.no_weapon"));
        return;
    };
    if charges == 0 || world.actor(actor).hands != Hands::Drawn(weapon_id) {
        fail(world, events, actor, crate::tr!("fail.cannot_fire"));
        return;
    }
    if !visible {
        fail(world, events, actor, crate::tr!("fail.no_line"));
        return;
    }
    if let Some(item) = world.items.iter_mut().find(|i| i.id == weapon_id) {
        item.charges -= 1;
    }
    if actor == world.player
        && matches!(
            world.constraint,
            Some(crate::contract::Constraint::NoFirearms)
        )
    {
        breach_constraint(world, events, crate::tr!("contract.no_firearms.breach"));
    }
    kill(world, events, actor, target);
    // Silenced, but still a local sound incident.
    world.incidents.push(crate::world::Incident {
        kind: crate::world::IncidentKind::Gunshot,
        pos: shooter_pos,
        radius: data.tuning.gunshot_sound_radius,
        turn: world.turn,
    });
    let name = world.actor(target).name.clone();
    events.messages.push(crate::trf!("log.shot", name = name));
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
        fail(world, events, actor, crate::tr!("fail.blow_missed"));
        return;
    }
    let damage = data.tuning.guard_attack_damage;
    let target_mut = world.actor_mut(target);
    target_mut.hp = target_mut.hp.saturating_sub(damage);
    if target_mut.hp == 0 {
        kill(world, events, actor, target);
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

fn resolve_arrest(world: &mut World, events: &mut TurnEvents, actor: ActorId, target: ActorId) {
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

fn resolve_take_disguise(
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

fn resolve_carry(world: &mut World, events: &mut TurnEvents, actor: ActorId, target: ActorId) {
    let actor_pos = world.actor(actor).pos;
    let target_ref = world.actor(target);
    let adjacent = actor_pos.is_adjacent(target_ref.pos) || actor_pos == target_ref.pos;
    if !target_ref.is_visible_body()
        || world.is_carried(target)
        || !adjacent
        || world.actor(actor).hands != Hands::Free
    {
        fail(world, events, actor, crate::tr!("fail.cannot_lift"));
        return;
    }
    world.actor_mut(actor).hands = Hands::CarryingBody(target);
    world.actor_mut(target).pos = actor_pos;
    events
        .messages
        .push(crate::tr!("log.body_lifted").to_string());
    complete(world, events, actor);
}

fn resolve_drop(world: &mut World, events: &mut TurnEvents, actor: ActorId, dir: Option<Dir4>) {
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

fn resolve_hide(world: &mut World, events: &mut TurnEvents, actor: ActorId, id: FurnitureId) {
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
                if !occupant_is_moving && world.is_displaceable(occupant) {
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

/// Applies one opportunity machine's effect.
fn resolve_interact(
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
            let ids: Vec<ActorId> = world
                .actors
                .iter()
                .filter(|a| !a.is_player() && a.alive() && !a.departed && a.ai.is_some())
                .map(|a| a.id)
                .collect();
            for npc in ids {
                let is_guard = world.actor(npc).role == Some(crate::data::Role::Guard);
                let ai = world.actor_mut(npc).ai.as_mut().unwrap();
                if is_guard {
                    if matches!(
                        ai.mood,
                        crate::world::Mood::Relaxed | crate::world::Mood::Suspicious
                    ) {
                        ai.mood = crate::world::Mood::Investigating;
                        ai.focus = Some(machine_pos);
                    }
                } else {
                    ai.mood = crate::world::Mood::Fleeing;
                    ai.focus = Some(machine_pos);
                }
            }
            events
                .messages
                .push(crate::tr!("log.fire_alarm").to_string());
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

fn kill(world: &mut World, events: &mut TurnEvents, killer: ActorId, target: ActorId) {
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

fn complete(world: &World, events: &mut TurnEvents, actor: ActorId) {
    if actor == world.player {
        events.player_result = Some(ActionResult::Completed);
    }
}

fn fail(world: &World, events: &mut TurnEvents, actor: ActorId, why: &'static str) {
    if actor == world.player {
        events.player_result = Some(ActionResult::Failed(why));
        events
            .messages
            .push(crate::trf!("log.failed", reason = why));
    }
}
