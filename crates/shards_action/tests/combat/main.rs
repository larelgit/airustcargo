//! `shards_action` integration tests, one binary: headless arenas with test content,
//! driven tick by tick through tools and `Intent`.

mod actions;
mod agents;
mod body;
mod common;
mod golden;
mod hits;
mod modifiers;
mod outcome;
mod parity;
mod reactions;
mod timeline;
mod replay;
#[cfg(feature = "render")]
mod render;
mod visuals;
