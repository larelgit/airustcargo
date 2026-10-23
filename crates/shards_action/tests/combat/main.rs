//! `shards_action` integration tests, one binary: headless arenas with test content,
//! driven tick by tick through tools and `Intent`.

mod actions;
mod agents;
mod body_motion;
mod body_state;
mod common;
mod context;
mod crush;
mod golden;
mod hits;
mod lethal;
mod lock;
mod lock_points;
mod modifiers;
mod objects;
mod outcome;
mod parity;
mod parts;
mod reactions;
mod shots;
mod timeline;
mod weapons;
mod replay;
mod rules_costs;
mod rules_damage;
#[cfg(feature = "render")]
mod render;
mod visuals;
