//! Mission configuration: what the campaign layer hands the generator.
//!
//! Milestone 2 grows this into full constrained contracts (venue, target
//! hook, payout, heat context, exactly one mandatory constraint). The
//! deterministic guarantee is config-for-config: the same [`MissionConfig`]
//! always generates the identical world and simulation.

use serde::{Deserialize, Serialize};

use crate::data::VenueId;

/// Everything the generator needs to build one mission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionConfig {
    pub seed: u64,
    pub venue: VenueId,
}

impl MissionConfig {
    pub fn new(seed: u64, venue: impl Into<VenueId>) -> Self {
        Self {
            seed,
            venue: venue.into(),
        }
    }
}
