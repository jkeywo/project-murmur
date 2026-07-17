//! The abstract capability-graph route planner.
//!
//! Extends the generated reachability proof into a deterministic search
//! over planner-visible facts: rooms, zones, disguises, keys, items,
//! the target's schedule, contract constraints, and exits. At generation
//! time it must certify that the mission is completable in three
//! different postures — social stealth (legitimate everywhere), physical
//! stealth (trespass, silent kill), and violence (any kill) — and, once
//! contracts land, that at least one route satisfies the contract's
//! mandatory constraint. A venue that fails any proof fails the
//! generation attempt and is retried.
//!
//! The search is a monotone capability closure (capabilities are only
//! ever gained), so a fixpoint over (reachable tiles, disguises, keys,
//! invitation) decides reachability exactly; schedule windows appear as
//! abstract availability (any room the target's routine touches is a
//! potential kill site), not a temporal model.

use serde::{Deserialize, Serialize};

use crate::data::GameData;
use crate::generator::layout::Layout;
use crate::generator::populate::Population;
use crate::generator::proof::{capability_closure, schedule_positions};
use crate::geom::Pos;
use crate::world::ItemLocation;

/// The three route postures every mission must support.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteClass {
    /// Legitimate access everywhere (disguises, invitations, keys) and a
    /// silent kill.
    Social,
    /// Trespass is acceptable, locks are not; the kill stays silent.
    Physical,
    /// Any reachable kill with any weapon.
    Violence,
}

impl RouteClass {
    pub fn name(self) -> &'static str {
        match self {
            RouteClass::Social => "social stealth",
            RouteClass::Physical => "physical stealth",
            RouteClass::Violence => "violence",
        }
    }
}

/// Extra conditions a route must satisfy (contract constraints compose
/// through these filters).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteFilters {
    /// Item spec ids the route may not rely on (a no-firearms contract
    /// forbids the pistol).
    pub forbid_items: Vec<String>,
    /// If set, the kill must happen in one of these rooms (by name).
    pub kill_rooms: Option<Vec<String>>,
    /// If set, extraction must use one of these tiles.
    pub allowed_exits: Option<Vec<Pos>>,
}

/// A certified route: class, kill site, exit, and the capability steps
/// that make it possible, in dependency order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteProof {
    pub class: RouteClass,
    pub kill_room: String,
    pub exit_room: String,
    pub steps: Vec<String>,
}

/// Every certified route, kept on the world for briefings and audits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteReport {
    pub proofs: Vec<RouteProof>,
    /// The route certifying the contract's mandatory constraint, when
    /// the mission runs under contract.
    #[serde(default)]
    pub constraint_proof: Option<RouteProof>,
}

impl RouteReport {
    pub fn class(&self, class: RouteClass) -> Option<&RouteProof> {
        self.proofs.iter().find(|p| p.class == class)
    }
}

/// Weapons the player starts with, by spec id, minus anything filtered.
fn player_weapons(data: &GameData, population: &Population, filters: &RouteFilters) -> Vec<String> {
    let mut weapons: Vec<String> = population
        .items
        .iter()
        .filter(|item| item.location == ItemLocation::CarriedBy(population.player))
        .filter(|item| !filters.forbid_items.contains(&item.spec))
        .filter(|item| data.item(&item.spec).is_some_and(|s| s.weapon))
        .map(|item| item.spec.clone())
        .collect();
    // The garrote is an innate capability until the equipment slice
    // turns it into loadout gear.
    if !weapons.iter().any(|w| w == "garrote")
        && !filters.forbid_items.iter().any(|i| i == "garrote")
    {
        weapons.push("garrote".to_string());
    }
    weapons
}

/// Whether a weapon kills quietly (the garrote and the silenced pistol
/// do; future loud weapons will not).
fn is_silent(spec: &str) -> bool {
    spec != "pistol-loud"
}

/// Proves one route class against the generated venue.
pub fn prove_route(
    data: &GameData,
    layout: &Layout,
    population: &Population,
    start: Pos,
    class: RouteClass,
    filters: &RouteFilters,
) -> Result<RouteProof, String> {
    let zone_free = !matches!(class, RouteClass::Social);
    let outcome = capability_closure(data, layout, population, start, zone_free);

    // A kill capability appropriate to the class.
    let weapons = player_weapons(data, population, filters);
    let usable: Vec<&String> = weapons
        .iter()
        .filter(|w| match class {
            RouteClass::Violence => true,
            _ => is_silent(w),
        })
        .collect();
    if usable.is_empty() {
        return Err(format!("no usable weapon for a {} route", class.name()));
    }

    // A reachable room the target's schedule visits (abstract schedule
    // window), honouring any kill-room restriction.
    let target = &population.actors[population.target.0 as usize];
    let kill_room = schedule_positions(target)
        .iter()
        .filter(|pos| outcome.seen.contains(**pos))
        .find_map(|pos| {
            let room = layout
                .rooms
                .iter()
                .find(|r| r.floor == pos.floor && r.bounds.contains(pos.x, pos.y))?;
            match &filters.kill_rooms {
                Some(allowed) if !allowed.contains(&room.name) => None,
                _ => Some(room.name.clone()),
            }
        })
        .ok_or_else(|| {
            format!(
                "no reachable kill site on the target's schedule for a {} route",
                class.name()
            )
        })?;

    // Extraction after the kill: capabilities only grow, so the same
    // closure decides it.
    let exit = layout
        .extraction_tiles
        .iter()
        .filter(|tile| match &filters.allowed_exits {
            Some(allowed) => allowed.contains(tile),
            None => true,
        })
        .find(|tile| outcome.seen.contains(**tile))
        .ok_or_else(|| format!("no reachable extraction for a {} route", class.name()))?;
    let exit_room = layout
        .rooms
        .iter()
        .find(|r| r.floor == exit.floor && r.bounds.contains(exit.x, exit.y))
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "the street".to_string());

    let mut steps = outcome.events;
    steps.push(format!(
        "kill the target in {} with the {}",
        kill_room, usable[0]
    ));
    steps.push(format!("extract via {exit_room}"));

    Ok(RouteProof {
        class,
        kill_room,
        exit_room,
        steps,
    })
}

/// Certifies a route that satisfies the contract's mandatory constraint,
/// composed through the standard filters over the physical-stealth model
/// (the least demanding posture that stays silent).
pub fn prove_constraint(
    data: &GameData,
    layout: &Layout,
    population: &Population,
    start: Pos,
    constraint: &crate::contract::Constraint,
) -> Result<RouteProof, String> {
    use crate::contract::Constraint;
    let filters = match constraint {
        Constraint::NoFirearms => RouteFilters {
            forbid_items: vec!["silenced-pistol".to_string()],
            ..Default::default()
        },
        Constraint::NoCivilianCasualties => RouteFilters::default(),
        Constraint::NoBodiesFound => {
            // Discretion needs somewhere to stow the body: at least one
            // container must be reachable under the trespass model.
            let outcome = capability_closure(data, layout, population, start, true);
            let stowable = layout.furniture.iter().any(|f| {
                f.kind == crate::world::FurnitureKind::Container
                    && crate::geom::Dir4::ALL
                        .into_iter()
                        .any(|d| outcome.seen.contains(f.pos.step(d)))
            });
            if !stowable {
                return Err("no reachable container to hide a body in".to_string());
            }
            RouteFilters::default()
        }
        Constraint::PrivateKill => {
            let personal: Vec<String> = layout
                .rooms
                .iter()
                .filter(|r| r.zone == crate::data::Zone::Personal)
                .map(|r| r.name.clone())
                .collect();
            if personal.is_empty() {
                return Err("venue has no personal-tier rooms".to_string());
            }
            RouteFilters {
                kill_rooms: Some(personal),
                ..Default::default()
            }
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
            RouteFilters {
                allowed_exits: Some(exits),
                ..Default::default()
            }
        }
    };
    prove_route(
        data,
        layout,
        population,
        start,
        RouteClass::Physical,
        &filters,
    )
}

/// Certifies the three base routes every mission must support.
pub fn prove_base_routes(
    data: &GameData,
    layout: &Layout,
    population: &Population,
    start: Pos,
) -> Result<RouteReport, String> {
    let mut proofs = Vec::new();
    for class in [
        RouteClass::Social,
        RouteClass::Physical,
        RouteClass::Violence,
    ] {
        proofs.push(prove_route(
            data,
            layout,
            population,
            start,
            class,
            &RouteFilters::default(),
        )?);
    }
    Ok(RouteReport {
        proofs,
        constraint_proof: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::{layout, populate, proof};
    use crate::rng::Pcg32;

    /// Mirrors the generator's front half: a proven venue plus its
    /// population, before world assembly.
    fn staged(seed: u64) -> (GameData, Layout, Population, Pos) {
        let data = GameData::embedded().unwrap();
        let venue = data.venue("nightclub").unwrap().clone();
        for attempt in 0..24 {
            let mut rng = Pcg32::new(seed, 0x4d75726d75720000 + attempt);
            let Ok(mut layout) = layout::build_layout(&data, &venue, &mut rng) else {
                continue;
            };
            let Ok(population) = populate::populate(&data, &layout, None, &mut rng) else {
                continue;
            };
            let start = population.actors[population.player.0 as usize].pos;
            if proof::prove_progression(&data, &mut layout, &population, start, &mut rng).is_err() {
                continue;
            }
            return (data, layout, population, start);
        }
        panic!("no attempt produced a proven venue for seed {seed}");
    }

    #[test]
    fn all_three_route_classes_certify_on_generated_venues() {
        for seed in 0..12u64 {
            let (data, layout, population, start) = staged(seed);
            let report = prove_base_routes(&data, &layout, &population, start)
                .unwrap_or_else(|e| panic!("seed {seed}: {e}"));
            for class in [
                RouteClass::Social,
                RouteClass::Physical,
                RouteClass::Violence,
            ] {
                let proof = report.class(class).expect("class proven");
                assert!(!proof.kill_room.is_empty());
                assert!(!proof.exit_room.is_empty());
                assert!(
                    proof.steps.iter().any(|s| s.starts_with("kill the target")),
                    "route must state its kill"
                );
            }
        }
    }

    #[test]
    fn forbidding_every_weapon_fails_the_route() {
        let (data, layout, population, start) = staged(3);
        let filters = RouteFilters {
            forbid_items: vec!["silenced-pistol".to_string(), "garrote".to_string()],
            ..Default::default()
        };
        for class in [
            RouteClass::Social,
            RouteClass::Physical,
            RouteClass::Violence,
        ] {
            assert!(
                prove_route(&data, &layout, &population, start, class, &filters).is_err(),
                "{} route cannot certify without any weapon",
                class.name()
            );
        }
    }

    #[test]
    fn forbidding_the_pistol_still_proves_through_the_garrote() {
        let (data, layout, population, start) = staged(3);
        let filters = RouteFilters {
            forbid_items: vec!["silenced-pistol".to_string()],
            ..Default::default()
        };
        let proof = prove_route(
            &data,
            &layout,
            &population,
            start,
            RouteClass::Violence,
            &filters,
        )
        .expect("garrote-only violence route");
        assert!(
            proof.steps.iter().any(|s| s.contains("garrote")),
            "the certified kill must use the garrote: {:?}",
            proof.steps
        );
    }

    #[test]
    fn kill_room_and_exit_filters_bind_the_proof() {
        let (data, layout, population, start) = staged(5);
        let base = prove_route(
            &data,
            &layout,
            &population,
            start,
            RouteClass::Physical,
            &RouteFilters::default(),
        )
        .unwrap();

        // Restricting the kill to the certified room still proves, and
        // the proof names that room.
        let filters = RouteFilters {
            kill_rooms: Some(vec![base.kill_room.clone()]),
            ..Default::default()
        };
        let bound = prove_route(
            &data,
            &layout,
            &population,
            start,
            RouteClass::Physical,
            &filters,
        )
        .unwrap();
        assert_eq!(bound.kill_room, base.kill_room);

        // Restricting extraction to a nonexistent tile fails.
        let filters = RouteFilters {
            allowed_exits: Some(vec![Pos::new(0, -1, -1)]),
            ..Default::default()
        };
        assert!(
            prove_route(
                &data,
                &layout,
                &population,
                start,
                RouteClass::Physical,
                &filters
            )
            .is_err()
        );
    }
}
