//! The co-op session on top of the link layer: [`NetPlugin`], its system sets and the resources
//! both roles share.
//!
//! All network I/O runs inside the fixed tick, so the same tick sees the same messages in every
//! run of a stand:
//!
//! * `FixedFirst`, before the replay's calls: [`NetSystems::Receive`] (both roles), then on the
//!   host [`NetSystems::ApplyCalls`] (joins, leaves, forwarded calls: recorded like any agent's)
//!   and [`NetSystems::ApplyInputs`] (frames into remote players' `Intent`, which the replay then
//!   records as inputs), on a client [`NetSystems::ApplyState`], [`NetSystems::Correct`] and
//!   [`NetSystems::Capture`].
//! * `FixedLast`, after the replay remembered the tick: [`NetSystems::Desync`] and
//!   [`NetSystems::Send`].
//! * `Last`, only in frames where no tick ran: [`NetSystems::Pump`] (a paused host still admits
//!   players; a joining client talks to its host while it loads).

use std::sync::Arc;

use ai_core::journal::{self, JournalKind};
use ai_core::replay::ReplaySystems;
use ai_core::sim::SimSet;
use ai_core::tools::NetRoute;
use ai_core::{AgentSessions, AiAppExt, AiTools, Scope};
use bevy::prelude::*;
use shards_content::ContentAppExt;
use shards_core::{CoreSystems, EngineAppExt, EngineConfig, Intent, NetLaunch, NetRole, NetSet};

use crate::client::{self, NetClient};
use crate::def::NetDef;
use crate::host::{self, NetHost};
use crate::hooks::{self, despawn_left};
use crate::hub::MemoryHub;
use crate::replicate::NetAlways;
use crate::transport::{PeerId, Transport};
use crate::udp::WallClock;

/// The network systems, in the order they run within a tick (see the module docs).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetSystems {
    Receive,
    ApplyCalls,
    ApplyInputs,
    ApplyState,
    Correct,
    Capture,
    Desync,
    Send,
    Pump,
}

/// Where the session's time comes from: the hub's clock in a stand and in CI, the wall clock
/// over UDP.
#[derive(Clone)]
pub enum NetClock {
    Hub(MemoryHub),
    Wall(Arc<WallClock>),
}

impl NetClock {
    /// Milliseconds on this clock.
    pub fn now(&self) -> u64 {
        match self {
            NetClock::Hub(hub) => hub.now(),
            NetClock::Wall(clock) => clock.now_ms(),
        }
    }

    pub fn is_wall(&self) -> bool {
        matches!(self, NetClock::Wall(_))
    }
}

/// This game's transport. Inserted before the first update by whoever builds a stand
/// (`stand::attach`); otherwise the plugin opens a UDP socket at startup (`--host`, `--join`).
#[derive(Resource)]
pub struct NetEndpoint {
    pub transport: Box<dyn Transport>,
    pub clock: NetClock,
    /// A client: its host on this transport.
    pub host: Option<PeerId>,
}

/// Where this game is in the session, for tools and conditions.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub enum NetState {
    /// No network.
    #[default]
    Offline,
    /// Hosting: players can join.
    Hosting,
    /// Joining a host: what it waits for.
    Joining(String),
    /// Playing on a host as `player`, in body `body`.
    Connected { player: u8, body: Option<u32> },
    /// The session is over (refused, kicked, ended by the host, lost): why.
    Ended(String),
}

/// The game a client must be to join: its name and version as the game calls itself.
/// Default `game`; a game sets it (`app.insert_resource(NetGame("my game 0.3".into()))`).
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct NetGame(pub String);

impl Default for NetGame {
    fn default() -> Self {
        NetGame("game".into())
    }
}

/// Ticks at which session events happened last (conditions `net_joined`, `net_left`,
/// `net_disconnected`, `net_desync`).
#[derive(Resource, Clone, Debug, Default)]
pub struct NetMarks {
    pub joined: Option<u64>,
    pub left: Option<u64>,
    pub disconnected: Option<u64>,
    pub desync: Option<u64>,
}

impl NetMarks {
    pub fn mark(world: &mut World, which: fn(&mut NetMarks) -> &mut Option<u64>) {
        let tick = ai_core::sim::tick(world);
        *which(&mut world.get_resource_or_init::<NetMarks>()) = Some(tick);
    }
}

/// Set when a tick ran in this frame: the pump then has nothing to do.
#[derive(Resource, Default)]
pub(crate) struct Ticked(pub bool);

/// The agent the session runs its own calls as (`player_join`, `player_leave`).
pub const NET_AGENT: &str = "net";

/// Why a client over UDP cannot `pause`, `resume` or `speed`.
pub const HOST_OWNS_TIME: &str = "the host owns time; call it on the host";

/// The session: roles from `EngineConfig::net`, join, inputs, replication, desync check, the
/// `net_*` tools. Without `--host`, `--join` or `--stand` it only adds `player_join` and
/// `player_leave` (co-op logic on one machine) and stays out of the way.
pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.add_engine_module("net", env!("CARGO_PKG_VERSION"))
            .register_type::<NetAlways>()
            .add_content::<NetDef>()
            .init_resource::<NetState>()
            .init_resource::<NetMarks>()
            .init_resource::<Ticked>()
            .init_resource::<crate::events::NetEvents>()
            .init_resource::<crate::events::EventOutbox>()
            .init_resource::<crate::events::EventInbox>()
            .init_resource::<crate::predict::NetCorrection>()
            // A client sends its own intent; the host's copy of it is never sent back.
            .net_predicted::<Intent>()
            .add_systems(FixedUpdate, despawn_left.in_set(SimSet::Cleanup).before(CoreSystems::ClearPresses));
        app.world_mut().get_resource_or_init::<hooks::NetHooks>();
        app.world_mut().get_resource_or_init::<NetGame>();
        hooks::register(app);
        crate::tools::register(app);
        crate::overlay::plugin(app);
        #[cfg(feature = "render")]
        crate::interp::plugin(app);

        app.configure_sets(
            FixedFirst,
            (
                NetSystems::Receive,
                NetSystems::ApplyCalls,
                NetSystems::ApplyInputs,
                NetSystems::ApplyState,
                NetSystems::Correct,
                NetSystems::Capture,
            )
                .chain()
                .before(ReplaySystems::Calls),
        )
        .configure_sets(FixedLast, (NetSystems::Desync, NetSystems::Send).chain().after(ReplaySystems::Remember).after(journal::drain_logs))
        .add_systems(
            FixedFirst,
            (
                host::receive.in_set(NetSystems::Receive).run_if(resource_exists::<NetHost>),
                host::apply_calls.in_set(NetSystems::ApplyCalls).run_if(resource_exists::<NetHost>),
                host::apply_inputs.in_set(NetSystems::ApplyInputs).run_if(resource_exists::<NetHost>),
                client::receive.in_set(NetSystems::Receive).run_if(resource_exists::<NetClient>),
                client::apply_state.in_set(NetSystems::ApplyState).run_if(resource_exists::<NetClient>),
                crate::predict::correct.in_set(NetSystems::Correct).run_if(resource_exists::<NetClient>),
                client::capture.in_set(NetSystems::Capture).run_if(resource_exists::<NetClient>),
            ),
        )
        .add_systems(
            FixedLast,
            (
                host::send.in_set(NetSystems::Send).run_if(resource_exists::<NetHost>),
                client::send.in_set(NetSystems::Send).run_if(resource_exists::<NetClient>),
            ),
        )
        // A client's own body: the inputs it sent for the tick in, and the tick recorded.
        .add_systems(
            FixedUpdate,
            (
                crate::predict::predict_input.in_set(NetSet::PredictInput),
                crate::predict::predict_end.in_set(NetSet::PredictEnd),
            ),
        )
        .add_systems(Last, pump.in_set(NetSystems::Pump))
        .add_systems(Startup, open_endpoint)
        .add_observer(host::on_world_rebuilt);

        let config = app.world().get_resource::<EngineConfig>().cloned().unwrap_or_default();
        match &config.net {
            NetLaunch::Off => {}
            NetLaunch::Host { .. } | NetLaunch::Stand { .. } => {
                app.insert_resource(NetRole::Host)
                    .insert_resource(NetState::Hosting)
                    .insert_resource(NetHost::new())
                    .init_resource::<host::PendingRebuild>();
                app.add_exit_hook("net", host::on_exit);
            }
            NetLaunch::Join { addr } => {
                let world = app.world_mut();
                world.insert_resource(NetRole::Client);
                shards_core::seed::hold_seed(world, &format!("net: connecting to {addr}"));
                journal::set_local_events(world, false);
                ai_core::replay::set_recording(world, false);
                world.insert_resource(NetState::Joining(format!("connecting to {addr}")));
                world.insert_resource(NetClient::new(addr, &config));
                app.add_exit_hook("net", client::on_exit);
            }
        }
    }
}

/// The exit code of a game started with `--exit-with-host` whose session could not open (a
/// port it cannot host on, an address it cannot reach).
pub const NO_SESSION_EXIT: u8 = 2;

/// `Startup`: the session's agent, and a UDP socket unless a stand put a transport in already.
fn open_endpoint(world: &mut World) {
    world.resource_mut::<AgentSessions>().configure(NET_AGENT, Scope::Dev, None);
    client::install_forwarder(world);
    if world.contains_resource::<NetEndpoint>() {
        return;
    }
    let config = world.get_resource::<EngineConfig>().cloned().unwrap_or_default();
    let clock = NetClock::Wall(Arc::new(WallClock::new()));
    match &config.net {
        NetLaunch::Host { port } => match crate::udp::UdpTransport::bind_port(*port) {
            Ok(udp) => {
                // The port actually bound (`--host 0` picks a free one).
                let at = udp.socket_addr().map_or_else(|_| format!("port {port}"), |a| a.to_string());
                journal::emit(world, JournalKind::Sim, format!("net: hosting on UDP {at}"));
                let transport = crate::conditioner_from(&config, udp);
                world.insert_resource(NetEndpoint { transport, clock, host: None });
            }
            Err(e) => {
                journal::emit(world, JournalKind::Error, format!("net: cannot host on UDP port {port}: {e}; pick another with --host PORT"));
                world.insert_resource(NetState::Ended(format!("cannot host on port {port}: {e}")));
                if config.exit_with_host {
                    world.write_message(AppExit::from_code(NO_SESSION_EXIT));
                }
            }
        },
        NetLaunch::Join { addr } => {
            let opened = crate::udp::UdpTransport::bind("0.0.0.0:0").map_err(|e| e.to_string()).and_then(|mut udp| {
                let host = udp.connect_str(addr)?;
                Ok((udp, host))
            });
            match opened {
                Ok((udp, host)) => {
                    let transport = crate::conditioner_from(&config, udp);
                    world.insert_resource(NetEndpoint { transport, clock, host: Some(host) });
                    // Over a real network the host owns time (in a stand, its driver runs every
                    // member's: a stand's client keeps them).
                    let mut tools = world.resource_mut::<AiTools>();
                    for tool in ["pause", "resume", "speed"] {
                        if let Err(e) = tools.set_route(tool, NetRoute::HostOnly(HOST_OWNS_TIME)) {
                            warn!("net: {e}");
                        }
                    }
                }
                Err(e) => {
                    journal::emit(world, JournalKind::Error, format!("net: cannot reach {addr}: {e}; give it as HOST:PORT"));
                    // Nothing will come: the world waits with the reason (`status`), or the game
                    // exits (`--exit-with-host`).
                    let reason = format!("cannot reach {addr}: {e}");
                    shards_core::seed::hold_seed(world, &format!("net: {reason}"));
                    if let Some(mut client) = world.remove_resource::<NetClient>() {
                        client::end(world, &mut client, &reason, AppExit::from_code(NO_SESSION_EXIT));
                        world.insert_resource(client);
                    }
                }
            }
        }
        _ => {}
    }
}

/// `Last`: the session's I/O in frames where no tick ran.
fn pump(world: &mut World) {
    let ticked = std::mem::take(&mut world.resource_mut::<Ticked>().0);
    if ticked {
        return;
    }
    if world.contains_resource::<NetHost>() {
        host::pump(world);
    }
    if world.contains_resource::<NetClient>() {
        client::pump(world);
    }
}

/// Milliseconds on the session's clock (0 without an endpoint).
pub fn now_ms(world: &World) -> u64 {
    world.get_resource::<NetEndpoint>().map_or(0, |e| e.clock.now())
}

/// The session's role, or single player.
pub fn role(world: &World) -> NetRole {
    world.get_resource::<NetRole>().copied().unwrap_or_default()
}
