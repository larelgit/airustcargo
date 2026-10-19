//! The render side of the action module on the real engine: a skinned glTF model, its clips in
//! a second file that shares the skeleton, characters driven by the action machine through
//! `Intent`, the pose that comes out of Bevy's animation, and, in a window, what the telegraphs,
//! the combat overlay and the off-screen markers look like.
//!
//! `headless` runs in the suite: the engine's render plugins stay registered without a GPU, so
//! glTF loading, skins and animation all work. `window` needs a display and a Vulkan driver
//! (software Vulkan will do: `mesa-vulkan-drivers`, lavapipe); it is how the windowed side was
//! looked at, and saves two pictures:
//!
//! ```text
//! xvfb-run -a cargo test -p shards --test action_render -- --ignored --nocapture
//! ```

#![cfg(feature = "render")]

mod fixtures;
mod headless;
mod modes;
mod sockets;
mod window;
