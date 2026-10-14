//! Cells, sides and quarter turns: the geometry every other module shares.
//!
//! A region is a grid of cells, `x` east (+X) and `y` south (+Z), row-major, its
//! north-west corner at the world origin. A tile's own frame has its center at the origin
//! and north at -Z; turning it is a whole number of quarter turns clockwise seen from
//! above. Positions in manifests are turned with exact swaps, never with sines, so a
//! manifest is the same bytes on every platform.

use std::fmt;

use bevy::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A side of a cell, clockwise from north: north (-Z), east (+X), south (+Z), west (-X).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema, Reflect)]
pub enum Dir {
    N,
    E,
    S,
    W,
}

impl Dir {
    /// Clockwise from north.
    pub const ALL: [Dir; 4] = [Dir::N, Dir::E, Dir::S, Dir::W];

    /// 0 for N up to 3 for W.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The side with index `i % 4`.
    pub fn from_index(i: usize) -> Dir {
        Dir::ALL[i % 4]
    }

    pub fn opposite(self) -> Dir {
        Dir::from_index(self.index() + 2)
    }

    /// This side after `turns` quarter turns clockwise.
    pub fn turned(self, turns: u8) -> Dir {
        Dir::from_index(self.index() + turns as usize)
    }

    /// The neighbor's offset in cells.
    pub fn offset(self) -> (i32, i32) {
        match self {
            Dir::N => (0, -1),
            Dir::E => (1, 0),
            Dir::S => (0, 1),
            Dir::W => (-1, 0),
        }
    }
}

impl fmt::Display for Dir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Dir::N => "N",
            Dir::E => "E",
            Dir::S => "S",
            Dir::W => "W",
        })
    }
}

/// Quarter turns for `degrees` (0, 90, 180, 270); `None` for any other angle.
pub fn turns_of(degrees: u16) -> Option<u8> {
    match degrees {
        0 => Some(0),
        90 => Some(1),
        180 => Some(2),
        270 => Some(3),
        _ => None,
    }
}

/// Degrees of `turns` quarter turns.
pub fn degrees_of(turns: u8) -> u16 {
    (turns % 4) as u16 * 90
}

/// `v` turned `turns` quarter turns clockwise about +Y, exactly: (x, z) -> (-z, x) per turn.
pub fn turn(v: Vec3, turns: u8) -> Vec3 {
    match turns % 4 {
        0 => v,
        1 => Vec3::new(-v.z, v.y, v.x),
        2 => Vec3::new(-v.x, v.y, -v.z),
        _ => Vec3::new(v.z, v.y, -v.x),
    }
}

/// The rotation of `turns` quarter turns clockwise seen from above.
pub fn rotation(turns: u8) -> Quat {
    Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2 * (turns % 4) as f32)
}

/// A footprint cell `(x, y)` of a `w`×`h` room after one quarter turn clockwise (the
/// footprint becomes `h`×`w`).
pub fn turn_cell((x, y): (u32, u32), (_, h): (u32, u32)) -> (u32, u32) {
    (h - 1 - y, x)
}

/// A position rounded to millimeters: what manifests store, so writing and reading one
/// gives the same numbers back.
pub fn mm(v: Vec3) -> [f32; 3] {
    let r = |x: f32| {
        let v = (x * 1000.0).round() / 1000.0;
        if v == 0.0 { 0.0 } else { v }
    };
    [r(v.x), r(v.y), r(v.z)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turns_go_clockwise_from_above() {
        assert_eq!(Dir::N.turned(1), Dir::E);
        assert_eq!(Dir::W.turned(1), Dir::N);
        assert_eq!(Dir::E.opposite(), Dir::W);
        // North (-Z) turns to east (+X), east to south.
        assert_eq!(turn(Vec3::NEG_Z, 1), Vec3::X);
        assert_eq!(turn(Vec3::X, 1), Vec3::Z);
        assert_eq!(turn(Vec3::new(1.0, 2.0, 3.0), 4), Vec3::new(1.0, 2.0, 3.0));
        for turns in 0..4 {
            let exact = turn(Vec3::new(2.0, 0.0, -5.0), turns);
            let quat = rotation(turns) * Vec3::new(2.0, 0.0, -5.0);
            assert!(exact.distance(quat) < 1e-5, "{turns}: {exact} vs {quat}");
        }
        // The NW corner of a 3×2 room ends up at the NE corner of the 2×3 one.
        assert_eq!(turn_cell((0, 0), (3, 2)), (1, 0));
        assert_eq!(turn_cell((2, 1), (3, 2)), (0, 2));
    }
}
