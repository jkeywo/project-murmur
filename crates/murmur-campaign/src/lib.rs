//! The campaign layer: everything that outlives a single mission.
//!
//! A campaign owns cash, the equipment stash, persistent police heat by
//! district, and the contract history. It sits strictly above the
//! deterministic mission core: contracts hand the generator a
//! [`MissionConfig`] and the finished mission reports back a
//! [`MissionResolution`]; the campaign never reaches into a running
//! world.
//!
//! Campaign evolution is deterministic from the campaign seed plus the
//! player's recorded choices, and the whole state serialises to a small
//! versioned JSON document — the single-slot autosave. Storage itself is
//! behind [`CampaignStore`], implemented by each delivery binary (a file
//! natively, localStorage on the web).
//!
//! The owner's standing rules: death ends the campaign at a final tally;
//! arrest fails the contract, levies the fine, and confiscates the
//! carried loadout; equipment is bought once and owned until lost.

use serde::{Deserialize, Serialize};

use murmur_core::contract::{Constraint, MissionConfig};
use murmur_core::data::{GameData, ItemSpecId, VenueId};
use murmur_core::rng::Pcg32;
use murmur_core::world::MissionOutcome;

/// Bump when [`CampaignState`] changes incompatibly; older saves are
/// rejected as stale rather than misread.
pub const SAVE_VERSION: u32 = 1;

/// Where the single campaign save lives. Implemented per delivery target.
pub trait CampaignStore: Send + Sync {
    /// Returns the stored save document, if one exists.
    fn load(&self) -> Option<String>;
    /// Persists the save document, replacing any previous one.
    fn save(&mut self, document: &str);
    /// Deletes the stored campaign (abandon / campaign over).
    fn clear(&mut self);
}

/// One contract on the board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractOffer {
    pub district: String,
    pub venue: VenueId,
    /// The narrative hook shown on the board ("the target has been
    /// skimming protection payments").
    pub hook: String,
    pub payout: i64,
    /// District heat at offer time; rides into generation.
    pub heat: u8,
    pub constraint: Constraint,
    pub seed: u64,
}

/// One resolved contract in the campaign history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractRecord {
    pub district: String,
    pub venue: String,
    pub result: ContractResult,
    pub payout: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContractResult {
    Completed,
    /// Target eliminated but the mandatory constraint was violated.
    CompletedUnclean,
    Abandoned,
    /// The target fled the venue alive.
    TargetEscaped,
    Arrested,
    Killed,
}

impl ContractResult {
    pub fn describe(self) -> &'static str {
        match self {
            ContractResult::Completed => "completed cleanly",
            ContractResult::CompletedUnclean => "completed, contract breached",
            ContractResult::Abandoned => "abandoned",
            ContractResult::TargetEscaped => "the target escaped",
            ContractResult::Arrested => "ended in arrest",
            ContractResult::Killed => "ended in death",
        }
    }
}

/// What the finished mission reports back to the campaign.
#[derive(Clone, Debug)]
pub struct MissionResolution {
    /// `None` means the player abandoned the run.
    pub outcome: Option<MissionOutcome>,
    /// The specific reason the contract's condition was broken, if it
    /// was; `None` when the condition held (or there was none).
    pub breach_reason: Option<String>,
    pub mission_heat: u16,
    /// What the player carried in (lost on arrest).
    pub loadout: Vec<ItemSpecId>,
}

/// What resolving a contract did, for the debrief screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolutionSummary {
    pub result: ContractResult,
    pub payout: i64,
    pub fine: i64,
    pub confiscated: Vec<ItemSpecId>,
    pub district_heat_change: i8,
    /// Why the contract's condition was broken, for the debrief.
    pub breach_reason: Option<String>,
}

/// The whole persistent campaign. Serialises to the versioned JSON save.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CampaignState {
    pub version: u32,
    pub seed: u64,
    pub cash: i64,
    /// Equipment spec ids the player owns (the stash).
    pub owned_equipment: Vec<ItemSpecId>,
    /// Persistent police heat per district, in district order.
    pub district_heat: Vec<(String, u8)>,
    pub history: Vec<ContractRecord>,
    /// Advances on accept or refresh so offers never repeat.
    pub offer_index: u64,
    /// Set when the operative died: the campaign is over.
    pub over: bool,
}

/// Stream selector for offer derivation.
const OFFER_STREAM: u64 = 0x4f66666572; // "Offer"

impl CampaignState {
    pub fn new(seed: u64, data: &GameData) -> Self {
        Self {
            version: SAVE_VERSION,
            seed,
            cash: data.campaign.starting_cash,
            owned_equipment: data.campaign.starting_equipment.clone(),
            district_heat: data
                .campaign
                .districts
                .iter()
                .map(|d| (d.clone(), 0))
                .collect(),
            history: Vec::new(),
            offer_index: 0,
            over: false,
        }
    }

    pub fn heat_in(&self, district: &str) -> u8 {
        self.district_heat
            .iter()
            .find(|(d, _)| d == district)
            .map(|(_, h)| *h)
            .unwrap_or(0)
    }

    /// The current contract board: deterministic from the campaign seed,
    /// the offer index, and current district heat.
    pub fn offers(&self, data: &GameData) -> Vec<ContractOffer> {
        let mut rng = Pcg32::new(self.seed ^ self.offer_index, OFFER_STREAM);
        let mut offers = Vec::new();
        for _slot in 0..data.campaign.offers_per_batch {
            let district = rng.pick(&data.campaign.districts).clone();
            let venue = rng.pick(&data.venues).id.clone();
            let heat = self.heat_in(&district);
            let constraint = pick_constraint(data, &venue, &mut rng);
            let payout = data.campaign.payout_base
                + data.campaign.payout_per_heat * i64::from(heat)
                + data.campaign.payout_constraint_bonus;
            let hook = rng.pick(&data.briefing.reasons).clone();
            let seed = (u64::from(rng.next_u32()) << 32) | u64::from(rng.next_u32());
            offers.push(ContractOffer {
                district,
                venue,
                hook,
                payout,
                heat,
                constraint,
                seed,
            });
        }
        offers
    }

    /// Declines the current board; the next one differs.
    pub fn refresh_offers(&mut self) {
        self.offer_index += 1;
    }

    /// Accepts an offer with a chosen loadout, producing the mission
    /// config. The offer board advances.
    pub fn accept(
        &mut self,
        offer: &ContractOffer,
        loadout: Vec<ItemSpecId>,
    ) -> Result<MissionConfig, String> {
        if self.over {
            return Err("the campaign is over".to_string());
        }
        if loadout.len() > murmur_core::contract::LOADOUT_SLOTS {
            return Err("a loadout carries at most three items".to_string());
        }
        for item in &loadout {
            if !self.owned_equipment.contains(item) {
                return Err(format!("you do not own '{item}'"));
            }
        }
        self.offer_index += 1;
        Ok(MissionConfig::new(offer.seed, offer.venue.clone())
            .with_constraint(offer.constraint.clone())
            .with_loadout(loadout)
            .with_heat(offer.heat))
    }

    /// Buys a catalogue item into the stash.
    pub fn buy(&mut self, data: &GameData, item: &str) -> Result<(), String> {
        let entry = data
            .equipment
            .iter()
            .find(|e| e.item == item)
            .ok_or_else(|| format!("'{item}' is not in the catalogue"))?;
        if self.owned_equipment.iter().any(|i| i == item) {
            return Err("already owned".to_string());
        }
        if self.cash < entry.price {
            return Err("not enough cash".to_string());
        }
        self.cash -= entry.price;
        self.owned_equipment.push(item.to_string());
        Ok(())
    }

    /// Applies a finished mission to the campaign per the standing
    /// rules, returning the debrief summary.
    pub fn resolve(
        &mut self,
        data: &GameData,
        offer: &ContractOffer,
        resolution: &MissionResolution,
    ) -> ResolutionSummary {
        let result = match resolution.outcome {
            Some(MissionOutcome::Extracted) if resolution.breach_reason.is_none() => {
                ContractResult::Completed
            }
            Some(MissionOutcome::Extracted) => ContractResult::CompletedUnclean,
            Some(MissionOutcome::TargetEscaped) => ContractResult::TargetEscaped,
            Some(MissionOutcome::Arrested) => ContractResult::Arrested,
            Some(MissionOutcome::PlayerKilled) => ContractResult::Killed,
            None => ContractResult::Abandoned,
        };

        let payout = if result == ContractResult::Completed {
            offer.payout
        } else {
            0
        };
        self.cash += payout;

        // Arrest: the fine, and everything carried is confiscated.
        let mut fine = 0;
        let mut confiscated = Vec::new();
        if result == ContractResult::Arrested {
            fine = data.campaign.arrest_fine.min(self.cash.max(0));
            self.cash -= fine;
            for item in &resolution.loadout {
                if let Some(index) = self.owned_equipment.iter().position(|i| i == item) {
                    self.owned_equipment.remove(index);
                    confiscated.push(item.clone());
                }
            }
        }

        if result == ContractResult::Killed {
            self.over = true;
        }

        // Heat: a hot mission raises the district; every other district
        // decays as contracts pass elsewhere.
        let hot = resolution.mission_heat >= data.campaign.hot_mission_threshold;
        let mut district_heat_change: i8 = 0;
        for (district, heat) in &mut self.district_heat {
            if district == &offer.district {
                if hot {
                    let before = *heat;
                    *heat = (*heat + 2).min(data.campaign.district_heat_max);
                    district_heat_change = (*heat - before) as i8;
                }
            } else {
                *heat = heat.saturating_sub(1);
            }
        }

        self.history.push(ContractRecord {
            district: offer.district.clone(),
            venue: offer.venue.clone(),
            result,
            payout,
        });

        ResolutionSummary {
            result,
            payout,
            fine,
            confiscated,
            district_heat_change,
            breach_reason: resolution.breach_reason.clone(),
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

/// Picks a constraint an offer's venue can actually host. SpecificExit
/// names one of the venue's external-exit room templates.
fn pick_constraint(data: &GameData, venue: &str, rng: &mut Pcg32) -> Constraint {
    let choice = rng.below(5);
    match choice {
        0 => Constraint::NoFirearms,
        1 => Constraint::NoCivilianCasualties,
        2 => Constraint::NoBodiesFound,
        3 => Constraint::PrivateKill,
        _ => {
            let exits: Vec<&String> = data
                .venue(venue)
                .map(|v| {
                    v.room_templates
                        .iter()
                        .filter(|t| {
                            data.room_template(t)
                                .is_some_and(|spec| spec.external_exit && !spec.circulation)
                        })
                        .collect()
                })
                .unwrap_or_default();
            if exits.is_empty() {
                Constraint::NoFirearms
            } else {
                Constraint::SpecificExit {
                    room_template: (*rng.pick(&exits)).clone(),
                }
            }
        }
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

    fn data() -> GameData {
        GameData::embedded().unwrap()
    }

    fn offer(state: &CampaignState, data: &GameData) -> ContractOffer {
        state.offers(data).into_iter().next().unwrap()
    }

    #[test]
    fn a_new_campaign_starts_per_the_authored_data() {
        let data = data();
        let state = CampaignState::new(7, &data);
        assert_eq!(state.cash, data.campaign.starting_cash);
        assert_eq!(state.owned_equipment, data.campaign.starting_equipment);
        assert_eq!(state.district_heat.len(), data.campaign.districts.len());
        assert!(!state.over);
    }

    #[test]
    fn offers_are_deterministic_and_refresh_changes_them() {
        let data = data();
        let mut state = CampaignState::new(7, &data);
        let a = state.offers(&data);
        let b = state.offers(&data);
        assert_eq!(a, b, "same state, same board");
        assert_eq!(a.len(), usize::from(data.campaign.offers_per_batch));
        state.refresh_offers();
        let c = state.offers(&data);
        assert_ne!(a, c, "declining rolls a fresh board");
    }

    #[test]
    fn buying_moves_cash_into_the_stash_once() {
        let data = data();
        let mut state = CampaignState::new(7, &data);
        state.cash = 1000;
        state.buy(&data, "lockpicks").unwrap();
        assert!(state.owned_equipment.iter().any(|i| i == "lockpicks"));
        assert!(state.cash < 1000);
        assert!(state.buy(&data, "lockpicks").is_err(), "no duplicates");
        state.cash = 0;
        assert!(state.buy(&data, "silenced-pistol").is_err(), "needs cash");
    }

    #[test]
    fn accept_requires_owned_equipment_and_produces_the_config() {
        let data = data();
        let mut state = CampaignState::new(7, &data);
        let offer = offer(&state, &data);
        assert!(
            state
                .accept(&offer, vec!["silenced-pistol".to_string()])
                .is_err(),
            "cannot carry what you do not own"
        );
        let config = state.accept(&offer, vec!["garrote".to_string()]).unwrap();
        assert_eq!(config.seed, offer.seed);
        assert_eq!(config.venue, offer.venue);
        assert_eq!(config.constraint, Some(offer.constraint.clone()));
        assert_eq!(config.heat, offer.heat);
    }

    #[test]
    fn resolution_follows_the_standing_rules() {
        let data = data();
        let mut state = CampaignState::new(7, &data);
        let the_offer = offer(&state, &data);

        // Clean completion pays.
        let summary = state.resolve(
            &data,
            &the_offer,
            &MissionResolution {
                outcome: Some(MissionOutcome::Extracted),
                breach_reason: None,
                mission_heat: 0,
                loadout: vec!["garrote".to_string()],
            },
        );
        assert_eq!(summary.result, ContractResult::Completed);
        assert_eq!(summary.payout, the_offer.payout);

        // A breached contract completes unclean and pays nothing.
        let summary = state.resolve(
            &data,
            &the_offer,
            &MissionResolution {
                outcome: Some(MissionOutcome::Extracted),
                breach_reason: Some("the pistol was fired".to_string()),
                mission_heat: 0,
                loadout: vec![],
            },
        );
        assert_eq!(summary.result, ContractResult::CompletedUnclean);
        assert_eq!(summary.payout, 0);

        // An escaped target pays nothing, keeps the kit, and the
        // campaign continues.
        let cash = state.cash;
        let summary = state.resolve(
            &data,
            &the_offer,
            &MissionResolution {
                outcome: Some(MissionOutcome::TargetEscaped),
                breach_reason: None,
                mission_heat: 0,
                loadout: vec!["garrote".to_string()],
            },
        );
        assert_eq!(summary.result, ContractResult::TargetEscaped);
        assert_eq!(summary.payout, 0);
        assert_eq!(state.cash, cash, "no fine for a blown job");
        assert!(!state.over);
        assert!(state.owned_equipment.iter().any(|i| i == "garrote"));

        // Arrest fines and confiscates the carried kit.
        let cash_before = state.cash;
        let summary = state.resolve(
            &data,
            &the_offer,
            &MissionResolution {
                outcome: Some(MissionOutcome::Arrested),
                breach_reason: None,
                mission_heat: 0,
                loadout: vec!["garrote".to_string()],
            },
        );
        assert_eq!(summary.result, ContractResult::Arrested);
        assert_eq!(state.cash, cash_before - summary.fine);
        assert!(
            !state.owned_equipment.iter().any(|i| i == "garrote"),
            "carried gear is confiscated"
        );

        // Death ends the campaign.
        let summary = state.resolve(
            &data,
            &the_offer,
            &MissionResolution {
                outcome: Some(MissionOutcome::PlayerKilled),
                breach_reason: None,
                mission_heat: 0,
                loadout: vec![],
            },
        );
        assert_eq!(summary.result, ContractResult::Killed);
        assert!(state.over);
    }

    #[test]
    fn hot_missions_raise_district_heat_and_others_decay() {
        let data = data();
        let mut state = CampaignState::new(7, &data);
        let mut the_offer = offer(&state, &data);
        the_offer.district = data.campaign.districts[0].clone();
        // Preload heat elsewhere to watch it decay.
        state.district_heat[1].1 = 3;

        state.resolve(
            &data,
            &the_offer,
            &MissionResolution {
                outcome: Some(MissionOutcome::Extracted),
                breach_reason: None,
                mission_heat: data.campaign.hot_mission_threshold,
                loadout: vec![],
            },
        );
        assert_eq!(
            state.district_heat[0].1, 2,
            "hot contract heats the district"
        );
        assert_eq!(state.district_heat[1].1, 2, "other districts cool by one");
    }

    #[test]
    fn save_round_trips_with_full_state() {
        let data = data();
        let mut state = CampaignState::new(99, &data);
        state.cash = 450;
        state.refresh_offers();
        state.history.push(ContractRecord {
            district: "Docklands".to_string(),
            venue: "nightclub".to_string(),
            result: ContractResult::Completed,
            payout: 500,
        });
        let restored = CampaignState::from_save(&state.to_save()).unwrap();
        assert_eq!(restored, state);
    }

    #[test]
    fn version_mismatch_and_garbage_are_rejected() {
        let data = data();
        let mut state = CampaignState::new(1, &data);
        state.version = SAVE_VERSION + 1;
        assert!(CampaignState::from_save(&state.to_save()).is_none());
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
