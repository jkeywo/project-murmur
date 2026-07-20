//! The authoritative world state.
//!
//! Rooms, access, actors, objects, containers, items, mission facts,
//! alertness, routines, and knowledge all live here. Everything is owned by
//! plain vectors indexed by stable domain identifiers — the same IDs that
//! queued player commands carry — and iteration order is always the vector
//! order, which is deterministic by construction.

use serde::{Deserialize, Serialize};

use crate::data::{
    DisguiseId, GameData, ItemSpecId, Lighting, Role, RoomTemplateId, WaypointKind, Zone,
};
use crate::geom::{Dir4, FloorId, Pos};
use crate::map::{DoorId, DoorState, GameMap};
use crate::rng::Pcg32;

/// Stable domain identifier of an actor for the whole mission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ActorId(pub u32);

/// Stable domain identifier of an item instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ItemId(pub u32);

/// Stable domain identifier of a furniture piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FurnitureId(pub u32);

/// Stable domain identifier of a generated room.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RoomId(pub u16);

/// Axis-aligned interior rectangle of a room (walls excluded).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub w: i16,
    pub h: i16,
}

impl Rect {
    pub fn contains(&self, x: i16, y: i16) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    pub fn center(&self) -> (i16, i16) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

/// A routine waypoint inside a room.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waypoint {
    pub kind: WaypointKind,
    pub pos: Pos,
}

/// One generated room with its metadata, recorded before tiles were laid.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Room {
    pub id: RoomId,
    pub template: RoomTemplateId,
    pub name: String,
    pub zone: Zone,
    pub floor: FloorId,
    pub bounds: Rect,
    pub lighting: Lighting,
    pub waypoints: Vec<Waypoint>,
    pub doors: Vec<DoorId>,
    pub external_exit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FurnitureKind {
    /// Desks, tables: block movement; block sight only against crouched
    /// endpoints.
    LowCover,
    /// Crates, freezers: block movement and sight; hold at most one body.
    Container,
    /// Wardrobes: like containers but hold a disguise instead of a body.
    Wardrobe,
    /// An opportunity machine (fuse box, hoist, alarm): blocks movement
    /// and sight like a container; interacted with, not opened.
    Machine,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Furniture {
    pub id: FurnitureId,
    pub kind: FurnitureKind,
    pub pos: Pos,
    /// Body hidden inside (containers only).
    pub body: Option<ActorId>,
    /// Disguise available inside (wardrobes only).
    pub disguise: Option<DisguiseId>,
    /// The opportunity spec realised here (machines only).
    #[serde(default)]
    pub machine: Option<String>,
    /// One-shot machines flip this when used.
    #[serde(default)]
    pub used: bool,
    /// Where the rigged load lands (accident machines only).
    #[serde(default)]
    pub drop_tile: Option<Pos>,
}

/// Where an item currently is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemLocation {
    Ground(Pos),
    CarriedBy(ActorId),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemInstance {
    pub id: ItemId,
    pub spec: ItemSpecId,
    pub location: ItemLocation,
    /// Remaining uses: pistol rounds for weapons, zero elsewhere.
    pub charges: u16,
}

/// Physical condition of an actor's body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyCondition {
    Healthy,
    Unconscious,
    Dead,
}

/// What an actor's hands are doing. Carrying a body occupies both hands;
/// a drawn weapon occupies them for garrote purposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hands {
    Free,
    CarryingBody(ActorId),
    Drawn(ItemId),
}

/// NPC behaviour states. Relaxed actors follow routines on a staggered
/// cadence; every other state acts each turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mood {
    Relaxed,
    /// Something looked wrong; watching, suspicion accumulating.
    Suspicious,
    /// Moving to a specific spot to check an incident or sighting.
    Investigating,
    /// Knows the player is hostile; pursuing to arrest (guards) .
    Alerted,
    /// Escorting the arrested player off the premises (mission-ending).
    Escorting,
    /// Civilians and unarmed staff running for an exit.
    Fleeing,
    /// Lethal combat (armed actors after violence or resistance).
    Combat,
}

impl Mood {
    /// How the state reads in words, for the threat list and the
    /// inspection line. The map says the same thing in colour; this is what
    /// makes the state legible without relying on hue.
    ///
    /// Presentation used to derive this from `{:?}`, which quietly made the
    /// Rust variant names into player-facing text no translator could
    /// reach.
    pub fn label(self) -> &'static str {
        match self {
            Mood::Relaxed => crate::tr!("mood.relaxed"),
            Mood::Suspicious => crate::tr!("mood.suspicious"),
            Mood::Investigating => crate::tr!("mood.investigating"),
            Mood::Alerted => crate::tr!("mood.alerted"),
            Mood::Escorting => crate::tr!("mood.escorting"),
            Mood::Fleeing => crate::tr!("mood.fleeing"),
            Mood::Combat => crate::tr!("mood.combat"),
        }
    }
}

/// One step of a generated routine: go somewhere, then linger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineStep {
    pub pos: Pos,
    pub wait: u16,
}

/// Whether the target is surrounded when it stands here. The whole
/// difficulty of a mission turns on this: escorted beats are attackable
/// only by accident, alone beats are where weapons work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Protection {
    #[default]
    Escorted,
    Alone,
}

pub type SignalId = String;

/// What advances the schedule onto a beat.
///
/// `Sequential` beats are visited every cycle, forever. `OnSignal` beats
/// are interrupts — strictly additive, never replacing the cycle. That
/// distinction is load-bearing: the route planner reasons atemporally, so
/// it may only assume a beat is *eventually* reachable in time if the
/// cycle guarantees it recurs. A trigger that fires once, or after a
/// deadline, would collapse that guarantee — which is why there is no
/// `AfterTurn` variant and must never be one.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BeatTrigger {
    #[default]
    Sequential,
    OnSignal(SignalId),
}

/// One stop on the target's day: where, how long, and how guarded.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Beat {
    pub pos: Pos,
    pub dwell: u16,
    pub protection: Protection,
    /// Bodyguards hold at the door rather than follow inside.
    pub no_follow: bool,
    pub trigger: BeatTrigger,
    /// Names this beat so an opportunity can summon the target to it.
    pub tag: String,
}

/// The target's day, as labelled beats.
///
/// `beats` is index-aligned with [`AiState::routine`] — same length, same
/// positions, same dwells — so everything that already reads the routine
/// (pathing, briefing facts, reachability proofs) keeps working untouched,
/// while systems that care about protection read the beat at the same
/// index. Generation asserts the alignment.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Schedule {
    pub beats: Vec<Beat>,
    pub index: usize,
    pub dwell_remaining: u16,
    /// Where to resume after an interrupt beat finishes.
    pub resume_index: Option<usize>,
}

impl Schedule {
    /// The beat the target is currently on, if the schedule is non-empty.
    pub fn current(&self) -> Option<&Beat> {
        self.beats.get(self.index)
    }

    /// Every beat where the target stands without its detail.
    pub fn alone_beats(&self) -> impl Iterator<Item = &Beat> {
        self.beats
            .iter()
            .filter(|b| b.protection == Protection::Alone)
    }
}

/// A standing assignment that outlives any single mood.
///
/// Deliberately *not* a [`Mood`]. `Mood::Escorting` already means
/// "escorting the arrested player off the premises", is rendered hostile,
/// and — fatally — every de-escalation path in perception and AI returns
/// only to `Mood::Relaxed`. A bodyguard knocked out of an escorting mood
/// by one noise would never resume, and details would quietly dissolve
/// mid-mission. Keeping the assignment orthogonal means perception retains
/// full authority over mood, and a guard that investigates a sound and
/// calms down goes back to escorting for free.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetailRole {
    Bodyguard {
        principal: ActorId,
        /// Which formation position this guard holds, indexing a fixed
        /// table — never "nearest", which would depend on iteration order.
        slot: u8,
        /// Where to wait while the principal is somewhere guards do not
        /// follow. Resolved when the principal enters such a beat.
        post: Option<Pos>,
        /// Turns spent holding that post. A detail waits, but not
        /// indefinitely — this is the clock that ends a player's window.
        #[serde(default)]
        waited: u16,
    },
}

/// Mutable NPC mind: routine progress, mood, memory, and knowledge.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiState {
    pub routine: Vec<RoutineStep>,
    pub routine_index: usize,
    /// Turns left lingering at the current routine step.
    pub wait_remaining: u16,
    pub mood: Mood,
    /// Suspicion meter (0..=tuning.suspicion_max).
    pub suspicion: u16,
    /// Where trouble was last perceived (incident or player sighting).
    /// Alert propagation shares only this, never the live player position.
    pub focus: Option<Pos>,
    /// Whether this NPC has concluded the player is hostile.
    pub knows_player_hostile: bool,
    /// The target's labelled day. Ordinary NPCs have none and run purely
    /// off `routine`.
    #[serde(default)]
    pub schedule: Option<Schedule>,
    /// A standing assignment, independent of mood.
    #[serde(default)]
    pub detail: Option<DetailRole>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Actor {
    pub id: ActorId,
    pub name: String,
    /// `None` for the player, who has no role.
    pub role: Option<Role>,
    pub pos: Pos,
    /// NPCs always face somewhere; the player has no facing.
    pub facing: Option<Dir4>,
    pub condition: BodyCondition,
    pub hp: u16,
    pub worn_disguise: DisguiseId,
    pub hands: Hands,
    pub crouched: bool,
    pub is_target: bool,
    /// Tagged VIP civilians carry invitations and favour the VIP lounge.
    pub is_vip: bool,
    /// Present on NPCs, absent on the player.
    pub ai: Option<AiState>,
    /// Set when this body has been stowed in a container (it is off-map).
    pub hidden_in: Option<FurnitureId>,
    /// Set when a fleeing NPC escaped through an extraction exit and left
    /// the premises for good.
    pub departed: bool,
    /// Set when the player caused this actor's death; constraint
    /// tracking (collateral, discovered bodies) keys off it.
    #[serde(default)]
    pub killed_by_player: bool,
    /// Whether this body's discovery has already been counted for heat.
    #[serde(default)]
    pub discovery_counted: bool,
}

impl Actor {
    pub fn is_player(&self) -> bool {
        self.role.is_none()
    }

    pub fn alive(&self) -> bool {
        self.condition == BodyCondition::Healthy
    }

    /// A body that can be seen lying on its tile (not hidden, not carried).
    pub fn is_visible_body(&self) -> bool {
        self.condition != BodyCondition::Healthy && self.hidden_in.is_none()
    }
}

/// What kind of transient event an incident is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IncidentKind {
    /// Heard within `radius` regardless of line of sight.
    Gunshot,
    /// A kill in the open; perceived by sight like any other evidence.
    Violence,
    /// A thrown noisemaker: heard within `radius`, draws investigators
    /// without raising an alarm.
    Noise,
}

/// A transient world event others can perceive during the same turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incident {
    pub kind: IncidentKind,
    pub pos: Pos,
    pub radius: i16,
    pub turn: u32,
}

/// Why the mission ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MissionOutcome {
    /// Target dead and the player left through an extraction exit.
    Extracted,
    PlayerKilled,
    Arrested,
    /// The living target fled the premises through an exit: the job is
    /// blown.
    TargetEscaped,
}

/// Raw facts derived from the generated world for the briefing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MissionFacts {
    pub target_name: String,
    pub target_reason: String,
    /// Room names the target's schedule visits.
    pub target_locations: Vec<String>,
    pub guard_count: usize,
    pub staff_count: usize,
    pub civilian_count: usize,
    /// Disguise names present in the mission (worn or in wardrobes).
    pub available_disguises: Vec<String>,
    /// Names of rooms a civilian cannot enter.
    pub restricted_rooms: Vec<String>,
    pub container_count: usize,
    /// Room names offering extraction exits.
    pub extraction_exits: Vec<String>,
    /// Discoverable opportunity hints ("a humming fuse box in the
    /// kitchen").
    #[serde(default)]
    pub opportunities: Vec<String>,
}

/// The complete authoritative simulation state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct World {
    pub seed: u64,
    /// The venue this mission was generated for; venue data supplies
    /// presentation flavour (zone labels, invitation naming).
    pub venue: crate::data::VenueId,
    pub turn: u32,
    pub map: GameMap,
    pub doors: Vec<DoorState>,
    pub rooms: Vec<Room>,
    pub furniture: Vec<Furniture>,
    pub items: Vec<ItemInstance>,
    pub actors: Vec<Actor>,
    pub player: ActorId,
    pub target: ActorId,
    /// Tiles that count as extraction exits when stepped on.
    pub extraction_tiles: Vec<Pos>,
    /// Incidents from the current turn's resolution.
    pub incidents: Vec<Incident>,
    /// Set once any NPC has witnessed the player committing violence;
    /// guards then fight lethally instead of arresting.
    pub player_violence_witnessed: bool,
    /// True while the player is visibly tampering (picking a lock);
    /// witnesses gain suspicion fast.
    #[serde(default)]
    pub player_tampering: bool,
    /// Mission heat: observed crimes accumulate here and drive the
    /// venue's tiered security response. High heat at resolution raises
    /// persistent district heat in the campaign.
    #[serde(default)]
    pub mission_heat: u16,
    /// The heat tier whose response has already been applied.
    #[serde(default)]
    pub heat_tier: u8,
    pub facts: MissionFacts,
    /// What the generated reachability proof established.
    pub proof: crate::generator::proof::ProofReport,
    /// The routes the planner certified at generation.
    pub routes: crate::planner::RouteReport,
    /// The contract's mandatory constraint, if any.
    pub constraint: Option<crate::contract::Constraint>,
    /// Set with a reason the first time the constraint is broken; the
    /// mission continues but the contract resolves unclean.
    pub constraint_breach: Option<String>,
    pub outcome: Option<MissionOutcome>,
    /// Tie-breaker randomness for simultaneous resolution. Command
    /// rejection never touches it.
    pub resolution_rng: Pcg32,
}

impl World {
    pub fn actor(&self, id: ActorId) -> &Actor {
        &self.actors[id.0 as usize]
    }

    pub fn actor_mut(&mut self, id: ActorId) -> &mut Actor {
        &mut self.actors[id.0 as usize]
    }

    pub fn player_actor(&self) -> &Actor {
        self.actor(self.player)
    }

    pub fn player_actor_mut(&mut self) -> &mut Actor {
        let id = self.player;
        self.actor_mut(id)
    }

    pub fn door(&self, id: DoorId) -> &DoorState {
        &self.doors[id.0 as usize]
    }

    pub fn door_mut(&mut self, id: DoorId) -> &mut DoorState {
        &mut self.doors[id.0 as usize]
    }

    pub fn furniture_at(&self, pos: Pos) -> Option<&Furniture> {
        self.furniture.iter().find(|f| f.pos == pos)
    }

    pub fn furniture_mut(&mut self, id: FurnitureId) -> &mut Furniture {
        &mut self.furniture[id.0 as usize]
    }

    /// The standing (conscious, unhidden, undeparted) actor on a tile.
    /// Bodies lying on the ground do not occupy their tile for movement.
    pub fn standing_actor_at(&self, pos: Pos) -> Option<&Actor> {
        self.actors.iter().find(|a| {
            a.alive()
                && !a.departed
                && a.hidden_in.is_none()
                && !self.is_carried(a.id)
                && a.pos == pos
        })
    }

    /// A visible body lying on a tile (for looting, carrying, and evidence).
    pub fn body_at(&self, pos: Pos) -> Option<&Actor> {
        self.actors
            .iter()
            .find(|a| a.is_visible_body() && !self.is_carried(a.id) && a.pos == pos)
    }

    pub fn is_carried(&self, id: ActorId) -> bool {
        self.actors
            .iter()
            .any(|a| a.hands == Hands::CarryingBody(id))
    }

    /// Whether a mover may swap places with this actor instead of being
    /// blocked. Civilians and staff step aside for anyone; guards and the
    /// player hold their ground.
    /// Whether `id` gives way to `mover` specifically.
    ///
    /// A bodyguard stands its ground against everyone except the person it
    /// is guarding. Without this the formation is a cage: guards are not
    /// displaceable, they occupy every side of their principal, and the
    /// principal cannot take a single step — the target simply stops
    /// walking its schedule, which is invisible from any single test and
    /// silently disables every protection rule downstream.
    pub fn is_displaceable_by(&self, id: ActorId, mover: ActorId) -> bool {
        if self.is_displaceable(id) {
            return true;
        }
        matches!(
            self.actor(id).ai.as_ref().and_then(|ai| ai.detail.as_ref()),
            Some(DetailRole::Bodyguard { principal, .. }) if *principal == mover
        ) && self.actor(id).alive()
            && !self.actor(id).departed
    }

    pub fn is_displaceable(&self, id: ActorId) -> bool {
        let actor = self.actor(id);
        !actor.is_player()
            && actor.alive()
            && !actor.departed
            && actor.role != Some(crate::data::Role::Guard)
    }

    pub fn room_at(&self, pos: Pos) -> Option<&Room> {
        room_containing(&self.rooms, pos)
    }

    /// The access zone governing a tile. Room interiors carry their room's
    /// zone; corridors, stairs, and doorways are public circulation space —
    /// access is enforced at room boundaries (recorded decision).
    pub fn zone_at(&self, pos: Pos) -> Zone {
        self.room_at(pos).map(|r| r.zone).unwrap_or(Zone::Public)
    }

    /// True when terrain or furniture blocks movement onto `pos` (actors
    /// are checked separately by resolution).
    pub fn blocks_move(&self, pos: Pos) -> bool {
        if !self.map.walkable(pos, |id| self.door(id).open) {
            // Closed doors are not hard blockers for movement planning;
            // resolution opens unlocked doors implicitly on approach. Walls
            // and void always block.
            if !matches!(self.map.tile(pos), crate::map::TileKind::Door(_)) {
                return true;
            }
        }
        self.furniture_at(pos).is_some()
    }

    /// Sight blocker for a viewer/target pair: terrain, high furniture,
    /// and low cover when either endpoint crouches behind it.
    pub fn sight_blocker<'a>(
        &'a self,
        either_endpoint_crouched: bool,
    ) -> impl Fn(Pos) -> bool + 'a {
        move |pos| {
            if self.map.terrain_blocks_sight(pos, |id| self.door(id).open) {
                return true;
            }
            match self.furniture_at(pos).map(|f| f.kind) {
                Some(FurnitureKind::Container)
                | Some(FurnitureKind::Wardrobe)
                | Some(FurnitureKind::Machine) => true,
                Some(FurnitureKind::LowCover) => either_endpoint_crouched,
                None => false,
            }
        }
    }

    /// Items lying on a tile.
    pub fn items_at(&self, pos: Pos) -> impl Iterator<Item = &ItemInstance> {
        self.items
            .iter()
            .filter(move |i| i.location == ItemLocation::Ground(pos))
    }

    /// Items carried by an actor.
    pub fn carried_items(&self, actor: ActorId) -> impl Iterator<Item = &ItemInstance> {
        self.items
            .iter()
            .filter(move |i| i.location == ItemLocation::CarriedBy(actor))
    }

    /// Whether an actor carries an item satisfying `predicate`.
    pub fn carries(
        &self,
        actor: ActorId,
        data: &GameData,
        predicate: impl Fn(&crate::data::ItemSpec) -> bool,
    ) -> bool {
        self.carried_items(actor)
            .any(|i| data.item(&i.spec).map(&predicate).unwrap_or(false))
    }
}

/// The room containing `pos`, if any — the one shared query for any room
/// set, whether a finished [`World`] or a mid-generation layout. Rooms
/// never overlap, so the first hit is the only hit.
pub fn room_containing(rooms: &[Room], pos: Pos) -> Option<&Room> {
    rooms
        .iter()
        .find(|r| r.floor == pos.floor && r.bounds.contains(pos.x, pos.y))
}
