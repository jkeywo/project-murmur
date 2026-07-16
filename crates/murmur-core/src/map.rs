//! The static tile map: two storeys of walls, floors, doors, and stairs.
//!
//! The map records what generation decided about terrain. Dynamic state
//! that changes during play (door open/closed, locks) lives in [`DoorState`]
//! records owned by the world alongside the map. Furniture, actors, and
//! items are world entities, not tiles; sight and movement queries accept
//! closures so callers decide how those entities block.

use serde::{Deserialize, Serialize};

use crate::geom::{FloorId, Pos, supercover_between};

/// Stable identifier of one door on the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DoorId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TileKind {
    /// Outside the building or otherwise nonexistent.
    Void,
    Wall,
    Floor,
    Door(DoorId),
    /// Stairs connect the same coordinates on both storeys: stepping onto a
    /// stairs tile carries the mover to the other floor (recorded decision;
    /// it keeps Move the only travel verb).
    Stairs,
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
        }
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
            TileKind::Floor | TileKind::Stairs => true,
            TileKind::Door(id) => door_open(id),
            TileKind::Wall | TileKind::Void => false,
        }
    }

    /// True when terrain at `pos` blocks sight (walls, closed doors, void).
    pub fn terrain_blocks_sight(&self, pos: Pos, door_open: impl Fn(DoorId) -> bool) -> bool {
        match self.tile(pos) {
            TileKind::Floor | TileKind::Stairs => false,
            TileKind::Door(id) => !door_open(id),
            TileKind::Wall | TileKind::Void => true,
        }
    }

    /// Where a mover ends up after stepping onto `pos`: stairs carry the
    /// mover to the matching tile on the other storey.
    pub fn resolve_step_destination(&self, pos: Pos) -> Pos {
        if self.tile(pos) == TileKind::Stairs && self.floors.len() == 2 {
            let other = Pos::new(if pos.floor == 0 { 1 } else { 0 }, pos.x, pos.y);
            if self.tile(other) == TileKind::Stairs {
                return other;
            }
        }
        pos
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
    /// `#` wall, `.` floor, `+` closed door, `'` open door, `<` stairs,
    /// space void.
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
                    '<' => TileKind::Stairs,
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
    fn stairs_transition_between_matching_tiles() {
        let mut map = GameMap::filled_void(3, 3, 2);
        map.set_tile(Pos::new(0, 1, 1), TileKind::Stairs);
        map.set_tile(Pos::new(1, 1, 1), TileKind::Stairs);
        assert_eq!(
            map.resolve_step_destination(Pos::new(0, 1, 1)),
            Pos::new(1, 1, 1)
        );
        assert_eq!(
            map.resolve_step_destination(Pos::new(1, 1, 1)),
            Pos::new(0, 1, 1)
        );
        // A lone stairs tile without a partner stays put.
        map.set_tile(Pos::new(1, 1, 1), TileKind::Wall);
        assert_eq!(
            map.resolve_step_destination(Pos::new(0, 1, 1)),
            Pos::new(0, 1, 1)
        );
    }

    #[test]
    fn walkability_by_tile_kind() {
        let (map, doors) = map_from_rows(&["#.+<"]);
        let open = |id: DoorId| doors[id.0 as usize].open;
        assert!(!map.walkable(Pos::new(0, 0, 0), open));
        assert!(map.walkable(Pos::new(0, 1, 0), open));
        assert!(!map.walkable(Pos::new(0, 2, 0), open), "closed door blocks");
        assert!(map.walkable(Pos::new(0, 3, 0), open));
        assert!(!map.walkable(Pos::new(0, 9, 0), open), "out of bounds");
    }

    #[test]
    fn dir4_marker_round_trip() {
        assert_eq!(Dir4::North.marker(), '^');
        assert_eq!(Dir4::East.marker(), '>');
        assert_eq!(Dir4::South.marker(), 'v');
        assert_eq!(Dir4::West.marker(), '<');
    }
}
