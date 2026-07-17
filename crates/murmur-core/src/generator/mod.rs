//! The nightclub generator.
//!
//! Room-graph first: rooms and their metadata are decided before tiles and
//! corridors, then the building is realised, populated, and proven
//! reachable. Everything derives from the mission seed through the
//! generation RNG stream; a fixed seed always produces the identical world.

pub mod layout;
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

    let mut layout = layout::build_layout(data, venue, &mut rng).map_err(|e| e.0)?;
    let mut population = populate::populate(data, &layout, &mut rng).map_err(|e| e.0)?;

    // Charge weapons with their generated rounds.
    for item in &mut population.items {
        if data.item(&item.spec).is_some_and(|s| s.weapon) {
            item.charges = data.tuning.pistol_rounds;
        }
    }

    let player_start = population.actors[population.player.0 as usize].pos;
    proof::prove_physical(data, &layout, player_start).map_err(|e| e.0)?;
    let report = proof::prove_progression(data, &mut layout, &population, player_start, &mut rng)
        .map_err(|e| e.0)?;

    let facts = build_facts(data, &layout, &population, &mut rng);

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
        facts,
        proof: report,
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
    fn generates_valid_worlds_across_many_seeds() {
        let data = data();
        for seed in 0..60u64 {
            let world = generate(
                &data,
                &crate::contract::MissionConfig::new(seed, "nightclub"),
            )
            .unwrap_or_else(|e| panic!("seed {seed} failed: {e}"));

            // Every required room template is present.
            for template in data.rooms.iter().filter(|t| t.required) {
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
