//! The out-of-combat guards: rules a change of rules must wait for. A guard is code a module registers at
//! build time ([`RulesAppExt::add_rules_guard`]); its numbers, if it has any, are content. The engine's own,
//! `npc_engaged` (an NPC is fighting), is registered by the NPC module.

use bevy::prelude::*;

/// Why the rules cannot change now (`Some("e6 `skeleton` is fighting (`chase`, target e5)")`), or `None`. In
/// English, for agents: the player reads `RulesDef::blocked`.
pub type RulesGuard = fn(&World) -> Option<String>;

/// The rules guards, registered at build time, in the order they are asked.
#[derive(Resource, Default, Clone)]
pub struct RulesGuards(pub Vec<(&'static str, RulesGuard)>);

pub trait RulesAppExt {
    /// A rule the change of rules must wait for; guards are asked in registration order, the first reason wins.
    /// The same name again replaces the guard where it stands.
    fn add_rules_guard(&mut self, name: &'static str, guard: RulesGuard) -> &mut Self;
}

impl RulesAppExt for App {
    fn add_rules_guard(&mut self, name: &'static str, guard: RulesGuard) -> &mut Self {
        let mut guards = self.world_mut().get_resource_or_init::<RulesGuards>();
        match guards.0.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 = guard,
            None => guards.0.push((name, guard)),
        }
        self
    }
}

/// `Ok` when no guard holds; otherwise the first reason, with the guard's name (`… [npc_engaged]`).
pub fn can_change(world: &World) -> Result<(), String> {
    let Some(guards) = world.get_resource::<RulesGuards>() else { return Ok(()) };
    match guards.0.iter().find_map(|(name, guard)| guard(world).map(|why| format!("{why} [{name}]"))) {
        Some(why) => Err(why),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource)]
    struct Fight;

    fn fighting(world: &World) -> Option<String> {
        world.contains_resource::<Fight>().then(|| "e6 `brute` is fighting (`chase`, target e5)".to_owned())
    }

    fn always(_: &World) -> Option<String> {
        Some("always".to_owned())
    }

    #[test]
    fn the_first_guard_that_holds_gives_its_reason_and_name() {
        let mut app = App::new();
        assert_eq!(can_change(app.world()), Ok(()), "no registry: nothing holds");
        app.add_rules_guard("fighting", fighting).add_rules_guard("late", always);
        assert_eq!(can_change(app.world()), Err("always [late]".to_owned()));
        app.world_mut().insert_resource(Fight);
        assert_eq!(can_change(app.world()), Err("e6 `brute` is fighting (`chase`, target e5) [fighting]".to_owned()), "registration order");
        app.add_rules_guard("fighting", |_| None);
        assert_eq!(can_change(app.world()), Err("always [late]".to_owned()), "the same name replaces the guard where it stands");
        assert_eq!(app.world().resource::<RulesGuards>().0.len(), 2);
    }
}
