//! Room-graph-first layout.
//!
//! The generator decides rooms — identity, zone, storey, size, lighting,
//! waypoints, containers, cover — before any tile exists, then realises
//! them into the grid. Each storey uses a corridor spine down the middle
//! with rooms packed on shelves either side, which guarantees by
//! construction that every room opens onto connected circulation space;
//! the separate reachability proof then verifies it rather than hoping.

use crate::data::{GameData, Lighting, RoomTemplate, WaypointKind};
use crate::geom::{FloorId, Pos};
use crate::map::{DoorId, DoorState, GameMap, TileKind};
use crate::rng::Pcg32;
use crate::world::{Furniture, FurnitureId, FurnitureKind, Rect, Room, RoomId, Waypoint};

/// A room the generator has decided on but not yet realised into tiles.
struct PlannedRoom {
    template_index: usize,
    name: String,
    floor: FloorId,
    width: i16,
    height: i16,
}

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

/// First corridor row for a floor of the given interior height. The spine
/// is two tiles tall (rows `spine_y` and `spine_y + 1`) so passing traffic
/// can flow without deadlocking in a single-file hallway.
fn spine_y(height: u16) -> i16 {
    height as i16 / 2
}

pub fn build_layout(data: &GameData, rng: &mut Pcg32) -> Result<Layout, LayoutError> {
    let width = data.tuning.floor_width;
    let height = data.tuning.floor_height;
    let floor_count = data.tuning.floor_count;

    let planned = plan_rooms(data, rng)?;
    let mut map = GameMap::filled_void(width, height, floor_count);
    let mut doors: Vec<DoorState> = Vec::new();
    let mut rooms: Vec<Room> = Vec::new();

    // Carve the two-tile corridor spine and its walls on each storey.
    for floor in 0..floor_count {
        let sy = spine_y(height);
        for x in 1..(width as i16 - 1) {
            map.set_tile(Pos::new(floor, x, sy), TileKind::Floor);
            map.set_tile(Pos::new(floor, x, sy + 1), TileKind::Floor);
            map.set_tile(Pos::new(floor, x, sy - 1), TileKind::Wall);
            map.set_tile(Pos::new(floor, x, sy + 2), TileKind::Wall);
        }
        for y in [sy, sy + 1] {
            map.set_tile(Pos::new(floor, 0, y), TileKind::Wall);
            map.set_tile(Pos::new(floor, width as i16 - 1, y), TileKind::Wall);
        }
    }

    // Pack rooms onto the two shelves of each storey.
    let mut shelf_cursor = vec![[1i16, 1i16]; usize::from(floor_count)]; // [top, bottom] x cursors
    for plan in &planned {
        place_room(
            data,
            &mut map,
            &mut doors,
            &mut rooms,
            &mut shelf_cursor,
            plan,
            rng,
        )?;
    }

    // Truncate the corridor just past the last room so the building ends
    // where its rooms do instead of trailing a long empty hallway. The end
    // column is shared across storeys so the stairwells align.
    let east_end: i16 = shelf_cursor
        .iter()
        .map(|c| c[0].max(c[1]))
        .max()
        .unwrap_or(1)
        .min(width as i16 - 2);
    for floor in 0..floor_count {
        let sy = spine_y(height);
        map.set_tile(Pos::new(floor, east_end + 1, sy), TileKind::Wall);
        map.set_tile(Pos::new(floor, east_end + 1, sy + 1), TileKind::Wall);
        for x in (east_end + 2)..(width as i16) {
            for y in [sy - 1, sy, sy + 1, sy + 2] {
                map.set_tile(Pos::new(floor, x, y), TileKind::Void);
            }
        }
    }

    // Stairs occupy the dead-end tiles at both ends of the spine, so
    // nobody changes storeys by accident while walking the corridor.
    let mut stairs = Vec::new();
    if floor_count == 2 {
        let sy = spine_y(height);
        for x in [1i16, east_end] {
            for y in [sy, sy + 1] {
                for floor in 0..floor_count {
                    map.set_tile(Pos::new(floor, x, y), TileKind::Stairs);
                }
            }
            stairs.push(Pos::new(0, x, sy));
        }
    }

    // Waypoints before furniture so lingering spots stay clear.
    for room in &mut rooms {
        let template = &data.rooms[template_index_of(data, &room.template)];
        room.waypoints = pick_waypoints(template, room, rng);
    }

    // Extraction exits: the interior tile of each external-exit room
    // nearest the outer wall.
    let mut extraction_tiles = Vec::new();
    for room in &rooms {
        if room.external_exit {
            let b = room.bounds;
            let y = if b.y < spine_y(height) {
                b.y
            } else {
                b.y + b.h - 1
            };
            extraction_tiles.push(Pos::new(room.floor, b.x + b.w / 2, y));
        }
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
        stairs,
    })
}

fn template_index_of(data: &GameData, id: &str) -> usize {
    data.rooms
        .iter()
        .position(|t| t.id == id)
        .expect("room template ids are validated at data load")
}

/// Decide the room list: counts, floors, and sizes, before any tiles.
fn plan_rooms(data: &GameData, rng: &mut Pcg32) -> Result<Vec<PlannedRoom>, LayoutError> {
    let height = data.tuning.floor_height;
    let shelf_h = shelf_heights(height);
    let mut planned = Vec::new();
    for (template_index, template) in data.rooms.iter().enumerate() {
        let count = rng.range_inclusive(template.count_min.into(), template.count_max.into());
        let count = if template.required {
            count.max(1)
        } else {
            count
        };
        for instance in 0..count {
            let floor = *rng.pick(&template.floors);
            let max_h = i16::try_from(template.max_size.1).unwrap().min(shelf_h);
            let min_h = i16::try_from(template.min_size.1).unwrap();
            if min_h > max_h {
                return Err(LayoutError(format!(
                    "room '{}' cannot fit a shelf of height {shelf_h}",
                    template.id
                )));
            }
            let name = if count > 1 {
                format!("{} {}", template.name, instance + 1)
            } else {
                template.name.clone()
            };
            planned.push(PlannedRoom {
                template_index,
                name,
                floor,
                width: rng.range_inclusive(template.min_size.0.into(), template.max_size.0.into())
                    as i16,
                height: rng.range_inclusive(min_h as u32, max_h as u32) as i16,
            });
        }
    }
    // Widest rooms first pack more reliably; ties keep authored order.
    planned.sort_by_key(|p| -(p.width));
    Ok(planned)
}

/// The shared shelf height for a storey (the smaller of the two shelves).
fn shelf_heights(height: u16) -> i16 {
    let sy = spine_y(height);
    // Top shelf: rows 1..=sy-2; bottom shelf: rows sy+3..=height-2.
    (sy - 2).min(height as i16 - sy - 4)
}

fn place_room(
    data: &GameData,
    map: &mut GameMap,
    doors: &mut Vec<DoorState>,
    rooms: &mut Vec<Room>,
    shelf_cursor: &mut [[i16; 2]],
    plan: &PlannedRoom,
    rng: &mut Pcg32,
) -> Result<(), LayoutError> {
    let template = &data.rooms[plan.template_index];
    let width_limit = map.width() as i16 - 1;
    let sy = spine_y(map.height());
    let cursors = &mut shelf_cursor[usize::from(plan.floor)];

    // Choose the emptier shelf that still fits; try to shrink if neither.
    let mut room_w = plan.width;
    let shelf = loop {
        let top_fits = cursors[0] + room_w < width_limit;
        let bottom_fits = cursors[1] + room_w < width_limit;
        match (top_fits, bottom_fits) {
            (true, true) => break usize::from(cursors[0] > cursors[1]),
            (true, false) => break 0,
            (false, true) => break 1,
            (false, false) => {
                if room_w > i16::try_from(template.min_size.0).unwrap() {
                    room_w -= 1;
                } else {
                    return Err(LayoutError(format!(
                        "no shelf space left for room '{}' on floor {}",
                        template.id, plan.floor
                    )));
                }
            }
        }
    };

    let x0 = cursors[shelf];
    cursors[shelf] = x0 + room_w + 1;

    // Interior anchored against the corridor-side wall.
    let bounds = if shelf == 0 {
        Rect {
            x: x0,
            y: sy - 1 - plan.height,
            w: room_w,
            h: plan.height,
        }
    } else {
        Rect {
            x: x0,
            y: sy + 3,
            w: room_w,
            h: plan.height,
        }
    };

    // Carve interior and surrounding walls.
    for y in (bounds.y - 1)..=(bounds.y + bounds.h) {
        for x in (bounds.x - 1)..=(bounds.x + bounds.w) {
            let pos = Pos::new(plan.floor, x, y);
            if !map.in_bounds(pos) {
                return Err(LayoutError(format!(
                    "room '{}' escapes the grid at {pos:?}",
                    template.id
                )));
            }
            let interior = bounds.contains(x, y);
            let tile = if interior {
                TileKind::Floor
            } else {
                TileKind::Wall
            };
            // Never overwrite the corridor or an existing door.
            if !matches!(map.tile(pos), TileKind::Door(_)) && map.tile(pos) != TileKind::Floor
                || interior
            {
                map.set_tile(pos, tile);
            }
        }
    }

    // Door(s) through the corridor-side wall.
    let door_wall_y = if shelf == 0 { sy - 1 } else { sy + 2 };
    let mut door_ids = Vec::new();
    let mut door_xs = vec![bounds.x + rng.below(bounds.w as u32) as i16];
    if bounds.w >= 6 && rng.chance(1, 4) {
        let second = bounds.x + rng.below(bounds.w as u32) as i16;
        if (second - door_xs[0]).abs() >= 2 {
            door_xs.push(second);
        }
    }
    for door_x in door_xs {
        let id = DoorId(doors.len() as u16);
        doors.push(DoorState {
            open: false,
            locked_by: template.locked_by.clone(),
        });
        map.set_tile(
            Pos::new(plan.floor, door_x, door_wall_y),
            TileKind::Door(id),
        );
        door_ids.push(id);
    }

    rooms.push(Room {
        id: RoomId(rooms.len() as u16),
        template: template.id.clone(),
        name: plan.name.clone(),
        zone: template.zone,
        floor: plan.floor,
        bounds,
        lighting: template.lighting,
        waypoints: Vec::new(),
        doors: door_ids,
        external_exit: template.external_exit,
    });
    Ok(())
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
