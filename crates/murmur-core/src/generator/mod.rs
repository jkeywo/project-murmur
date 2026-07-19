//! The venue generator.
//!
//! Districts first: a recursive tree of districts decides every room,
//! tier, and connection before tiles exist, then the engine carves the
//! building, populates it, and proves it reachable. Which
//! venue is generated is data (see `data/venues.ron`); everything
//! derives from the mission config through the generation RNG stream, so
//! a fixed config always produces the identical world.

pub mod district;
pub mod layout;
pub mod opportunities;
pub mod populate;
pub mod proof;
pub mod schedule;

use crate::contract::MissionConfig;
use crate::data::GameData;
use crate::rng::{Pcg32, Stream};
use crate::world::{FurnitureKind, MissionFacts, World};

/// Generation retries use derived stream selectors so one mission seed
/// still deterministically defines the final world even when early layout
/// attempts fail.
const ATTEMPT_STREAM_BASE: u64 = 0x4d75726d75720000; // "Murmur" tag

const MAX_ATTEMPTS: u64 = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenError {
    pub attempts: Vec<String>,
}

impl std::fmt::Display for GenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "world generation failed after {} attempts: {}",
            self.attempts.len(),
            self.attempts.join(" | ")
        )
    }
}

impl std::error::Error for GenError {}

/// Generates the world for one mission configuration.
pub fn generate(data: &GameData, config: &MissionConfig) -> Result<World, GenError> {
    let mut attempts = Vec::new();
    if data.venue(&config.venue).is_none() {
        return Err(GenError {
            attempts: vec![format!("unknown venue '{}'", config.venue)],
        });
    }
    for attempt in 0..MAX_ATTEMPTS {
        match try_generate(data, config, attempt) {
            Ok(world) => return Ok(world),
            Err(reason) => attempts.push(format!("attempt {attempt}: {reason}")),
        }
    }
    Err(GenError { attempts })
}

fn try_generate(data: &GameData, config: &MissionConfig, attempt: u64) -> Result<World, String> {
    let seed = config.seed;
    let venue = data.venue(&config.venue).expect("checked by generate");
    let mut rng = Pcg32::new(seed, ATTEMPT_STREAM_BASE + attempt);

    if config.loadout.len() > crate::contract::LOADOUT_SLOTS {
        return Err("loadout exceeds the three-item limit".to_string());
    }
    for spec_id in &config.loadout {
        if data.item(spec_id).is_none() {
            return Err(format!("loadout item '{spec_id}' is unknown"));
        }
    }

    let mut layout = district::build_layout(data, venue, &mut rng).map_err(|e| e.0)?;
    let population = populate::populate(
        data,
        &layout,
        venue,
        config.constraint.as_ref(),
        &config.loadout,
        config.heat,
        &mut rng,
    )
    .map_err(|e| e.0)?;

    // Opportunity machines land before the proofs so the closure and
    // the planner see their capabilities.
    let opportunity_lines =
        opportunities::place_opportunities(data, &mut layout, &population, &mut rng);

    let player_start = population.actors[population.player.0 as usize].pos;
    proof::prove_physical(data, &layout, player_start).map_err(|e| e.0)?;
    let report = proof::prove_progression(data, &mut layout, &population, player_start, &mut rng)
        .map_err(|e| e.0)?;

    // Every mission must be completable three ways: social stealth,
    // physical stealth, and violence, each with extraction — and, under
    // contract, in at least one constraint-compliant way.
    let mut routes = crate::planner::prove_base_routes(data, &layout, &population, player_start)?;
    if let Some(constraint) = &config.constraint {
        routes.constraint_proof = Some(crate::planner::prove_constraint(
            data,
            &layout,
            &population,
            player_start,
            constraint,
        )?);
    }

    let mut facts = build_facts(data, &layout, &population, &mut rng);
    facts.opportunities = opportunity_lines;

    Ok(World {
        seed,
        venue: config.venue.clone(),
        turn: 0,
        map: layout.map,
        doors: layout.doors,
        rooms: layout.rooms,
        furniture: layout.furniture,
        items: population.items,
        actors: population.actors,
        player: population.player,
        target: population.target,
        extraction_tiles: layout.extraction_tiles,
        incidents: Vec::new(),
        player_violence_witnessed: false,
        player_tampering: false,
        mission_heat: 0,
        heat_tier: 0,
        facts,
        proof: report,
        routes,
        constraint: config.constraint.clone(),
        constraint_breach: None,
        outcome: None,
        resolution_rng: Pcg32::for_stream(seed, Stream::Resolution),
    })
}

/// Derives the briefing facts from the generated world. Every statement is
/// a fact about this mission; no hand-authored story is involved.
fn build_facts(
    data: &GameData,
    layout: &layout::Layout,
    population: &populate::Population,
    rng: &mut Pcg32,
) -> MissionFacts {
    let target = &population.actors[population.target.0 as usize];

    let mut target_locations = Vec::new();
    if let Some(ai) = &target.ai {
        for step in &ai.routine {
            if let Some(room) = layout
                .rooms
                .iter()
                .find(|r| r.floor == step.pos.floor && r.bounds.contains(step.pos.x, step.pos.y))
                && !target_locations.contains(&room.name)
            {
                target_locations.push(room.name.clone());
            }
        }
    }

    let mut available_disguises: Vec<String> = Vec::new();
    for actor in &population.actors {
        if actor.is_player() {
            continue;
        }
        let name = data
            .disguise(&actor.worn_disguise)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| actor.worn_disguise.clone());
        if !available_disguises.contains(&name) {
            available_disguises.push(name);
        }
    }
    for furniture in &layout.furniture {
        if let Some(disguise) = &furniture.disguise {
            let name = data
                .disguise(disguise)
                .map(|d| d.name.clone())
                .unwrap_or_else(|| disguise.clone());
            if !available_disguises.contains(&name) {
                available_disguises.push(name);
            }
        }
    }

    let mut restricted_rooms: Vec<String> = Vec::new();
    for room in &layout.rooms {
        if room.zone != crate::data::Zone::Public && !restricted_rooms.contains(&room.name) {
            restricted_rooms.push(room.name.clone());
        }
    }

    let role_of = |actor: &crate::world::Actor| actor.role;
    MissionFacts {
        target_name: target.name.clone(),
        target_reason: rng.pick(&data.briefing.reasons).clone(),
        target_locations,
        guard_count: population
            .actors
            .iter()
            .filter(|a| role_of(a) == Some(crate::data::Role::Guard))
            .count(),
        staff_count: population
            .actors
            .iter()
            .filter(|a| role_of(a).is_some_and(|r| r.is_staff()))
            .count(),
        civilian_count: population
            .actors
            .iter()
            .filter(|a| role_of(a) == Some(crate::data::Role::Civilian))
            .count(),
        available_disguises,
        restricted_rooms,
        container_count: layout
            .furniture
            .iter()
            .filter(|f| f.kind == FurnitureKind::Container)
            .count(),
        extraction_exits: layout
            .rooms
            .iter()
            .filter(|r| r.external_exit)
            .map(|r| r.name.clone())
            .collect(),
        opportunities: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::TileKind;

    fn data() -> GameData {
        GameData::embedded().unwrap()
    }

    /// Every shipped venue, whatever its form. Structural invariants are
    /// properties of a playable venue, not of the realiser that built it,
    /// so they are asserted over the catalogue rather than a hardcoded
    /// list — a new venue is covered the moment it is authored.
    fn venues(data: &GameData) -> Vec<String> {
        data.venues.iter().map(|v| v.id.clone()).collect()
    }

    /// Stairwells join consecutive storeys through linked pairs, with a
    /// distinct tile for up and for down. That is what lets a venue be
    /// taller than two floors: a middle storey needs both, and they
    /// cannot share one tile.
    #[test]
    fn stairwells_link_every_consecutive_storey() {
        let data = data();
        for venue in venues(&data) {
            let venue = venue.as_str();
            for seed in 0..8u64 {
                let world =
                    generate(&data, &crate::contract::MissionConfig::new(seed, venue)).unwrap();
                let floors = world.map.floor_count();
                assert!(
                    !world.map.stair_links().is_empty(),
                    "{venue}: a multi-storey venue needs stairs"
                );
                for link in world.map.stair_links() {
                    assert_eq!(
                        link.a.floor + 1,
                        link.b.floor,
                        "{venue}: a link joins adjacent storeys"
                    );
                    assert_ne!(link.a, link.b);
                    // Stepping either end lands on the other.
                    assert_eq!(world.map.resolve_step_destination(link.a), link.b);
                    assert_eq!(world.map.resolve_step_destination(link.b), link.a);
                }
                // Every storey above the ground is served by a stairwell.
                for floor in 1..floors {
                    assert!(
                        world
                            .map
                            .stair_links()
                            .iter()
                            .any(|l| l.b.floor == floor || l.a.floor == floor),
                        "{venue} seed {seed}: storey {floor} has no stairs"
                    );
                }
            }
        }
    }

    /// Cost, in doors crossed, of reaching each tile from the player's
    /// start. Everything but a door is free, so the number counts the
    /// thresholds between the street and a place — which is what "deeper
    /// into the building" means to a player.
    fn doors_from_spawn(world: &World) -> std::collections::HashMap<crate::geom::Pos, u32> {
        use crate::geom::{Dir4, Pos};
        use std::collections::VecDeque;

        let start = world.actors[world.player.0 as usize].pos;
        let mut cost: std::collections::HashMap<Pos, u32> = std::collections::HashMap::new();
        let mut queue: VecDeque<Pos> = VecDeque::new();
        cost.insert(start, 0);
        queue.push_back(start);
        while let Some(pos) = queue.pop_front() {
            let here = cost[&pos];
            let mut visit = |next: Pos, step: u32, queue: &mut VecDeque<Pos>| {
                if cost.get(&next).copied().is_none_or(|c| c > here + step) {
                    cost.insert(next, here + step);
                    if step == 0 {
                        queue.push_front(next);
                    } else {
                        queue.push_back(next);
                    }
                }
            };
            for dir in Dir4::ALL {
                let next = pos.step(dir);
                match world.map.tile(next) {
                    TileKind::Floor => visit(next, 0, &mut queue),
                    TileKind::Stairs(_) => {
                        visit(next, 0, &mut queue);
                        let across = world.map.resolve_step_destination(next);
                        if across != next {
                            visit(across, 0, &mut queue);
                        }
                    }
                    TileKind::Door(_) => visit(next, 1, &mut queue),
                    TileKind::Wall | TileKind::Void => {}
                }
            }
        }
        cost
    }

    /// The shallowest way into any room of one tier.
    fn cheapest_into_tier(
        world: &World,
        cost: &std::collections::HashMap<crate::geom::Pos, u32>,
        tier: u8,
    ) -> Option<u32> {
        use crate::geom::Pos;
        let mut best: Option<u32> = None;
        for room in world.rooms.iter().filter(|r| r.zone.depth() == tier) {
            let b = room.bounds;
            for y in b.y..b.y + b.h {
                for x in b.x..b.x + b.w {
                    if let Some(c) = cost.get(&Pos::new(room.floor, x, y)) {
                        best = Some(best.map_or(*c, |k: u32| k.min(*c)));
                    }
                }
            }
        }
        best
    }

    /// The security gradient must read outward-to-inward in the geometry,
    /// not merely in the recipe: the shallowest way into a deeper tier is
    /// never shorter than the shallowest way into a shallower one. This is
    /// what makes a venue feel layered, and it is a property of the
    /// finished building, so it holds whatever form realised it.
    #[test]
    fn tiers_get_no_closer_to_the_street_as_they_get_deeper() {
        let data = data();
        for venue in venues(&data) {
            let venue = venue.as_str();
            for seed in 0..40u64 {
                let world =
                    generate(&data, &crate::contract::MissionConfig::new(seed, venue)).unwrap();
                let cost = doors_from_spawn(&world);
                let mut outer = 0u32;
                for tier in 0..3u8 {
                    let Some(c) = cheapest_into_tier(&world, &cost, tier) else {
                        continue;
                    };
                    assert!(
                        c >= outer,
                        "{venue} seed {seed}: tier {tier} is {c} doors from the street but                          the tier outside it is {outer} — the gradient runs backwards"
                    );
                    outer = c;
                }
            }
        }
    }

    /// An onion is a chain of districts, so each tier sits strictly
    /// further from the street than the one wrapping it — not merely no
    /// closer, as the catalogue-wide invariant allows for branching forms.
    #[test]
    fn the_onion_venue_nests_every_tier_strictly() {
        let data = data();
        for seed in 0..20u64 {
            let world = generate(
                &data,
                &crate::contract::MissionConfig::new(seed, "embassy-villa"),
            )
            .unwrap();
            let cost = doors_from_spawn(&world);
            let mut previous: Option<u32> = None;
            for tier in 0..3u8 {
                let Some(c) = cheapest_into_tier(&world, &cost, tier) else {
                    continue;
                };
                if let Some(p) = previous {
                    assert!(
                        c > p,
                        "seed {seed}: tier {tier} is {c} doors in, the tier around it {p} —                          an onion must wrap, not branch"
                    );
                }
                previous = Some(c);
            }
        }
    }

    /// An archipelago's fortresses are independent: each is sealed by its
    /// own key, so taking one does not open the other.
    #[test]
    fn the_archipelago_venue_locks_each_fortress_separately() {
        let data = data();
        for seed in 0..20u64 {
            let world = generate(
                &data,
                &crate::contract::MissionConfig::new(seed, "port-authority"),
            )
            .unwrap();
            let mut keys: Vec<String> = Vec::new();
            for room in &world.rooms {
                for door in &room.doors {
                    if let Some(key) = &world.doors[door.0 as usize].locked_by
                        && !keys.contains(key)
                    {
                        keys.push(key.clone());
                    }
                }
            }
            assert!(
                keys.len() >= 2,
                "seed {seed}: the fortresses share a single key {keys:?}"
            );
        }
    }

    /// Generation retries are a safety net, not the mechanism. A recipe
    /// that routinely needs several attempts is over-constrained, and this
    /// is the early warning — it fails long before the 24-attempt ceiling
    /// turns into an outright generation failure.
    #[test]
    fn every_venue_generates_on_the_first_attempt_almost_always() {
        let data = data();
        for venue in venues(&data) {
            let venue = venue.as_str();
            let mut retried = 0;
            let mut reasons: Vec<String> = Vec::new();
            for seed in 0..40u64 {
                let config = crate::contract::MissionConfig::new(seed, venue);
                if let Err(why) = try_generate(&data, &config, 0) {
                    retried += 1;
                    reasons.push(why);
                }
            }
            assert!(
                retried <= 3,
                "{venue}: {retried} of 40 seeds failed their first attempt — the recipe                  is too tight for its footprint: {reasons:?}"
            );
        }
    }

    /// The mission's whole difficulty rests on this: if the target is
    /// never alone somewhere the player can eventually stand, no weapon
    /// route exists and the mission is only winnable by accident. The
    /// route planner cannot catch it — reachability says nothing about
    /// protection — so generation must.
    #[test]
    fn every_target_is_alone_somewhere_reachable() {
        use crate::generator::proof::vulnerable_positions;
        let data = data();
        for venue in venues(&data) {
            let venue = venue.as_str();
            for seed in 0..20u64 {
                let world =
                    generate(&data, &crate::contract::MissionConfig::new(seed, venue)).unwrap();
                let target = world.actor(world.target);
                let schedule = target
                    .ai
                    .as_ref()
                    .and_then(|ai| ai.schedule.as_ref())
                    .unwrap_or_else(|| panic!("{venue} seed {seed}: the target has no schedule"));
                assert!(
                    schedule.alone_beats().count() >= 1,
                    "{venue} seed {seed}: the target is never alone"
                );
                // Vulnerable positions are spawn plus the alone beats, and
                // they must be somewhere the player can get to.
                let cost = doors_from_spawn(&world);
                assert!(
                    vulnerable_positions(target)
                        .iter()
                        .any(|p| cost.contains_key(p)),
                    "{venue} seed {seed}: every alone beat is unreachable"
                );
            }
        }
    }

    /// Beats and the routine are index-aligned by construction. Systems
    /// that predate beats read the routine and systems that care about
    /// protection read the beats; if the two drift they disagree about
    /// where the target is, and nothing would say so.
    #[test]
    fn the_targets_beats_and_routine_stay_aligned() {
        let data = data();
        for venue in venues(&data) {
            let venue = venue.as_str();
            for seed in 0..20u64 {
                let world =
                    generate(&data, &crate::contract::MissionConfig::new(seed, venue)).unwrap();
                let ai = world.actor(world.target).ai.as_ref().unwrap();
                let schedule = ai.schedule.as_ref().unwrap();
                crate::generator::schedule::assert_aligned(schedule, &ai.routine)
                    .unwrap_or_else(|e| panic!("{venue} seed {seed}: {}", e.0));
            }
        }
    }

    /// Every beat generated is sequential, so the cycle recurs forever and
    /// a reachable alone beat is one that eventually arrives. An interrupt
    /// beat may be added later, but it must never replace a cycle beat —
    /// that is what would break the atemporal planner's guarantee.
    #[test]
    fn schedules_are_cyclic() {
        let data = data();
        for venue in venues(&data) {
            let venue = venue.as_str();
            for seed in 0..20u64 {
                let world =
                    generate(&data, &crate::contract::MissionConfig::new(seed, venue)).unwrap();
                let ai = world.actor(world.target).ai.as_ref().unwrap();
                let schedule = ai.schedule.as_ref().unwrap();
                assert!(
                    schedule
                        .beats
                        .iter()
                        .any(|b| b.trigger == crate::world::BeatTrigger::Sequential),
                    "{venue} seed {seed}: a schedule of pure interrupts never recurs"
                );
                assert!(
                    schedule.beats.iter().all(|b| b.dwell > 0),
                    "{venue} seed {seed}: a zero dwell is a beat the player can never catch"
                );
            }
        }
    }

    /// The mechanical asymmetry of the milestone, asserted end to end:
    /// a route that kills with a weapon must name a room the target is
    /// *alone* in, because a bodyguard ring denies the adjacency a garrote
    /// needs and a bullet finds a guard first. Accidents are exempt — a
    /// crate falls on an escorted target just as well, which is what makes
    /// them the answer to a target you cannot get near.
    #[test]
    fn weapon_routes_kill_where_the_target_is_alone() {
        let data = data();
        for venue in venues(&data) {
            let venue = venue.as_str();
            for seed in 0..20u64 {
                let world =
                    generate(&data, &crate::contract::MissionConfig::new(seed, venue)).unwrap();
                let schedule = world
                    .actor(world.target)
                    .ai
                    .as_ref()
                    .and_then(|ai| ai.schedule.as_ref())
                    .unwrap();
                let alone_rooms: Vec<&str> = schedule
                    .alone_beats()
                    .filter_map(|b| {
                        world
                            .rooms
                            .iter()
                            .find(|r| r.floor == b.pos.floor && r.bounds.contains(b.pos.x, b.pos.y))
                    })
                    .map(|r| r.name.as_str())
                    .collect();
                for proof in &world.routes.proofs {
                    if proof.steps.iter().any(|s| s.contains("rigged accident"))
                        || proof.kill_room.contains("rigged")
                    {
                        continue;
                    }
                    assert!(
                        alone_rooms.contains(&proof.kill_room.as_str()),
                        "{venue} seed {seed}: {} route kills in '{}', which is not a room the                          target is ever alone in {alone_rooms:?}",
                        proof.class.name(),
                        proof.kill_room
                    );
                }
            }
        }
    }

    /// Every door must connect two walkable tiles across it — no door
    #[test]
    fn generates_valid_worlds_across_many_seeds() {
        let data = data();
        for seed in 0..60u64 {
            let world = generate(
                &data,
                &crate::contract::MissionConfig::new(seed, "nightclub"),
            )
            .unwrap_or_else(|e| panic!("seed {seed} failed: {e}"));

            // Every required room template of this venue is present.
            let venue = data.venue("nightclub").unwrap();
            for template in data
                .rooms
                .iter()
                .filter(|t| t.required && venue.room_templates.contains(&t.id))
            {
                assert!(
                    world.rooms.iter().any(|r| r.template == template.id),
                    "seed {seed}: required room '{}' missing",
                    template.id
                );
            }
            // Every declared storey, both extraction exits, a live target.
            assert_eq!(
                usize::from(world.map.floor_count()),
                usize::from(data.venue("nightclub").unwrap().floor_count)
            );
            assert!(
                world.extraction_tiles.len() >= 2,
                "seed {seed}: entrance and loading bay exits expected"
            );
            assert!(world.actor(world.target).is_target);
            assert!(world.actor(world.target).alive());

            // Actors stand on walkable, unoccupied, furniture-free tiles.
            let mut positions = Vec::new();
            for actor in &world.actors {
                assert!(
                    matches!(world.map.tile(actor.pos), TileKind::Floor),
                    "seed {seed}: actor {} on non-floor tile {:?}",
                    actor.name,
                    actor.pos
                );
                assert!(
                    world.furniture_at(actor.pos).is_none(),
                    "seed {seed}: actor inside furniture"
                );
                assert!(
                    !positions.contains(&actor.pos),
                    "seed {seed}: two actors share {:?}",
                    actor.pos
                );
                positions.push(actor.pos);
            }

            // NPCs face somewhere; the player does not.
            assert!(world.player_actor().facing.is_none());
            assert!(
                world
                    .actors
                    .iter()
                    .filter(|a| !a.is_player())
                    .all(|a| a.facing.is_some())
            );

            // The briefing facts are populated.
            assert!(!world.facts.target_name.is_empty());
            assert!(!world.facts.target_reason.is_empty());
            assert!(!world.facts.target_locations.is_empty());
            assert!(world.facts.guard_count >= 4);
            assert!(!world.facts.extraction_exits.is_empty());
            assert!(!world.facts.available_disguises.is_empty());

            // The player starts with the pistol, charged with 6 rounds.
            let pistol = world
                .carried_items(world.player)
                .find(|i| i.spec == "silenced-pistol")
                .expect("player must start with the silenced pistol");
            assert_eq!(pistol.charges, 6);
        }
    }

    #[test]
    fn generation_is_deterministic_for_a_seed() {
        let data = data();
        let a = generate(
            &data,
            &crate::contract::MissionConfig::new(12345, "nightclub"),
        )
        .unwrap();
        let b = generate(
            &data,
            &crate::contract::MissionConfig::new(12345, "nightclub"),
        )
        .unwrap();
        let a_text = ron::to_string(&a).unwrap();
        let b_text = ron::to_string(&b).unwrap();
        assert_eq!(a_text, b_text, "same seed must produce identical worlds");
    }

    #[test]
    fn different_seeds_differ() {
        let data = data();
        let a = generate(&data, &crate::contract::MissionConfig::new(1, "nightclub")).unwrap();
        let b = generate(&data, &crate::contract::MissionConfig::new(2, "nightclub")).unwrap();
        assert_ne!(
            ron::to_string(&a).unwrap(),
            ron::to_string(&b).unwrap(),
            "different seeds should not collide"
        );
    }

    #[test]
    fn keys_exist_for_every_locked_room() {
        let data = data();
        let world = generate(&data, &crate::contract::MissionConfig::new(77, "nightclub")).unwrap();
        for room in &world.rooms {
            let template = data.room_template(&room.template).unwrap();
            if let Some(key) = &template.locked_by {
                assert!(
                    world.items.iter().any(|i| &i.spec == key),
                    "locked room '{}' has no key '{key}' in the mission",
                    room.name
                );
            }
        }
    }

    #[test]
    fn proof_reports_civilian_baseline() {
        let data = data();
        let world = generate(&data, &crate::contract::MissionConfig::new(5, "nightclub")).unwrap();
        assert!(
            world
                .proof
                .obtainable_disguises
                .contains(&"civilian".to_string())
        );
        // Guards patrol public space, so their uniform must be in the
        // progression; likewise staff via the public bar.
        assert!(
            world
                .proof
                .obtainable_disguises
                .contains(&"staff".to_string())
        );
    }
}
