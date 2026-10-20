//! The map widget (U3b): a picture of a stretch of the world with the places a game knows of.
//!
//! ```ron
//! Map(id: "map", bind: "quests.places", image: "ui/valley_map.png",
//!     area: (min: (-128.0, -128.0), max: (128.0, 128.0)), width: 320.0, height: 320.0)
//! ```
//!
//! - **The area.** `area` is the world's `x` and `z` the picture covers: `min` is its north-west
//!   corner (north is `-z`), `max` its south-east one. The picture fills `width` × `height` logical
//!   pixels, north up; a place outside the area is drawn on the edge nearest to it.
//! - **The places** come from the game's source `bind` ([`UiValue::Map`](crate::UiValue::Map)): a
//!   label, where it is, its [`MapState`] and its category. Which places a player knows of, and in
//!   what state, is the game's, kept in its save; the engine draws what the source says.
//! - **A rumour** is drawn as a direction, not a place: a mark on the edge of the map, on the line
//!   from the viewer (the map's middle without one) toward where the place is said to be.
//! - **The viewer** is drawn where it stands, in the theme's `you` colour.
//! - **The look.** The theme's `map_colors` give each state a colour (`rumor`, `discovered`,
//!   `completed`, and `you`), and its `symbols` draw a category as on the compass.
//! - **Agents and screen readers** read the places in words, a rumour as a compass direction:
//!   `you (0, 0) · Old Shrine (shrine) discovered (12, -40) · Ruins rumor NE`.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// The world's stretch a map shows: `x` and `z`, `min` the north-west corner, `max` the south-east.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapArea {
    pub min: (f32, f32),
    pub max: (f32, f32),
}

impl MapArea {
    /// What is wrong with it, if anything.
    pub fn check(&self) -> Result<(), String> {
        let numbers = [self.min.0, self.min.1, self.max.0, self.max.1];
        if numbers.iter().any(|n| !n.is_finite()) {
            return Err(format!("area: finite numbers, not {self:?}"));
        }
        if self.max.0 <= self.min.0 || self.max.1 <= self.min.1 {
            return Err(format!(
                "area: `max` is east and south of `min` (both its x and z are larger), not min ({}, {}) max ({}, {})",
                self.min.0, self.min.1, self.max.0, self.max.1
            ));
        }
        Ok(())
    }

    /// Where world `at` (`x`, `z`) is on a map `size` pixels big, north up: from the top-left
    /// corner, kept on the map (a place outside is on the edge nearest to it).
    pub fn to_map(&self, size: Vec2, at: Vec2) -> Vec2 {
        let span = Vec2::new(self.max.0 - self.min.0, self.max.1 - self.min.1);
        let share = (at - Vec2::new(self.min.0, self.min.1)) / span;
        (share * size).clamp(Vec2::ZERO, size)
    }

    /// Whether world `at` is inside the area.
    pub fn contains(&self, at: Vec2) -> bool {
        (self.min.0..=self.max.0).contains(&at.x) && (self.min.1..=self.max.1).contains(&at.y)
    }
}

/// Where the line from `from` toward `to` (both on a map `size` pixels big) leaves the map: a
/// rumour's mark. Straight up (north) when the two are the same point.
pub fn edge_toward(size: Vec2, from: Vec2, to: Vec2) -> Vec2 {
    let from = from.clamp(Vec2::ZERO, size);
    let dir = to - from;
    let dir = if dir.length_squared() < 1e-6 { Vec2::NEG_Y } else { dir };
    // The nearest of the sides the line meets.
    let mut t = f32::INFINITY;
    for (d, at, side) in [(dir.x, from.x, size.x), (dir.y, from.y, size.y)] {
        if d > 0.0 {
            t = t.min((side - at) / d);
        } else if d < 0.0 {
            t = t.min(-at / d);
        }
    }
    (from + dir * t).clamp(Vec2::ZERO, size)
}

/// What a game knows of a place on the map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapState {
    /// Heard of: drawn as a direction on the edge, not where it is.
    #[default]
    Rumor,
    /// Found: drawn where it is.
    Discovered,
    /// Done with: drawn where it is, in its own colour.
    Completed,
}

impl MapState {
    pub const ALL: [MapState; 3] = [MapState::Rumor, MapState::Discovered, MapState::Completed];

    /// `rumor`, `discovered`, `completed`: as the theme's `map_colors` and agents name it.
    pub fn name(self) -> &'static str {
        match self {
            MapState::Rumor => "rumor",
            MapState::Discovered => "discovered",
            MapState::Completed => "completed",
        }
    }
}

/// A place on a map, as a game's source gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct MapPoint {
    pub label: String,
    /// Where it is (or is said to be): world `x` and `z`.
    pub at: Vec2,
    pub state: MapState,
    /// Its category (`shrine`), which the theme's `symbols` draw.
    pub category: Option<String>,
}

/// The viewer on a map: where it stands (world `x`, `z`) and which way it faces (degrees
/// clockwise from north).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapYou {
    pub at: Vec2,
    pub deg: f32,
}

/// Where a map widget's mark goes, in pixels from its top-left corner.
pub fn mark_at(area: &MapArea, size: Vec2, point: &MapPoint, you: Option<&MapYou>) -> Vec2 {
    match point.state {
        MapState::Rumor => {
            let from = you.map_or(size * 0.5, |y| area.to_map(size, y.at));
            // Toward the unclamped place: the direction, not the nearest edge.
            let span = Vec2::new(area.max.0 - area.min.0, area.max.1 - area.min.1);
            let to = (point.at - Vec2::new(area.min.0, area.min.1)) / span * size;
            edge_toward(size, from, to)
        }
        MapState::Discovered | MapState::Completed => area.to_map(size, point.at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: MapArea = MapArea { min: (-100.0, -50.0), max: (100.0, 50.0) };
    const SIZE: Vec2 = Vec2::new(400.0, 200.0);

    #[test]
    fn the_area_maps_north_up_and_keeps_places_on_the_map() {
        assert_eq!(AREA.to_map(SIZE, Vec2::new(-100.0, -50.0)), Vec2::ZERO, "the north-west corner is the top left");
        assert_eq!(AREA.to_map(SIZE, Vec2::new(100.0, 50.0)), SIZE);
        assert_eq!(AREA.to_map(SIZE, Vec2::ZERO), Vec2::new(200.0, 100.0));
        assert_eq!(AREA.to_map(SIZE, Vec2::new(50.0, -25.0)), Vec2::new(300.0, 50.0), "north-east of the middle: up and right");
        assert_eq!(AREA.to_map(SIZE, Vec2::new(500.0, 0.0)), Vec2::new(400.0, 100.0), "far east: on the east edge");
        assert!(AREA.contains(Vec2::ZERO) && !AREA.contains(Vec2::new(0.0, 60.0)));
    }

    #[test]
    fn a_rumour_is_a_mark_on_the_edge_toward_the_place() {
        let middle = SIZE * 0.5;
        assert_eq!(edge_toward(SIZE, middle, Vec2::new(200.0, 0.0)), Vec2::new(200.0, 0.0), "north");
        assert_eq!(edge_toward(SIZE, middle, Vec2::new(250.0, 100.0)), Vec2::new(400.0, 100.0), "east, however near");
        assert_eq!(edge_toward(SIZE, middle, Vec2::new(150.0, 150.0)), Vec2::new(100.0, 200.0), "south-west: the bottom first");
        assert_eq!(edge_toward(SIZE, middle, middle), Vec2::new(200.0, 0.0), "no direction: north");
        // From the viewer, not the middle: a viewer in the west sees a place due east toward the east edge.
        let point = MapPoint { label: "Ruins".into(), at: Vec2::new(0.0, -25.0), state: MapState::Rumor, category: None };
        let you = MapYou { at: Vec2::new(-50.0, -25.0), deg: 0.0 };
        assert_eq!(mark_at(&AREA, SIZE, &point, Some(&you)), Vec2::new(400.0, 50.0));
        assert_eq!(mark_at(&AREA, SIZE, &point, None), Vec2::new(200.0, 0.0), "from the middle: due north");
        let found = MapPoint { state: MapState::Discovered, ..point };
        assert_eq!(mark_at(&AREA, SIZE, &found, Some(&you)), Vec2::new(200.0, 50.0), "a place found is where it is");
    }

    #[test]
    fn an_area_names_its_mistakes() {
        assert!(AREA.check().is_ok());
        let flat = MapArea { min: (0.0, 0.0), max: (10.0, 0.0) };
        assert!(flat.check().unwrap_err().contains("`max` is east and south of `min`"));
        assert!(MapArea { min: (f32::NAN, 0.0), max: (1.0, 1.0) }.check().unwrap_err().starts_with("area: finite numbers"));
    }
}
