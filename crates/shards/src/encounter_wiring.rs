//! Where encounters meet the rest of the engine: the events travel to clients, the cues play,
//! a fight can keep joiners waiting and keep its members in every client's view, and the cues
//! are checked against the effects, sounds and messages content has.
//!
//! The encounter crate depends on none of `shards_net`, `shards_fx`, `shards_audio`,
//! `shards_locale` or `shards_save`; this plugin, which knows them all, joins them to it. Each
//! part is added only when its module is in the game (`EngineConfig::without`).

use bevy::prelude::*;
use ai_core::sim::SimSet;
use ai_core::AiAppExt;
use shards_content::{ContentAppExt, ContentDb, Refs};
use shards_encounter::{
    EncounterCompleted, EncounterDef, EncounterMember, EncounterReset, EncounterResumed, EncounterRuns, EncounterStarted, EncounterWiped,
    RunState, WaveCleared, WaveStarted, WaveWarned,
};
use shards_net::{JoinRequest, NetAlways, NetEventAppExt, NetHooks, NetPlugin};

/// The encounter module's join to the others. Part of [`EnginePlugins`](crate::EnginePlugins),
/// after the modules it joins.
pub struct EngineEncounterPlugin;

impl Plugin for EngineEncounterPlugin {
    fn build(&self, app: &mut App) {
        // On entities with an `AiId`: snapshots save it (see `OurNetAlways`). Bookkeeping of whose
        // `NetAlways` it is, not a fact about the world: a replay that differs here differs in
        // `NetAlways`, which is checked, so replay checks (and replication) leave it out, and every
        // recording made before it reflected still verifies.
        app.register_type::<OurNetAlways>().replay_ignore::<OurNetAlways>();
        if app.is_plugin_added::<NetPlugin>() {
            app.net_event::<EncounterStarted>()
                .net_event::<WaveWarned>()
                .net_event::<WaveStarted>()
                .net_event::<WaveCleared>()
                .net_event::<EncounterCompleted>()
                .net_event::<EncounterWiped>()
                .net_event::<EncounterResumed>()
                .net_event::<EncounterReset>();
            // A fight that blocks joining keeps joiners waiting (the default policy asks the
            // gates; a game with its own policy calls `shards::encounter::join_blocked`).
            app.world_mut().get_resource_or_init::<NetHooks>().join_gates.push(encounter_join_gate);
            // Members of a `net_always` fight are in every client's view while it is on.
            app.add_systems(FixedUpdate, encounter_net_always.in_set(SimSet::Resolve).after(shards_encounter::EncounterSystems::Step));
        }
        if app.is_plugin_added::<shards_fx::FxPlugin>() {
            app.add_observer(play_effect);
        }
        if app.is_plugin_added::<shards_audio::AudioPlugin>() {
            app.add_observer(play_sound);
        }
        app.add_content_check::<EncounterDef>("encounter cues", check_cues);
    }
}

/// The join gate: a fight with `blocks_join` is on. Joining a session is forbidden during the
/// fight only; the gate lets the peer in after it ends or resets.
fn encounter_join_gate(world: &World, _: &JoinRequest) -> Option<String> {
    shards_encounter::join_blocked(world)
}

/// Marks a member whose `NetAlways` this module added, so that only this module's is taken off.
/// State on an entity with an `AiId`, so it reflects and snapshots save it: a game loaded in the
/// middle of a fight must take off what the original put on (issue #72: without it, a heavy twin
/// left `NetAlways` on the members the run let go). Replay checks and replication leave it out
/// (`replay_ignore`): what it decides shows in `NetAlways`, which they check.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
struct OurNetAlways;

/// Adds `NetAlways` to the members of every `net_always` run in a fighting state, and takes it
/// off the members of those that stopped and the bodies a run let go. It touches only members of
/// a `net_always` run: another
/// module's `NetAlways` on a member of an ordinary run is its own.
fn encounter_net_always(
    mut commands: Commands,
    runs: Option<Res<EncounterRuns>>,
    db: Res<ContentDb>,
    members: Query<(Entity, &EncounterMember, Has<NetAlways>, Has<OurNetAlways>)>,
    left: Query<Entity, (With<OurNetAlways>, Without<EncounterMember>)>,
) {
    // A body the run let go (a corpse at a wave clear, a released add, a dead member of a won run)
    // lost its marker: take off what this module put on it, and only that.
    for entity in &left {
        commands.entity(entity).remove::<(NetAlways, OurNetAlways)>();
    }
    for (entity, member, has, ours) in &members {
        let Some(run) = runs.as_ref().and_then(|r| r.get(&member.key)) else { continue };
        // Peeked: presentation of replication, not something the simulation's outcome reads.
        if !db.peek::<EncounterDef>(&run.encounter).is_some_and(|d| d.net_always) {
            continue;
        }
        let fighting = matches!(run.state, RunState::Running | RunState::Between { .. } | RunState::Wiped { .. } | RunState::Resetting { .. });
        if fighting && !has {
            commands.entity(entity).insert((NetAlways, OurNetAlways));
        } else if !fighting && ours {
            commands.entity(entity).remove::<(NetAlways, OurNetAlways)>();
        }
    }
}

/// A wave's warning plays its effect at the anchor. Presentation only: it changes no state, so it
/// runs on clients too, from the retriggered event.
fn play_effect(event: On<WaveWarned>, mut commands: Commands) {
    if let Some(effect) = event.cue.as_ref().and_then(|c| c.effect.clone()) {
        commands.trigger(shards_fx::PlayEffect::new(effect, shards_fx::EffectAt::Point(event.at)));
    }
}

/// A wave's warning plays its sound at the anchor.
fn play_sound(event: On<WaveWarned>, mut commands: Commands) {
    if let Some(sound) = event.cue.as_ref().and_then(|c| c.sound.clone()) {
        commands.trigger(shards_audio::PlaySound::new(sound, shards_audio::SoundAt::Point { position: event.at, source: None }));
    }
}

/// The cues of an encounter name effects and sounds that exist, and messages every language has.
fn check_cues(def: &EncounterDef, _: &str, refs: &mut Refs) {
    let mut texts: Vec<(&str, String)> = Vec::new();
    for (w, wave) in def.waves.iter().enumerate() {
        let Some(cue) = &wave.warn else { continue };
        if let Some(effect) = &cue.effect
            && refs.db().has_kind("effect")
        {
            refs.require::<shards_fx::EffectDef>(effect, &format!("waves[{w}].warn.effect"));
        }
        if let Some(sound) = &cue.sound
            && refs.db().has_kind("sound")
        {
            refs.require::<shards_audio::SoundDef>(sound, &format!("waves[{w}].warn.sound"));
        }
        if let Some(message) = &cue.message {
            texts.push((message.as_str(), format!("waves[{w}].warn.message")));
        }
    }
    if !texts.is_empty() && refs.db().has_kind("locale") {
        shards_locale::check_messages(refs, texts);
    }
}

#[cfg(test)]
mod tests {
    use ai_core::{snapshot, AiId, AiIndex, AiKind};

    use super::*;

    /// The marker sits on encounter members, entities with an `AiId`: a snapshot holds it and a load
    /// puts it back. Otherwise a game loaded in the middle of a `net_always` fight never takes that
    /// `NetAlways` off (issue #72: heavy mode stopped the nightly fuzz campaign on `left_out differs`).
    #[test]
    fn a_snapshot_holds_the_marker_and_a_load_puts_it_back() {
        let mut app = crate::testing::test_app(|_| {}, |_| {});
        let member = app.world_mut().spawn((AiKind::new("Member"), Transform::default(), NetAlways, OurNetAlways)).id();
        let id = *app.world().get::<AiId>(member).expect("the engine gives it an id");
        let saved = snapshot::capture(app.world()).unwrap();
        assert!(saved.left_out.is_empty(), "the snapshot leaves out {:?}", saved.left_out);
        let mut twin = crate::testing::test_app(|_| {}, |_| {});
        let report = snapshot::restore(twin.world_mut(), &saved, false).unwrap();
        assert!(report.exact(), "{:?}", report.differences);
        let loaded = twin.world().resource::<AiIndex>().get(id).expect("the member is loaded");
        assert!(twin.world().get::<NetAlways>(loaded).is_some(), "NetAlways is loaded");
        assert!(twin.world().get::<OurNetAlways>(loaded).is_some(), "so is the mark that this module put it there");
        // Bookkeeping: the replay checks hash NetAlways, not the mark, as before it reflected.
        let hashing = twin.world().resource::<ai_core::replay::ReplayHashing>();
        assert!(hashing.is_ignored(std::any::TypeId::of::<OurNetAlways>()) && !hashing.is_ignored(std::any::TypeId::of::<NetAlways>()));
    }
}
