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

use crate::data::{GameData, Role, RoomTemplateId, VenueId, Zone};
use crate::generator::layout::Layout;
use crate::generator::populate::Population;
use crate::geom::Pos;
use crate::planner::RouteFilters;
use crate::world::World;

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

    /// The planner filters that certify this constraint at generation
    /// time, or why this venue cannot host it. The planner composes the
    /// result over its standard physical-stealth proof; everything the
    /// constraint itself demands lives here.
    pub fn certify_filters(
        &self,
        data: &GameData,
        layout: &Layout,
        population: &Population,
        start: Pos,
    ) -> Result<RouteFilters, String> {
        match self {
            Constraint::NoFirearms => Ok(RouteFilters {
                forbid_items: vec!["silenced-pistol".to_string()],
                ..Default::default()
            }),
            Constraint::NoCivilianCasualties => Ok(RouteFilters::default()),
            Constraint::NoBodiesFound => {
                // Discretion needs somewhere to stow the body: at least one
                // container must be reachable under the trespass model.
                let outcome = crate::generator::proof::capability_closure(
                    data, layout, population, start, true, false,
                );
                let stowable = layout.furniture.iter().any(|f| {
                    f.kind == crate::world::FurnitureKind::Container
                        && crate::geom::Dir4::ALL
                            .into_iter()
                            .any(|d| outcome.seen.contains(f.pos.step(d)))
                });
                if !stowable {
                    return Err("no reachable container to hide a body in".to_string());
                }
                Ok(RouteFilters::default())
            }
            Constraint::PrivateKill => {
                let personal: Vec<String> = layout
                    .rooms
                    .iter()
                    .filter(|r| r.zone == Zone::Personal)
                    .map(|r| r.name.clone())
                    .collect();
                if personal.is_empty() {
                    return Err("venue has no personal-tier rooms".to_string());
                }
                Ok(RouteFilters {
                    kill_rooms: Some(personal),
                    ..Default::default()
                })
            }
            Constraint::SpecificExit { room_template } => {
                let exits: Vec<Pos> = layout
                    .extraction_tiles
                    .iter()
                    .copied()
                    .filter(|tile| {
                        layout
                            .rooms
                            .iter()
                            .find(|r| r.floor == tile.floor && r.bounds.contains(tile.x, tile.y))
                            .is_some_and(|r| &r.template == room_template)
                    })
                    .collect();
                if exits.is_empty() {
                    return Err(format!("venue has no '{room_template}' exit"));
                }
                Ok(RouteFilters {
                    allowed_exits: Some(exits),
                    ..Default::default()
                })
            }
        }
    }

    /// Breach check when the player fires a shot.
    pub fn on_shot(&self) -> Option<String> {
        match self {
            Constraint::NoFirearms => Some(crate::tr!("contract.no_firearms.breach").to_string()),
            _ => None,
        }
    }

    /// Breach check when the player kills somebody at `pos`.
    pub fn on_kill(
        &self,
        world: &World,
        pos: Pos,
        victim_is_target: bool,
        victim_role: Option<Role>,
    ) -> Option<String> {
        match self {
            Constraint::NoCivilianCasualties
                if !victim_is_target && victim_role != Some(Role::Guard) =>
            {
                Some(crate::tr!("contract.no_collateral.breach").to_string())
            }
            Constraint::PrivateKill if victim_is_target => {
                let private = world.room_at(pos).is_some_and(|r| r.zone == Zone::Personal);
                if private {
                    return None;
                }
                let where_ = world
                    .room_at(pos)
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| crate::tr!("contract.private_kill.open_floor").to_string());
                let offices: Vec<String> = world
                    .rooms
                    .iter()
                    .filter(|r| r.zone == Zone::Personal)
                    .map(|r| r.name.clone())
                    .collect();
                let needed = join_or(&offices, crate::tr!("contract.private_kill.fallback_room"));
                Some(crate::loc::fmt(
                    "contract.private_kill.breach",
                    &[("where", &where_), ("needed", &needed)],
                ))
            }
            _ => None,
        }
    }

    /// Breach check when the player extracts standing at `pos`.
    pub fn on_exit(&self, world: &World, pos: Pos) -> Option<String> {
        match self {
            Constraint::SpecificExit { room_template } => {
                let via_required = world
                    .room_at(pos)
                    .is_some_and(|r| &r.template == room_template);
                if via_required {
                    return None;
                }
                let exit_name = world
                    .rooms
                    .iter()
                    .find(|r| &r.template == room_template)
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| room_template.clone());
                Some(crate::trf!("contract.exit_via.breach", room = exit_name))
            }
            _ => None,
        }
    }

    /// Breach check when somebody first discovers a body of the player's
    /// making.
    pub fn on_body_found(&self) -> Option<&'static str> {
        match self {
            Constraint::NoBodiesFound => Some(crate::tr!("perception.body_found")),
            _ => None,
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
