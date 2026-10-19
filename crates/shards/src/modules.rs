//! Which engine modules a game runs: [`EngineConfig::without`](shards_core::EngineConfig::without)
//! leaves some out (`--without audio,scatter` on the command line), and [`check_without`] says
//! whether that set can run.

/// An engine module a game may leave out, by the name `ai/status` lists it under
/// `engine.modules`, and the modules it cannot run without.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionalModule {
    pub name: &'static str,
    pub needs: &'static [&'static str],
    /// What the game goes without, for the error that lists them.
    pub what: &'static str,
}

/// The modules every game runs: the simulation, its content and settings, bodies, the rules and the fight.
pub const REQUIRED_MODULES: &[&str] = &["core", "content", "settings", "motion", "rules", "action"];

/// The modules a game may leave out, in the order [`EnginePlugins`](crate::EnginePlugins) adds
/// them. The plugins that join two modules (level files as tiles, co-op saves) go with either.
pub const OPTIONAL_MODULES: &[OptionalModule] = &[
    OptionalModule { name: "level", needs: &[], what: "level files from Blender" },
    OptionalModule { name: "world", needs: &[], what: "regions and their generator" },
    OptionalModule { name: "npc", needs: &[], what: "NPC brains, senses and the navmesh" },
    OptionalModule { name: "encounter", needs: &["npc", "world"], what: "encounters: waves of enemies at a place (out with npc or world)" },
    OptionalModule { name: "net", needs: &[], what: "co-op sessions" },
    OptionalModule { name: "save", needs: &[], what: "saves" },
    OptionalModule { name: "coop", needs: &["net", "save"], what: "co-op saves (out with net or save)" },
    OptionalModule { name: "locale", needs: &[], what: "languages" },
    OptionalModule { name: "ui", needs: &["locale"], what: "screens, menus and the HUD" },
    OptionalModule { name: "audio", needs: &[], what: "sound and music" },
    OptionalModule { name: "look", needs: &[], what: "mesh looks" },
    OptionalModule { name: "scatter", needs: &[], what: "foliage" },
    OptionalModule { name: "voxel", needs: &[], what: "voxel terrain" },
    OptionalModule { name: "fx", needs: &[], what: "particle effects and telegraph decals" },
    OptionalModule { name: "hitstop", needs: &[], what: "hit stop (a brief hold when a blow lands)" },
    OptionalModule { name: "steam", needs: &["net"], what: "Steam (feature `steam`)" },
    OptionalModule { name: "dev", needs: &[], what: "the developer's menu and console" },
];

/// Modules that only join others: out with any of them, without being named.
const JOINS: &[&str] = &["coop", "encounter"];

/// Whether a game can leave out `without`: every name is a module a game may leave out, and
/// nothing that runs needs one of them. Answers the modules that go, with the joining ones
/// (`coop` with `net` or `save`), in the engine's order.
pub fn check_without(without: &[String]) -> Result<Vec<&'static str>, String> {
    let mut out: Vec<&'static str> = Vec::new();
    for name in without.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
        if REQUIRED_MODULES.contains(&name) {
            return Err(format!("`{name}` runs in every game ({}); only these can be left out: {}", REQUIRED_MODULES.join(", "), optional_names()));
        }
        match OPTIONAL_MODULES.iter().find(|m| m.name == name) {
            Some(module) => out.push(module.name),
            None => return Err(format!("`{name}` is not an engine module a game can leave out: {}", optional_names())),
        }
    }
    for join in JOINS {
        let module = OPTIONAL_MODULES.iter().find(|m| m.name == *join).expect("a joining module is optional");
        if module.needs.iter().any(|n| out.contains(n)) && !out.contains(join) {
            out.push(join);
        }
    }
    for module in OPTIONAL_MODULES.iter().filter(|m| built(m.name) && !out.contains(&m.name)) {
        if let Some(gone) = module.needs.iter().find(|n| out.contains(n)) {
            return Err(format!(
                "`{}` ({}) needs `{gone}`: leave out both (`--without {gone},{}`) or neither",
                module.name, module.what, module.name
            ));
        }
    }
    out.sort_by_key(|name| OPTIONAL_MODULES.iter().position(|m| m.name == *name));
    Ok(out)
}

/// Whether this build has the module: Steam only with the `steam` feature.
fn built(name: &str) -> bool {
    name != "steam" || cfg!(feature = "steam")
}

fn optional_names() -> String {
    OPTIONAL_MODULES.iter().map(|m| m.name).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn modules_go_with_the_joins_they_leave_nothing_to_join() {
        assert_eq!(check_without(&[]), Ok(vec![]));
        assert_eq!(check_without(&names(&["scatter", "audio"])), Ok(vec!["audio", "scatter"]));
        assert_eq!(check_without(&names(&["save"])), Ok(vec!["save", "coop"]), "co-op saves go with saves");
        assert_eq!(check_without(&names(&["coop"])), Ok(vec!["coop"]));
        assert_eq!(check_without(&names(&["ui", "locale"])), Ok(vec!["locale", "ui"]));
        // Steam needs the network only in a build that has Steam.
        assert_eq!(check_without(&names(&["net"])).is_ok(), !cfg!(feature = "steam"));
        assert_eq!(check_without(&names(&["net", "steam"])), Ok(vec!["net", "coop", "steam"]));
    }

    #[test]
    fn what_cannot_be_left_out_is_named_with_what_can() {
        let err = check_without(&names(&["motion"])).unwrap_err();
        assert!(err.starts_with("`motion` runs in every game (core, content, settings, motion, rules, action)") && err.contains("level, world"), "{err}");
        let err = check_without(&names(&["sound"])).unwrap_err();
        assert!(err.starts_with("`sound` is not an engine module a game can leave out: level, world,"), "{err}");
        let err = check_without(&names(&["locale"])).unwrap_err();
        assert_eq!(err, "`ui` (screens, menus and the HUD) needs `locale`: leave out both (`--without locale,ui`) or neither");
    }
}
