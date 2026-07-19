//! Authored game data.
//!
//! Everything tunable about the MVP — the disguise permission matrix, room
//! templates, roles and population, action durations, perception and
//! suspicion numbers, name pools, and briefing phrasing — lives in RON files
//! under `data/` at the repository root. The files are embedded at compile
//! time so the native and web builds are guaranteed to ship byte-identical
//! data (a requirement for cross-target deterministic replay), but they are
//! authored and validated as data, never as code.
//!
//! Parsing is strict: unknown fields are rejected, and [`GameData::validate`]
//! cross-checks references between files so a typo fails fast in tests
//! rather than misbehaving mid-mission.
//!
//! # Where the words live
//!
//! Display text is *not* in these files. Every name, label, and authored
//! phrase comes from `data/loc/strings.csv` via [`crate::loc`], keyed by the
//! id the RON file already carries: an item with `id: "garrote"` takes its
//! name from `item.garrote.name`. The RON files hold structure and numbers,
//! the CSV holds words, and neither repeats the other.
//!
//! [`GameData::resolve_text`] fills those fields in after parsing, so every
//! `spec.name` call site keeps working unchanged. Because the RON files no
//! longer carry the text at all, strict parsing means a leftover `name:`
//! field is a load error rather than a value that silently loses to the
//! catalogue.

use serde::{Deserialize, Serialize};

use crate::geom::FloorId;
use crate::loc;

/// Access tiers. The four-tier security gradient is fixed (public,
/// staff, secure, personal); which disguise may enter which tier is
/// data-driven via [`DisguiseSpec`], and each venue supplies display
/// labels (a nightclub calls its secure tier "VIP", a warehouse "the
/// cage").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Zone {
    Public,
    Secure,
    Staff,
    Personal,
}

impl Zone {
    /// The neutral tier name; venue data supplies flavoured labels.
    pub fn name(self) -> &'static str {
        match self {
            Zone::Public => crate::tr!("zone.public"),
            Zone::Secure => crate::tr!("zone.secure"),
            Zone::Staff => crate::tr!("zone.staff"),
            Zone::Personal => crate::tr!("zone.personal"),
        }
    }

    /// How far into the venue this tier sits. A district may nest into an
    /// equal or deeper tier, never a shallower one: that monotone gradient is
    /// what makes the layout read outward-to-inward whatever its shape.
    /// `Secure` and `Staff` are siblings — two different ways to be one step
    /// off the street, neither behind the other.
    pub fn depth(self) -> u8 {
        match self {
            Zone::Public => 0,
            Zone::Secure | Zone::Staff => 1,
            Zone::Personal => 2,
        }
    }

    pub const ALL: [Zone; 4] = [Zone::Public, Zone::Secure, Zone::Staff, Zone::Personal];
}

/// Fixed MVP roles. Civilians and guards have their own behaviour sets;
/// bartender, cleaner, technician, and manager share the staff model. The
/// mission target is a staff-model actor flagged on the world, not a role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Role {
    Civilian,
    Bartender,
    Cleaner,
    Technician,
    Manager,
    Guard,
}

impl Role {
    pub fn is_staff(self) -> bool {
        matches!(
            self,
            Role::Bartender | Role::Cleaner | Role::Technician | Role::Manager
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Role::Civilian => crate::tr!("role.civilian"),
            Role::Bartender => crate::tr!("role.bartender"),
            Role::Cleaner => crate::tr!("role.cleaner"),
            Role::Technician => crate::tr!("role.technician"),
            Role::Manager => crate::tr!("role.manager"),
            Role::Guard => crate::tr!("role.guard"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lighting {
    Bright,
    Dim,
}

/// What a routine waypoint is for; roles declare which kinds they use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WaypointKind {
    /// Drinking, dancing, chatting — civilian leisure.
    Social,
    /// Behind the bar, kitchen stations, cleaning spots, office desks.
    Work,
    /// Guard patrol posts.
    Patrol,
    /// Quiet corners where an actor may simply stand.
    Idle,
}

pub type DisguiseId = String;
pub type RoomTemplateId = String;
pub type ItemSpecId = String;
pub type VenueId = String;

/// Display labels a venue gives the four access tiers. Filled from
/// `venue.<id>.zone.*` in the string catalogue, not from RON.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneLabels {
    pub public: String,
    pub staff: String,
    pub secure: String,
    pub personal: String,
}

/// A venue definition: its footprint, which room templates it draws
/// from, its district tree, and its presentation flavour. The same
/// contract, planner, opportunity, heat, and campaign systems apply to
/// every venue; nothing outside data may special-case one.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueSpec {
    pub id: VenueId,
    /// From `venue.<id>.name`.
    #[serde(default)]
    pub name: String,
    /// From `venue.<id>.zone.*`.
    #[serde(default)]
    pub zone_labels: ZoneLabels,
    /// Flavoured name of the secure-tier invitation ("VIP invitation",
    /// "visitor pass"). From `venue.<id>.invitation`.
    #[serde(default)]
    pub invitation_label: String,
    /// Interior tile size of each storey and the storey count.
    pub floor_width: u16,
    pub floor_height: u16,
    pub floor_count: u8,
    /// Room templates this venue may place.
    pub room_templates: Vec<RoomTemplateId>,
    /// Per-venue overrides of the population role counts (a warehouse
    /// has few guests and more guards than a nightclub).
    #[serde(default)]
    pub role_counts: Vec<RoleCountOverride>,
    /// The district tree this venue is carved from. Its shape alone
    /// decides the topology — see `generator::district`.
    pub districts: DistrictPattern,
}

/// One district in a venue's tree. The tree's *shape* is the venue's
/// topology: a chain nests (onion), siblings branch off one spine
/// (festival), and siblings that are themselves chains give independent
/// fortresses (archipelago). The engine reads no form flag.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistrictPattern {
    pub zone: Zone,
    /// Room templates this district may place, in authored order.
    #[serde(default)]
    pub rooms: Vec<RoomTemplateId>,
    /// How many sibling districts this pattern expands into.
    #[serde(default = "one")]
    pub count_min: u8,
    #[serde(default = "one")]
    pub count_max: u8,
    /// Put this district (and its subtree) on its own storey, reached by
    /// a stairwell from the parent's spine rather than a doorway.
    #[serde(default)]
    pub own_storey: bool,
    /// Lock the gateway into this district with the named room's key.
    #[serde(default)]
    pub locked_by: Option<ItemSpecId>,
    #[serde(default)]
    pub children: Vec<DistrictPattern>,
}

fn one() -> u8 {
    1
}

/// One per-venue role count override.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleCountOverride {
    pub role: Role,
    pub count_min: u8,
    pub count_max: u8,
}

impl VenueSpec {
    pub fn zone_label(&self, zone: Zone) -> &str {
        match zone {
            Zone::Public => &self.zone_labels.public,
            Zone::Staff => &self.zone_labels.staff,
            Zone::Secure => &self.zone_labels.secure,
            Zone::Personal => &self.zone_labels.personal,
        }
    }
}

/// One row of the disguise permission matrix.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisguiseSpec {
    pub id: DisguiseId,
    /// From `disguise.<id>.name`.
    #[serde(default)]
    pub name: String,
    /// Zones this disguise may enter unconditionally.
    pub zones: Vec<Zone>,
    /// Room templates accessible regardless of zone (the authored "partial
    /// private access" for staff).
    pub extra_rooms: Vec<RoomTemplateId>,
    /// Whether carrying an invitation item additionally grants VIP access.
    pub secure_with_invitation: bool,
    /// Roles that treat a player wearing this disguise as suspicious on
    /// sight (they would know the real person).
    pub suspicious_observers: Vec<Role>,
    /// Whether a drawn weapon is legal while wearing this disguise.
    pub drawn_weapon_legal: bool,
}

/// A carriable item definition.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemSpec {
    pub id: ItemSpecId,
    /// From `item.<id>.name`.
    #[serde(default)]
    pub name: String,
    pub glyph: char,
    /// Room template whose locked doors this item opens, if it is a key.
    pub unlocks: Option<RoomTemplateId>,
    /// True for the persistent, non-consumable VIP invitation.
    pub invitation: bool,
    /// True for weapons (illegal when drawn, per disguise matrix).
    pub weapon: bool,
    /// True for ranged weapons that draw, aim, and consume rounds; a
    /// weapon without it is a silent melee tool.
    #[serde(default)]
    pub firearm: bool,
    /// Campaign equipment: enters missions only through the loadout,
    /// never generated into the venue.
    #[serde(default)]
    pub purchasable: bool,
    /// Opens locked doors without their key (the pick-lock action).
    #[serde(default)]
    pub lockpick: bool,
    /// Can be thrown to create a noise that draws investigators.
    #[serde(default)]
    pub noisemaker: bool,
    /// Grants staff-tier legitimacy while carried, whatever is worn.
    #[serde(default)]
    pub staff_pass: bool,
    /// Opens every locked door in the venue (the key-cache opportunity).
    #[serde(default)]
    pub master_key: bool,
    /// Charges the item starts with (pistol rounds, noisemaker uses).
    #[serde(default)]
    pub charges: u16,
    /// Whether the item can be pickpocketed from a carrying NPC.
    pub pickpocketable: bool,
    /// Role that carries this item at generation, if any.
    pub carried_by: Option<Role>,
    /// Room template on whose floor this item is placed at generation,
    /// if any.
    pub placed_in: Option<RoomTemplateId>,
}

/// The three approaches the equipment catalogue serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Approach {
    Physical,
    Social,
    Violence,
}

impl Approach {
    pub fn name(self) -> &'static str {
        match self {
            Approach::Physical => crate::tr!("approach.physical"),
            Approach::Social => crate::tr!("approach.social"),
            Approach::Violence => crate::tr!("approach.violence"),
        }
    }

    pub const ALL: [Approach; 3] = [Approach::Physical, Approach::Social, Approach::Violence];
}

/// One catalogue entry: a purchasable item, its approach, and its price.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EquipmentSpec {
    pub item: ItemSpecId,
    pub approach: Approach,
    pub price: i64,
}

/// Which route posture an opportunity machine primarily serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpportunityApproach {
    Physical,
    Social,
    Violence,
    Universal,
}

/// What using (or placing) an opportunity machine does. Effects are the
/// vocabulary shared by generation, the planner, and resolution — the
/// same vocabulary a future mission-scripting layer will compose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpportunityEffect {
    /// Cuts the lights: every bright room on the machine's storey dims.
    CutLights,
    /// Placement stocks a wardrobe with a disguise (no interaction).
    StockWardrobe { disguise: DisguiseId },
    /// Drops a load on whoever stands beneath: a deniable accident kill.
    AccidentDrop,
    /// Placement drops an item on the floor nearby (no interaction).
    PlaceKey { item: ItemSpecId },
    /// Triggers an evacuation: civilians and staff flee, guards respond.
    Evacuate,
}

/// One authored opportunity machine.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpportunitySpec {
    pub id: String,
    /// From `opportunity.<id>.name`.
    #[serde(default)]
    pub name: String,
    pub glyph: char,
    pub approach: OpportunityApproach,
    pub effect: OpportunityEffect,
    /// Zones whose rooms may host this machine.
    pub zones: Vec<Zone>,
    /// Turns the interaction takes.
    pub interact_turns: u16,
    /// Authored risk statement (briefing and inspection). From
    /// `opportunity.<id>.risk`.
    #[serde(default)]
    pub risk: String,
    /// Discoverable presentation: what looking at it tells the player.
    /// From `opportunity.<id>.presentation`.
    #[serde(default)]
    pub presentation: String,
}

/// How many waypoints of one kind a room offers.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaypointSlot {
    pub kind: WaypointKind,
    pub count: u8,
}

/// A room the generator may (or must) place.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomTemplate {
    pub id: RoomTemplateId,
    /// From `room.<id>.name`.
    #[serde(default)]
    pub name: String,
    pub zone: Zone,
    /// Storeys this room may be placed on (0 = ground).
    pub floors: Vec<FloorId>,
    pub count_min: u8,
    pub count_max: u8,
    /// Required rooms must be placed; optional rooms may be dropped if
    /// placement fails.
    pub required: bool,
    /// Interior size bounds in tiles, excluding walls.
    pub min_size: (u16, u16),
    pub max_size: (u16, u16),
    pub lighting: Lighting,
    pub waypoints: Vec<WaypointSlot>,
    /// One-body containers (crates, freezers, laundry carts).
    pub containers_min: u8,
    pub containers_max: u8,
    /// Low cover objects (desks, tables) that block sight to crouchers.
    pub low_cover_min: u8,
    pub low_cover_max: u8,
    /// Whether the reachability proof may add a wardrobe here.
    pub wardrobe_allowed: bool,
    /// Whether this room connects to the outside as an extraction exit.
    pub external_exit: bool,
    /// Item id of the key that locks this room's doors, if locked.
    pub locked_by: Option<ItemSpecId>,
    /// Whether this room gets a service connection (a door onto the
    /// service corridor) when the graph grammar places it on the service
    /// side of a storey.
    #[serde(default)]
    pub service_access: bool,
    /// Circulation templates (service corridors) are realised
    /// structurally by the grammar, not packed as rooms.
    #[serde(default)]
    pub circulation: bool,
}

/// Population and behaviour parameters for one role.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSpec {
    pub role: Role,
    pub glyph: char,
    pub count_min: u8,
    pub count_max: u8,
    /// Waypoint kinds this role's generated routine draws from.
    pub waypoint_kinds: Vec<WaypointKind>,
    /// Turns spent waiting at each routine waypoint.
    pub wait_min: u16,
    pub wait_max: u16,
    /// Disguise obtainable from this actor's clothing, if any.
    pub disguise: Option<DisguiseId>,
    /// Armed actors fight lethally once combat starts.
    pub armed: bool,
}

/// Target-specific generation parameters (the target uses the staff model).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetSpec {
    pub glyph: char,
    /// Number of scheduled waypoints in the target's richer routine.
    pub schedule_min: u8,
    pub schedule_max: u8,
    /// Disguise obtainable from the target's clothing, if any.
    pub disguise: Option<DisguiseId>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PopulationData {
    pub roles: Vec<RoleSpec>,
    /// How many civilians are tagged VIP (carrying invitations, favouring
    /// the VIP lounge).
    pub vip_civilians_min: u8,
    pub vip_civilians_max: u8,
    pub target: TargetSpec,
    /// Turns one full pass of the target's schedule should take. Dwells
    /// are fitted to hit this budget, so mission length is authored here
    /// rather than falling out of how far apart rooms landed.
    pub cycle_turns_min: u16,
    pub cycle_turns_max: u16,
    /// How many beats of the cycle the target spends alone — the windows
    /// a weapon kill needs.
    pub private_beats_min: u8,
    pub private_beats_max: u8,
}

/// Given and family name pools for generated actors. Both come wholly from
/// the string catalogue (`names.first.*`, `names.last.*`); there is no RON
/// file behind them.
#[derive(Clone, Debug, Default)]
pub struct NamePools {
    pub first: Vec<String>,
    pub last: Vec<String>,
}

/// Briefing phrasing, wholly from the catalogue (`briefing.reason.*`).
#[derive(Clone, Debug, Default)]
pub struct BriefingData {
    /// Generated target-elimination reasons; the seed picks one.
    pub reasons: Vec<String>,
}

/// Per-action durations in turns. Multi-turn actions progress once per turn
/// and apply their effect on the final turn; the design allows partially
/// completed actions to continue the command queue.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDurations {
    pub step: u16,
    /// A step while carrying a body (halved movement cadence).
    pub carry_step: u16,
    pub garrote: u16,
    pub shoot: u16,
    pub change_disguise: u16,
    pub carry_body: u16,
    pub drop_body: u16,
    pub hide_body: u16,
    pub pickpocket: u16,
    pub door: u16,
    pub draw_holster: u16,
    pub pick_lock: u16,
    pub throw: u16,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tuning {
    /// Exact visible player command queue capacity.
    pub queue_capacity: u16,
    /// Simulation turns resolved per cooperative batch before the driver
    /// yields for input and presentation.
    pub batch_turns: u16,

    pub pistol_rounds: u16,
    pub pistol_range: i16,
    pub gunshot_sound_radius: i16,
    pub player_max_hp: u16,
    pub guard_attack_damage: u16,

    /// NPC vision distance in tiles.
    pub vision_range: i16,
    /// Vision distance in dim rooms.
    pub vision_range_dim: i16,
    /// Cone ratio: a tile is inside the facing cone when
    /// `perp * cone_den <= along * cone_num`.
    pub cone_num: i32,
    pub cone_den: i32,

    /// Suspicion meter bounds and state thresholds.
    pub suspicion_max: u16,
    pub suspicion_suspicious_at: u16,
    pub suspicion_investigate_at: u16,
    /// Per-turn-seen suspicion gains by cause.
    pub gain_illegal_zone: u16,
    pub gain_crouching: u16,
    pub gain_disguise_recognised: u16,
    pub suspicion_decay: u16,

    /// Relaxed NPCs prepare an action every `relaxed_cadence` turns,
    /// staggered by actor id; escalated NPCs act every turn.
    pub relaxed_cadence: u16,
    /// Turns an investigator lingers at the spot they checked.
    pub investigate_linger: u16,
    /// Noisemaker throw distance and how far its crack carries.
    pub noisemaker_range: i16,
    pub noise_radius: i16,
    /// Suspicion per turn an NPC watches the player pick a lock.
    pub gain_tampering: u16,
    /// Mission heat weights and tier thresholds.
    pub heat_gunshot: u16,
    pub heat_violence: u16,
    pub heat_body_found: u16,
    pub heat_tier1: u16,
    pub heat_tier2: u16,
    /// Guards spawned at the entrance when heat reaches tier two.
    pub heat_reinforcements: u8,
    /// The most extra guards persistent district heat may add at
    /// generation.
    pub heat_extra_guard_cap: u8,
    /// Tiles of the player's own field of view.
    pub player_vision_range: i16,

    pub durations: ActionDurations,
}

/// Campaign-layer tunables: districts, economy, heat persistence.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignData {
    /// From `campaign.district.*`; the RON file carries no district names.
    #[serde(default)]
    pub districts: Vec<String>,
    pub starting_cash: i64,
    /// Equipment a fresh campaign owns from the start.
    pub starting_equipment: Vec<ItemSpecId>,
    pub arrest_fine: i64,
    pub payout_base: i64,
    pub payout_per_heat: i64,
    pub payout_constraint_bonus: i64,
    /// Mission heat at or above this raises the district's persistent
    /// heat on resolution.
    pub hot_mission_threshold: u16,
    pub district_heat_max: u8,
    pub offers_per_batch: u8,
}

/// All authored data, cross-validated.
#[derive(Clone, Debug)]
pub struct GameData {
    pub tuning: Tuning,
    pub venues: Vec<VenueSpec>,
    pub disguises: Vec<DisguiseSpec>,
    pub items: Vec<ItemSpec>,
    pub equipment: Vec<EquipmentSpec>,
    pub opportunities: Vec<OpportunitySpec>,
    pub rooms: Vec<RoomTemplate>,
    pub population: PopulationData,
    pub names: NamePools,
    pub briefing: BriefingData,
    pub campaign: CampaignData,
}

/// A data authoring error: which file, and what is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataError {
    pub file: &'static str,
    pub message: String,
}

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

impl std::error::Error for DataError {}

const TUNING_RON: &str = include_str!("../../../data/tuning.ron");
const VENUES_RON: &str = include_str!("../../../data/venues.ron");
const DISGUISES_RON: &str = include_str!("../../../data/disguises.ron");
const ITEMS_RON: &str = include_str!("../../../data/items.ron");
const EQUIPMENT_RON: &str = include_str!("../../../data/equipment.ron");
const OPPORTUNITIES_RON: &str = include_str!("../../../data/opportunities.ron");
const ROOMS_RON: &str = include_str!("../../../data/rooms.ron");
const POPULATION_RON: &str = include_str!("../../../data/population.ron");
const CAMPAIGN_RON: &str = include_str!("../../../data/campaign.ron");

fn parse<T: serde::de::DeserializeOwned>(file: &'static str, text: &str) -> Result<T, DataError> {
    ron::from_str(text).map_err(|err| DataError {
        file,
        message: err.to_string(),
    })
}

impl GameData {
    /// Loads the data files embedded in the binary.
    pub fn embedded() -> Result<GameData, DataError> {
        Self::from_sources(
            TUNING_RON,
            VENUES_RON,
            DISGUISES_RON,
            ITEMS_RON,
            EQUIPMENT_RON,
            OPPORTUNITIES_RON,
            ROOMS_RON,
            POPULATION_RON,
            CAMPAIGN_RON,
        )
    }

    /// Parses and cross-validates a full data set from RON sources.
    #[allow(clippy::too_many_arguments)]
    pub fn from_sources(
        tuning: &str,
        venues: &str,
        disguises: &str,
        items: &str,
        equipment: &str,
        opportunities: &str,
        rooms: &str,
        population: &str,
        campaign: &str,
    ) -> Result<GameData, DataError> {
        let mut data = GameData {
            tuning: parse("tuning.ron", tuning)?,
            venues: parse("venues.ron", venues)?,
            disguises: parse("disguises.ron", disguises)?,
            items: parse("items.ron", items)?,
            equipment: parse("equipment.ron", equipment)?,
            opportunities: parse("opportunities.ron", opportunities)?,
            rooms: parse("rooms.ron", rooms)?,
            population: parse("population.ron", population)?,
            names: NamePools::default(),
            briefing: BriefingData::default(),
            campaign: parse("campaign.ron", campaign)?,
        };
        // Words before cross-checks: `validate` reports on empty pools, and
        // the pools do not exist until the catalogue has been read.
        data.resolve_text();
        data.validate()?;
        Ok(data)
    }

    /// Fills every display field from `data/loc/strings.csv`.
    ///
    /// Ids are derived from the structural id a spec already has, so adding
    /// a room template means adding one `room.<id>.name` row and nothing
    /// else. A spec whose row is missing keeps the catalogue's loud
    /// placeholder rather than an empty string, so the gap shows up on
    /// screen instead of rendering as a blank name; `validate` then turns
    /// it into a load error.
    fn resolve_text(&mut self) {
        for venue in &mut self.venues {
            venue.name = loc::text(&format!("venue.{}.name", venue.id)).to_string();
            venue.invitation_label =
                loc::text(&format!("venue.{}.invitation", venue.id)).to_string();
            venue.zone_labels = ZoneLabels {
                public: loc::text(&format!("venue.{}.zone.public", venue.id)).to_string(),
                staff: loc::text(&format!("venue.{}.zone.staff", venue.id)).to_string(),
                secure: loc::text(&format!("venue.{}.zone.secure", venue.id)).to_string(),
                personal: loc::text(&format!("venue.{}.zone.personal", venue.id)).to_string(),
            };
        }
        for disguise in &mut self.disguises {
            disguise.name = loc::text(&format!("disguise.{}.name", disguise.id)).to_string();
        }
        for item in &mut self.items {
            item.name = loc::text(&format!("item.{}.name", item.id)).to_string();
        }
        for spec in &mut self.opportunities {
            spec.name = loc::text(&format!("opportunity.{}.name", spec.id)).to_string();
            spec.risk = loc::text(&format!("opportunity.{}.risk", spec.id)).to_string();
            spec.presentation =
                loc::text(&format!("opportunity.{}.presentation", spec.id)).to_string();
        }
        for room in &mut self.rooms {
            room.name = loc::text(&format!("room.{}.name", room.id)).to_string();
        }
        let pool = |prefix: &str| -> Vec<String> {
            loc::catalogue()
                .with_prefix(prefix)
                .into_iter()
                .map(str::to_string)
                .collect()
        };
        self.names.first = pool("names.first.");
        self.names.last = pool("names.last.");
        self.briefing.reasons = pool("briefing.reason.");
        self.campaign.districts = pool("campaign.district.");
    }

    pub fn venue(&self, id: &str) -> Option<&VenueSpec> {
        self.venues.iter().find(|v| v.id == id)
    }

    pub fn opportunity(&self, id: &str) -> Option<&OpportunitySpec> {
        self.opportunities.iter().find(|o| o.id == id)
    }

    pub fn disguise(&self, id: &str) -> Option<&DisguiseSpec> {
        self.disguises.iter().find(|d| d.id == id)
    }

    pub fn item(&self, id: &str) -> Option<&ItemSpec> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn room_template(&self, id: &str) -> Option<&RoomTemplate> {
        self.rooms.iter().find(|r| r.id == id)
    }

    pub fn role_spec(&self, role: Role) -> Option<&RoleSpec> {
        self.population.roles.iter().find(|r| r.role == role)
    }

    fn validate(&self) -> Result<(), DataError> {
        let mut errors: Vec<String> = Vec::new();

        let mut disguise_ids: Vec<&str> = self.disguises.iter().map(|d| d.id.as_str()).collect();
        disguise_ids.sort_unstable();
        disguise_ids.dedup();
        if disguise_ids.len() != self.disguises.len() {
            errors.push("duplicate disguise ids".into());
        }

        for disguise in &self.disguises {
            for room in &disguise.extra_rooms {
                if self.room_template(room).is_none() {
                    errors.push(format!(
                        "disguise '{}' grants access to unknown room '{room}'",
                        disguise.id
                    ));
                }
            }
        }

        for item in &self.items {
            if let Some(room) = &item.unlocks
                && self.room_template(room).is_none()
            {
                errors.push(format!("item '{}' unlocks unknown room '{room}'", item.id));
            }
            if let Some(room) = &item.placed_in
                && self.room_template(room).is_none()
            {
                errors.push(format!(
                    "item '{}' placed in unknown room '{room}'",
                    item.id
                ));
            }
        }

        for room in &self.rooms {
            if room.count_min > room.count_max {
                errors.push(format!("room '{}' has count_min > count_max", room.id));
            }
            if room.min_size.0 > room.max_size.0 || room.min_size.1 > room.max_size.1 {
                errors.push(format!("room '{}' has min_size > max_size", room.id));
            }
            if room.floors.is_empty() {
                errors.push(format!("room '{}' allows no floors", room.id));
            }
            if let Some(key) = &room.locked_by {
                match self.item(key) {
                    None => errors.push(format!(
                        "room '{}' is locked by unknown item '{key}'",
                        room.id
                    )),
                    Some(item) if item.unlocks.as_deref() != Some(room.id.as_str()) => {
                        errors.push(format!(
                            "room '{}' is locked by '{key}' but that item does not unlock it",
                            room.id
                        ))
                    }
                    Some(_) => {}
                }
            }
        }

        for role_spec in &self.population.roles {
            if role_spec.count_min > role_spec.count_max {
                errors.push(format!(
                    "role '{}' has count_min > count_max",
                    role_spec.role.name()
                ));
            }
            if let Some(disguise) = &role_spec.disguise
                && self.disguise(disguise).is_none()
            {
                errors.push(format!(
                    "role '{}' yields unknown disguise '{disguise}'",
                    role_spec.role.name()
                ));
            }
        }
        if let Some(disguise) = &self.population.target.disguise
            && self.disguise(disguise).is_none()
        {
            errors.push(format!("target yields unknown disguise '{disguise}'"));
        }

        if self.names.first.is_empty() || self.names.last.is_empty() {
            errors.push("name pools must not be empty".into());
        }
        if self.briefing.reasons.is_empty() {
            errors.push("briefing must offer at least one elimination reason".into());
        }

        // Every spec that took its words from the catalogue must actually
        // have found them. Without this a new room template silently ships
        // as "!!MISSING STRING!!" on the briefing.
        let mut missing = |what: &str, id: &str, text: &str| {
            if text == loc::MISSING {
                errors.push(format!(
                    "no localised {what} for '{id}' in data/loc/strings.csv"
                ));
            }
        };
        for venue in &self.venues {
            missing("venue name", &venue.id, &venue.name);
            missing("invitation label", &venue.id, &venue.invitation_label);
            for zone in Zone::ALL {
                missing("zone label", &venue.id, venue.zone_label(zone));
            }
        }
        for disguise in &self.disguises {
            missing("disguise name", &disguise.id, &disguise.name);
        }
        for item in &self.items {
            missing("item name", &item.id, &item.name);
        }
        for spec in &self.opportunities {
            missing("opportunity name", &spec.id, &spec.name);
            missing("opportunity risk", &spec.id, &spec.risk);
            missing("opportunity presentation", &spec.id, &spec.presentation);
        }
        for room in &self.rooms {
            missing("room name", &room.id, &room.name);
        }
        if self.tuning.queue_capacity == 0 {
            errors.push("queue_capacity must be positive".into());
        }
        if self.tuning.cone_den <= 0 || self.tuning.cone_num <= 0 {
            errors.push("cone ratio must be positive".into());
        }
        for entry in &self.equipment {
            match self.item(&entry.item) {
                None => errors.push(format!(
                    "equipment references unknown item '{}'",
                    entry.item
                )),
                Some(spec) if !spec.purchasable => errors.push(format!(
                    "equipment item '{}' must be purchasable",
                    entry.item
                )),
                Some(_) => {}
            }
            if entry.price <= 0 {
                errors.push(format!("equipment '{}' must cost something", entry.item));
            }
        }
        for approach in Approach::ALL {
            let count = self
                .equipment
                .iter()
                .filter(|e| e.approach == approach)
                .count();
            if count != 2 {
                errors.push(format!(
                    "equipment catalogue needs exactly two {} choices, found {count}",
                    approach.name()
                ));
            }
        }
        if let Some(pistol) = self.item("silenced-pistol")
            && pistol.charges != self.tuning.pistol_rounds
        {
            errors.push("silenced pistol charges must match tuning pistol_rounds".into());
        }

        {
            use OpportunityApproach as OA;
            let count_of = |a: OA| {
                self.opportunities
                    .iter()
                    .filter(|o| o.approach == a)
                    .count()
            };
            if self.opportunities.len() != 5
                || count_of(OA::Physical) != 1
                || count_of(OA::Social) != 1
                || count_of(OA::Violence) != 1
                || count_of(OA::Universal) != 2
            {
                errors.push(
                    "opportunities must be exactly five: one physical, one social, one violence, two universal"
                        .into(),
                );
            }
            for spec in &self.opportunities {
                if spec.zones.is_empty() {
                    errors.push(format!("opportunity '{}' allows no zones", spec.id));
                }
                if spec.interact_turns == 0 {
                    errors.push(format!("opportunity '{}' must take time", spec.id));
                }
                match &spec.effect {
                    OpportunityEffect::StockWardrobe { disguise }
                        if self.disguise(disguise).is_none() =>
                    {
                        errors.push(format!(
                            "opportunity '{}' stocks unknown disguise '{disguise}'",
                            spec.id
                        ));
                    }
                    OpportunityEffect::PlaceKey { item } if self.item(item).is_none() => {
                        errors.push(format!(
                            "opportunity '{}' places unknown item '{item}'",
                            spec.id
                        ));
                    }
                    _ => {}
                }
            }
        }

        if self.campaign.districts.is_empty() {
            errors.push("campaign needs at least one district".into());
        }
        if self.campaign.offers_per_batch == 0 {
            errors.push("campaign must offer at least one contract".into());
        }
        for item in &self.campaign.starting_equipment {
            match self.item(item) {
                None => errors.push(format!("starting equipment '{item}' is unknown")),
                Some(spec) if !spec.purchasable => {
                    errors.push(format!("starting equipment '{item}' must be purchasable"))
                }
                Some(_) => {}
            }
        }
        if self.campaign.starting_cash < 0 || self.campaign.arrest_fine < 0 {
            errors.push("campaign cash values must be non-negative".into());
        }

        if self.venues.is_empty() {
            errors.push("at least one venue must be defined".into());
        }
        let mut venue_ids: Vec<&str> = self.venues.iter().map(|v| v.id.as_str()).collect();
        venue_ids.sort_unstable();
        venue_ids.dedup();
        if venue_ids.len() != self.venues.len() {
            errors.push("duplicate venue ids".into());
        }
        for venue in &self.venues {
            if venue.floor_width == 0 || venue.floor_height == 0 || venue.floor_count == 0 {
                errors.push(format!("venue '{}' has a degenerate footprint", venue.id));
            }
            // Stair links carry any height, but the map panel only names
            // storeys sensibly this far and taller venues stop being
            // navigable in a terminal.
            if venue.floor_count > 4 {
                errors.push(format!(
                    "venue '{}' has {} storeys; four is the ceiling",
                    venue.id, venue.floor_count
                ));
            }
            if venue.room_templates.is_empty() {
                errors.push(format!("venue '{}' places no rooms", venue.id));
            }
            for over in &venue.role_counts {
                if over.count_min > over.count_max {
                    errors.push(format!(
                        "venue '{}' role override for {} has min > max",
                        venue.id,
                        over.role.name()
                    ));
                }
            }
            let mut has_required_exit = false;
            for template_id in &venue.room_templates {
                let Some(template) = self.room_template(template_id) else {
                    errors.push(format!(
                        "venue '{}' references unknown room '{template_id}'",
                        venue.id
                    ));
                    continue;
                };
                if template.required && template.external_exit {
                    has_required_exit = true;
                }
                for floor in &template.floors {
                    if *floor >= venue.floor_count {
                        errors.push(format!(
                            "venue '{}' places room '{template_id}' on floor {floor} but has {} floors",
                            venue.id, venue.floor_count
                        ));
                    }
                }
            }
            if !has_required_exit {
                errors.push(format!(
                    "venue '{}' needs a required external-exit room",
                    venue.id
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(DataError {
                file: "(cross-validation)",
                message: errors.join("; "),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_data_parses_and_validates() {
        let data = GameData::embedded().expect("embedded data must be valid");
        assert_eq!(data.tuning.queue_capacity, 32, "spec fixes capacity at 32");
        assert_eq!(
            data.tuning.pistol_rounds, 6,
            "spec fixes pistol rounds at 6"
        );
        assert!(data.disguise("civilian").is_some());
        assert!(data.disguise("guard").is_some());
    }

    #[test]
    fn permission_matrix_matches_foundation() {
        let data = GameData::embedded().unwrap();
        let civilian = data.disguise("civilian").unwrap();
        assert!(civilian.zones.contains(&Zone::Public));
        assert!(!civilian.zones.contains(&Zone::Secure));
        assert!(civilian.secure_with_invitation);

        let staff = data.disguise("staff").unwrap();
        assert!(staff.zones.contains(&Zone::Public));
        assert!(staff.zones.contains(&Zone::Staff));
        assert!(!staff.zones.contains(&Zone::Personal));
        assert!(
            !staff.extra_rooms.is_empty(),
            "staff must have authored partial private access"
        );

        for id in ["guard", "manager"] {
            let disguise = data.disguise(id).unwrap();
            for zone in [Zone::Public, Zone::Secure, Zone::Staff, Zone::Personal] {
                assert!(disguise.zones.contains(&zone), "{id} must access {zone:?}");
            }
        }
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let result: Result<EquipmentSpec, _> =
            ron::from_str("(item: \"garrote\", approach: Violence, price: 1, surprise: 1)");
        assert!(result.is_err());
    }

    /// Words come from the catalogue now, and the RON files must not carry
    /// them any more.
    ///
    /// Serde cannot enforce this: the fields still exist on the structs, so
    /// a `name:` in RON parses happily and is then overwritten by
    /// [`GameData::resolve_text`] — authored data that reads as live but
    /// silently loses. So the files themselves are the thing checked.
    #[test]
    fn ron_files_carry_no_display_text() {
        let files = [
            ("venues.ron", VENUES_RON),
            ("disguises.ron", DISGUISES_RON),
            ("items.ron", ITEMS_RON),
            ("opportunities.ron", OPPORTUNITIES_RON),
            ("rooms.ron", ROOMS_RON),
            ("campaign.ron", CAMPAIGN_RON),
        ];
        let text_keys = [
            "name:",
            "risk:",
            "presentation:",
            "invitation_label:",
            "zone_labels:",
        ];
        for (file, source) in files {
            for (number, line) in source.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("").trim();
                for key in text_keys {
                    assert!(
                        !code.starts_with(key),
                        "{file}:{}: '{key}' belongs in data/loc/strings.csv",
                        number + 1
                    );
                }
            }
        }
    }

    /// Display text reaches the specs, and reaches them from the catalogue.
    #[test]
    fn specs_take_their_names_from_the_catalogue() {
        let data = GameData::embedded().unwrap();
        assert_eq!(
            data.item("garrote").unwrap().name,
            crate::loc::text("item.garrote.name")
        );
        assert_eq!(
            data.venue("nightclub").unwrap().zone_label(Zone::Secure),
            crate::loc::text("venue.nightclub.zone.secure")
        );
        assert!(!data.names.first.is_empty() && !data.campaign.districts.is_empty());
    }
}
