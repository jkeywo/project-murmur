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

use serde::{Deserialize, Serialize};

use crate::geom::FloorId;

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
            Zone::Public => "public",
            Zone::Secure => "secure",
            Zone::Staff => "staff",
            Zone::Personal => "personal",
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
            Role::Civilian => "civilian",
            Role::Bartender => "bartender",
            Role::Cleaner => "cleaner",
            Role::Technician => "technician",
            Role::Manager => "manager",
            Role::Guard => "guard",
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

/// Display labels a venue gives the four access tiers.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneLabels {
    pub public: String,
    pub staff: String,
    pub secure: String,
    pub personal: String,
}

/// A venue definition: its footprint, which room templates it draws
/// from, and its presentation flavour. The same contract, planner,
/// grammar, opportunity, heat, and campaign systems apply to every
/// venue; nothing outside data may special-case one.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VenueSpec {
    pub id: VenueId,
    pub name: String,
    pub zone_labels: ZoneLabels,
    /// Flavoured name of the secure-tier invitation ("VIP invitation",
    /// "visitor pass").
    pub invitation_label: String,
    /// Interior tile size of each storey and the storey count.
    pub floor_width: u16,
    pub floor_height: u16,
    pub floor_count: u8,
    /// Room templates this venue may place.
    pub room_templates: Vec<RoomTemplateId>,
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
            Approach::Physical => "physical stealth",
            Approach::Social => "social stealth",
            Approach::Violence => "violence",
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
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamePools {
    pub first: Vec<String>,
    pub last: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Tiles of the player's own field of view.
    pub player_vision_range: i16,

    pub durations: ActionDurations,
}

/// All authored data, cross-validated.
#[derive(Clone, Debug)]
pub struct GameData {
    pub tuning: Tuning,
    pub venues: Vec<VenueSpec>,
    pub disguises: Vec<DisguiseSpec>,
    pub items: Vec<ItemSpec>,
    pub equipment: Vec<EquipmentSpec>,
    pub rooms: Vec<RoomTemplate>,
    pub population: PopulationData,
    pub names: NamePools,
    pub briefing: BriefingData,
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
const ROOMS_RON: &str = include_str!("../../../data/rooms.ron");
const POPULATION_RON: &str = include_str!("../../../data/population.ron");
const NAMES_RON: &str = include_str!("../../../data/names.ron");
const BRIEFING_RON: &str = include_str!("../../../data/briefing.ron");

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
            ROOMS_RON,
            POPULATION_RON,
            NAMES_RON,
            BRIEFING_RON,
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
        rooms: &str,
        population: &str,
        names: &str,
        briefing: &str,
    ) -> Result<GameData, DataError> {
        let data = GameData {
            tuning: parse("tuning.ron", tuning)?,
            venues: parse("venues.ron", venues)?,
            disguises: parse("disguises.ron", disguises)?,
            items: parse("items.ron", items)?,
            equipment: parse("equipment.ron", equipment)?,
            rooms: parse("rooms.ron", rooms)?,
            population: parse("population.ron", population)?,
            names: parse("names.ron", names)?,
            briefing: parse("briefing.ron", briefing)?,
        };
        data.validate()?;
        Ok(data)
    }

    pub fn venue(&self, id: &str) -> Option<&VenueSpec> {
        self.venues.iter().find(|v| v.id == id)
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
            if venue.room_templates.is_empty() {
                errors.push(format!("venue '{}' places no rooms", venue.id));
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
        let result: Result<NamePools, _> =
            ron::from_str("(first: [\"A\"], last: [\"B\"], surprise: 1)");
        assert!(result.is_err());
    }
}
