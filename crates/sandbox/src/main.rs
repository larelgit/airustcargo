use clap::Parser;
use sandbox::{build_app, GameConfig};

/// Sandbox arena for the ai_core layer.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// No window and no rendering (CI, parallel playtests).
    #[arg(long)]
    headless: bool,
    /// World seed: same seed + same inputs = same game.
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// BRP port for `shards` (use different ports for parallel instances).
    #[arg(long, default_value_t = ai_core::DEFAULT_PORT)]
    port: u16,
    /// Do not start the BRP server.
    #[arg(long)]
    no_remote: bool,
    /// Start paused; advance with `shards step`.
    #[arg(long)]
    paused: bool,
    /// Simulation ticks per second.
    #[arg(long, default_value_t = 60.0)]
    hz: f64,
}

fn main() {
    let args = Args::parse();
    let cfg = GameConfig {
        headless: args.headless,
        seed: args.seed,
        port: (!args.no_remote).then_some(args.port),
        paused: args.paused,
        tick_hz: args.hz,
    };
    build_app(&cfg).run();
}
