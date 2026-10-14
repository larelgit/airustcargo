//! The standard `main` of a Shards game: `shards::run` reads the command line
//! (`--headless --seed --port --port-file --no-remote --paused --hz --replay --verify
//! --check-content`), builds the game with the config it made, and runs it.

fn main() -> std::process::ExitCode {
    shards::run(my_game::build_app)
}
