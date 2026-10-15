//! What a player keeps and what the world remembers: the game's own save sections.
//!
//! The engine already saves where a body stands and how healthy it is (sections `position` and
//! `vitals`, kept with the character) and the region the world is in (`region`). A game adds what
//! is its own, as explicit structs, never a dump of the world:
//!
//! - [`Trophies`], a **character** section: how many grunts this hero has defeated. It is the
//!   player's, so in co-op it travels with them: a guest brings it into the host's yard and is
//!   sent it back when they leave (`shards::coop` does that for every character section).
//! - [`Fallen`], a **world** section: which of the yard's grunts are down. The world is the
//!   host's: a loaded yard has them lying where they fell.
//!
//! Clearing the yard saves both at the end of that tick ([`YardCleared`], `autosave_on`).
//! `shards act save_game what=all`, `load_game` and `save_check` work on them like on the
//! engine's own sections; `shards act save_registry` lists them.

use shards::action::combat::kill;
use shards::action::{revive, Died};
use shards::prelude::*;

use crate::brain::Grunt;

pub struct ProgressPlugin;

impl Plugin for ProgressPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Trophies>()
            .save_component::<Trophies>(SaveKind::Character, "trophies", 1)
            .add_save_section::<Fallen>()
            .autosave_on::<YardCleared>(SaveWhat::All)
            .add_observer(count_the_fallen);
    }
}

/// Grunts this hero has defeated (on the hero prefab, `characters.prefab.ron`).
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(crate = "shards::serde")]
#[reflect(Component, Default)]
pub struct Trophies {
    pub defeated: u32,
}

/// The last grunt of the yard fell: autosave everything (the journal says `yard_cleared`).
#[derive(Event, Clone, Debug)]
pub struct YardCleared;

/// A grunt that dies at a hero's hand is that hero's trophy, and the last one to fall that way
/// clears the yard. A grunt that falls with no killer (a loaded save puts it down again, a dev
/// `set` of its health) earns nobody anything and saves nothing: loading a cleared yard must not
/// autosave over the file it came from.
///
/// A co-op client sees every `Died` its host triggers (the engine sends it over for effects), but the
/// host keeps score: there the guest earns the trophy, and the client is sent the result.
fn count_the_fallen(
    died: On<Died>,
    role: Option<Res<NetRole>>,
    mut heroes: Query<&mut Trophies>,
    grunts: Query<Has<Dead>, With<Grunt>>,
    mut commands: Commands,
) {
    if !shards::core::simulates(role) || !grunts.contains(died.entity) {
        return;
    }
    if let Some(killer) = died.killer
        && let Ok(mut trophies) = heroes.get_mut(killer)
    {
        trophies.defeated += 1;
        // `Died` comes after `Dead` is on the body, so the grunt that just fell counts as down.
        if grunts.iter().all(|dead| dead) {
            commands.trigger(YardCleared);
        }
    }
}

/// The yard's grunts that are down, by their [`SaveKey`] (the arena gives every character it
/// spawns one: `yard.0`, `yard.1`, …, its place in the arena's `spawns`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(crate = "shards::serde")]
pub struct Fallen {
    pub grunts: Vec<String>,
}

impl SaveSection for Fallen {
    const NAME: &'static str = "fallen";
    const VERSION: u32 = 1;
    const KIND: SaveKind = SaveKind::World;

    fn capture(world: &World, _: Option<Entity>) -> Result<Self, String> {
        let mut grunts: Vec<String> = keyed_grunts(world).into_iter().filter(|(_, _, dead)| *dead).map(|(key, _, _)| key).collect();
        grunts.sort();
        Ok(Self { grunts })
    }

    /// The yard was built afresh: a grunt the save names falls again (no killer, so no trophy
    /// twice); one it does not name and that is down now stands up whole.
    fn apply(self, world: &mut World, _: Option<Entity>) -> Result<(), String> {
        for (key, grunt, dead) in keyed_grunts(world) {
            let fallen = self.grunts.contains(&key);
            if fallen && !dead {
                kill(world, grunt, None, None);
            } else if !fallen && dead {
                if let Some(mut health) = world.get_mut::<Health>(grunt) {
                    health.current = health.max;
                }
                revive(world, grunt);
            }
        }
        Ok(())
    }
}

/// `(key, grunt, down)` for every grunt with a [`SaveKey`], in key order.
fn keyed_grunts(world: &World) -> Vec<(String, Entity, bool)> {
    let Some(mut query) = world.try_query_filtered::<(Entity, &SaveKey, Has<Dead>), With<Grunt>>() else { return Vec::new() };
    let mut out: Vec<(String, Entity, bool)> = query.iter(world).map(|(e, key, dead)| (key.0.clone(), e, dead)).collect();
    out.sort();
    out
}
