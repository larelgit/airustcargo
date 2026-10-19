//! `shards_action` integration tests, one binary: headless arenas with test content,
//! driven tick by tick through tools and `Intent`.

mod actions;
mod agents;
mod body_motion;
mod body_state;
mod common;
mod context;
mod golden;
mod hits;
mod lethal;
mod lock;
mod modifiers;
mod objects;
mod outcome;
mod parity;
mod reactions;
mod timeline;
mod replay;
#[cfg(feature = "render")]
mod render;
mod visuals;
