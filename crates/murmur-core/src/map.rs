//! The static tile map: two to four storeys of walls, floors, doors, and
//! stairs.
//!
//! The map records what generation decided about terrain. Dynamic state
//! that changes during play (door open/closed, locks) lives in [`DoorState`]
//! records owned by the world alongside the map. Furniture, actors, and
//! items are world entities, not tiles; sight and movement queries accept
//! closures so callers decide how those entities block.
//!
//! Stairs are *linked pairs* rather than matching coordinates, so a
//! stairwell can serve any number of storeys: the tile carries a
//! [`StairId`] and the map owns the [`StairLink`] naming both ends. A
//! middle storey therefore has a distinct tile for up and for down, which
//! coordinate-matching could never express.

use serde::{Deserialize, Serialize};

use crate::geom::{FloorId, Pos, supercover_between};

/// Stable identifier of one door on the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DoorId(pub u16);

/// Stable identifier of one stair link on the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StairId(pub u16);

/// The two tiles one stairwell step connects. Stepping onto either end
/// carries the mover to the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StairLink {
    pub a: Pos,
    pub b: Pos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TileKind {
    /// Outside the building or otherwise nonexistent.
    Void,
    Wall,
    Floor,
    Door(DoorId),
    /// One end of a stair link: stepping onto it carries the mover to the
    /// link's other end (recorded decision; it keeps Move the only travel
    /// verb).
    Stairs(StairId),
}

/// The current state of one door.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoorState {
    pub open: bool,
    /// Item id of the key required while the door is locked, if any.
    /// Locked doors cannot be opened without the key; unlocking is implicit
    /// when a key holder opens the door.
    pub locked_by: Option<String>,
}

/// One storey of tiles.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FloorGrid {
    tiles: Vec<TileKind>,
}

/// The full static map.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameMap {
    width: u16,
    height: u16,
    floors: Vec<FloorGrid>,
    /// Indexed by [`StairId`]; generation order, so deterministic.
    #[serde(default)]
    stair_links: Vec<StairLink>,
}

impl GameMap {
    /// Creates a map of `floor_count` storeys filled with void.
    pub fn filled_void(width: u16, height: u16, floor_count: u8) -> Self {
        let tiles = vec![TileKind::Void; usize::from(width) * usize::from(height)];
        Self {
            width,
            height,
            floors: (0..floor_count)
                .map(|_| FloorGrid {
                    tiles: tiles.clone(),
                })
                .collect(),
            stair_links: Vec::new(),
        }
    }

    /// Links two tiles into one stairwell step and marks both as stairs.
    /// Returns the new link's id.
    pub fn link_stairs(&mut self, a: Pos, b: Pos) -> StairId {
        debug_assert!(self.in_bounds(a) && self.in_bounds(b), "stair link off-map");
        let id = StairId(self.stair_links.len() as u16);
        self.stair_links.push(StairLink { a, b });
        self.set_tile(a, TileKind::Stairs(id));
        self.set_tile(b, TileKind::Stairs(id));
        id
    }

    pub fn stair_links(&self) -> &[StairLink] {
        &self.stair_links
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn floor_count(&self) -> u8 {
        self.floors.len() as u8
    }

    pub fn in_bounds(&self, pos: Pos) -> bool {
        usize::from(pos.floor) < self.floors.len()
            && pos.x >= 0
            && pos.y >= 0
            && pos.x < self.width as i16
            && pos.y < self.height as i16
    }

    fn index(&self, pos: Pos) -> usize {
        usize::try_from(pos.y).unwrap() * usize::from(self.width) + usize::try_from(pos.x).unwrap()
    }

    /// The tile at `pos`; out-of-bounds positions read as void.
    pub fn tile(&self, pos: Pos) -> TileKind {
        if !self.in_bounds(pos) {
            return TileKind::Void;
        }
        self.floors[usize::from(pos.floor)].tiles[self.index(pos)]
    }

    pub fn set_tile(&mut self, pos: Pos, tile: TileKind) {
        debug_assert!(self.in_bounds(pos), "set_tile out of bounds: {pos:?}");
        let index = self.index(pos);
        self.floors[usize::from(pos.floor)].tiles[index] = tile;
    }

    /// True when terrain alone permits standing on `pos`. Door passability
    /// depends on dynamic state, so callers supply the door lookup.
    pub fn walkable(&self, pos: Pos, door_open: impl Fn(DoorId) -> bool) -> bool {
        match self.tile(pos) {
            TileKind::Floor | TileKind::Stairs(_) => true,
            TileKind::Door(id) => door_open(id),
            TileKind::Wall | TileKind::Void => false,
        }
    }

    /// True when terrain at `pos` blocks sight (walls, closed doors, void).
    pub fn terrain_blocks_sight(&self, pos: Pos, door_open: impl Fn(DoorId) -> bool) -> bool {
        match self.tile(pos) {
            TileKind::Floor | TileKind::Stairs(_) => false,
            TileKind::Door(id) => !door_open(id),
            TileKind::Wall | TileKind::Void => true,
        }
    }

    /// Where a mover ends up after stepping onto `pos`: a stair tile
    /// carries the mover to its link's other end. A stair whose link is
    /// missing or malformed leaves the mover where they stand.
    pub fn resolve_step_destination(&self, pos: Pos) -> Pos {
        let TileKind::Stairs(id) = self.tile(pos) else {
            return pos;
        };
        let Some(link) = self.stair_links.get(usize::from(id.0)) else {
            return pos;
        };
        let other = if link.a == pos {
            link.b
        } else if link.b == pos {
            link.a
        } else {
            return pos;
        };
        if matches!(self.tile(other), TileKind::Stairs(other_id) if other_id == id) {
            other
        } else {
            pos
        }
    }

    /// All in-bounds positions of one floor in row-major (deterministic)
    /// order.
    pub fn floor_positions(&self, floor: FloorId) -> impl Iterator<Item = Pos> + '_ {
        let width = self.width as i16;
        let height = self.height as i16;
        (0..height).flat_map(move |y| (0..width).map(move |x| Pos::new(floor, x, y)))
    }
}

/// True when an unobstructed sight line exists between `from` and `to`.
///
/// Endpoints never block themselves; `blocks` is consulted for every tile
/// the ray passes through (terrain plus whatever entity rules the caller
/// adds, such as low cover against crouched endpoints). Sight never crosses
/// storeys.
pub fn line_of_sight(from: Pos, to: Pos, blocks: impl Fn(Pos) -> bool) -> bool {
    if from.floor != to.floor {
        return false;
    }
    supercover_between(from, to).iter().all(|pos| !blocks(*pos))
}

/// Every tile within `range` (Chebyshev) of `origin` that has line of
/// sight from it, in deterministic row-major order. Perception and player
/// field-of-view both build on this so they can never disagree about
/// sight lines.
pub fn tiles_visible_from(
    origin: Pos,
    range: i16,
    map: &GameMap,
    blocks: impl Fn(Pos) -> bool,
) -> Vec<Pos> {
    let mut visible = Vec::new();
    for y in (origin.y - range)..=(origin.y + range) {
        for x in (origin.x - range)..=(origin.x + range) {
            let target = Pos::new(origin.floor, x, y);
            if !map.in_bounds(target) {
                continue;
            }
            if line_of_sight(origin, target, &blocks) {
                visible.push(target);
            }
        }
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Dir4;

    /// Builds a single-floor map from rows of characters:
    /// `#` wall, `.` floor, `+` closed door, `'` open door, space void.
    /// Stairs need a partner tile, so they are linked with
    /// [`GameMap::link_stairs`] rather than drawn.
    pub fn map_from_rows(rows: &[&str]) -> (GameMap, Vec<DoorState>) {
        let height = rows.len() as u16;
        let width = rows.iter().map(|r| r.len()).max().unwrap_or(0) as u16;
        let mut map = GameMap::filled_void(width, height, 1);
        let mut doors = Vec::new();
        for (y, row) in rows.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                let pos = Pos::new(0, x as i16, y as i16);
                let tile = match ch {
                    '#' => TileKind::Wall,
                    '.' => TileKind::Floor,
                    '+' | '\'' => {
                        doors.push(DoorState {
                            open: ch == '\'',
                            locked_by: None,
                        });
                        TileKind::Door(DoorId(doors.len() as u16 - 1))
                    }
                    _ => TileKind::Void,
                };
                map.set_tile(pos, tile);
            }
        }
        (map, doors)
    }

    fn terrain_blocker<'a>(map: &'a GameMap, doors: &'a [DoorState]) -> impl Fn(Pos) -> bool + 'a {
        move |pos| map.terrain_blocks_sight(pos, |id| doors[id.0 as usize].open)
    }

    #[test]
    fn walls_block_sight_and_floors_do_not() {
        let (map, doors) = map_from_rows(&["#####", "#...#", "#.#.#", "#...#", "#####"]);
        let blocks = terrain_blocker(&map, &doors);
        assert!(line_of_sight(Pos::new(0, 1, 1), Pos::new(0, 3, 1), &blocks));
        assert!(!line_of_sight(
            Pos::new(0, 1, 2),
            Pos::new(0, 3, 2),
            &blocks
        ));
    }

    #[test]
    fn closed_doors_block_sight_and_open_doors_do_not() {
        let (map, mut doors) = map_from_rows(&["#####", "#.+.#", "#####"]);
        {
            let blocks = terrain_blocker(&map, &doors);
            assert!(!line_of_sight(
                Pos::new(0, 1, 1),
                Pos::new(0, 3, 1),
                &blocks
            ));
        }
        doors[0].open = true;
        let blocks = terrain_blocker(&map, &doors);
        assert!(line_of_sight(Pos::new(0, 1, 1), Pos::new(0, 3, 1), &blocks));
    }

    #[test]
    fn sight_does_not_slip_between_diagonal_wall_corners() {
        let (map, doors) = map_from_rows(&["#####", "#.#.#", "##..#", "#####"]);
        let blocks = terrain_blocker(&map, &doors);
        assert!(!line_of_sight(
            Pos::new(0, 1, 1),
            Pos::new(0, 2, 2),
            &blocks
        ));
    }

    #[test]
    fn sight_never_crosses_storeys() {
        let _map = GameMap::filled_void(4, 4, 2);
        assert!(!line_of_sight(Pos::new(0, 1, 1), Pos::new(1, 1, 1), |_| {
            false
        }));
    }

    #[test]
    fn visible_tiles_respect_range_and_walls() {
        let (map, doors) = map_from_rows(&["#######", "#..#..#", "#######"]);
        let blocks = terrain_blocker(&map, &doors);
        let visible = tiles_visible_from(Pos::new(0, 1, 1), 5, &map, &blocks);
        assert!(visible.contains(&Pos::new(0, 2, 1)));
        assert!(visible.contains(&Pos::new(0, 3, 1)), "first wall is seen");
        assert!(
            !visible.contains(&Pos::new(0, 4, 1)),
            "tiles behind the wall are not"
        );
    }

    #[test]
    fn stairs_transition_between_linked_tiles() {
        let mut map = GameMap::filled_void(3, 3, 2);
        map.link_stairs(Pos::new(0, 1, 1), Pos::new(1, 1, 1));
        assert_eq!(
            map.resolve_step_destination(Pos::new(0, 1, 1)),
            Pos::new(1, 1, 1)
        );
        assert_eq!(
            map.resolve_step_destination(Pos::new(1, 1, 1)),
            Pos::new(0, 1, 1)
        );
        // A stair whose partner was overwritten stays put.
        map.set_tile(Pos::new(1, 1, 1), TileKind::Wall);
        assert_eq!(
            map.resolve_step_destination(Pos::new(0, 1, 1)),
            Pos::new(0, 1, 1)
        );
    }

    /// A middle storey needs a separate tile for up and for down, which
    /// the old coordinate-matching model could not express.
    #[test]
    fn stairs_link_three_storeys_with_distinct_up_and_down_tiles() {
        let mut map = GameMap::filled_void(4, 4, 3);
        // Ground up -> first down; first up -> second down.
        map.link_stairs(Pos::new(0, 1, 1), Pos::new(1, 1, 2));
        map.link_stairs(Pos::new(1, 1, 1), Pos::new(2, 1, 2));

        // Climb the whole building and come back down.
        let first = map.resolve_step_destination(Pos::new(0, 1, 1));
        assert_eq!(first, Pos::new(1, 1, 2));
        let second = map.resolve_step_destination(Pos::new(1, 1, 1));
        assert_eq!(second, Pos::new(2, 1, 2));
        assert_eq!(
            map.resolve_step_destination(second),
            Pos::new(1, 1, 1),
            "the top storey's down tile returns to the first"
        );
        assert_eq!(
            map.resolve_step_destination(first),
            Pos::new(0, 1, 1),
            "the first storey's down tile returns to the ground"
        );
        // Every stair tile is walkable terrain.
        for link in map.stair_links() {
            assert!(map.walkable(link.a, |_| true));
            assert!(map.walkable(link.b, |_| true));
        }
    }

    #[test]
    fn walkability_by_tile_kind() {
        let (map, doors) = map_from_rows(&["#.+."]);
        let open = |id: DoorId| doors[id.0 as usize].open;
        assert!(!map.walkable(Pos::new(0, 0, 0), open));
        assert!(map.walkable(Pos::new(0, 1, 0), open));
        assert!(!map.walkable(Pos::new(0, 2, 0), open), "closed door blocks");
        assert!(map.walkable(Pos::new(0, 3, 0), open));
        assert!(!map.walkable(Pos::new(0, 9, 0), open), "out of bounds");

        // Stairs are walkable terrain too, but they come in linked pairs
        // rather than being drawn as a lone glyph.
        let mut stairwell = GameMap::filled_void(2, 2, 2);
        stairwell.link_stairs(Pos::new(0, 0, 0), Pos::new(1, 0, 1));
        assert!(stairwell.walkable(Pos::new(0, 0, 0), open));
        assert!(stairwell.walkable(Pos::new(1, 0, 1), open));
    }

    #[test]
    fn dir4_marker_round_trip() {
        assert_eq!(Dir4::North.marker(), '^');
        assert_eq!(Dir4::East.marker(), '>');
        assert_eq!(Dir4::South.marker(), 'v');
        assert_eq!(Dir4::West.marker(), '<');
    }
}
