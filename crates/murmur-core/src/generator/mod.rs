//! The venue generator.
//!
//! Graph first: the security-gradient grammar decides every room,
//! band, and connection before tiles exist, then the banded realisation
//! carves the building, populates it, and proves it reachable. Which
//! venue is generated is data (see `data/venues.ron`); everything
//! derives from the mission config through the generation RNG stream, so
//! a fixed config always produces the identical world.

pub mod grammar;
pub mod layout;
pub mod opportunities;
pub mod populate;
pub mod proof;

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

    let mut layout = layout::build_layout(data, venue, &mut rng).map_err(|e| e.0)?;
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

    #[test]
    fn banded_realisation_keeps_the_graph_guarantees() {
        let data = data();
        for seed in 0..40u64 {
            let world = generate(
                &data,
                &crate::contract::MissionConfig::new(seed, "nightclub"),
            )
            .unwrap_or_else(|e| panic!("seed {seed} failed: {e}"));

            for floor in 0..world.map.floor_count() {
                // Every storey has its staff-tier service corridor.
                let corridor = world
                    .rooms
                    .iter()
                    .find(|r| r.floor == floor && r.template == "service-corridor")
                    .unwrap_or_else(|| panic!("seed {seed}: no service corridor on {floor}"));
                assert_eq!(corridor.zone, crate::data::Zone::Staff);

                // Both stubs join the corridors into a loop: the west
                // stub at column 3 and the east stub at the corridor's
                // end column are walkable from top shelf to spine.
                let sx = corridor.bounds.x + corridor.bounds.w - 2;
                for stub_x in [3i16, sx] {
                    for y in 3..(world.map.height() as i16 / 2) {
                        assert_eq!(
                            world.map.tile(crate::geom::Pos::new(floor, stub_x, y)),
                            TileKind::Floor,
                            "seed {seed}: stub column {stub_x} blocked at y={y} floor {floor}"
                        );
                    }
                }

                // The security gradient holds per shelf, west to east.
                let depth = |zone: crate::data::Zone| match zone {
                    crate::data::Zone::Public => 0,
                    crate::data::Zone::Staff => 1,
                    crate::data::Zone::Secure => 2,
                    crate::data::Zone::Personal => 3,
                };
                let spine = world.map.height() as i16 / 2;
                for service_side in [true, false] {
                    let mut shelf: Vec<_> = world
                        .rooms
                        .iter()
                        .filter(|r| {
                            r.floor == floor
                                && r.template != "service-corridor"
                                && (r.bounds.y < spine) == service_side
                        })
                        .collect();
                    shelf.sort_by_key(|r| r.bounds.x);
                    let depths: Vec<u8> = shelf.iter().map(|r| depth(r.zone)).collect();
                    assert!(
                        depths.windows(2).all(|w| w[0] <= w[1]),
                        "seed {seed}: floor {floor} gradient broken: {depths:?}"
                    );
                }
            }

            // Three extraction paths: the public entrance first (the
            // player's spawn side), then the loading bay, then the
            // service corridor's fire exit in staff space.
            assert!(world.extraction_tiles.len() >= 3, "seed {seed}");
            let first_room = world.room_at(world.extraction_tiles[0]).unwrap();
            assert_eq!(first_room.zone, crate::data::Zone::Public, "seed {seed}");
            assert!(
                world.extraction_tiles.iter().any(|t| world
                    .room_at(*t)
                    .is_some_and(
                        |r| r.template == "service-corridor" && r.zone == crate::data::Zone::Staff
                    )),
                "seed {seed}: no staff-space fire exit"
            );
        }
    }

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
            // Two storeys, both extraction exits, a live target.
            assert_eq!(world.map.floor_count(), 2);
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
