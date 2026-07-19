//! The district engine: one recursive rule, three venue topologies.
//!
//! A **district** is a rectangle on one storey that owns a two-tile
//! corridor — its *spine* — along its top edge. Everything else the
//! district contains hangs off that spine as a **cell**: either a leaf
//! room, or a child district nested in the space below. Each cell is
//! reached through a single doorway punched in the spine's far wall, so
//! reaching anything inside a district means first standing in its
//! spine.
//!
//! That one rule is the whole engine. The topology comes from the shape
//! of the authored tree, never from code:
//!
//! * a **chain** (one child per level) is an *onion* — every tier's
//!   corridor must be crossed to reach the next, so the security
//!   gradient is enforced by construction;
//! * a **star** (several children on one spine) is a *festival* — a
//!   public spine with branching backstage;
//! * **siblings that are themselves chains** is an *archipelago* — one
//!   public plaza serving several self-contained fortresses.
//!
//! A district marked `own_storey` is laid out on its own floor instead
//! and reached by a stairwell from its parent's spine, which is how a
//! venue gains height without giving up the gradient.
//!
//! Rectangles only, no nesting of room bounds, and a fixed depth-first
//! traversal in authored order — so the RNG stream depends on the
//! recipe, never on iteration accidents.

use crate::data::{DistrictPattern, GameData, RoomTemplate, VenueSpec};
use crate::generator::layout::{Layout, LayoutError, finish_layout};
use crate::geom::{FloorId, Pos};
use crate::map::{DoorId, DoorState, GameMap, TileKind};
use crate::rng::Pcg32;
use crate::world::{Rect, Room, RoomId};

/// Rows a spine costs: two of corridor plus the wall below it.
const SPINE_COST: i16 = 3;
/// Smallest room depth worth carving below a spine.
const MIN_CELL_DEPTH: i16 = 3;

/// One expanded district: a pattern instance with its own rect.
struct Node {
    floor: FloorId,
    /// Room template indices, in authored order.
    rooms: Vec<usize>,
    children: Vec<usize>,
    own_storey: bool,
    locked_by: Option<String>,
    /// Filled during carving.
    rect: Rect,
    /// The spine's first row; the corridor is `spine_y..=spine_y + 1`.
    spine_y: i16,
}

/// Expands the authored pattern into a deterministic tree of nodes.
/// Depth-first in authored order, so the RNG stream is a function of the
/// recipe alone.
fn expand(
    data: &GameData,
    venue: &VenueSpec,
    pattern: &DistrictPattern,
    parent_floor: FloorId,
    next_floor: &mut FloorId,
    nodes: &mut Vec<Node>,
    rng: &mut Pcg32,
) -> Result<usize, LayoutError> {
    let floor = if pattern.own_storey {
        *next_floor += 1;
        if usize::from(*next_floor) >= usize::from(venue.floor_count) {
            return Err(LayoutError(format!(
                "venue '{}' asks for more storeys than it declares",
                venue.id
            )));
        }
        *next_floor
    } else {
        parent_floor
    };

    let rooms: Vec<usize> = pattern
        .rooms
        .iter()
        .filter_map(|id| data.rooms.iter().position(|t| &t.id == id))
        .collect();

    let index = nodes.len();
    nodes.push(Node {
        floor,
        rooms,
        children: Vec::new(),
        own_storey: pattern.own_storey,
        locked_by: pattern.locked_by.clone(),
        rect: Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
        spine_y: 0,
    });

    let mut children = Vec::new();
    for child in &pattern.children {
        if child.zone.depth() < pattern.zone.depth() {
            return Err(LayoutError(format!(
                "venue '{}': a {} district nests inside a {} one, which would let \
                 the security gradient run backwards",
                venue.id,
                child.zone.name(),
                pattern.zone.name(),
            )));
        }
        let count = rng.range_inclusive(child.count_min.into(), child.count_max.into());
        for _ in 0..count.max(1) {
            let id = expand(data, venue, child, floor, next_floor, nodes, rng)?;
            children.push(id);
        }
    }
    nodes[index].children = children;
    Ok(index)
}

/// The narrowest this district can be: enough for its widest room and
/// for every same-storey child beside it.
fn min_width(data: &GameData, nodes: &[Node], index: usize) -> i16 {
    let node = &nodes[index];
    let rooms: i16 = node
        .rooms
        .iter()
        .map(|t| i16::try_from(data.rooms[*t].min_size.0).unwrap() + 1)
        .sum();
    let kids: i16 = node
        .children
        .iter()
        .filter(|c| !nodes[**c].own_storey)
        .map(|c| min_width(data, nodes, *c) + 1)
        .sum();
    (rooms + kids).max(4)
}

/// The rows this district needs: its own spine plus the deepest of its
/// rooms and its same-storey children.
fn min_depth(data: &GameData, nodes: &[Node], index: usize) -> i16 {
    let node = &nodes[index];
    let rooms = node
        .rooms
        .iter()
        .map(|t| i16::try_from(data.rooms[*t].min_size.1).unwrap())
        .max()
        .unwrap_or(0);
    let kids = node
        .children
        .iter()
        .filter(|c| !nodes[**c].own_storey)
        .map(|c| min_depth(data, nodes, *c))
        .max()
        .unwrap_or(0);
    SPINE_COST + rooms.max(kids).max(MIN_CELL_DEPTH) + 1
}

/// Builds a venue from a district pattern.
pub fn build_layout(
    data: &GameData,
    venue: &VenueSpec,
    rng: &mut Pcg32,
) -> Result<Layout, LayoutError> {
    let pattern = &venue.districts;
    let mut nodes: Vec<Node> = Vec::new();
    let mut next_floor: FloorId = 0;
    let root = expand(data, venue, pattern, 0, &mut next_floor, &mut nodes, rng)?;

    let width = venue.floor_width as i16;
    let height = venue.floor_height as i16;
    let mut map = GameMap::filled_void(venue.floor_width, venue.floor_height, venue.floor_count);
    let mut doors: Vec<DoorState> = Vec::new();
    let mut rooms: Vec<Room> = Vec::new();

    // The root and every own-storey district get a whole storey's
    // interior, inset by the outer wall.
    let storey_rect = Rect {
        x: 1,
        y: 1,
        w: width - 2,
        h: height - 2,
    };
    for (index, node) in nodes.iter_mut().enumerate() {
        if index == root || node.own_storey {
            node.rect = storey_rect;
        }
    }

    // Carve the root, then every district that took a storey of its own.
    // Nested children recurse inside those. Index order is the expansion
    // order, so the RNG stream stays a function of the recipe.
    let mut stair_anchors: Vec<(usize, usize)> = Vec::new();
    for index in 0..nodes.len() {
        if index == root || nodes[index].own_storey {
            carve(
                data,
                &mut nodes,
                index,
                &mut map,
                &mut doors,
                &mut rooms,
                &mut stair_anchors,
                rng,
            )?;
        }
    }

    // Storey jumps: a stairwell from the parent's spine to the child's.
    for (parent, child) in stair_anchors {
        let up = spine_free_tile(&map, &nodes[parent], 2);
        let down = spine_free_tile(&map, &nodes[child], 3);
        match (up, down) {
            (Some(a), Some(b)) => {
                map.link_stairs(a, b);
            }
            _ => {
                return Err(LayoutError(
                    "no room in a spine for a stairwell".to_string(),
                ));
            }
        }
    }

    finish_layout(data, map, doors, rooms, rng)
}

/// A walkable spine tile at least `inset` columns in from the district's
/// west end, reserved for a stairwell.
fn spine_free_tile(map: &GameMap, node: &Node, inset: i16) -> Option<Pos> {
    for y in [node.spine_y, node.spine_y + 1] {
        for dx in 0..node.rect.w {
            let pos = Pos::new(node.floor, node.rect.x + inset + dx, y);
            if map.tile(pos) == TileKind::Floor {
                return Some(pos);
            }
        }
    }
    None
}

/// Carves one district and everything below it.
#[allow(clippy::too_many_arguments)]
fn carve(
    data: &GameData,
    nodes: &mut Vec<Node>,
    index: usize,
    map: &mut GameMap,
    doors: &mut Vec<DoorState>,
    rooms: &mut Vec<Room>,
    stair_anchors: &mut Vec<(usize, usize)>,
    rng: &mut Pcg32,
) -> Result<(), LayoutError> {
    let rect = nodes[index].rect;
    let floor = nodes[index].floor;
    if rect.w < 4 || rect.h < SPINE_COST + MIN_CELL_DEPTH {
        return Err(LayoutError(format!(
            "district on floor {floor} has no room to carve"
        )));
    }

    // The spine: two rows of corridor along the top of the district,
    // walled above and below.
    let spine_y = rect.y;
    nodes[index].spine_y = spine_y;
    for x in rect.x..(rect.x + rect.w) {
        map.set_tile(Pos::new(floor, x, spine_y), TileKind::Floor);
        map.set_tile(Pos::new(floor, x, spine_y + 1), TileKind::Floor);
        for wall_y in [spine_y - 1, spine_y + 2] {
            if map.tile(Pos::new(floor, x, wall_y)) == TileKind::Void {
                map.set_tile(Pos::new(floor, x, wall_y), TileKind::Wall);
            }
        }
    }
    for wall_x in [rect.x - 1, rect.x + rect.w] {
        for y in [spine_y, spine_y + 1] {
            let pos = Pos::new(floor, wall_x, y);
            if map.in_bounds(pos) && map.tile(pos) == TileKind::Void {
                map.set_tile(pos, TileKind::Wall);
            }
        }
    }

    // Everything below the spine wall is cell space.
    let cells_y = spine_y + SPINE_COST;
    let cells_h = rect.y + rect.h - cells_y;
    if cells_h < MIN_CELL_DEPTH {
        return Err(LayoutError(format!(
            "district on floor {floor} has no depth below its spine"
        )));
    }

    // Cells, in authored order: this district's rooms, then its
    // same-storey children. Own-storey children take a stairwell instead.
    let room_templates = nodes[index].rooms.clone();
    let children = nodes[index].children.clone();
    let same_storey: Vec<usize> = children
        .iter()
        .copied()
        .filter(|c| !nodes[*c].own_storey)
        .collect();
    for child in children.iter().copied() {
        if nodes[child].own_storey {
            stair_anchors.push((index, child));
        }
    }

    // Width budget: give every cell its minimum, then share the slack
    // left to right so the storey is filled rather than left ragged.
    let mut mins: Vec<i16> = Vec::new();
    for t in &room_templates {
        mins.push(i16::try_from(data.rooms[*t].min_size.0).unwrap());
    }
    for c in &same_storey {
        mins.push(min_width(data, nodes, *c));
    }
    if mins.is_empty() {
        return Ok(());
    }
    let walls = mins.len() as i16 - 1;
    let needed: i16 = mins.iter().sum::<i16>() + walls;
    if needed > rect.w {
        return Err(LayoutError(format!(
            "district on floor {floor} needs {needed} columns but has {}",
            rect.w
        )));
    }
    let mut slack = rect.w - needed;
    let mut left = mins.len() as i16;
    let widths: Vec<i16> = mins
        .iter()
        .map(|m| {
            let share = slack / left;
            slack -= share;
            left -= 1;
            m + share
        })
        .collect();

    // Lay the cells out left to right.
    let mut cursor = rect.x;
    for (slot, width) in widths.iter().enumerate() {
        let cell = Rect {
            x: cursor,
            y: cells_y,
            w: *width,
            h: cells_h,
        };
        cursor += width + 1;

        if slot < room_templates.len() {
            let template_index = room_templates[slot];
            place_room(
                data,
                map,
                doors,
                rooms,
                template_index,
                floor,
                cell,
                spine_y + 2,
                nodes[index].locked_by.clone(),
                rng,
            )?;
        } else {
            let child = same_storey[slot - room_templates.len()];
            let needed = min_depth(data, nodes, child);
            if cell.h < needed {
                return Err(LayoutError(format!(
                    "a district needs {needed} rows below the spine but has {}",
                    cell.h
                )));
            }
            nodes[child].rect = cell;
            // A gateway from this spine down into the child's spine.
            let gate_x = cell.x + 1 + rng.below((cell.w.max(3) - 2) as u32) as i16;
            let locked_by = nodes[child].locked_by.clone();
            let id = DoorId(doors.len() as u16);
            doors.push(DoorState {
                open: false,
                locked_by,
            });
            map.set_tile(Pos::new(floor, gate_x, spine_y + 2), TileKind::Door(id));
            carve(data, nodes, child, map, doors, rooms, stair_anchors, rng)?;
        }
    }
    Ok(())
}

/// Carves one leaf room into its cell and doors it onto the spine.
#[allow(clippy::too_many_arguments)]
fn place_room(
    data: &GameData,
    map: &mut GameMap,
    doors: &mut Vec<DoorState>,
    rooms: &mut Vec<Room>,
    template_index: usize,
    floor: FloorId,
    cell: Rect,
    door_wall_y: i16,
    district_lock: Option<String>,
    rng: &mut Pcg32,
) -> Result<(), LayoutError> {
    let template: &RoomTemplate = &data.rooms[template_index];
    let max_h = i16::try_from(template.max_size.1).unwrap().min(cell.h);
    let min_h = i16::try_from(template.min_size.1).unwrap();
    if min_h > max_h {
        return Err(LayoutError(format!(
            "room '{}' cannot fit a {}-deep cell",
            template.id, cell.h
        )));
    }
    let h = rng.range_inclusive(min_h as u32, max_h as u32) as i16;
    let bounds = Rect {
        x: cell.x,
        y: cell.y,
        w: cell.w,
        h,
    };

    for y in (bounds.y - 1)..=(bounds.y + bounds.h) {
        for x in (bounds.x - 1)..=(bounds.x + bounds.w) {
            let pos = Pos::new(floor, x, y);
            if !map.in_bounds(pos) {
                return Err(LayoutError(format!(
                    "room '{}' escapes the storey",
                    template.id
                )));
            }
            let interior = bounds.contains(x, y);
            if interior {
                map.set_tile(pos, TileKind::Floor);
            } else if map.tile(pos) == TileKind::Void {
                map.set_tile(pos, TileKind::Wall);
            }
        }
    }

    // One door onto the district's spine.
    let door_x = bounds.x + rng.below(bounds.w as u32) as i16;
    let id = DoorId(doors.len() as u16);
    doors.push(DoorState {
        open: false,
        locked_by: template.locked_by.clone().or(district_lock),
    });
    map.set_tile(Pos::new(floor, door_x, door_wall_y), TileKind::Door(id));
    // Connect the room up to that doorway if the room sits below it.
    for y in (door_wall_y + 1)..bounds.y {
        map.set_tile(Pos::new(floor, door_x, y), TileKind::Floor);
        for wx in [door_x - 1, door_x + 1] {
            let pos = Pos::new(floor, wx, y);
            if map.in_bounds(pos) && map.tile(pos) == TileKind::Void {
                map.set_tile(pos, TileKind::Wall);
            }
        }
    }

    rooms.push(Room {
        id: RoomId(rooms.len() as u16),
        template: template.id.clone(),
        name: template.name.clone(),
        zone: template.zone,
        floor,
        bounds,
        lighting: template.lighting,
        waypoints: Vec::new(),
        doors: vec![id],
        external_exit: template.external_exit,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Zone;

    fn hotel(data: &GameData) -> VenueSpec {
        data.venue("grand-hotel")
            .expect("the festival venue ships")
            .clone()
    }

    /// The deepest authored district, following the first child each time.
    fn innermost(pattern: &mut DistrictPattern) -> &mut DistrictPattern {
        if pattern.children.is_empty() {
            return pattern;
        }
        innermost(&mut pattern.children[0])
    }

    /// The gradient is the engine's one guarantee, so a recipe that would
    /// break it must be refused while it is still a tree — before any
    /// geometry exists to debug.
    #[test]
    fn a_district_may_not_nest_into_a_shallower_tier() {
        let data = GameData::embedded().unwrap();
        let mut venue = hotel(&data);
        // Hang a public district off the innermost tier: reaching the
        // street would mean going further in.
        let deepest = innermost(&mut venue.districts);
        let mut backwards = deepest.clone();
        backwards.zone = Zone::Public;
        backwards.children.clear();
        deepest.children.push(backwards);

        let mut rng = Pcg32::new(1, 1);
        let Err(err) = build_layout(&data, &venue, &mut rng) else {
            panic!("a backwards gradient must be refused");
        };
        assert!(
            err.0.contains("gradient"),
            "the error should name the gradient, got: {}",
            err.0
        );
    }

    /// Every leaf room is a cell hung off a spine, so it is entered
    /// through a door punched in its own boundary — never by opening onto
    /// another room.
    #[test]
    fn every_room_is_entered_through_its_own_boundary() {
        let data = GameData::embedded().unwrap();
        let venue = hotel(&data);
        for seed in 0..20u64 {
            let mut rng = Pcg32::new(seed, 7);
            let layout = build_layout(&data, &venue, &mut rng)
                .unwrap_or_else(|e| panic!("seed {seed}: {}", e.0));
            for room in &layout.rooms {
                assert!(
                    !room.doors.is_empty(),
                    "seed {seed}: room '{}' has no door",
                    room.name
                );
                // At least one door tile sits in the room's own wall ring.
                let b = room.bounds;
                let ring =
                    (b.x - 1..=b.x + b.w).flat_map(|x| (b.y - 1..=b.y + b.h).map(move |y| (x, y)));
                let has_door = ring.filter(|(x, y)| !b.contains(*x, *y)).any(|(x, y)| {
                    matches!(
                        layout.map.tile(Pos::new(room.floor, x, y)),
                        TileKind::Door(_)
                    )
                });
                assert!(
                    has_door,
                    "seed {seed}: room '{}' at {b:?} has no door in its own wall",
                    room.name
                );
            }
        }
    }
}
