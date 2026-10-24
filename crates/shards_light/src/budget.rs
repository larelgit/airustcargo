//! The shadow budget: of the lights that want shadows, only the nearest `budget` to the viewer get them.
//!
//! A pure function of the lights and the viewer, so a picture and a headless `lights` query agree and a
//! shot taken twice from one place shows the same shadows. There is no hysteresis: a light that is
//! nearly as near as one that has a shadow takes it when it comes nearer, once.

use std::cmp::Ordering;

/// One light that is a candidate for a shadow.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate<'a> {
    /// How far it is from the viewer (m).
    pub distance: f32,
    /// A stable name for ties: the content id, the node and the place.
    pub key: &'a str,
    /// Whether its content asks for shadows.
    pub wants: bool,
}

/// Distances are compared to the centimetre, so a viewer that stands still on a boundary does not flip a
/// pair of lights that are as near as each other by float noise; equals go by `key`.
fn rank(a: &Candidate, b: &Candidate) -> Ordering {
    let cm = |c: &Candidate| (c.distance * 100.0).round() as i64;
    cm(a).cmp(&cm(b)).then_with(|| a.key.cmp(b.key))
}

/// Which of `lights` cast a shadow, in the same order: the first `budget` of those that want one, nearest
/// first. A light that does not want shadows never has one.
pub fn casters(lights: &[Candidate], budget: usize) -> Vec<bool> {
    let mut wanting: Vec<usize> = (0..lights.len()).filter(|i| lights[*i].wants && lights[*i].distance.is_finite()).collect();
    wanting.sort_by(|a, b| rank(&lights[*a], &lights[*b]).then(a.cmp(b)));
    let mut out = vec![false; lights.len()];
    for i in wanting.into_iter().take(budget) {
        out[i] = true;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lights<'a>(list: &[(f32, &'a str, bool)]) -> Vec<Candidate<'a>> {
        list.iter().map(|(distance, key, wants)| Candidate { distance: *distance, key, wants: *wants }).collect()
    }

    #[test]
    fn the_nearest_that_want_a_shadow_get_one() {
        let all = lights(&[(9.0, "c", true), (2.0, "a", true), (5.0, "b", true), (1.0, "n", false), (30.0, "z", true)]);
        assert_eq!(casters(&all, 2), [false, true, true, false, false]);
        assert_eq!(casters(&all, 0), [false; 5], "a budget of 0 is no shadows");
        assert_eq!(casters(&all, 99), [true, true, true, false, true], "a light that does not want one never has one");
    }

    #[test]
    fn ties_go_by_key_and_the_order_of_the_list_does_not_matter() {
        let a = lights(&[(4.0, "lamp.b", true), (4.001, "lamp.a", true), (4.0, "lamp.c", true)]);
        // To the centimetre all three are 4 m: the key decides.
        assert_eq!(casters(&a, 1), [false, true, false]);
        let b = lights(&[(4.0, "lamp.c", true), (4.0, "lamp.a", true), (4.0, "lamp.b", true)]);
        assert_eq!(casters(&b, 2), [false, true, true]);
        let nan = lights(&[(f32::NAN, "x", true), (1.0, "y", true)]);
        assert_eq!(casters(&nan, 2), [false, true], "a light with no distance has no shadow");
    }
}
