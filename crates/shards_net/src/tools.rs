//! What agents see of the session: `net_status`, `net_sim`, `net_kick`, `net_resync`,
//! `net_end`, the conditions `net_desync`, `net_joined`, `net_left`, `net_disconnected`, the
//! `net` field of `ai/status` and the `NET` line of `observe`.

use ai_core::tools::{register_tool, NetRoute, ToolKind, ON_THE_HOST};
use ai_core::{sim, AiAppExt, Call, NoArgs, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use shards_core::NetRole;

use crate::client::{NetClient, NetDesync};
use crate::conditioner::LinkConditions;
use crate::host::{end_session, NetHost};
use crate::session::{role, NetEndpoint, NetMarks, NetState};

pub(crate) fn register(app: &mut App) {
    let world = app.world_mut();
    register_tool(world, "net_status", Scope::Player, ToolKind::Query, net_status);
    register_tool(world, "net_sim", Scope::Dev, ToolKind::Control, net_sim);
    register_tool(world, "net_kick", Scope::Dev, ToolKind::Control, net_kick);
    register_tool(world, "net_resync", Scope::Dev, ToolKind::Control, net_resync);
    register_tool(world, "net_end", Scope::Dev, ToolKind::Control, net_end);
    app.net_route("net_status", NetRoute::Local)
        .net_route("net_sim", NetRoute::Local)
        .net_route("net_kick", NetRoute::HostOnly(ON_THE_HOST))
        .net_route("net_resync", NetRoute::HostOnly(ON_THE_HOST))
        .net_route("net_end", NetRoute::HostOnly(ON_THE_HOST))
        .add_ai_condition("net_desync", |w| recent(w, |m| m.desync))
        .add_ai_condition("net_joined", |w| recent(w, |m| m.joined))
        .add_ai_condition("net_left", |w| recent(w, |m| m.left))
        .add_ai_condition("net_disconnected", |w| recent(w, |m| m.disconnected))
        .add_ai_status("net", status)
        .register_ai_observe_lines(observe_lines);
}

/// The mark is from this tick or the two before (a condition checked after each tick).
fn recent(world: &World, which: fn(&NetMarks) -> Option<u64>) -> bool {
    let now = sim::tick(world);
    world.get_resource::<NetMarks>().and_then(which).is_some_and(|t| t + 2 >= now && t <= now)
}

fn role_name(role: NetRole) -> &'static str {
    match role {
        NetRole::Single => "single",
        NetRole::Host => "host",
        NetRole::Client => "client",
    }
}

fn state_text(world: &World) -> String {
    match world.get_resource::<NetState>().cloned().unwrap_or_default() {
        NetState::Offline => "offline".into(),
        NetState::Hosting => "hosting".into(),
        NetState::Joining(why) => format!("joining: {why}"),
        NetState::Connected { player, body } => format!("connected as p{player}{}", body.map_or_else(String::new, |b| format!(" (e{b})"))),
        NetState::Ended(why) => format!("ended: {why}"),
    }
}

/// `ai/status.net`: `{role, member, player, body, session, host_tick, peers}` (and the
/// desync count; a UDP host's bound address in `udp`), what `shards wait` prints of a member.
fn status(world: &World) -> Value {
    let role = role(world);
    let member = world.get_resource::<shards_core::EngineConfig>().map(|c| c.player_name.clone()).unwrap_or_default();
    let mut out = json!({ "role": role_name(role), "session": state_text(world), "member": member });
    // A stand's host knows how many clients it was started with (`shards wait --peer all`).
    if let Some(shards_core::NetLaunch::Stand { clients, udp }) = world.get_resource::<shards_core::EngineConfig>().map(|c| &c.net) {
        out["stand"] = json!({ "clients": clients, "udp": udp });
    }
    if let Some(host) = world.get_resource::<NetHost>() {
        let body = world
            .try_query_filtered::<&ai_core::AiId, With<ai_core::AiPlayer>>()
            .and_then(|mut q| q.iter(world).next().map(|id| format!("e{}", id.0)));
        out["player"] = json!(0);
        out["body"] = json!(body);
        out["host_tick"] = json!(sim::tick(world));
        let peers: Vec<Value> = host
            .connected()
            .map(|p| json!({ "player": p.player, "name": p.hello.as_ref().map(|h| h.name.clone()), "body": p.body.map(|b| format!("e{b}")) }))
            .collect();
        out["peers"] = json!(peers);
        out["desyncs"] = json!(host.desyncs);
        // The address clients `--join` (the port actually bound: `--host 0` picks one).
        if let Some(udp) = world.get_resource::<NetEndpoint>().and_then(|e| e.transport.local_addr()) {
            out["udp"] = json!(udp);
        }
    }
    if let Some(client) = world.get_resource::<NetClient>() {
        out["host"] = json!(client.address);
        out["player"] = json!(client.player);
        out["body"] = json!(client.body.map(|b| format!("e{b}")));
        out["host_tick"] = json!(client.newest_host_tick);
        out["desyncs"] = json!(world.get_resource::<NetDesync>().map_or(0, |d| d.count));
    }
    out
}

/// The `NET` line under `YOU`.
fn observe_lines(world: &World, _: Entity) -> Vec<String> {
    if let Some(host) = world.get_resource::<NetHost>() {
        let players: Vec<String> = host
            .connected()
            .map(|p| format!("p{} {} rtt {:.0}ms", p.player, p.hello.as_ref().map_or("?", |h| h.name.as_str()), p.link.stats().srtt_ms))
            .collect();
        let players = if players.is_empty() { "no players joined".to_owned() } else { players.join(" · ") };
        return vec![format!("NET host, {players}")];
    }
    if let Some(client) = world.get_resource::<NetClient>() {
        let rtt = client.link().map_or(0.0, |l| l.stats().srtt_ms);
        let desyncs = world.get_resource::<NetDesync>().map_or(0, |d| d.count);
        return vec![format!(
            "NET {} on host {}, rtt {rtt:.0}ms, host tick {}, {} ticks ahead, desyncs {desyncs}",
            state_text(world),
            client.address,
            client.newest_host_tick,
            sim::tick(world).saturating_sub(client.newest_host_tick)
        )];
    }
    Vec::new()
}

/// The ten components (or resources) that weighed most in the state sent to every peer so
/// far: JSON bytes before deflate, and their share.
fn by_component(host: &NetHost) -> Value {
    let mut total: std::collections::BTreeMap<&str, u64> = std::collections::BTreeMap::new();
    for peer in host.peers.values() {
        for (path, bytes) in &peer.by_component {
            *total.entry(path.as_str()).or_default() += bytes;
        }
    }
    let all: u64 = total.values().sum();
    let mut top: Vec<(&str, u64)> = total.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let top: Vec<Value> = top
        .into_iter()
        .take(10)
        .map(|(path, bytes)| {
            let share = if all > 0 { (bytes as f64 * 1000.0 / all as f64).round() / 10.0 } else { 0.0 };
            json!({ "component": short_path(path), "bytes": bytes, "percent": share })
        })
        .collect();
    json!(top)
}

/// `shards_core::state::Health` → `Health` (generic arguments kept).
fn short_path(path: &str) -> String {
    let (name, args) = path.split_at(path.find('<').unwrap_or(path.len()));
    format!("{}{args}", name.rsplit("::").next().unwrap_or(name))
}

/// The session: role, players, links (RTT, loss, bytes per second), inputs (late, early,
/// dropped), state sends, desyncs.
fn net_status(In(_): In<Call<NoArgs>>, world: &mut World) -> ToolResult<Value> {
    let mut out = status(world);
    out["tick"] = json!(sim::tick(world));
    if let Some(endpoint) = world.get_resource::<NetEndpoint>() {
        out["transport"] = json!(endpoint.transport.describe());
        out["conditions"] = json!(endpoint.transport.conditions().map(|c| c.to_string()));
    }
    if let Some(host) = world.get_resource::<NetHost>() {
        let peers: Vec<Value> = host
            .peers
            .values()
            .map(|p| {
                let s = p.link.stats();
                json!({
                    "player": p.player,
                    "name": p.hello.as_ref().map(|h| h.name.clone()),
                    "stage": format!("{:?}", p.stage),
                    "body": p.body.map(|b| format!("e{b}")),
                    "rtt_ms": s.srtt_ms,
                    "loss": s.loss,
                    "bytes_in_per_s": s.bytes_in_per_s,
                    "bytes_out_per_s": s.bytes_out_per_s,
                    "inputs": { "late": p.inputs.late, "early": p.inputs.early, "dropped": p.inputs.dropped, "duplicates": p.inputs.duplicates, "waiting": p.inputs.waiting() },
                    "margin": p.margin.map(|m| m.1),
                    "deltas": p.deltas,
                    "delta_bytes_avg": p.delta_bytes.checked_div(p.deltas).unwrap_or(0),
                    "held_back": p.held_back,
                    "view": p.view.len(),
                    "desyncs": p.desyncs,
                    "resyncs": p.resyncs,
                })
            })
            .collect();
        let (encoded, reused) = host.encode_counts();
        out["peers"] = json!(peers);
        out["bandwidth_by_component"] = by_component(host);
        let mean = host.encode_total.0.checked_div(host.encode_total.1).unwrap_or(0);
        out["encode_us"] = json!({ "last": host.encode_us.0, "max": host.encode_us.1, "mean": mean });
        out["encode_cache"] = json!({ "encoded": encoded, "reused": reused });
        let slots: Vec<Value> = host.slots.iter().map(|(n, s)| json!({ "player": n, "name": s.name, "left_at": s.left_at })).collect();
        out["slots"] = json!(slots);
    }
    if let Some(client) = world.get_resource::<NetClient>() {
        let s = client.link().map(|l| l.stats()).unwrap_or_default();
        out["stage"] = json!(format!("{:?}", client.stage));
        out["body"] = json!(client.body.map(|b| format!("e{b}")));
        out["rtt_ms"] = json!(s.srtt_ms);
        out["loss"] = json!(s.loss);
        out["bytes_in_per_s"] = json!(s.bytes_in_per_s);
        out["bytes_out_per_s"] = json!(s.bytes_out_per_s);
        out["newest_host_tick"] = json!(client.newest_host_tick);
        out["lead"] = json!(client.lead);
        out["clock"] = json!({ "error": client.clock.error(), "target": client.clock.target, "adjusted": client.clock.adjusted });
        out["inputs_unacked"] = json!(client.capture.unacked.len());
        out["deltas"] = json!({ "applied": client.deltas, "skipped": client.skipped, "stale": client.ring.stale, "missing_baseline": client.ring.missing_baseline, "bytes": client.delta_bytes });
        out["rebases"] = json!(client.rebases);
        if let Some(desync) = world.get_resource::<NetDesync>().and_then(|d| d.last.clone()) {
            out["last_desync"] = json!({ "host_tick": desync.0, "tick": desync.1, "first": desync.2 });
        }
    }
    Ok(out)
}

/// Simulate network conditions on this machine's links: `latency=150,jitter=10,loss=0.02,dup=0`
/// (`none` to stop).
#[derive(Deserialize, JsonSchema)]
pub struct NetSimArgs {
    pub conditions: String,
}

fn net_sim(In(call): In<Call<NetSimArgs>>, world: &mut World) -> ToolResult<Value> {
    let conditions: LinkConditions = if call.args.conditions.trim() == "none" { LinkConditions::NONE } else { call.args.conditions.parse()? };
    conditions.validate()?;
    let mut endpoint = world.get_resource_mut::<NetEndpoint>().ok_or("this game has no network link; start it with --host, --join or --stand")?;
    if !endpoint.transport.set_conditions(None, conditions) {
        return Err(format!("this transport ({}) cannot simulate conditions; start it with --net-sim", endpoint.transport.describe()));
    }
    Ok(json!(format!("links now {conditions}")))
}

/// Send a player away (they may rejoin with the same token while their body waits).
#[derive(Deserialize, JsonSchema)]
pub struct NetKickArgs {
    pub player: u8,
    /// Shown to the player.
    pub reason: Option<String>,
}

fn net_kick(In(call): In<Call<NetKickArgs>>, world: &mut World) -> ToolResult<Value> {
    let mut host = world.get_resource_mut::<NetHost>().ok_or("this game is not hosting")?;
    let reason = format!("kicked: {}", call.args.reason.unwrap_or_else(|| "by the host".into()));
    host.kick(call.args.player, &reason)?;
    Ok(json!(format!("p{} leaves at the next tick ({reason})", call.args.player)))
}

/// Send the whole world again to one player (or all).
#[derive(Deserialize, JsonSchema)]
pub struct NetResyncArgs {
    pub player: Option<u8>,
}

fn net_resync(In(call): In<Call<NetResyncArgs>>, world: &mut World) -> ToolResult<Value> {
    let mut host = world.get_resource_mut::<NetHost>().ok_or("this game is not hosting")?;
    match host.resync(call.args.player, "asked") {
        0 => Err("no such player is connected; net_status lists them".into()),
        n => Ok(json!(format!("the world goes to {n} player(s) at the next send"))),
    }
}

/// End the session: every player gets their character and is told.
#[derive(Deserialize, JsonSchema)]
pub struct NetEndArgs {
    pub reason: Option<String>,
}

fn net_end(In(call): In<Call<NetEndArgs>>, world: &mut World) -> ToolResult<Value> {
    match world.get_resource::<NetHost>() {
        None => return Err("this game is not hosting".into()),
        Some(host) if host.ended.is_some() => {
            return Err(format!("the session has ended already ({}); net_status shows it", host.ended.as_deref().unwrap_or_default()));
        }
        Some(_) => {}
    }
    let reason = call.args.reason.unwrap_or_else(|| "the host ended the session".into());
    end_session(world, &reason);
    Ok(json!(format!("session ended: {reason}")))
}
