//! Typed events a host's clients see too ([`NetEventAppExt::net_event`]): what a client that
//! simulates nothing still has to show (a hit spark, a death cry, a telegraph's sound). The
//! host encodes every such event it triggers, entities as their `eN`; a client triggers it
//! again once the host's world it shows is at the event's tick or later, so the entities the
//! event names are there.
//!
//! A client's observers of these events run on a world already past the event, and a client
//! does not simulate: keep them to presentation, as every observer that changes gameplay
//! state returns early unless [`shards_core::state::simulates`].

use std::collections::{BTreeMap, VecDeque};

use ai_core::state::{decode_value, encode_value};
use ai_core::{journal::JournalKind, sim};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::reflect::{FromReflect, GetTypeRegistration, TypePath};
use serde_json::Value;

use crate::host::NetHost;

/// Turns an event's JSON back into the event, in this world, and triggers it.
type Retrigger = fn(&mut World, &Value) -> Result<(), String>;

/// The events registered with `net_event`, by type path.
#[derive(Resource, Default, Clone)]
pub struct NetEvents {
    retrigger: BTreeMap<String, Retrigger>,
}

impl NetEvents {
    /// The registered type paths, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.retrigger.keys().map(String::as_str)
    }
}

/// Host: events triggered since the last send, `(tick, type path, value)`.
#[derive(Resource, Default)]
pub(crate) struct EventOutbox(pub Vec<(u64, String, Value)>);

/// Client: the host's events waiting for its world to reach their tick.
#[derive(Resource, Default)]
pub(crate) struct EventInbox(pub VecDeque<(u64, String, Value)>);

pub trait NetEventAppExt {
    /// Every `E` a host triggers is sent to its clients, which trigger it once they show the
    /// host's world at its tick. Entities in `E` travel as their `eN`; one a client does not
    /// have reads as `Entity::PLACEHOLDER`.
    fn net_event<E>(&mut self) -> &mut Self
    where
        E: for<'a> Event<Trigger<'a> = bevy::ecs::event::GlobalTrigger> + Reflect + FromReflect + TypePath + GetTypeRegistration + Clone;
}

impl NetEventAppExt for App {
    fn net_event<E>(&mut self) -> &mut Self
    where
        E: for<'a> Event<Trigger<'a> = bevy::ecs::event::GlobalTrigger> + Reflect + FromReflect + TypePath + GetTypeRegistration + Clone,
    {
        self.register_type::<E>();
        let mut events = self.world_mut().get_resource_or_init::<NetEvents>();
        if events.retrigger.contains_key(E::type_path()) {
            return self;
        }
        events.retrigger.insert(E::type_path().to_owned(), |world, value| {
            let reflected = decode_value(world, E::type_path(), value)?;
            let event = E::from_reflect(reflected.as_ref()).ok_or_else(|| format!("`{}` does not read back", E::type_path()))?;
            world.trigger(event);
            Ok(())
        });
        self.add_observer(|event: On<E>, mut world: DeferredWorld| {
            if !world.contains_resource::<NetHost>() {
                return;
            }
            let tick = sim::tick(&world);
            match encode_value(&world, event.event()) {
                Ok(value) => {
                    if let Some(mut outbox) = world.get_resource_mut::<EventOutbox>() {
                        outbox.0.push((tick, E::type_path().to_owned(), value));
                    }
                }
                Err(e) => warn!("net: `{}` not sent: {e}", E::type_path()),
            }
        });
        self
    }
}

/// Client: triggers every waiting event of a host tick up to `shown`.
pub(crate) fn retrigger_due(world: &mut World, shown: u64) {
    let due: Vec<(u64, String, Value)> = {
        let Some(mut inbox) = world.get_resource_mut::<EventInbox>() else { return };
        let n = inbox.0.iter().take_while(|(tick, _, _)| *tick <= shown).count();
        inbox.0.drain(..n).collect()
    };
    if due.is_empty() {
        return;
    }
    let events = world.get_resource::<NetEvents>().cloned().unwrap_or_default();
    for (tick, path, value) in due {
        if let Some(mut prediction) = world.get_resource_mut::<shards_core::NetPrediction>() {
            prediction.event_tick = tick;
        }
        let result = match events.retrigger.get(&path) {
            Some(retrigger) => retrigger(world, &value),
            None => Err(format!("`{path}` is not a net_event of this game")),
        };
        if let Err(e) = result {
            crate::hooks::net_line(world, "event", JournalKind::Warn, format!("event of host tick {tick} not shown: {e}"));
        }
    }
}
