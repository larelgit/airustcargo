//! The net overlay: the session's links on screen, one line per peer (ping, loss, kbps in
//! and out, the input margin, desyncs) and, on a client, its prediction (error, snaps,
//! mispredicted actions). Toggle it with the `net_overlay` tool or F6; it draws in a window
//! only (`render` feature), the tool answers the same lines anywhere.

use ai_core::dev::toggle_line;
use ai_core::tools::{register_tool, ToolKind};
use ai_core::{sim, AiAppExt, Call, DevCommand, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_core::NetPrediction;

use crate::client::{NetClient, NetDesync};
use crate::host::NetHost;
use crate::predict::NetCorrection;
use crate::stats::LinkStats;

/// Whether the net overlay is drawn, and the key that toggles it in a window.
#[derive(Resource, Clone, Copy, Debug)]
pub struct NetOverlay {
    pub enabled: bool,
    pub key: Option<KeyCode>,
}

impl Default for NetOverlay {
    fn default() -> Self {
        Self { enabled: false, key: Some(KeyCode::F6) }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<NetOverlay>();
    register_tool(app.world_mut(), "net_overlay", Scope::Dev, ToolKind::Query, overlay_tool);
    app.add_dev_entry(
        "net",
        "net_overlay",
        "Links: ping, loss, desyncs (F6)",
        DevCommand::Live(|w, _| Ok(toggle_line("net_overlay", w.resource::<NetOverlay>().enabled))),
    )
    .add_dev_entry("net", "net_status", "Session status", DevCommand::Run("net_status".into()))
    .add_dev_entry("net", "net_lag", "Simulate 150 ms, 2% loss", DevCommand::Run("net_sim conditions=latency=150,loss=0.02".into()))
    .add_dev_entry("net", "net_no_lag", "No simulated lag", DevCommand::Run("net_sim conditions=none".into()));
    #[cfg(feature = "render")]
    window::plugin(app);
}

/// Show the session's links in the game window (F6): per peer ping, loss, kbps in and out,
/// the input margin, desyncs; a client's prediction error, snaps and mispredicted actions.
/// Answers the same lines.
#[derive(Deserialize, JsonSchema)]
pub struct NetOverlayArgs {
    /// Default true.
    pub on: Option<bool>,
}

fn overlay_tool(In(call): In<Call<NetOverlayArgs>>, world: &mut World) -> ToolResult {
    let on = call.args.on.unwrap_or(true);
    world.resource_mut::<NetOverlay>().enabled = on;
    if !on {
        return Ok("net overlay off".into());
    }
    let lines = overlay_lines(world);
    let lines = if lines.is_empty() { "not in a session".to_owned() } else { lines.join("\n") };
    Ok(format!("net overlay on\n{lines}"))
}

fn kbps(bytes_per_s: u32) -> f32 {
    bytes_per_s as f32 * 8.0 / 1000.0
}

fn link_text(s: &LinkStats) -> String {
    format!(
        "ping {:.0} ms · loss {:.1}% · in {:.1} kbps · out {:.1} kbps",
        s.srtt_ms,
        s.loss * 100.0,
        kbps(s.bytes_in_per_s),
        kbps(s.bytes_out_per_s)
    )
}

/// The overlay's lines: a header, then one per peer (a client has one, the host).
pub fn overlay_lines(world: &World) -> Vec<String> {
    let tick = sim::tick(world);
    if let Some(host) = world.get_resource::<NetHost>() {
        let mut lines = vec![format!("NET host · tick {tick} · desyncs {}", host.desyncs)];
        for p in host.peers.values() {
            let name = p.hello.as_ref().map_or("?", |h| h.name.as_str());
            let margin = p.margin.map_or("-".to_owned(), |m| m.1.to_string());
            lines.push(format!("p{} {name}: {} · margin {margin} · desyncs {}", p.player, link_text(&p.link.stats()), p.desyncs));
        }
        return lines;
    }
    let Some(client) = world.get_resource::<NetClient>() else { return Vec::new() };
    let desyncs = world.get_resource::<NetDesync>().map_or(0, |d| d.count);
    let player = client.player.map_or("-".to_owned(), |p| format!("p{p}"));
    let mut lines = vec![format!(
        "NET client {player} · tick {tick} · host tick {} · {} ticks ahead · desyncs {desyncs}",
        client.newest_host_tick,
        tick.saturating_sub(client.newest_host_tick)
    )];
    let stats = client.link().map(|l| l.stats()).unwrap_or_default();
    let margin = client.clock.error().map_or("-".to_owned(), |e| (e + client.clock.target).to_string());
    lines.push(format!("host {}: {} · margin {margin} (target {})", client.address, link_text(&stats), client.clock.target));
    if let Some(c) = world.get_resource::<NetCorrection>() {
        let (mean, max) = c.error();
        let p = world.get_resource::<NetPrediction>().cloned().unwrap_or_default();
        lines.push(format!(
            "prediction: error {mean:.2} m (max {max:.2}) · snaps {} · mispredicted {}/{}",
            c.snaps, p.mispredicted, p.predicted
        ));
    }
    lines
}

#[cfg(feature = "render")]
mod window {
    use bevy::prelude::*;

    use super::{overlay_lines, NetOverlay};

    /// The overlay's text on screen.
    #[derive(Component)]
    struct NetOverlayText;

    pub(super) fn plugin(app: &mut App) {
        app.add_systems(Update, (toggle_with_key.run_if(ai_core::dev::keys_free), show).chain().run_if(any_with_component::<Window>));
    }

    fn toggle_with_key(keys: Option<Res<ButtonInput<KeyCode>>>, mut overlay: ResMut<NetOverlay>) {
        let (Some(keys), Some(key)) = (keys, overlay.key) else { return };
        if keys.just_pressed(key) {
            overlay.enabled = !overlay.enabled;
        }
    }

    /// Bottom left, refreshed every frame while on; gone while off.
    fn show(world: &mut World) {
        let enabled = world.resource::<NetOverlay>().enabled;
        let mut q = world.query_filtered::<Entity, With<NetOverlayText>>();
        let shown = q.iter(world).next();
        match (enabled, shown) {
            (false, Some(entity)) => {
                world.despawn(entity);
            }
            (false, None) => {}
            (true, shown) => {
                // The built-in font is ASCII only.
                let text = overlay_lines(world).join("\n").replace('·', "|");
                match shown {
                    Some(entity) => {
                        if let Some(mut t) = world.get_mut::<Text>(entity)
                            && t.0 != text
                        {
                            t.0 = text;
                        }
                    }
                    None => {
                        world.spawn((
                            Node { position_type: PositionType::Absolute, bottom: Val::Px(10.0), left: Val::Px(12.0), ..default() },
                            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
                            Text::new(text),
                            TextFont::from_font_size(14.0),
                            TextColor(Color::srgb(0.75, 0.95, 0.8)),
                            NetOverlayText,
                        ));
                    }
                }
            }
        }
    }
}
