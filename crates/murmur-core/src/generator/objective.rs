//! Designating the entity a non-Assassinate objective centres on.
//!
//! Every mission is generated the same way — the same layout, population,
//! opportunities, and reachability proof — regardless of its goal. Only
//! *after* that standard build does a non-default objective pick out (or,
//! for a stolen item, place) the thing the player must act on. Assassination
//! never reaches this module: it is built inline from `population.target`
//! and draws no RNG, so every default mission stays byte-identical.
//!
//! Each objective reuses an entity that generation already guarantees is
//! reachable, and phrases completion as "reach a qualifying position", which
//! the planner's capability closure already answers unchanged:
//!  - **Steal** puts a ledger in the mark's pockets — lifted at an alone
//!    beat in its private (secured) space, reusing the whole target model.
//!  - **Sabotage** designates an opportunity machine, preferring secured
//!    space.
//!  - **Rescue** designates an ordinary person (never a guard) and clears
//!    their routine so they wait to be led out.
//!  - **Plant** designates a mark to slip the loadout's bug onto.

use crate::data::{GameData, Role, Zone};
use crate::geom::Pos;
use crate::world::{
    Actor, ActorId, Furniture, FurnitureId, FurnitureKind, ItemId, ItemInstance, ItemLocation,
    Objective, ObjectiveKind, PlantTarget,
};

use super::layout::Layout;
use super::populate::Population;

/// Builds the objective for a non-Assassinate mission, mutating the
/// population where an objective needs an item placed or a captive stilled.
/// Draws no RNG — every choice is the first match in generation order — so
/// the objective is a deterministic function of the generated world.
pub fn build_objective(
    kind: ObjectiveKind,
    data: &GameData,
    layout: &Layout,
    population: &mut Population,
    _rng: &mut crate::rng::Pcg32,
) -> Result<Objective, String> {
    let secured = |pos: Pos| {
        matches!(
            layout.room_at(pos).map(|r| r.zone),
            Some(Zone::Secure) | Some(Zone::Personal)
        )
    };

    match kind {
        ObjectiveKind::Assassinate => {
            // Built inline in the generator so it draws no RNG; reaching here
            // would mean the default path changed.
            Err("assassination is built inline, not through build_objective".to_string())
        }
        ObjectiveKind::Steal => {
            // The mark carries the ledger; it is lifted at an alone beat in
            // its private space. Reusing the target keeps the theft inside
            // guaranteed-secured, guaranteed-reachable space with no new cast.
            let holder = population.target;
            let item = ItemId(population.items.len() as u32);
            population.items.push(ItemInstance {
                id: item,
                spec: "secret-ledger".to_string(),
                location: ItemLocation::CarriedBy(holder),
                charges: 0,
            });
            Ok(Objective::Steal { item })
        }
        ObjectiveKind::Sabotage => {
            let machine = designate_machine(layout, &secured)
                .ok_or_else(|| "no opportunity machine to sabotage".to_string())?;
            Ok(Objective::Sabotage { machine })
        }
        ObjectiveKind::Rescue => {
            let person = designate_person(population, &secured)
                .ok_or_else(|| "no eligible person to rescue".to_string())?;
            // A captive waits where they are rather than walking a routine,
            // until the player leads them out.
            if let Some(ai) = population.actors[person.0 as usize].ai.as_mut() {
                ai.routine.clear();
                ai.schedule = None;
            }
            Ok(Objective::Rescue { person })
        }
        ObjectiveKind::Plant => {
            let item = population
                .items
                .iter()
                .find(|i| {
                    i.location == ItemLocation::CarriedBy(population.player)
                        && i.spec == "listening-bug"
                })
                .map(|i| i.id)
                .ok_or_else(|| "a plant mission carries no listening bug".to_string())?;
            let mark = designate_person(population, &secured)
                .ok_or_else(|| "no eligible mark to plant on".to_string())?;
            let _ = data;
            Ok(Objective::Plant {
                item,
                on: PlantTarget::Person(mark),
            })
        }
    }
}

/// An opportunity machine to designate, preferring one in secured space and
/// otherwise the first machine in generation order.
fn designate_machine(layout: &Layout, secured: &impl Fn(Pos) -> bool) -> Option<FurnitureId> {
    let is_machine = |f: &&Furniture| f.kind == FurnitureKind::Machine && f.machine.is_some();
    layout
        .furniture
        .iter()
        .filter(is_machine)
        .find(|f| secured(f.pos))
        .or_else(|| layout.furniture.iter().find(is_machine))
        .map(|f| f.id)
}

/// A non-player, non-target, non-guard, living person to designate,
/// preferring one whose spawn or routine reaches secured space.
fn designate_person(population: &Population, secured: &impl Fn(Pos) -> bool) -> Option<ActorId> {
    let eligible = |a: &&Actor| {
        a.id != population.player
            && a.id != population.target
            && a.role != Some(Role::Guard)
            && a.alive()
    };
    let touches_secured = |a: &&Actor| {
        secured(a.pos)
            || a.ai
                .as_ref()
                .is_some_and(|ai| ai.routine.iter().any(|s| secured(s.pos)))
    };
    population
        .actors
        .iter()
        .filter(eligible)
        .find(touches_secured)
        .or_else(|| population.actors.iter().find(eligible))
        .map(|a| a.id)
}
