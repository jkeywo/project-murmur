//! Opportunity machine placement.
//!
//! Machines are placed after population and before the proofs, so the
//! reachability closure and the planner see their effects. A machine is
//! placed only when it improves a planner-validated route, with a
//! deterministic, data-visible rule per effect:
//!
//! * cut-lights: only where its storey still has bright rooms to darken;
//! * stock-wardrobe: only when no wardrobe already offers that disguise;
//! * accident-drop: only above a stop on the target's schedule (the
//!   weapon-free kill the violence and constraint routes may lean on);
//! * place-key: only when the venue has locked doors to open;
//! * evacuate: wherever an eligible room exists (clearing the crowds
//!   improves any route).

use crate::data::{GameData, OpportunityEffect};
use crate::generator::layout::{Layout, find_free_spot};
use crate::generator::populate::Population;
use crate::generator::proof::schedule_positions;
use crate::rng::Pcg32;
use crate::world::{Furniture, FurnitureId, FurnitureKind};

/// Places every opportunity whose improvement rule fires. Returns the
/// discoverable briefing lines ("a humming fuse box in the kitchen").
pub fn place_opportunities(
    data: &GameData,
    layout: &mut Layout,
    population: &Population,
    rng: &mut Pcg32,
) -> Vec<String> {
    let mut placed: Vec<String> = Vec::new();
    let target = &population.actors[population.target.0 as usize];
    let target_stops = schedule_positions(target);
    // Machines land after population: never on top of someone.
    let occupied: Vec<crate::geom::Pos> = population.actors.iter().map(|a| a.pos).collect();

    for spec in &data.opportunities {
        // Eligible rooms for the machine body.
        let eligible: Vec<usize> = layout
            .rooms
            .iter()
            .enumerate()
            .filter(|(_, r)| spec.zones.contains(&r.zone))
            .map(|(i, _)| i)
            .collect();
        if eligible.is_empty() {
            continue;
        }

        match &spec.effect {
            OpportunityEffect::StockWardrobe { disguise } => {
                // Improves social routes only when that disguise has no
                // wardrobe source yet.
                let already = layout.furniture.iter().any(|f| {
                    f.kind == FurnitureKind::Wardrobe && f.disguise.as_deref() == Some(disguise)
                });
                if already {
                    continue;
                }
                for room_index in &eligible {
                    let room = layout.rooms[*room_index].clone();
                    let extraction = layout.extraction_tiles.clone();
                    if let Some(pos) = find_free_spot(
                        &room,
                        &layout.map,
                        &extraction,
                        &layout.furniture,
                        &occupied,
                        rng,
                    ) {
                        layout.furniture.push(Furniture {
                            id: FurnitureId(layout.furniture.len() as u32),
                            kind: FurnitureKind::Wardrobe,
                            pos,
                            body: None,
                            disguise: Some(disguise.clone()),
                            machine: Some(spec.id.clone()),
                            used: false,
                            drop_tile: None,
                        });
                        placed.push(crate::loc::fmt(
                            "opportunity.hint",
                            &[("presentation", &spec.presentation), ("room", &room.name)],
                        ));
                        break;
                    }
                }
            }
            OpportunityEffect::AccidentDrop => {
                // Improves violence and constraint routes: a weapon-free,
                // deniable kill above a stop the target will visit.
                //
                // Escorted stops are preferred, which makes the asymmetry
                // structural rather than incidental — the accident is the
                // answer to a target you cannot get near, so it wants to
                // sit where the detail is, not where the player could have
                // used a garrote anyway.
                let escorted: Vec<crate::geom::Pos> = target
                    .ai
                    .as_ref()
                    .and_then(|ai| ai.schedule.as_ref())
                    .map(|s| {
                        s.beats
                            .iter()
                            .filter(|b| b.protection == crate::world::Protection::Escorted)
                            .map(|b| b.pos)
                            .collect()
                    })
                    .unwrap_or_default();
                let ordered: Vec<crate::geom::Pos> = escorted
                    .iter()
                    .chain(target_stops.iter().filter(|p| !escorted.contains(p)))
                    .copied()
                    .collect();
                let mut spots: Vec<(usize, crate::geom::Pos)> = Vec::new();
                for stop in &ordered {
                    if let Some((index, _)) = layout.rooms.iter().enumerate().find(|(_, r)| {
                        spec.zones.contains(&r.zone)
                            && r.floor == stop.floor
                            && r.bounds.contains(stop.x, stop.y)
                    }) && !spots.iter().any(|(_, p)| p == stop)
                    {
                        spots.push((index, *stop));
                    }
                }
                for (room_index, drop_tile) in spots {
                    let room = layout.rooms[room_index].clone();
                    let extraction = layout.extraction_tiles.clone();
                    let Some(pos) = find_free_spot(
                        &room,
                        &layout.map,
                        &extraction,
                        &layout.furniture,
                        &occupied,
                        rng,
                    ) else {
                        continue;
                    };
                    if pos == drop_tile {
                        continue;
                    }
                    layout.furniture.push(Furniture {
                        id: FurnitureId(layout.furniture.len() as u32),
                        kind: FurnitureKind::Machine,
                        pos,
                        body: None,
                        disguise: None,
                        machine: Some(spec.id.clone()),
                        used: false,
                        drop_tile: Some(drop_tile),
                    });
                    placed.push(crate::loc::fmt(
                        "opportunity.hint",
                        &[("presentation", &spec.presentation), ("room", &room.name)],
                    ));
                    break;
                }
            }
            OpportunityEffect::CutLights
            | OpportunityEffect::PlaceKey { .. }
            | OpportunityEffect::Evacuate
            | OpportunityEffect::SummonTarget { .. } => {
                let improves = match &spec.effect {
                    OpportunityEffect::CutLights => true, // checked per floor below
                    OpportunityEffect::PlaceKey { .. } => {
                        layout.doors.iter().any(|d| d.locked_by.is_some())
                    }
                    // A paging desk that summons a beat this target does
                    // not have is a lever wired to nothing.
                    OpportunityEffect::SummonTarget { tag } => target
                        .ai
                        .as_ref()
                        .and_then(|ai| ai.schedule.as_ref())
                        .is_some_and(|s| s.beats.iter().any(|b| b.tag == *tag)),
                    _ => true,
                };
                if !improves {
                    continue;
                }
                for room_index in &eligible {
                    let room = layout.rooms[*room_index].clone();
                    if matches!(spec.effect, OpportunityEffect::CutLights) {
                        // Only worth placing where there is light to cut.
                        let any_bright = layout.rooms.iter().any(|r| {
                            r.floor == room.floor && r.lighting == crate::data::Lighting::Bright
                        });
                        if !any_bright {
                            continue;
                        }
                    }
                    let extraction = layout.extraction_tiles.clone();
                    if let Some(pos) = find_free_spot(
                        &room,
                        &layout.map,
                        &extraction,
                        &layout.furniture,
                        &occupied,
                        rng,
                    ) {
                        layout.furniture.push(Furniture {
                            id: FurnitureId(layout.furniture.len() as u32),
                            kind: FurnitureKind::Machine,
                            pos,
                            body: None,
                            disguise: None,
                            machine: Some(spec.id.clone()),
                            used: false,
                            drop_tile: None,
                        });
                        placed.push(crate::loc::fmt(
                            "opportunity.hint",
                            &[("presentation", &spec.presentation), ("room", &room.name)],
                        ));
                        break;
                    }
                }
            }
        }
    }
    placed
}
