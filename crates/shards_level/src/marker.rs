//! Markers: the named points and zones an artist leaves in a level file for the game
//! (where the player starts, where a tile connects to the next, where a camp stands).

use std::fmt;
use std::str::FromStr;

use bevy::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use shards_content::db::closest;

/// What a marker is. Written `spawn`, `socket`, `navlink`, `trigger`, `interactable`,
/// `camp`, `landmark` or `custom:<name>` (a kind of the game's own) in Blender's `marker`
/// property and in content (`required_markers`).
#[derive(Reflect, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MarkerKind {
    /// Where characters appear: the player's start, a respawn point.
    SpawnPoint,
    /// Where a tile or room connects to the next one; the marker's name is the socket
    /// type (`road`, `river`, ...).
    Socket,
    /// A jump, a ladder or a drop the walkable grid does not show: from this node to the
    /// node named by its `to` property.
    NavLink,
    /// A zone that tells the game a character is inside it (a sensor volume).
    Trigger,
    /// Something a character can use: a door, a chest, a lever.
    Interactable,
    /// Where a group of enemies lives.
    Camp,
    /// Scenery worth knowing about; the AI sees it on its LANDMARKS line.
    Landmark,
    /// A kind of the game's own, written `custom:vendor`.
    Custom(String),
}

impl MarkerKind {
    /// The names Blender authors write, in documentation order.
    pub const NAMES: [&'static str; 7] = ["spawn", "socket", "navlink", "trigger", "interactable", "camp", "landmark"];

    /// The word the AI sees for the entity: `SpawnPoint`, `Camp`, ...
    pub fn label(&self) -> String {
        match self {
            MarkerKind::SpawnPoint => "SpawnPoint".into(),
            MarkerKind::Socket => "Socket".into(),
            MarkerKind::NavLink => "NavLink".into(),
            MarkerKind::Trigger => "Trigger".into(),
            MarkerKind::Interactable => "Interactable".into(),
            MarkerKind::Camp => "Camp".into(),
            MarkerKind::Landmark => "Landmark".into(),
            MarkerKind::Custom(name) => name.clone(),
        }
    }
}

impl fmt::Display for MarkerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MarkerKind::SpawnPoint => f.write_str("spawn"),
            MarkerKind::Socket => f.write_str("socket"),
            MarkerKind::NavLink => f.write_str("navlink"),
            MarkerKind::Trigger => f.write_str("trigger"),
            MarkerKind::Interactable => f.write_str("interactable"),
            MarkerKind::Camp => f.write_str("camp"),
            MarkerKind::Landmark => f.write_str("landmark"),
            MarkerKind::Custom(name) => write!(f, "custom:{name}"),
        }
    }
}

impl FromStr for MarkerKind {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let key = text.trim();
        if let Some(name) = key.strip_prefix("custom:") {
            let name = name.trim();
            if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(format!("`{text}`: a custom marker kind is `custom:` and a name without blanks, e.g. `custom:vendor`"));
            }
            return Ok(MarkerKind::Custom(name.to_owned()));
        }
        Ok(match key.to_ascii_lowercase().as_str() {
            "spawn" | "spawn_point" | "spawnpoint" => MarkerKind::SpawnPoint,
            "socket" => MarkerKind::Socket,
            "navlink" | "nav_link" => MarkerKind::NavLink,
            "trigger" => MarkerKind::Trigger,
            "interactable" => MarkerKind::Interactable,
            "camp" => MarkerKind::Camp,
            "landmark" => MarkerKind::Landmark,
            _ => {
                let hint = closest(key, Self::NAMES.into_iter()).map(|k| format!(" (did you mean `{k}`?)")).unwrap_or_default();
                return Err(format!(
                    "unknown marker kind `{text}`{hint}; kinds: {}, or `custom:<name>` for your own",
                    Self::NAMES.join(", ")
                ));
            }
        })
    }
}

impl Serialize for MarkerKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MarkerKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A marker on a spawned level entity: what it is and what it is called.
///
/// Query it to find the level's spawn points, sockets and camps:
/// `markers: Query<(&LevelMarker, &Transform)>`.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
pub struct LevelMarker {
    pub kind: MarkerKind,
    /// `start` for a spawn point, `road` for a socket, the trigger's name. The node's name
    /// unless the file says otherwise (the `name` property, or `socket`).
    pub name: String,
    /// Where a navlink ends, in world space (its `to` node, placed like the level).
    pub to: Option<Vec3>,
}

/// A marker a level must have: `content check` fails without it.
///
/// ```ron
/// required_markers: [(kind: "spawn", name: "start"), (kind: "landmark", min: 2)]
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredMarker {
    pub kind: MarkerKind,
    /// Only markers with this name count. Any name when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// At least this many (1 when left out).
    #[serde(default = "one")]
    pub min: u32,
}

fn one() -> u32 {
    1
}

impl RequiredMarker {
    /// `spawn "start"`, `landmark`.
    pub fn describe(&self) -> String {
        match &self.name {
            Some(name) => format!("`{}` named `{name}`", self.kind),
            None => format!("`{}`", self.kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_read_and_print_the_way_artists_write_them() {
        for name in MarkerKind::NAMES {
            assert_eq!(name.parse::<MarkerKind>().unwrap().to_string(), name);
        }
        assert_eq!("Spawn".parse::<MarkerKind>().unwrap(), MarkerKind::SpawnPoint);
        assert_eq!("nav_link".parse::<MarkerKind>().unwrap(), MarkerKind::NavLink);
        assert_eq!("custom:vendor".parse::<MarkerKind>().unwrap(), MarkerKind::Custom("vendor".into()));
        assert_eq!(MarkerKind::Custom("vendor".into()).to_string(), "custom:vendor");
    }

    #[test]
    fn a_typo_in_a_kind_is_an_error_with_a_hint() {
        let err = "spwn".parse::<MarkerKind>().unwrap_err();
        assert!(err.contains("unknown marker kind `spwn` (did you mean `spawn`?)"), "{err}");
        assert!(err.contains("custom:<name>"), "{err}");
        assert!("custom:".parse::<MarkerKind>().is_err());
        assert!("custom:a b".parse::<MarkerKind>().is_err());
    }

    #[test]
    fn required_markers_come_from_ron_with_a_default_minimum() {
        // Content reads RON with `Some(...)` left out of options.
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let ron = r#"[(kind: "spawn", name: "start"), (kind: "landmark", min: 2)]"#;
        let markers: Vec<RequiredMarker> = options.from_str(ron).unwrap();
        assert_eq!(markers[0], RequiredMarker { kind: MarkerKind::SpawnPoint, name: Some("start".into()), min: 1 });
        assert_eq!(markers[1], RequiredMarker { kind: MarkerKind::Landmark, name: None, min: 2 });
        let err = options.from_str::<Vec<RequiredMarker>>(r#"[(kind: "spwn")]"#).unwrap_err().to_string();
        assert!(err.contains("did you mean `spawn`?"), "{err}");
    }
}
