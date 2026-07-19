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
use crate::generator::grammar::{self, RoomNode, Shelf, VenueGraph};
use crate::geom::{FloorId, Pos};
use crate::map::{DoorId, DoorState, GameMap, TileKind};
use crate::rng::Pcg32;
use crate::world::{Furniture, FurnitureId, FurnitureKind, Rect, Room, RoomId, Waypoint};

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
/// traffic can flow without deadlocking.
fn spine_y(height: u16) -> i16 {
    height as i16 / 2
}

/// Packing cursors and geometry for one storey during realisation.
struct FloorRealisation {
    /// Next free x per shelf.
    service_cursor: i16,
    main_cursor: i16,
    /// Placed room indices (into `rooms`) per shelf, west to east.
    service_rooms: Vec<usize>,
    main_rooms: Vec<usize>,
}

/// Builds a venue's tiles with whichever realiser its form selects.
pub fn build_layout(
    data: &GameData,
    venue: &crate::data::VenueSpec,
    rng: &mut Pcg32,
) -> Result<Layout, LayoutError> {
    match &venue.form {
        crate::data::Form::Banded => {
            let graph = grammar::build_graph(data, venue, rng).map_err(LayoutError)?;
            realise(data, venue, &graph, rng)
        }
        crate::data::Form::Districts(pattern) => {
            super::district::build_layout(data, venue, pattern, rng)
        }
    }
}

/// Realises a venue graph into tiles.
fn realise(
    data: &GameData,
    venue: &crate::data::VenueSpec,
    graph: &VenueGraph,
    rng: &mut Pcg32,
) -> Result<Layout, LayoutError> {
    let width = venue.floor_width;
    let height = venue.floor_height;
    let floor_count = venue.floor_count;
    let sy = spine_y(height);

    let mut map = GameMap::filled_void(width, height, floor_count);
    let mut doors: Vec<DoorState> = Vec::new();
    let mut rooms: Vec<Room> = Vec::new();

    // Main corridor spine and its walls, full width (truncated later).
    for floor in 0..floor_count {
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

    // Pack each storey's shelves in graph order. Service shelf leaves
    // room for the west stub (columns 3-4, walls at 2 and 5).
    let mut realisations: Vec<FloorRealisation> = Vec::new();
    for plan in &graph.floors {
        let mut real = FloorRealisation {
            service_cursor: 6,
            main_cursor: 3,
            service_rooms: Vec::new(),
            main_rooms: Vec::new(),
        };
        for node in &plan.service_shelf {
            let index = place_room(data, &mut map, &mut doors, &mut rooms, &mut real, node, rng)?;
            real.service_rooms.push(index);
        }
        for node in &plan.main_shelf {
            let index = place_room(data, &mut map, &mut doors, &mut rooms, &mut real, node, rng)?;
            real.main_rooms.push(index);
        }
        realisations.push(real);
    }

    // Service corridor, stubs, and the shared east end.
    let mut east_end: i16 = 4;
    let mut service_ends: Vec<i16> = Vec::new();
    for real in &realisations {
        let sx = real.service_cursor; // east stub columns: sx, sx+1
        service_ends.push(sx);
        east_end = east_end.max(real.main_cursor).max(sx + 2);
    }
    let east_end = east_end.min(width as i16 - 2);

    for (floor, real) in realisations.iter().enumerate() {
        let floor = floor as FloorId;
        let sx = service_ends[usize::from(floor)];

        // Corridor interior (rows 1-2) and its walls (rows 0 and 3);
        // row 3 only fills gaps rooms have not walled already.
        for x in 1..=(sx + 1) {
            map.set_tile(Pos::new(floor, x, 1), TileKind::Floor);
            map.set_tile(Pos::new(floor, x, 2), TileKind::Floor);
            map.set_tile(Pos::new(floor, x, 0), TileKind::Wall);
            if map.tile(Pos::new(floor, x, 3)) == TileKind::Void {
                map.set_tile(Pos::new(floor, x, 3), TileKind::Wall);
            }
        }
        for y in 0..=2 {
            map.set_tile(Pos::new(floor, 0, y), TileKind::Wall);
        }
        // East wall of the service corridor and stub.
        for y in 0..=(sy - 1) {
            map.set_tile(Pos::new(floor, sx + 2, y), TileKind::Wall);
        }

        // Stubs join the corridors into a loop: west at columns 3-4,
        // east at the service corridor's end.
        for (x0, wall_w, wall_e) in [(3i16, 2i16, 5i16), (sx, sx - 1, sx + 2)] {
            for y in 3..=(sy - 1) {
                for x in [x0, x0 + 1] {
                    map.set_tile(Pos::new(floor, x, y), TileKind::Floor);
                }
                for x in [wall_w, wall_e] {
                    if map.tile(Pos::new(floor, x, y)) == TileKind::Void {
                        map.set_tile(Pos::new(floor, x, y), TileKind::Wall);
                    }
                }
            }
        }

        // Service connections: a back-of-house route from the service
        // corridor into each service-access room. The room is anchored
        // against the main corridor, so its north wall sits below the
        // corridor with a void gap between; carve a walled passage down
        // that gap and hang the door flush with the corridor wall.
        let plan = &graph.floors[usize::from(floor)];
        for (node, room_index) in plan.service_shelf.iter().zip(&real.service_rooms) {
            if !node.service_door {
                continue;
            }
            let room_bounds = rooms[*room_index].bounds;
            if room_bounds.w < 3 {
                // Too narrow to carry a passage without breaching a wall.
                continue;
            }
            // A column strictly inside the room, so the passage walls
            // stay within the room's own (void) airspace above it.
            let door_x = room_bounds.x + 1 + rng.below((room_bounds.w - 2) as u32) as i16;
            let id = DoorId(doors.len() as u16);
            doors.push(DoorState {
                open: false,
                locked_by: data.rooms[node.template_index].locked_by.clone(),
            });
            map.set_tile(Pos::new(floor, door_x, 3), TileKind::Door(id));
            // Drop the passage to the room's north wall, walling its sides.
            for y in 4..=(room_bounds.y - 1) {
                map.set_tile(Pos::new(floor, door_x, y), TileKind::Floor);
                for wx in [door_x - 1, door_x + 1] {
                    if map.tile(Pos::new(floor, wx, y)) == TileKind::Void {
                        map.set_tile(Pos::new(floor, wx, y), TileKind::Wall);
                    }
                }
            }
            rooms[*room_index].doors.push(id);
        }

        // Pass-through doors between consecutive restricted neighbours.
        for (shelf_rooms, passes, door_y) in [
            (&real.service_rooms, &plan.service_pass, sy - 2),
            (&real.main_rooms, &plan.main_pass, sy + 3),
        ] {
            for (i, has_door) in passes.iter().enumerate() {
                if !has_door {
                    continue;
                }
                let left = &rooms[shelf_rooms[i]];
                let right = &rooms[shelf_rooms[i + 1]];
                let wall_x = right.bounds.x - 1;
                let locked_by = data.rooms[graph_template(data, right)]
                    .locked_by
                    .clone()
                    .or_else(|| data.rooms[graph_template(data, left)].locked_by.clone());
                let id = DoorId(doors.len() as u16);
                doors.push(DoorState {
                    open: false,
                    locked_by,
                });
                map.set_tile(Pos::new(floor, wall_x, door_y), TileKind::Door(id));
                // The deeper room owns the door so NPC implicit keys
                // resolve against the stricter zone.
                let owner = shelf_rooms[i + 1];
                rooms[owner].doors.push(id);
            }
        }

        // The service corridor is a staff-tier room: access rules, zone
        // tinting, and waypoints all come from its template.
        let template = &data.rooms[graph.circulation_template];
        rooms.push(Room {
            id: RoomId(rooms.len() as u16),
            template: template.id.clone(),
            name: if floor == 0 {
                template.name.clone()
            } else {
                format!("upper {}", template.name)
            },
            zone: template.zone,
            floor,
            bounds: Rect {
                x: 1,
                y: 1,
                w: sx + 1,
                h: 2,
            },
            lighting: template.lighting,
            waypoints: Vec::new(),
            doors: Vec::new(),
            external_exit: template.external_exit && floor == 0,
        });
    }

    // Truncate the main corridor past the last construction and seal it.
    for floor in 0..floor_count {
        map.set_tile(Pos::new(floor, east_end + 1, sy), TileKind::Wall);
        map.set_tile(Pos::new(floor, east_end + 1, sy + 1), TileKind::Wall);
        for x in (east_end + 2)..(width as i16) {
            for y in [sy - 1, sy, sy + 1, sy + 2] {
                if map.tile(Pos::new(floor, x, y)) == TileKind::Floor {
                    map.set_tile(Pos::new(floor, x, y), TileKind::Void);
                }
            }
        }
    }

    // Stairwells at both dead ends of the spine: two vertical routes.
    // Each storey uses a distinct tile to go up (row `sy`) and to come
    // down (row `sy + 1`), so a stairwell can serve any number of
    // storeys — a middle floor needs both and they cannot share a tile.
    let mut stairs = Vec::new();
    for x in [1i16, east_end] {
        for floor in 0..floor_count.saturating_sub(1) {
            map.link_stairs(Pos::new(floor, x, sy), Pos::new(floor + 1, x, sy + 1));
        }
        stairs.push(Pos::new(0, x, sy));
    }

    // Waypoints before furniture so lingering spots stay clear.
    for room in &mut rooms {
        let template = &data.rooms[template_index_of(data, &room.template)];
        room.waypoints = pick_waypoints(template, room, rng);
    }

    // Extraction exits: public rooms first (the entrance is the player's
    // spawn), then restricted exits, then the service fire exit.
    let mut extraction_tiles = Vec::new();
    let mut ordered: Vec<&Room> = rooms.iter().filter(|r| r.external_exit).collect();
    ordered.sort_by_key(|r| {
        let template = &data.rooms[template_index_of(data, &r.template)];
        (
            template.circulation,
            r.zone != crate::data::Zone::Public,
            r.id.0,
        )
    });
    for room in ordered {
        let b = room.bounds;
        let template = &data.rooms[template_index_of(data, &room.template)];
        let tile = if template.circulation {
            // Fire exit at the far east end of the service corridor.
            Pos::new(room.floor, b.x + b.w - 1, b.y)
        } else if b.y < sy {
            Pos::new(room.floor, b.x + b.w / 2, b.y)
        } else {
            Pos::new(room.floor, b.x + b.w / 2, b.y + b.h - 1)
        };
        extraction_tiles.push(tile);
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

fn graph_template(data: &GameData, room: &Room) -> usize {
    template_index_of(data, &room.template)
}

fn template_index_of(data: &GameData, id: &str) -> usize {
    data.rooms
        .iter()
        .position(|t| t.id == id)
        .expect("room template ids are validated at data load")
}

/// Carves one room onto its shelf and punches its main-corridor door(s).
/// Returns the index of the new room record.
fn place_room(
    data: &GameData,
    map: &mut GameMap,
    doors: &mut Vec<DoorState>,
    rooms: &mut Vec<Room>,
    real: &mut FloorRealisation,
    node: &RoomNode,
    rng: &mut Pcg32,
) -> Result<usize, LayoutError> {
    let template = &data.rooms[node.template_index];
    let width_limit = map.width() as i16 - 1;
    let sy = spine_y(map.height());
    let cursor = match node.shelf {
        Shelf::Service => &mut real.service_cursor,
        Shelf::Main => &mut real.main_cursor,
    };

    // Shrink toward the minimum if the shelf is running out; the grammar
    // fit check makes minimum sizes always land.
    let mut room_w = node.width;
    // Reserve two columns plus a wall for the east stub on the service
    // shelf.
    let reserve = match node.shelf {
        Shelf::Service => 3,
        Shelf::Main => 0,
    };
    while *cursor + room_w + reserve >= width_limit {
        if room_w > i16::try_from(template.min_size.0).unwrap() {
            room_w -= 1;
        } else {
            return Err(LayoutError(format!(
                "no shelf space left for room '{}' on floor {}",
                template.id, node.floor
            )));
        }
    }

    let x0 = *cursor;
    *cursor = x0 + room_w + 1;

    // Interior anchored against the main-corridor-side wall.
    let bounds = match node.shelf {
        Shelf::Service => Rect {
            x: x0,
            y: sy - 1 - node.height,
            w: room_w,
            h: node.height,
        },
        Shelf::Main => Rect {
            x: x0,
            y: sy + 3,
            w: room_w,
            h: node.height,
        },
    };

    // Carve interior and surrounding walls.
    for y in (bounds.y - 1)..=(bounds.y + bounds.h) {
        for x in (bounds.x - 1)..=(bounds.x + bounds.w) {
            let pos = Pos::new(node.floor, x, y);
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
            // Never overwrite corridors or an existing door.
            if !matches!(map.tile(pos), TileKind::Door(_)) && map.tile(pos) != TileKind::Floor
                || interior
            {
                map.set_tile(pos, tile);
            }
        }
    }

    // Door(s) through the main-corridor-side wall.
    let door_wall_y = match node.shelf {
        Shelf::Service => sy - 1,
        Shelf::Main => sy + 2,
    };
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
            Pos::new(node.floor, door_x, door_wall_y),
            TileKind::Door(id),
        );
        door_ids.push(id);
    }

    rooms.push(Room {
        id: RoomId(rooms.len() as u16),
        template: template.id.clone(),
        name: node.name.clone(),
        zone: template.zone,
        floor: node.floor,
        bounds,
        lighting: template.lighting,
        waypoints: Vec::new(),
        doors: door_ids,
        external_exit: template.external_exit,
    });
    Ok(rooms.len() - 1)
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
