//! Positions, directions, and integer geometry.
//!
//! Murmur uses 4-way movement and 4-way facing throughout. The authored
//! foundation shows NPC facing markers as `^ > v <`, which fixes facing to
//! four directions; movement matches so that "the tile behind an actor" is
//! always well defined (garrote requires it) and simultaneous resolution
//! never has to arbitrate diagonal crossings. All geometry is integer-only
//! to keep native and wasm results bit-identical.

use serde::{Deserialize, Serialize};

/// Which storey of the building a position is on.
pub type FloorId = u8;

/// A tile position: storey plus grid coordinates. `y` grows downward
/// (screen order), so north is `-y`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pos {
    pub floor: FloorId,
    pub x: i16,
    pub y: i16,
}

impl Pos {
    pub const fn new(floor: FloorId, x: i16, y: i16) -> Self {
        Self { floor, x, y }
    }

    /// The neighbouring position one step in `dir` on the same floor.
    pub fn step(self, dir: Dir4) -> Self {
        let (dx, dy) = dir.delta();
        Self {
            floor: self.floor,
            x: self.x + dx,
            y: self.y + dy,
        }
    }

    /// Chebyshev distance on the same floor; `None` across floors.
    pub fn chebyshev(self, other: Pos) -> Option<i16> {
        if self.floor != other.floor {
            return None;
        }
        Some((self.x - other.x).abs().max((self.y - other.y).abs()))
    }

    /// True when `other` is exactly one orthogonal step away on this floor.
    pub fn is_adjacent(self, other: Pos) -> bool {
        self.floor == other.floor && (self.x - other.x).abs() + (self.y - other.y).abs() == 1
    }

    /// The direction from `self` towards an orthogonally adjacent `other`.
    pub fn dir_towards_adjacent(self, other: Pos) -> Option<Dir4> {
        if !self.is_adjacent(other) {
            return None;
        }
        Dir4::ALL.into_iter().find(|dir| self.step(*dir) == other)
    }
}

/// One of the four cardinal directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Dir4 {
    North,
    East,
    South,
    West,
}

impl Dir4 {
    /// Stable iteration order used everywhere a direction set is scanned.
    pub const ALL: [Dir4; 4] = [Dir4::North, Dir4::East, Dir4::South, Dir4::West];

    pub const fn delta(self) -> (i16, i16) {
        match self {
            Dir4::North => (0, -1),
            Dir4::East => (1, 0),
            Dir4::South => (0, 1),
            Dir4::West => (-1, 0),
        }
    }

    pub const fn opposite(self) -> Dir4 {
        match self {
            Dir4::North => Dir4::South,
            Dir4::East => Dir4::West,
            Dir4::South => Dir4::North,
            Dir4::West => Dir4::East,
        }
    }

    /// ASCII facing marker as shown next to NPC glyphs.
    pub const fn marker(self) -> char {
        match self {
            Dir4::North => '^',
            Dir4::East => '>',
            Dir4::South => 'v',
            Dir4::West => '<',
        }
    }

    /// Direction that best approximates the vector from `from` to `to`
    /// (same floor). Ties prefer the horizontal axis, deterministically.
    pub fn towards(from: Pos, to: Pos) -> Option<Dir4> {
        if from.floor != to.floor || from == to {
            return None;
        }
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        if dx.abs() >= dy.abs() {
            Some(if dx > 0 { Dir4::East } else { Dir4::West })
        } else {
            Some(if dy > 0 { Dir4::South } else { Dir4::North })
        }
    }
}

/// True when `target` lies inside the vision cone of an observer at
/// `origin` facing `facing`.
///
/// The cone test is pure integer math: with `along` the component of the
/// offset in the facing direction and `perp` the sideways component, a tile
/// is inside the cone when `along > 0` and
/// `perp * cone_den <= along * cone_num`. The authored tuning data supplies
/// `cone_num / cone_den` (3/2 gives a total cone of roughly 112 degrees).
/// The observer's own tile is always considered inside.
pub fn in_cone(origin: Pos, facing: Dir4, target: Pos, cone_num: i32, cone_den: i32) -> bool {
    if origin.floor != target.floor {
        return false;
    }
    if origin == target {
        return true;
    }
    let dx = i32::from(target.x - origin.x);
    let dy = i32::from(target.y - origin.y);
    let (fx, fy) = facing.delta();
    let (fx, fy) = (i32::from(fx), i32::from(fy));
    let along = dx * fx + dy * fy;
    let perp = (dx * fy - dy * fx).abs();
    along > 0 && perp * cone_den <= along * cone_num
}

/// All integer points on the open segment between `a` and `b` (exclusive of
/// both endpoints), in order from `a` to `b`, using a deterministic
/// supercover walk: every tile the ray passes through is included, so
/// diagonal rays cannot slip between two blocking corners.
pub fn supercover_between(a: Pos, b: Pos) -> Vec<Pos> {
    debug_assert_eq!(a.floor, b.floor);
    let mut points = Vec::new();
    let dx = i32::from(b.x - a.x);
    let dy = i32::from(b.y - a.y);
    let nx = dx.abs();
    let ny = dy.abs();
    let sign_x: i16 = if dx > 0 { 1 } else { -1 };
    let sign_y: i16 = if dy > 0 { 1 } else { -1 };

    let mut p = a;
    let (mut ix, mut iy) = (0i32, 0i32);
    while ix < nx || iy < ny {
        // Compare (0.5 + ix) / nx against (0.5 + iy) / ny without floats.
        let decision = (1 + 2 * ix) * ny - (1 + 2 * iy) * nx;
        if decision == 0 {
            // Exact diagonal corner: include both flanking tiles so walls
            // meeting at a corner block sight, then step diagonally. The
            // flanks count even when the diagonal step lands on `b` itself,
            // otherwise sight would slip between two touching corners.
            points.push(Pos::new(p.floor, p.x + sign_x, p.y));
            points.push(Pos::new(p.floor, p.x, p.y + sign_y));
            p = Pos::new(p.floor, p.x + sign_x, p.y + sign_y);
            ix += 1;
            iy += 1;
            if p != b {
                points.push(p);
            }
        } else if decision < 0 {
            p = Pos::new(p.floor, p.x + sign_x, p.y);
            ix += 1;
            if p != b {
                points.push(p);
            }
        } else {
            p = Pos::new(p.floor, p.x, p.y + sign_y);
            iy += 1;
            if p != b {
                points.push(p);
            }
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i16, y: i16) -> Pos {
        Pos::new(0, x, y)
    }

    #[test]
    fn adjacency_is_orthogonal_only() {
        assert!(p(3, 3).is_adjacent(p(3, 4)));
        assert!(p(3, 3).is_adjacent(p(2, 3)));
        assert!(!p(3, 3).is_adjacent(p(4, 4)));
        assert!(!p(3, 3).is_adjacent(p(3, 3)));
        assert!(!Pos::new(0, 3, 3).is_adjacent(Pos::new(1, 3, 4)));
    }

    #[test]
    fn step_and_dir_towards_adjacent_agree() {
        for dir in Dir4::ALL {
            let from = p(5, 5);
            let to = from.step(dir);
            assert_eq!(from.dir_towards_adjacent(to), Some(dir));
        }
    }

    #[test]
    fn cone_contains_facing_axis_and_excludes_behind() {
        // 3/2 ratio: total cone ~112 degrees.
        assert!(in_cone(p(0, 0), Dir4::East, p(4, 0), 3, 2));
        assert!(in_cone(p(0, 0), Dir4::East, p(4, 3), 3, 2));
        assert!(!in_cone(p(0, 0), Dir4::East, p(4, 7), 3, 2));
        assert!(!in_cone(p(0, 0), Dir4::East, p(-1, 0), 3, 2));
        assert!(!in_cone(p(0, 0), Dir4::East, p(0, 1), 3, 2));
    }

    #[test]
    fn supercover_between_walks_straight_lines() {
        assert_eq!(supercover_between(p(0, 0), p(3, 0)), vec![p(1, 0), p(2, 0)]);
        assert_eq!(supercover_between(p(0, 0), p(0, -2)), vec![p(0, -1)]);
        assert!(supercover_between(p(0, 0), p(1, 0)).is_empty());
    }

    #[test]
    fn supercover_between_includes_corner_flanks_on_exact_diagonals() {
        let cells = supercover_between(p(0, 0), p(2, 2));
        assert!(cells.contains(&p(1, 0)));
        assert!(cells.contains(&p(0, 1)));
        assert!(cells.contains(&p(1, 1)));
    }

    #[test]
    fn supercover_between_is_symmetric_in_coverage() {
        let forward = supercover_between(p(0, 0), p(5, 3));
        let mut backward = supercover_between(p(5, 3), p(0, 0));
        backward.reverse();
        let mut f = forward.clone();
        let mut b = backward.clone();
        f.sort();
        b.sort();
        assert_eq!(f, b);
    }
}
