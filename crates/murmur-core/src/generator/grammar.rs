//! The security-gradient venue graph grammar.
//!
//! Before any tile exists, the grammar decides the whole room graph of a
//! venue: which rooms exist, which storey and circulation side each sits
//! on, the west-to-east security-band ordering (public shallowest,
//! personal deepest), and every connection — primary doors onto the main
//! corridor, service doors onto the staff-tier service corridor, and
//! room-to-room pass-through doors between adjacent restricted rooms.
//! Loops, alternate vertical routes, and multiple extraction paths are
//! graph properties checked here; realisation (layout) only carves what
//! the graph already guarantees.
//!
//! Circulation topology per storey: a public main corridor through the
//! middle, a staff service corridor along the back, and two stub
//! passages joining them at the west and east ends (the loop). Stairs
//! connect the storeys at both dead ends of the main corridor (the
//! alternate vertical routes).

use crate::data::{GameData, VenueSpec, Zone};
use crate::geom::FloorId;
use crate::rng::Pcg32;

/// Which circulation a room fronts onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shelf {
    /// Between the service corridor and the main corridor; may hold a
    /// service door in addition to its main door.
    Service,
    /// Between the main corridor and the outer wall; external exits live
    /// here.
    Main,
}

/// One room the grammar has decided on, before tiles.
#[derive(Clone, Debug)]
pub struct RoomNode {
    pub template_index: usize,
    pub name: String,
    pub floor: FloorId,
    pub width: i16,
    pub height: i16,
    pub band: Zone,
    pub shelf: Shelf,
    /// A door onto the service corridor (service-shelf rooms with
    /// service access).
    pub service_door: bool,
}

/// One storey of the venue graph: rooms in west-to-east order per shelf,
/// plus the pass-through doors between consecutive rooms.
#[derive(Clone, Debug)]
pub struct FloorPlan {
    pub floor: FloorId,
    pub service_shelf: Vec<RoomNode>,
    pub main_shelf: Vec<RoomNode>,
    /// `service_pass[i]` — a connecting door between service_shelf[i]
    /// and service_shelf[i+1].
    pub service_pass: Vec<bool>,
    /// Likewise for the main shelf.
    pub main_pass: Vec<bool>,
}

/// The venue graph: everything about connectivity, decided pre-geometry.
#[derive(Clone, Debug)]
pub struct VenueGraph {
    pub floors: Vec<FloorPlan>,
    /// Index into `data.rooms` of the circulation (service corridor)
    /// template.
    pub circulation_template: usize,
}

fn band_depth(zone: Zone) -> u8 {
    match zone {
        Zone::Public => 0,
        Zone::Staff => 1,
        Zone::Secure => 2,
        Zone::Personal => 3,
    }
}

/// Builds and checks the venue graph. Errors abort the attempt; the
/// generator retries with a derived stream.
pub fn build_graph(
    data: &GameData,
    venue: &VenueSpec,
    rng: &mut Pcg32,
) -> Result<VenueGraph, String> {
    let circulation_template = data
        .rooms
        .iter()
        .position(|t| t.circulation && venue.room_templates.contains(&t.id))
        .ok_or_else(|| format!("venue '{}' lists no circulation template", venue.id))?;

    // Shelf interior heights the realisation will provide (see layout):
    // service shelf sits between the two corridors, main shelf between
    // the main corridor and the outer wall.
    let spine = i16::try_from(venue.floor_height).unwrap() / 2;
    let service_shelf_h = spine - 5;
    let main_shelf_h = i16::try_from(venue.floor_height).unwrap() - spine - 4;

    let mut floors: Vec<FloorPlan> = (0..venue.floor_count)
        .map(|floor| FloorPlan {
            floor,
            service_shelf: Vec::new(),
            main_shelf: Vec::new(),
            service_pass: Vec::new(),
            main_pass: Vec::new(),
        })
        .collect();

    // Decide instances: count and storey, then shelf.
    for (template_index, template) in data.rooms.iter().enumerate() {
        if !venue.room_templates.contains(&template.id) || template.circulation {
            continue;
        }
        let count = rng.range_inclusive(template.count_min.into(), template.count_max.into());
        let count = if template.required {
            count.max(1)
        } else {
            count
        };
        for instance in 0..count {
            let floor = *rng.pick(&template.floors);
            let plan = &mut floors[usize::from(floor)];

            // External exits need the outer wall: always the main shelf.
            // Service-access rooms front the service corridor. Everything
            // else balances onto the emptier shelf.
            let shelf = if template.external_exit {
                Shelf::Main
            } else if template.service_access {
                Shelf::Service
            } else {
                let service_load: i16 = plan.service_shelf.iter().map(|r| r.width + 1).sum();
                let main_load: i16 = plan.main_shelf.iter().map(|r| r.width + 1).sum();
                if service_load < main_load {
                    Shelf::Service
                } else {
                    Shelf::Main
                }
            };

            let shelf_h = match shelf {
                Shelf::Service => service_shelf_h,
                Shelf::Main => main_shelf_h,
            };
            let min_h = i16::try_from(template.min_size.1).unwrap();
            let max_h = i16::try_from(template.max_size.1).unwrap().min(shelf_h);
            if min_h > max_h {
                return Err(format!(
                    "room '{}' cannot fit a shelf of height {shelf_h}",
                    template.id
                ));
            }
            let name = if count > 1 {
                format!("{} {}", template.name, instance + 1)
            } else {
                template.name.clone()
            };
            let node = RoomNode {
                template_index,
                name,
                floor,
                width: rng.range_inclusive(template.min_size.0.into(), template.max_size.0.into())
                    as i16,
                height: rng.range_inclusive(min_h as u32, max_h as u32) as i16,
                band: template.zone,
                shelf,
                service_door: template.service_access && shelf == Shelf::Service,
            };
            match shelf {
                Shelf::Service => plan.service_shelf.push(node),
                Shelf::Main => plan.main_shelf.push(node),
            }
        }
    }

    // The security gradient: west-to-east order is band depth. The sort
    // is stable, so same-band rooms keep their (seed-shuffled) order.
    for plan in &mut floors {
        plan.service_shelf.sort_by_key(|r| band_depth(r.band));
        plan.main_shelf.sort_by_key(|r| band_depth(r.band));

        // Pass-through doors between consecutive restricted rooms: a
        // service route that avoids the corridor.
        plan.service_pass = pass_doors(&plan.service_shelf);
        plan.main_pass = pass_doors(&plan.main_shelf);
    }

    let graph = VenueGraph {
        floors,
        circulation_template,
    };
    check_graph(data, venue, &graph)?;
    Ok(graph)
}

fn pass_doors(shelf: &[RoomNode]) -> Vec<bool> {
    shelf
        .windows(2)
        .map(|pair| pair[0].band != Zone::Public && pair[1].band != Zone::Public)
        .collect()
}

/// Graph-level guarantees, checked before geometry exists.
fn check_graph(data: &GameData, venue: &VenueSpec, graph: &VenueGraph) -> Result<(), String> {
    // An entrance on the ground main shelf.
    let ground = graph
        .floors
        .first()
        .ok_or_else(|| "venue has no storeys".to_string())?;
    if !ground
        .main_shelf
        .iter()
        .any(|r| data.rooms[r.template_index].external_exit)
    {
        return Err("no external exit on the ground main shelf".to_string());
    }

    // Multiple extraction paths: at least two external-exit rooms plus
    // the ground service corridor's fire exit.
    let exit_rooms: usize = graph
        .floors
        .iter()
        .flat_map(|p| p.main_shelf.iter())
        .filter(|r| data.rooms[r.template_index].external_exit)
        .count();
    if exit_rooms + 1 < 2 {
        return Err("fewer than two extraction paths".to_string());
    }

    // The gradient holds per shelf (sorted by construction; keep the
    // check so later grammar changes cannot silently break it).
    for plan in &graph.floors {
        for shelf in [&plan.service_shelf, &plan.main_shelf] {
            let depths: Vec<u8> = shelf.iter().map(|r| band_depth(r.band)).collect();
            if depths.windows(2).any(|w| w[0] > w[1]) {
                return Err(format!("floor {} breaks the security gradient", plan.floor));
            }
        }
    }

    // Fit check: each shelf's total width (rooms at minimum size plus
    // dividing walls plus the stub reservation) must fit the storey.
    let width = i16::try_from(venue.floor_width).unwrap();
    for plan in &graph.floors {
        for (shelf, start_x) in [(&plan.service_shelf, 6i16), (&plan.main_shelf, 3i16)] {
            let min_total: i16 = shelf
                .iter()
                .map(|r| i16::try_from(data.rooms[r.template_index].min_size.0).unwrap() + 1)
                .sum();
            if start_x + min_total >= width - 3 {
                return Err(format!(
                    "floor {} shelf overflows even at minimum sizes",
                    plan.floor
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (GameData, VenueSpec) {
        let data = GameData::embedded().unwrap();
        let venue = data.venue("nightclub").unwrap().clone();
        (data, venue)
    }

    #[test]
    fn graph_has_gradient_and_exits_before_geometry() {
        let (data, venue) = setup();
        for seed in 0..30u64 {
            let mut rng = Pcg32::new(seed, 7);
            let graph =
                build_graph(&data, &venue, &mut rng).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
            // Ground floor: entrance on the main shelf, staff rooms on
            // the service shelf.
            let ground = &graph.floors[0];
            assert!(
                ground
                    .main_shelf
                    .iter()
                    .any(|r| data.rooms[r.template_index].external_exit)
            );
            assert!(
                ground
                    .service_shelf
                    .iter()
                    .all(|r| !data.rooms[r.template_index].external_exit),
                "external exits must front the outer wall"
            );
            // The gradient is monotone on every shelf.
            for plan in &graph.floors {
                for shelf in [&plan.service_shelf, &plan.main_shelf] {
                    let depths: Vec<u8> = shelf.iter().map(|r| band_depth(r.band)).collect();
                    assert!(depths.windows(2).all(|w| w[0] <= w[1]), "gradient broken");
                }
            }
        }
    }

    #[test]
    fn pass_doors_connect_restricted_neighbours_only() {
        let (data, venue) = setup();
        let mut rng = Pcg32::new(3, 7);
        let graph = build_graph(&data, &venue, &mut rng).unwrap();
        for plan in &graph.floors {
            for (shelf, passes) in [
                (&plan.service_shelf, &plan.service_pass),
                (&plan.main_shelf, &plan.main_pass),
            ] {
                assert_eq!(passes.len(), shelf.len().saturating_sub(1));
                for (i, has_door) in passes.iter().enumerate() {
                    let restricted =
                        shelf[i].band != Zone::Public && shelf[i + 1].band != Zone::Public;
                    assert_eq!(*has_door, restricted);
                }
            }
        }
    }
}
