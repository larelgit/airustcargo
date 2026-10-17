//! The standard `main` of a Shards game: `shards::run` reads the command line
//! (`--headless --seed --port --port-file --no-remote --paused --hz --replay --verify
//! --check-content --smoke`), builds the game with the config it made, and runs it.

// The build for players opens no console window beside the game's on Windows (`shards::run`
// still writes to the terminal it was started from).
#![cfg_attr(all(windows, feature = "dist"), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    shards::run(my_game::build_app)
}
