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
//!
//! What a route must *complete* is not baked into the search: [`prove_route`]
//! computes the closure, then hands it to an [`Objective`]-dispatched
//! completion proof (see [`prove_completion`]) that answers "can this
//! objective be completed at a position the closure reaches, with the
//! capabilities it grants?" — after which extraction is proved from the same
//! closure. Today the only objective is assassination, whose completion is a
//! weapon kill at a vulnerable beat or a rigged accident on the schedule; the
//! closure itself is objective-agnostic and did not have to learn about kills.

use serde::{Deserialize, Serialize};

use crate::data::GameData;
use crate::generator::layout::Layout;
use crate::generator::populate::Population;
use crate::generator::proof::{
    ClosureOutcome, capability_closure, schedule_positions, vulnerable_positions,
};
use crate::geom::Pos;
use crate::world::{ItemLocation, Objective};

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
    /// Venue-potential mode: assume the whole equipment catalogue is
    /// available, not just the actual loadout. The three base proofs are
    /// statements about the venue; the loadout and constraint proofs are
    /// statements about this run.
    pub assume_catalogue: bool,
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
    /// A route completable with the actual loadout (the base proofs
    /// describe venue potential instead).
    #[serde(default)]
    pub loadout_proof: Option<RouteProof>,
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

/// Weapons available to the route, by spec id, minus anything filtered:
/// the actual loadout, or the whole purchasable catalogue in
/// venue-potential mode.
fn player_weapons(data: &GameData, population: &Population, filters: &RouteFilters) -> Vec<String> {
    let mut weapons: Vec<String> = population
        .items
        .iter()
        .filter(|item| item.location == ItemLocation::CarriedBy(population.player))
        .filter(|item| !filters.forbid_items.contains(&item.spec))
        .filter(|item| data.item(&item.spec).is_some_and(|s| s.weapon))
        .map(|item| item.spec.clone())
        .collect();
    if filters.assume_catalogue {
        for spec in &data.items {
            if spec.weapon
                && spec.purchasable
                && !weapons.contains(&spec.id)
                && !filters.forbid_items.contains(&spec.id)
            {
                weapons.push(spec.id.clone());
            }
        }
    }
    weapons
}

/// Whether a weapon kills quietly (the garrote and the silenced pistol
/// do; future loud weapons will not).
fn is_silent(spec: &str) -> bool {
    spec != "pistol-loud"
}

/// Where and how the mission's objective is completed on the venue, proven
/// against a capability closure. `site_room` is the room the completion
/// happens in — extraction is then proved from the closure, since
/// capabilities only grow — and `step` is the narration line for the route.
///
/// `kill_room` on the emitted [`RouteProof`] is this `site_room`; the field
/// keeps its assassination-era name until a later slice generalises the
/// report's shape, so that the Assassinate content stays byte-identical.
struct Completion {
    site_room: String,
    step: String,
}

/// Proves the mission's [`Objective`] is completable under `class`/`filters`
/// against an already-computed capability closure, and reports where it
/// completes. This is the seam: [`prove_route`] no longer knows the goal is a
/// kill — it asks the objective. Completion is always phrased as "reach a
/// qualifying position with the required capabilities", which is exactly what
/// the closure's `seen` set answers, so no objective needs the closure to
/// change.
fn prove_completion(
    data: &GameData,
    layout: &Layout,
    population: &Population,
    outcome: &ClosureOutcome,
    class: RouteClass,
    filters: &RouteFilters,
    objective: &Objective,
) -> Result<Completion, String> {
    match objective {
        Objective::Assassinate { target } => {
            prove_assassination(data, layout, population, outcome, class, filters, *target)
        }
    }
}

/// Completion proof for [`Objective::Assassinate`]: a weapon kill at a
/// vulnerable beat, or a rigged accident on the schedule when no usable
/// weapon is reachable. This is the exact logic the planner ran when a kill
/// was the only shape a route could take — now reachable through the
/// objective dispatch rather than being hardcoded into `prove_route`.
fn prove_assassination(
    data: &GameData,
    layout: &Layout,
    population: &Population,
    outcome: &ClosureOutcome,
    class: RouteClass,
    filters: &RouteFilters,
    target: crate::world::ActorId,
) -> Result<Completion, String> {
    // Kill capabilities appropriate to the class (an empty set may
    // still be rescued by a rigged accident below).
    let weapons = player_weapons(data, population, filters);
    let usable: Vec<String> = weapons
        .iter()
        .filter(|w| match class {
            RouteClass::Violence => true,
            _ => is_silent(w),
        })
        .cloned()
        .collect();

    // A reachable room the target's schedule visits (abstract schedule
    // window), honouring any kill-room restriction. Weapon kills need a
    // usable weapon; a reachable rigged accident above a schedule stop
    // kills without one.
    let target = &population.actors[target.0 as usize];
    let stops = schedule_positions(target);
    // A weapon needs the target alone; an accident does not, which is why
    // the two proofs read different position sets. This is the whole
    // mechanical asymmetry of the milestone, expressed in two lines.
    let vulnerable = vulnerable_positions(target);
    let weapon_kill = if usable.is_empty() {
        None
    } else {
        vulnerable
            .iter()
            .filter(|pos| outcome.seen.contains(**pos))
            .find_map(|pos| {
                let room = layout.room_at(*pos)?;
                match &filters.kill_rooms {
                    Some(allowed) if !allowed.contains(&room.name) => None,
                    _ => Some((room.name.clone(), usable[0].clone())),
                }
            })
    };
    let accident_kill = layout.furniture.iter().find_map(|f| {
        if f.kind != crate::world::FurnitureKind::Machine {
            return None;
        }
        let spec = f.machine.as_deref().and_then(|s| data.opportunity(s))?;
        if !matches!(spec.effect, crate::data::OpportunityEffect::AccidentDrop) {
            return None;
        }
        let drop = f.drop_tile?;
        if !stops.contains(&drop) {
            return None;
        }
        // The player must be able to reach the lever.
        if !crate::geom::Dir4::ALL
            .into_iter()
            .any(|d| outcome.seen.contains(f.pos.step(d)))
        {
            return None;
        }
        let room = layout.room_at(drop)?;
        match &filters.kill_rooms {
            Some(allowed) if !allowed.contains(&room.name) => None,
            _ => Some((room.name.clone(), "rigged accident".to_string())),
        }
    });
    let (kill_room, kill_method) = weapon_kill.or(accident_kill).ok_or_else(|| {
        format!(
            "no reachable kill site on the target's schedule for a {} route",
            class.name()
        )
    })?;

    Ok(Completion {
        step: format!("kill the target in {kill_room} with the {kill_method}"),
        site_room: kill_room,
    })
}

/// Proves one route class against the generated venue: complete the
/// mission's objective, then extract.
pub fn prove_route(
    data: &GameData,
    layout: &Layout,
    population: &Population,
    start: Pos,
    class: RouteClass,
    filters: &RouteFilters,
    objective: &Objective,
) -> Result<RouteProof, String> {
    let zone_free = !matches!(class, RouteClass::Social);
    let outcome = capability_closure(
        data,
        layout,
        population,
        start,
        zone_free,
        filters.assume_catalogue,
    );

    // Objective completion, dispatched on the objective. For an Assassinate
    // mission this proves the kill; the closure it is handed is the same one
    // that then decides extraction.
    let completion = prove_completion(
        data, layout, population, &outcome, class, filters, objective,
    )?;

    // Extraction after completion: capabilities only grow, so the same
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
        .room_at(*exit)
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "the street".to_string());

    let mut steps = outcome.events;
    steps.push(completion.step);
    steps.push(format!("extract via {exit_room}"));

    Ok(RouteProof {
        class,
        kill_room: completion.site_room,
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
    objective: &Objective,
) -> Result<RouteProof, String> {
    let filters = constraint.certify_filters(data, layout, population, start)?;
    prove_route(
        data,
        layout,
        population,
        start,
        RouteClass::Physical,
        &filters,
        objective,
    )
}

/// Certifies the three base routes (venue potential: the catalogue is
/// assumed available) plus one route completable with the actual
/// loadout. A venue failing any of the four fails the attempt.
pub fn prove_base_routes(
    data: &GameData,
    layout: &Layout,
    population: &Population,
    start: Pos,
    objective: &Objective,
) -> Result<RouteReport, String> {
    let venue_potential = RouteFilters {
        assume_catalogue: true,
        ..Default::default()
    };
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
            &venue_potential,
            objective,
        )?);
    }
    // And this specific run, with what the player actually carries.
    let loadout_proof = prove_route(
        data,
        layout,
        population,
        start,
        RouteClass::Violence,
        &RouteFilters::default(),
        objective,
    )
    .map_err(|e| format!("loadout cannot complete the mission: {e}"))?;
    Ok(RouteReport {
        proofs,
        loadout_proof: Some(loadout_proof),
        constraint_proof: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::{ATTEMPT_STREAM_BASE, MAX_ATTEMPTS, populate, proof};
    use crate::rng::Pcg32;

    /// The mission objective for a staged population — assassination of the
    /// generated target, exactly as the generator builds it.
    fn objective(population: &Population) -> Objective {
        Objective::Assassinate {
            target: population.target,
        }
    }

    /// Mirrors the generator's front half: a proven venue plus its
    /// population, before world assembly. Streams and retry budget come
    /// from the generator's own constants so the mirror cannot drift.
    fn staged(seed: u64) -> (GameData, Layout, Population, Pos) {
        let data = GameData::embedded().unwrap();
        let venue = data.venue("nightclub").unwrap().clone();
        let config_loadout = vec!["garrote".to_string(), "silenced-pistol".to_string()];
        for attempt in 0..MAX_ATTEMPTS {
            let mut rng = Pcg32::new(seed, ATTEMPT_STREAM_BASE + attempt);
            let Ok(mut layout) = crate::generator::district::build_layout(&data, &venue, &mut rng)
            else {
                continue;
            };
            let Ok(population) =
                populate::populate(&data, &layout, &venue, None, &config_loadout, 0, &mut rng)
            else {
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
            let report =
                prove_base_routes(&data, &layout, &population, start, &objective(&population))
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
                prove_route(
                    &data,
                    &layout,
                    &population,
                    start,
                    class,
                    &filters,
                    &objective(&population)
                )
                .is_err(),
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
            &objective(&population),
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
            &objective(&population),
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
            &objective(&population),
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
                &filters,
                &objective(&population)
            )
            .is_err()
        );
    }
}
