//! Mission configuration and contract constraints.
//!
//! A contract carries exactly one mandatory constraint. Constraints are
//! incorporated before level generation (they may bias the generator,
//! e.g. a private-kill contract guarantees the target's schedule visits
//! personal space) and the planner must certify a constraint-compliant
//! route or the generation attempt is retried. In the mission they are
//! tracked as breach conditions: breaking one never ends the mission,
//! but the contract resolves unclean — no payout.
//!
//! The deterministic guarantee is config-for-config: the same
//! [`MissionConfig`] always generates the identical world and simulation.

use serde::{Deserialize, Serialize};

use crate::data::{GameData, RoomTemplateId, VenueId, Zone};

/// The mandatory condition a contract imposes. Exactly one per contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Constraint {
    /// The pistol may not be fired.
    NoFirearms,
    /// Nobody but the target may die by your hand (guards excepted:
    /// they signed up for this).
    NoCivilianCasualties,
    /// No body you made may be discovered before you extract.
    NoBodiesFound,
    /// The target must die in personal-tier space, away from the crowd.
    PrivateKill,
    /// Extraction must use a named exit.
    SpecificExit { room_template: RoomTemplateId },
}

impl Constraint {
    /// The briefing sentence. Naming rooms concretely — the private
    /// offices a kill must land in, the exact exit to leave by — so the
    /// player is never guessing which rooms the rule means.
    pub fn describe(&self, data: &GameData, venue: &str) -> String {
        match self {
            Constraint::NoFirearms => crate::tr!("contract.no_firearms.long").to_string(),
            Constraint::NoCivilianCasualties => {
                crate::tr!("contract.no_collateral.long").to_string()
            }
            Constraint::NoBodiesFound => crate::tr!("contract.no_bodies.long").to_string(),
            Constraint::PrivateKill => {
                let rooms = personal_room_names(data, venue);
                let where_ = join_or(&rooms, crate::tr!("contract.private_kill.fallback_room"));
                crate::trf!("contract.private_kill.long", rooms = where_)
            }
            Constraint::SpecificExit { room_template } => {
                let name = room_display_name(data, room_template);
                crate::trf!("contract.exit_via.long", room = name)
            }
        }
    }

    /// The HUD chip: short and space-limited.
    pub fn short(&self, data: &GameData, venue: &str) -> String {
        match self {
            Constraint::NoFirearms => crate::tr!("contract.no_firearms.short").to_string(),
            Constraint::NoCivilianCasualties => {
                crate::tr!("contract.no_collateral.short").to_string()
            }
            Constraint::NoBodiesFound => crate::tr!("contract.no_bodies.short").to_string(),
            Constraint::PrivateKill => {
                let rooms = personal_room_names(data, venue);
                match rooms.first() {
                    Some(first) if rooms.len() == 1 => {
                        crate::trf!("contract.private_kill.short_named", room = first)
                    }
                    Some(_) => crate::tr!("contract.private_kill.short_office").to_string(),
                    None => crate::tr!("contract.private_kill.short_private").to_string(),
                }
            }
            Constraint::SpecificExit { room_template } => {
                crate::trf!(
                    "contract.exit_via.short",
                    room = room_display_name(data, room_template)
                )
            }
        }
    }
}

/// Display names of a venue's personal-tier rooms (the management
/// offices), for the private-kill condition text.
fn personal_room_names(data: &GameData, venue: &str) -> Vec<String> {
    let Some(spec) = data.venue(venue) else {
        return Vec::new();
    };
    data.rooms
        .iter()
        .filter(|t| spec.room_templates.contains(&t.id) && t.zone == Zone::Personal)
        .map(|t| t.name.clone())
        .collect()
}

/// A room template's authored display name, falling back to its id.
fn room_display_name(data: &GameData, template: &str) -> String {
    data.room_template(template)
        .map(|t| t.name.clone())
        .unwrap_or_else(|| template.to_string())
}

/// Joins names with " or ", or a fallback phrase when the list is empty.
fn join_or(names: &[String], fallback: &str) -> String {
    if names.is_empty() {
        fallback.to_string()
    } else {
        names.join(" or ")
    }
}

/// The most equipment a mission loadout may carry.
pub const LOADOUT_SLOTS: usize = 3;

/// Everything the generator needs to build one mission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionConfig {
    pub seed: u64,
    pub venue: VenueId,
    /// The contract's mandatory constraint, if this mission runs under
    /// contract.
    #[serde(default)]
    pub constraint: Option<Constraint>,
    /// Equipment (item spec ids, at most [`LOADOUT_SLOTS`]) the player
    /// carries in. The campaign chooses from owned gear; the default
    /// matches the original briefing kit.
    #[serde(default)]
    pub loadout: Vec<crate::data::ItemSpecId>,
    /// Persistent district heat at contract time: raises the venue's
    /// guard count (capped) and, at two or more, its baseline wariness.
    #[serde(default)]
    pub heat: u8,
}

impl MissionConfig {
    pub fn new(seed: u64, venue: impl Into<VenueId>) -> Self {
        Self {
            seed,
            venue: venue.into(),
            constraint: None,
            loadout: vec!["garrote".to_string(), "silenced-pistol".to_string()],
            heat: 0,
        }
    }

    pub fn with_heat(mut self, heat: u8) -> Self {
        self.heat = heat;
        self
    }

    pub fn with_constraint(mut self, constraint: Constraint) -> Self {
        self.constraint = Some(constraint);
        self
    }

    pub fn with_loadout(mut self, loadout: Vec<crate::data::ItemSpecId>) -> Self {
        self.loadout = loadout;
        self
    }
}
