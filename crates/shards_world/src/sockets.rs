//! Sockets: what meets along the edge between two tiles, and which may meet.
//!
//! A tile names the sockets of each edge in text: `grass`, `road@1` (a height level), or
//! several parts separated by spaces (`"grass road grass"`), listed clockwise around the
//! tile: west to east on the north edge, north to south on the east edge, east to west on
//! the south edge, south to north on the west edge. Clockwise order survives a turn, and
//! two neighbors read their shared edge in opposite directions, so an edge matches its
//! neighbor's reversed. An edge of one part stands for the whole edge.
//!
//! Kinds and what may meet are data ([`SocketRulesDef`](crate::content::SocketRulesDef)):
//! a kind meets itself and the kinds paired with it, heights must agree within a
//! tolerance (except for kinds that ignore height, such as cliff faces), and `forbid`
//! kinds meet nothing.

use std::collections::BTreeSet;
use std::fmt;

use crate::content::SocketRulesDef;

/// One socket: its kind and height level.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Socket {
    pub kind: String,
    pub height: i32,
}

impl fmt::Display for Socket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.height == 0 { f.write_str(&self.kind) } else { write!(f, "{}@{}", self.kind, self.height) }
    }
}

/// How an edge is written, for errors that teach it.
pub const EDGE_SYNTAX: &str =
    "write an edge as `kind` or `kind@height`, several parts separated by spaces, e.g. \"grass road@1 grass\"";

/// Parses one edge: its sockets clockwise around the tile.
pub fn parse_edge(text: &str) -> Result<Vec<Socket>, String> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.is_empty() {
        return Err(format!("empty edge; {EDGE_SYNTAX}"));
    }
    parts
        .into_iter()
        .map(|part| {
            let (kind, height) = match part.split_once('@') {
                Some((kind, h)) => {
                    let height = h.parse::<i32>().map_err(|_| format!("`{part}`: height `{h}` is not a whole number; {EDGE_SYNTAX}"))?;
                    (kind, height)
                }
                None => (part, 0),
            };
            let plain = !kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
            if !plain {
                return Err(format!("`{part}`: socket kinds are lowercase letters, digits, `_` and `-`; {EDGE_SYNTAX}"));
            }
            Ok(Socket { kind: kind.to_owned(), height })
        })
        .collect()
}

/// `grass road grass`: an edge as it is written.
pub fn edge_text(edge: &[Socket]) -> String {
    edge.iter().map(ToString::to_string).collect::<Vec<_>>().join(" ")
}

/// The socket rules of one region, ready for matching.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    pub kinds: BTreeSet<String>,
    pairs: BTreeSet<(String, String)>,
    forbid: BTreeSet<String>,
    any_height: BTreeSet<String>,
    tolerance: u32,
    passable: BTreeSet<String>,
    roads: BTreeSet<String>,
}

impl Rules {
    pub fn new(def: &SocketRulesDef) -> Self {
        let set = |v: &[String]| v.iter().cloned().collect::<BTreeSet<_>>();
        let pairs = def.pairs.iter().map(|(a, b)| if a <= b { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) }).collect();
        Self {
            kinds: set(&def.kinds),
            pairs,
            forbid: set(&def.forbid),
            any_height: set(&def.any_height),
            tolerance: def.height_tolerance,
            passable: set(&def.passable),
            roads: set(&def.roads),
        }
    }

    /// Whether socket `a` may face socket `b`.
    pub fn sockets_match(&self, a: &Socket, b: &Socket) -> bool {
        if self.forbid.contains(&a.kind) || self.forbid.contains(&b.kind) {
            return false;
        }
        let kinds = a.kind == b.kind || {
            let key = if a.kind <= b.kind { (a.kind.clone(), b.kind.clone()) } else { (b.kind.clone(), a.kind.clone()) };
            self.pairs.contains(&key)
        };
        let free = self.any_height.contains(&a.kind) || self.any_height.contains(&b.kind);
        kinds && (free || a.height.abs_diff(b.height) <= self.tolerance)
    }

    /// Whether edge `a` (clockwise on its tile) may meet edge `b` (clockwise on the
    /// neighbor).
    pub fn edges_match(&self, a: &[Socket], b: &[Socket]) -> bool {
        match facing(a, b) {
            Some(pairs) => pairs.iter().all(|(x, y)| self.sockets_match(x, y)),
            None => false,
        }
    }

    /// Walkers cross between `a` and `b` where some facing parts are both passable.
    pub fn edges_passable(&self, a: &[Socket], b: &[Socket]) -> bool {
        facing(a, b).is_some_and(|pairs| pairs.iter().any(|(x, y)| self.passable.contains(&x.kind) && self.passable.contains(&y.kind)))
    }

    /// The edge carries a road.
    pub fn has_road(&self, edge: &[Socket]) -> bool {
        edge.iter().any(|s| self.roads.contains(&s.kind))
    }

    /// There are road kinds at all.
    pub fn knows_roads(&self) -> bool {
        !self.roads.is_empty()
    }

    /// Kind `kind` meets nothing.
    pub fn forbids(&self, kind: &str) -> bool {
        self.forbid.contains(kind)
    }
}

/// The parts of `a` and `b` that face each other along the shared edge (a single part
/// faces every part), or `None` when the parts cannot line up.
fn facing<'a>(a: &'a [Socket], b: &'a [Socket]) -> Option<Vec<(&'a Socket, &'a Socket)>> {
    if a.len() == b.len() {
        Some(a.iter().zip(b.iter().rev()).collect())
    } else if a.len() == 1 {
        Some(b.iter().map(|y| (&a[0], y)).collect())
    } else if b.len() == 1 {
        Some(a.iter().map(|x| (x, &b[0])).collect())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules::new(&SocketRulesDef {
            kinds: ["grass", "road", "flowers", "cliff", "void"].map(String::from).to_vec(),
            pairs: vec![("grass".into(), "flowers".into())],
            forbid: vec!["void".into()],
            any_height: vec!["cliff".into()],
            height_tolerance: 0,
            passable: vec!["grass".into(), "road".into()],
            roads: vec!["road".into()],
        })
    }

    fn edge(text: &str) -> Vec<Socket> {
        parse_edge(text).unwrap()
    }

    #[test]
    fn edges_parse_with_heights_and_parts() {
        assert_eq!(edge("grass road@1 grass@-2").iter().map(|s| s.height).collect::<Vec<_>>(), [0, 1, -2]);
        assert_eq!(edge_text(&edge(" road@1  grass ")), "road@1 grass");
        let err = parse_edge("Grass").unwrap_err();
        assert!(err.contains("lowercase") && err.contains("kind@height"), "{err}");
        assert!(parse_edge("road@x").unwrap_err().contains("not a whole number"));
        assert!(parse_edge("  ").unwrap_err().starts_with("empty edge"));
    }

    #[test]
    fn kinds_pairs_heights_and_forbidden_kinds() {
        let r = rules();
        let s = |k: &str, h| Socket { kind: k.into(), height: h };
        assert!(r.sockets_match(&s("grass", 0), &s("grass", 0)));
        assert!(r.sockets_match(&s("flowers", 0), &s("grass", 0)), "pairs are symmetric");
        assert!(!r.sockets_match(&s("grass", 0), &s("road", 0)));
        assert!(!r.sockets_match(&s("grass", 1), &s("grass", 0)), "heights must agree");
        assert!(r.sockets_match(&s("cliff", 2), &s("cliff", 0)), "cliff faces ignore height");
        assert!(!r.sockets_match(&s("void", 0), &s("void", 0)), "forbidden kinds meet nothing");
    }

    #[test]
    fn neighbors_read_their_shared_edge_in_opposite_directions() {
        let r = rules();
        // West to east on one tile's south edge is east to west on the other's north edge.
        assert!(r.edges_match(&edge("grass road flowers"), &edge("grass road grass")));
        assert!(!r.edges_match(&edge("road grass grass"), &edge("road grass grass")));
        assert!(r.edges_match(&edge("road grass grass"), &edge("grass grass road")));
        assert!(r.edges_match(&edge("grass"), &edge("grass flowers grass")), "one part is the whole edge");
        assert!(!r.edges_match(&edge("grass grass"), &edge("grass grass grass")), "parts must line up");
        assert!(r.edges_passable(&edge("cliff road cliff"), &edge("cliff road cliff")));
        assert!(!r.edges_passable(&edge("cliff"), &edge("cliff")));
        assert!(r.has_road(&edge("grass road grass")) && !r.has_road(&edge("grass")));
    }
}
