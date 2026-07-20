//! Layout finishing: the shared tail of venue realisation.
//!
//! The district engine (see [`super::district`]) carves the map, doors,
//! and room records; this module turns that carved shell into a playable
//! layout — routine waypoints, extraction tiles ordered public-first so
//! the player spawns at the front door, and furniture placed so every
//! room keeps its doors, waypoints, and exits mutually reachable. It
//! also owns the shared placement rule (`find_free_spot`,
//! `insert_wardrobe`) the proof and opportunity phases use when they
//! patch furniture in after the fact.

use crate::data::{GameData, RoomTemplate, WaypointKind};
use crate::geom::{Dir4, Pos};
use crate::map::{DoorState, GameMap, TileKind};
use crate::rng::Pcg32;
use crate::world::{Furniture, FurnitureId, FurnitureKind, Room, Waypoint};

/// Layout output: the map plus room records and furniture.
pub struct Layout {
    pub map: GameMap,
    pub doors: Vec<DoorState>,
    pub rooms: Vec<Room>,
    pub furniture: Vec<Furniture>,
    pub extraction_tiles: Vec<Pos>,
}

impl Layout {
    /// The room containing `pos`, if any: the same deep query the
    /// finished world answers with `World::room_at`.
    pub fn room_at(&self, pos: Pos) -> Option<&Room> {
        crate::world::room_containing(&self.rooms, pos)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutError(pub String);

/// The form-agnostic tail of realisation: waypoints, extraction tiles,
/// and furniture. It reads only the finished rooms and map, so it does
/// not care what carved them.
pub(crate) fn finish_layout(
    data: &GameData,
    map: GameMap,
    doors: Vec<DoorState>,
    mut rooms: Vec<Room>,
    rng: &mut Pcg32,
) -> Result<Layout, LayoutError> {
    if rooms.is_empty() {
        return Err(LayoutError("the venue placed no rooms".to_string()));
    }

    // Waypoints before furniture so lingering spots stay clear.
    for room in &mut rooms {
        let template = &data.rooms[template_index_of(data, &room.template)];
        room.waypoints = pick_waypoints(template, room, rng);
    }

    // Extraction exits, public first so the player still spawns at the
    // front door, then restricted ones.
    let mut extraction_tiles = Vec::new();
    let mut ordered: Vec<&Room> = rooms.iter().filter(|r| r.external_exit).collect();
    ordered.sort_by_key(|r| (r.zone != crate::data::Zone::Public, r.id.0));
    for room in ordered {
        extraction_tiles.push(street_side_tile(room, &map));
    }
    if extraction_tiles.is_empty() {
        return Err(LayoutError("the venue has no way out".to_string()));
    }

    let mut furniture = Vec::new();
    for room in &rooms {
        let template = &data.rooms[template_index_of(data, &room.template)];
        place_furniture(template, room, &map, &extraction_tiles, &mut furniture, rng);
    }

    Ok(Layout {
        map,
        doors,
        rooms,
        furniture,
        extraction_tiles,
    })
}

/// The interior tile on whichever side of the room lies closest to the
/// building's outer wall — the side the street is on.
///
/// Never the tile in front of a doorway. The exit marker is drawn over
/// whatever tile it sits on, so an exit on the room's threshold reads as
/// a blocked entrance — and in a room with one door that is the only way
/// in. Ties are broken towards the street first, then by position, so
/// the choice stays a pure function of the geometry.
fn street_side_tile(room: &Room, map: &GameMap) -> Pos {
    let b = room.bounds;
    let sides = [
        Pos::new(room.floor, b.x + b.w / 2, b.y),
        Pos::new(room.floor, b.x + b.w / 2, b.y + b.h - 1),
        Pos::new(room.floor, b.x, b.y + b.h / 2),
        Pos::new(room.floor, b.x + b.w - 1, b.y + b.h / 2),
    ];
    let edge_gap = |p: &Pos| {
        let dx = p.x.min(map.width() as i16 - 1 - p.x);
        let dy = p.y.min(map.height() as i16 - 1 - p.y);
        dx.min(dy)
    };
    let beside_a_door = |p: &Pos| {
        Dir4::ALL
            .into_iter()
            .any(|d| matches!(map.tile(p.step(d)), TileKind::Door(_)))
    };
    // The four sides first; if every one of them fronts a door, widen to
    // the whole room rather than give up and block a threshold.
    sides
        .into_iter()
        .filter(|p| !beside_a_door(p))
        .chain(
            interior_tiles(room)
                .into_iter()
                .filter(|p| !beside_a_door(p)),
        )
        .min_by_key(|p| (edge_gap(p), p.y, p.x))
        .unwrap_or_else(|| {
            sides
                .into_iter()
                .min_by_key(|p| (edge_gap(p), p.y, p.x))
                .expect("four sides")
        })
}

fn template_index_of(data: &GameData, id: &str) -> usize {
    data.rooms
        .iter()
        .position(|t| t.id == id)
        .expect("room template ids are validated at data load")
}

fn interior_tiles(room: &Room) -> Vec<Pos> {
    let b = room.bounds;
    let mut tiles = Vec::new();
    for y in b.y..(b.y + b.h) {
        for x in b.x..(b.x + b.w) {
            tiles.push(Pos::new(room.floor, x, y));
        }
    }
    tiles
}

fn pick_waypoints(template: &RoomTemplate, room: &Room, rng: &mut Pcg32) -> Vec<Waypoint> {
    let mut free = interior_tiles(room);
    let mut waypoints = Vec::new();
    for slot in &template.waypoints {
        for _ in 0..slot.count {
            if free.is_empty() {
                break;
            }
            let pos = rng.take(&mut free);
            waypoints.push(Waypoint {
                kind: slot.kind,
                pos,
            });
        }
    }
    waypoints
}

/// Tiles a room must keep walkable: waypoints, extraction tiles, and the
/// tile inside each door.
fn protected_tiles(room: &Room, map: &GameMap, extraction: &[Pos]) -> Vec<Pos> {
    let mut protected: Vec<Pos> = room.waypoints.iter().map(|w| w.pos).collect();
    protected.extend(
        extraction
            .iter()
            .copied()
            .filter(|p| room.floor == p.floor && room.bounds.contains(p.x, p.y)),
    );
    // Interior tiles adjacent to a door.
    for pos in interior_tiles(room) {
        let adjacent_door = crate::geom::Dir4::ALL
            .into_iter()
            .any(|d| matches!(map.tile(pos.step(d)), TileKind::Door(_)));
        if adjacent_door {
            protected.push(pos);
        }
    }
    protected
}

/// Whether all protected tiles stay mutually connected through the room
/// interior if `candidate` becomes blocked.
fn placement_keeps_room_connected(
    room: &Room,
    blocked: &[Pos],
    candidate: Pos,
    protected: &[Pos],
) -> bool {
    let interior = interior_tiles(room);
    let open: Vec<Pos> = interior
        .iter()
        .copied()
        .filter(|p| *p != candidate && !blocked.contains(p))
        .collect();
    let Some(&start) = protected.first() else {
        return true;
    };
    if protected.contains(&candidate) {
        return false;
    }
    // BFS over open interior tiles.
    let mut frontier = vec![start];
    let mut seen = vec![start];
    while let Some(pos) = frontier.pop() {
        for dir in crate::geom::Dir4::ALL {
            let next = pos.step(dir);
            if open.contains(&next) && !seen.contains(&next) {
                seen.push(next);
                frontier.push(next);
            }
        }
    }
    protected.iter().all(|p| seen.contains(p))
}

fn place_furniture(
    template: &RoomTemplate,
    room: &Room,
    map: &GameMap,
    extraction: &[Pos],
    furniture: &mut Vec<Furniture>,
    rng: &mut Pcg32,
) {
    let protected = protected_tiles(room, map, extraction);
    let mut blocked: Vec<Pos> = Vec::new();
    let place_kind = |kind: FurnitureKind,
                      count: u32,
                      furniture: &mut Vec<Furniture>,
                      blocked: &mut Vec<Pos>,
                      rng: &mut Pcg32| {
        for _ in 0..count {
            let mut candidates: Vec<Pos> = interior_tiles(room)
                .into_iter()
                .filter(|p| !protected.contains(p) && !blocked.contains(p))
                .collect();
            let mut placed = false;
            for _ in 0..8 {
                if candidates.is_empty() {
                    break;
                }
                let pos = rng.take(&mut candidates);
                if placement_keeps_room_connected(room, blocked, pos, &protected) {
                    furniture.push(Furniture {
                        id: FurnitureId(furniture.len() as u32),
                        kind,
                        pos,
                        body: None,
                        disguise: None,
                        machine: None,
                        used: false,
                        drop_tile: None,
                    });
                    blocked.push(pos);
                    placed = true;
                    break;
                }
            }
            if !placed {
                break;
            }
        }
    };

    let containers = rng.range_inclusive(
        template.containers_min.into(),
        template.containers_max.into(),
    );
    place_kind(
        FurnitureKind::Container,
        containers,
        furniture,
        &mut blocked,
        rng,
    );
    let cover = rng.range_inclusive(template.low_cover_min.into(), template.low_cover_max.into());
    place_kind(FurnitureKind::LowCover, cover, furniture, &mut blocked, rng);
}

/// A free interior tile in `room` that keeps the room connected if
/// blocked: the shared placement rule for wardrobes and machines.
pub fn find_free_spot(
    room: &Room,
    map: &GameMap,
    extraction: &[Pos],
    furniture: &[Furniture],
    occupied: &[Pos],
    rng: &mut Pcg32,
) -> Option<Pos> {
    let protected = protected_tiles(room, map, extraction);
    let mut blocked: Vec<Pos> = furniture
        .iter()
        .filter(|f| f.pos.floor == room.floor && room.bounds.contains(f.pos.x, f.pos.y))
        .map(|f| f.pos)
        .collect();
    blocked.extend(
        occupied
            .iter()
            .copied()
            .filter(|p| p.floor == room.floor && room.bounds.contains(p.x, p.y)),
    );
    let mut candidates: Vec<Pos> = interior_tiles(room)
        .into_iter()
        .filter(|p| !protected.contains(p) && !blocked.contains(p))
        .collect();
    for _ in 0..12 {
        if candidates.is_empty() {
            return None;
        }
        let pos = rng.take(&mut candidates);
        if placement_keeps_room_connected(room, &blocked, pos, &protected) {
            return Some(pos);
        }
    }
    None
}

/// Adds a wardrobe holding `disguise` to `room` if a legal tile exists.
/// Used by the reachability proof when a disguise has no obtainable source.
pub fn insert_wardrobe(
    room: &Room,
    map: &GameMap,
    extraction: &[Pos],
    furniture: &mut Vec<Furniture>,
    disguise: &str,
    rng: &mut Pcg32,
) -> bool {
    let protected = protected_tiles(room, map, extraction);
    let blocked: Vec<Pos> = furniture
        .iter()
        .filter(|f| f.pos.floor == room.floor && room.bounds.contains(f.pos.x, f.pos.y))
        .map(|f| f.pos)
        .collect();
    let mut candidates: Vec<Pos> = interior_tiles(room)
        .into_iter()
        .filter(|p| !protected.contains(p) && !blocked.contains(p))
        .collect();
    for _ in 0..12 {
        if candidates.is_empty() {
            return false;
        }
        let pos = rng.take(&mut candidates);
        if placement_keeps_room_connected(room, &blocked, pos, &protected) {
            furniture.push(Furniture {
                id: FurnitureId(furniture.len() as u32),
                kind: FurnitureKind::Wardrobe,
                pos,
                body: None,
                disguise: Some(disguise.to_string()),
                machine: None,
                used: false,
                drop_tile: None,
            });
            return true;
        }
    }
    false
}

/// All waypoints of the given kinds in the given rooms.
pub fn waypoints_of_kinds<'a>(
    rooms: impl Iterator<Item = &'a Room>,
    kinds: &[WaypointKind],
) -> Vec<Waypoint> {
    let mut result = Vec::new();
    for room in rooms {
        for waypoint in &room.waypoints {
            if kinds.contains(&waypoint.kind) {
                result.push(*waypoint);
            }
        }
    }
    result
}
