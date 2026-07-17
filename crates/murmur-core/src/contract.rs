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

use crate::data::{RoomTemplateId, VenueId};

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
    /// The briefing sentence.
    pub fn describe(&self) -> String {
        match self {
            Constraint::NoFirearms => {
                "the client demands no gunfire: the pistol stays cold".to_string()
            }
            Constraint::NoCivilianCasualties => {
                "the client demands no collateral: only the target and their guards may die"
                    .to_string()
            }
            Constraint::NoBodiesFound => {
                "the client demands discretion: no body of your making may be found".to_string()
            }
            Constraint::PrivateKill => {
                "the client demands it happen in private, away from the crowd".to_string()
            }
            Constraint::SpecificExit { room_template } => {
                format!("the client dictates your extraction: leave via the {room_template}")
            }
        }
    }

    /// The HUD label.
    pub fn short(&self) -> String {
        match self {
            Constraint::NoFirearms => "no gunfire".to_string(),
            Constraint::NoCivilianCasualties => "no collateral".to_string(),
            Constraint::NoBodiesFound => "no bodies found".to_string(),
            Constraint::PrivateKill => "kill in private".to_string(),
            Constraint::SpecificExit { room_template } => format!("exit via {room_template}"),
        }
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
