//! Graph realisation: banded shelves on a two-corridor circulation loop.
//!
//! The grammar (see [`super::grammar`]) decides the whole room graph
//! first; this module only carves what the graph guarantees. Each storey
//! is realised as: outer wall, staff-tier service corridor along the
//! back, the service shelf of rooms, the public main corridor through
//! the middle, the main shelf of rooms, outer wall. Stub passages at the
//! west and east ends join the two corridors into a loop, stairs sit at
//! both dead ends of the main corridor, and the ground-floor service
//! corridor ends in a fire exit — a staff-space extraction path beside
//! the public entrance and the loading bay.

use crate::data::{GameData, Lighting, RoomTemplate, WaypointKind};
use crate::geom::Pos;
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
    pub stairs: Vec<Pos>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutError(pub String);

/// First corridor row of the main spine. Two tiles tall so passing
/// The shared tail of every realiser: waypoints, extraction tiles, and
/// furniture. Form-agnostic — it reads only the finished rooms and map,
/// so any topology can hand off to it.
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

    let stairs = map.stair_links().iter().map(|link| link.a).collect();
    Ok(Layout {
        map,
        doors,
        rooms,
        furniture,
        extraction_tiles,
        stairs,
    })
}

/// The interior tile on whichever side of the room lies closest to the
/// building's outer wall — the side the street is on.
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
    sides
        .into_iter()
        .min_by_key(|p| (edge_gap(p), p.y, p.x))
        .expect("four sides")
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

/// Lighting of the room containing `pos`, defaulting to bright corridors.
pub fn lighting_at(rooms: &[Room], pos: Pos) -> Lighting {
    rooms
        .iter()
        .find(|r| r.floor == pos.floor && r.bounds.contains(pos.x, pos.y))
        .map(|r| r.lighting)
        .unwrap_or(Lighting::Bright)
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
