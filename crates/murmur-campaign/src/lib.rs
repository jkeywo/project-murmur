//! The campaign layer: everything that outlives a single mission.
//!
//! A campaign owns cash, the equipment stash, persistent police heat by
//! district, and the contract history. It sits strictly above the
//! deterministic mission core: missions receive a [`MissionConfig`]
//! (murmur-core) and report back an outcome; the campaign never reaches
//! into a running world.
//!
//! Campaign evolution is deterministic from the campaign seed plus the
//! player's recorded choices, and the whole state serialises to a small
//! versioned JSON document — the single-slot autosave. Storage itself is
//! behind [`CampaignStore`], implemented by each delivery binary (a file
//! natively, localStorage on the web).

use serde::{Deserialize, Serialize};

/// Bump when [`CampaignState`] changes incompatibly; older saves are
/// rejected as stale rather than misread.
pub const SAVE_VERSION: u32 = 1;

/// Where the single campaign save lives. Implemented per delivery target.
pub trait CampaignStore {
    /// Returns the stored save document, if one exists.
    fn load(&self) -> Option<String>;
    /// Persists the save document, replacing any previous one.
    fn save(&mut self, document: &str);
    /// Deletes the stored campaign (abandon / campaign over).
    fn clear(&mut self);
}

/// One resolved contract in the campaign history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractRecord {
    pub district: String,
    pub venue: String,
    pub target_name: String,
    /// The mission outcome, campaign-side vocabulary.
    pub result: ContractResult,
    pub payout: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContractResult {
    Completed,
    /// Target eliminated but the mandatory constraint was violated.
    CompletedUnclean,
    Abandoned,
    Arrested,
    Killed,
}

/// The whole persistent campaign. Serialises to the versioned JSON save.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CampaignState {
    pub version: u32,
    pub seed: u64,
    pub cash: i64,
    /// Equipment spec ids the player owns (the stash).
    pub owned_equipment: Vec<String>,
    /// Persistent police heat per district.
    pub district_heat: Vec<(String, u16)>,
    pub history: Vec<ContractRecord>,
    /// Index of the next contract offer batch (advances on accept or
    /// refresh so offers never repeat).
    pub offer_index: u64,
}

impl CampaignState {
    pub fn new(seed: u64) -> Self {
        Self {
            version: SAVE_VERSION,
            seed,
            cash: 0,
            owned_equipment: Vec::new(),
            district_heat: Vec::new(),
            history: Vec::new(),
            offer_index: 0,
        }
    }

    /// Serialises the campaign to its save document.
    pub fn to_save(&self) -> String {
        serde_json::to_string(self).expect("campaign state serialises")
    }

    /// Restores a campaign from a save document. Returns `None` for
    /// unparseable or version-mismatched documents (the caller starts a
    /// fresh campaign rather than misreading an old one).
    pub fn from_save(document: &str) -> Option<Self> {
        let state: CampaignState = serde_json::from_str(document).ok()?;
        (state.version == SAVE_VERSION).then_some(state)
    }
}

/// An in-memory store for tests and defaults.
#[derive(Default)]
pub struct MemoryStore {
    document: Option<String>,
}

impl CampaignStore for MemoryStore {
    fn load(&self) -> Option<String> {
        self.document.clone()
    }

    fn save(&mut self, document: &str) {
        self.document = Some(document.to_string());
    }

    fn clear(&mut self) {
        self.document = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_round_trips() {
        let mut state = CampaignState::new(99);
        state.cash = 450;
        state.owned_equipment.push("garrote".to_string());
        state.district_heat.push(("docklands".to_string(), 3));
        let restored = CampaignState::from_save(&state.to_save()).unwrap();
        assert_eq!(restored, state);
    }

    #[test]
    fn version_mismatch_is_rejected() {
        let mut state = CampaignState::new(1);
        state.version = SAVE_VERSION + 1;
        assert!(CampaignState::from_save(&state.to_save()).is_none());
    }

    #[test]
    fn garbage_documents_are_rejected() {
        assert!(CampaignState::from_save("not json").is_none());
        assert!(CampaignState::from_save("{}").is_none());
    }

    #[test]
    fn memory_store_round_trips() {
        let mut store = MemoryStore::default();
        assert!(store.load().is_none());
        store.save("doc");
        assert_eq!(store.load().as_deref(), Some("doc"));
        store.clear();
        assert!(store.load().is_none());
    }
}
