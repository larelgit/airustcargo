//! A soak: a long co-op session that seeded agents play on every member of a stand at once,
//! to show that the members never disagree (E7: host and two clients, half an hour at 150 ms
//! and 2% loss, without a desync).
//!
//! Every round, the agent of each member looks (`observe`) and makes one random player input
//! with what it sees ([`crate::inputs`]: move, attack, dodge); a dead body is healed by the host
//! instead. Now and then the host changes the region (`regen`) and saves (`save_game
//! what=all`), the two things that send every client the world or its character. Then the
//! whole stand steps a random few ticks, stopping at once when the host hears of a desync
//! (`named:net_desync`). At the end every member is asked for its `net_status`.
//!
//! Like a scenario, a soak only moves JSON through a [`Driver`]: in-process on a test stand
//! (`shards::testing`), or over BRP against a live one (`shards soak`).

use std::fmt;

use serde::Serialize;
use serde_json::{json, Value};

use crate::condition::parse_condition;
use crate::inputs::{ids_in, own_body, own_body_dead, random_input, SplitMix};
use crate::run::{Driver, Outcome};
use crate::script::HOST;

/// The agent a soak plays every member as (dev scope, the member's own body).
pub const SOAK_AGENT: &str = "soak";

/// What a soak does: how long, how hard and what else besides the inputs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SoakPlan {
    /// Seed of the agents' inputs (the game has its own): the same seed on the same fresh
    /// stand makes the same calls.
    pub seed: u64,
    /// Stand ticks to play (30 minutes at 60 Hz: 108000).
    pub ticks: u64,
    /// Longest step between two rounds of inputs, in ticks.
    pub max_step: u64,
    /// Regions the host changes to in turn (`regen region=R`, a seed from the soak's own);
    /// none: the world stays.
    pub regions: Vec<String>,
    /// Ticks between region changes (0: none).
    pub regen_every: u64,
    /// Ticks between saves of everything on the host (0: none).
    pub save_every: u64,
    /// Ticks between progress lines.
    pub report_every: u64,
    /// Ticks a second of the game (`ai/status` `tick_hz`): only for telling game time.
    pub tick_hz: f64,
}

impl Default for SoakPlan {
    fn default() -> Self {
        Self { seed: 1, ticks: 3600, max_step: 30, regions: Vec::new(), regen_every: 0, save_every: 0, report_every: 3600, tick_hz: 60.0 }
    }
}

/// What one member's agent did, and what its session says at the end.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MemberTally {
    pub member: String,
    /// Inputs made, and how many of them the game refused (part of play: no stamina, busy).
    pub inputs: u64,
    pub refused: u64,
    /// Times its body was found dead and healed by the host.
    pub revived: u64,
    /// Desyncs its session counted (`net_status`): a client's own, or the host's for all.
    /// `None`: its `net_status` was refused, so nobody can say it stayed in sync.
    pub desyncs: Option<u64>,
    /// Its link at the end, one line (`rtt 301 ms, loss 2.1%, …`).
    pub net: String,
}

/// How a soak went.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SoakReport {
    pub plan: SoakPlan,
    pub from_tick: u64,
    pub to_tick: u64,
    pub rounds: u64,
    pub regens: u64,
    pub saves: u64,
    pub members: Vec<MemberTally>,
    /// Why the soak ended early: a desync (`step` stopped on `net_desync`), a halted game (a
    /// panicked system, a broken invariant). `None`: it played every tick of the plan.
    pub stopped: Option<String>,
    /// Progress lines, as they were printed.
    pub log: Vec<String>,
}

impl SoakReport {
    /// No early end, and every member was asked at the end and counted no desync.
    pub fn passed(&self) -> bool {
        self.stopped.is_none() && self.members.iter().all(|m| m.desyncs == Some(0))
    }

    pub fn ticks(&self) -> u64 {
        self.to_tick.saturating_sub(self.from_tick)
    }
}

impl fmt::Display for SoakReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in &self.log {
            writeln!(f, "{line}")?;
        }
        for m in &self.members {
            let desyncs = m.desyncs.map_or("?".to_owned(), |d| d.to_string());
            writeln!(f, "  {}: {} inputs ({} refused), revived {}, desyncs {desyncs}; {}", m.member, m.inputs, m.refused, m.revived, m.net)?;
        }
        let verdict = if self.passed() { "PASS" } else { "FAIL" };
        write!(
            f,
            "{verdict} soak seed {}: ticks {} → {} ({}), {} rounds, {} region changes, {} saves",
            self.plan.seed,
            self.from_tick,
            self.to_tick,
            game_time(self.ticks(), self.plan.tick_hz),
            self.rounds,
            self.regens,
            self.saves
        )?;
        if let Some(why) = &self.stopped {
            write!(f, "; stopped: {why}")?;
        }
        let unasked: Vec<&str> = self.members.iter().filter(|m| m.desyncs.is_none()).map(|m| m.member.as_str()).collect();
        if !unasked.is_empty() {
            write!(f, "; no net_status from {}", unasked.join(", "))?;
        }
        Ok(())
    }
}

/// `m:ss` of game time at `hz` ticks a second.
fn game_time(ticks: u64, hz: f64) -> String {
    let secs = (ticks as f64 / hz.max(1e-6)).floor() as u64;
    format!("{}:{:02} of game time", secs / 60, secs % 60)
}

/// Plays `plan` on the game `driver` reaches (every member of a stand, or a single game) and
/// says how it went; `progress` gets every progress line as it is made. `Err` when the game
/// cannot be reached, a scheduled call (`regen`, `save_game`, `step`) is refused (the error
/// names it), or the plan ends past the last tick a game can count.
pub fn run(driver: &mut dyn Driver, plan: &SoakPlan, progress: &mut dyn FnMut(&str)) -> Result<SoakReport, String> {
    let peers = driver.peers()?;
    let from = driver.status_on(HOST)?.tick;
    let end = from
        .checked_add(plan.ticks)
        .ok_or_else(|| format!("a soak of {} ticks from tick {from} ends past the last tick a game counts ({}): ask for fewer minutes", plan.ticks, u64::MAX))?;
    let until = vec![parse_condition("named:net_desync")?];
    let mut rng = SplitMix(plan.seed);
    let mut report = SoakReport {
        plan: plan.clone(),
        from_tick: from,
        to_tick: from,
        members: peers.iter().map(|p| MemberTally { member: p.clone(), ..MemberTally::default() }).collect(),
        ..SoakReport::default()
    };
    // A point past the last tick is never reached (`tick < end`), so it saturates there.
    let every = |n: u64| (n > 0).then_some(n);
    let mut next_regen = every(plan.regen_every).filter(|_| !plan.regions.is_empty()).map(|n| from.saturating_add(n));
    let mut next_save = every(plan.save_every).map(|n| from.saturating_add(n));
    let mut next_report = every(plan.report_every).map(|n| from.saturating_add(n));
    let mut say = |report: &mut SoakReport, line: String| {
        progress(&line);
        report.log.push(line);
    };
    say(&mut report, format!("soak seed {}: {} on {} from tick {from}", plan.seed, game_time(plan.ticks, plan.tick_hz), peers.join(", ")));

    let mut tick = from;
    while tick < end {
        report.rounds += 1;
        for (member, peer) in peers.iter().enumerate() {
            let seen = driver.call_on(peer, SOAK_AGENT, "observe", json!({}))?;
            let tally = &mut report.members[member];
            if !seen.ok {
                // A client between worlds has nothing to look at yet: its turn passes.
                tally.refused += 1;
                continue;
            }
            let text = seen.text();
            if own_body_dead(&text) {
                if let Some(body) = own_body(&text) {
                    let healed = driver.call_on(HOST, SOAK_AGENT, "heal", json!({"id": body}))?;
                    tally.revived += u64::from(healed.ok);
                }
                continue;
            }
            let (tool, args) = random_input(&mut rng, &ids_in(&text));
            let out = driver.call_on(peer, SOAK_AGENT, tool, args)?;
            tally.inputs += 1;
            tally.refused += u64::from(!out.ok);
        }

        // Due points keep to the plan's grid (`from + k * every`), not to when the last one came.
        if let Some(at) = next_regen.filter(|at| tick >= *at) {
            let region = &plan.regions[(report.regens % plan.regions.len() as u64) as usize];
            let seed = rng.below(1_000_000);
            scheduled(driver, "regen", json!({"region": region, "seed": seed}))?;
            report.regens += 1;
            next_regen = Some(at.saturating_add(plan.regen_every));
            say(&mut report, format!("tick {tick}: the host goes to {region} (seed {seed})"));
        }
        if let Some(at) = next_save.filter(|at| tick >= *at) {
            scheduled(driver, "save_game", json!({"what": "all"}))?;
            report.saves += 1;
            next_save = Some(at.saturating_add(plan.save_every));
        }

        // A step ends where the next scheduled call is due, so each comes on its tick: a step
        // over two due points would skip a region change or a save.
        let due = [next_regen, next_save].into_iter().flatten().map(|at| at.saturating_sub(tick)).filter(|n| *n > 0).min();
        let ticks = (1 + rng.below(plan.max_step.max(1))).min(end - tick).min(due.unwrap_or(u64::MAX));
        let out = scheduled(driver, "step", json!({"ticks": ticks, "until": until}))?;
        tick = match out.tick {
            Some(t) => t,
            None => driver.status_on(HOST)?.tick,
        };
        report.to_tick = tick;
        let text = out.text();
        if let Some(why) = stopped_early(&text) {
            report.stopped = Some(why);
            break;
        }
        if next_report.is_some_and(|at| tick >= at) {
            next_report = Some(tick.saturating_add(plan.report_every));
            let line = progress_line(driver, &report, tick)?;
            say(&mut report, line);
        }
    }

    for (member, peer) in peers.iter().enumerate() {
        let status = driver.call_on(peer, SOAK_AGENT, "net_status", json!({}))?;
        let tally = &mut report.members[member];
        if status.ok {
            tally.desyncs = Some(desyncs(&status.result));
            tally.net = net_line(&status.result);
        } else {
            // Left `None`: a member that cannot say how it went does not pass.
            tally.net = format!("net_status refused: {}", status.text());
        }
    }
    Ok(report)
}

/// A call on the host the soak cannot go on without.
fn scheduled(driver: &mut dyn Driver, tool: &str, args: Value) -> Result<Outcome, String> {
    let out = driver.call_on(HOST, SOAK_AGENT, tool, args.clone())?;
    if out.ok { Ok(out) } else { Err(format!("{tool} {args} was refused: {}", out.text())) }
}

/// Why a `step` ended the soak: it stopped on the desync condition, or the game halted. A step
/// cut short by its wall-clock budget is not a reason.
fn stopped_early(step: &str) -> Option<String> {
    let (_, why) = step.split_once(", stopped: ")?;
    if why.contains("wall-clock budget") {
        return None;
    }
    Some(why.lines().next().unwrap_or(why).to_owned())
}

/// Desyncs a member's `net_status` counted: the host's total, or a client's own.
fn desyncs(status: &Value) -> u64 {
    if let Some(peers) = status["peers"].as_array() {
        return peers.iter().filter_map(|p| p["desyncs"].as_u64()).sum();
    }
    status["desyncs"].as_u64().unwrap_or(0)
}

/// A member's link on one line: the host's players, or the client's own link and prediction.
fn net_line(status: &Value) -> String {
    let ms = |v: &Value| v.as_f64().map_or("?".to_owned(), |x| format!("{x:.0} ms"));
    let pct = |v: &Value| v.as_f64().map_or("?".to_owned(), |x| format!("{:.1}%", x * 100.0));
    if let Some(peers) = status["peers"].as_array() {
        let each: Vec<String> = peers
            .iter()
            .map(|p| {
                format!(
                    "p{} rtt {} loss {} resyncs {} late inputs {}",
                    p["player"],
                    ms(&p["rtt_ms"]),
                    pct(&p["loss"]),
                    p["resyncs"],
                    p["inputs"]["late"]
                )
            })
            .collect();
        return if each.is_empty() { "host, no players".to_owned() } else { format!("host: {}", each.join(" · ")) };
    }
    if status.get("stage").is_some() {
        let prediction = &status["prediction"];
        return format!(
            "client {}: rtt {}, loss {}, rebases {}, prediction error mean {:.2} m max {:.2} m, snaps {}",
            status["stage"].as_str().unwrap_or("?"),
            ms(&status["rtt_ms"]),
            pct(&status["loss"]),
            status["rebases"],
            prediction["error_mean_m"].as_f64().unwrap_or(0.0),
            prediction["error_max_m"].as_f64().unwrap_or(0.0),
            prediction["snaps"]
        );
    }
    "no session".to_owned()
}

/// `tick 36000 (10:00 of game time): 1203 inputs, desyncs 0; host: p1 rtt 301 ms …`
fn progress_line(driver: &mut dyn Driver, report: &SoakReport, tick: u64) -> Result<String, String> {
    let status = driver.call_on(HOST, SOAK_AGENT, "net_status", json!({}))?;
    let inputs: u64 = report.members.iter().map(|m| m.inputs).sum();
    let (desyncs, net) = if status.ok { (desyncs(&status.result), net_line(&status.result)) } else { (0, status.text()) };
    Ok(format!(
        "tick {tick} ({}): {} rounds, {inputs} inputs, desyncs {desyncs}; {net}",
        game_time(tick.saturating_sub(report.from_tick), report.plan.tick_hz),
        report.rounds
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::run::{JournalRead, Status};
    use crate::script::SessionConfig;

    /// A pretend stand: bodies e5 (host), e15 (c1), e16 (c2); c2's body is dead until healed;
    /// `step` stops on a desync at tick `desync_at`, if any.
    struct Pretend {
        tick: u64,
        dead: bool,
        desync_at: Option<u64>,
        /// The member whose `net_status` is refused, if any.
        mute: Option<&'static str>,
        calls: Vec<(String, String, Value)>,
    }

    impl Pretend {
        fn new(desync_at: Option<u64>) -> Self {
            Self { tick: 0, dead: true, desync_at, mute: None, calls: Vec::new() }
        }

        fn count(&self, peer: &str, tool: &str) -> usize {
            self.calls.iter().filter(|(p, t, _)| p == peer && t == tool).count()
        }
    }

    impl Driver for Pretend {
        fn call(&mut self, agent: &str, tool: &str, args: Value) -> Result<Outcome, String> {
            self.call_on(HOST, agent, tool, args)
        }

        fn session(&mut self, _: &str, _: &SessionConfig) -> Result<Outcome, String> {
            Ok(Outcome::ok(json!({})))
        }

        fn status(&mut self) -> Result<Status, String> {
            Ok(Status { tick: self.tick, seed: Some(42), journal_seq: 0 })
        }

        fn journal_since(&mut self, _: u64) -> Result<JournalRead, String> {
            Ok(JournalRead::default())
        }

        fn peers(&mut self) -> Result<Vec<String>, String> {
            Ok(vec!["host".into(), "c1".into(), "c2".into()])
        }

        fn call_on(&mut self, peer: &str, agent: &str, tool: &str, args: Value) -> Result<Outcome, String> {
            assert_eq!(agent, SOAK_AGENT);
            self.calls.push((peer.to_owned(), tool.to_owned(), args.clone()));
            let body = BTreeMap::from([("host", "e5"), ("c1", "e15"), ("c2", "e16")])[peer];
            Ok(match tool {
                "observe" => {
                    let state = if peer == "c2" && self.dead { "hp 0/100 · dead" } else { "hp 100/100" };
                    Outcome::ok(format!("tick {} | paused | you {body}\nYOU  {body} Player  {state}\nSEEN\n  e6 Boss 18.0m N\n", self.tick))
                }
                "heal" => {
                    assert_eq!(args["id"], "e16", "the host heals the dead body it was told of");
                    self.dead = false;
                    Outcome::ok("ok: healed")
                }
                "step" => {
                    assert_eq!(peer, HOST);
                    assert_eq!(args["until"], json!([{"named": "net_desync"}]));
                    let ticks = args["ticks"].as_u64().unwrap();
                    let to = self.tick + ticks;
                    let text = match self.desync_at {
                        Some(at) if to >= at => {
                            self.tick = at;
                            format!("ran {} ticks, stopped: net_desync", at - (to - ticks))
                        }
                        _ => {
                            self.tick = to;
                            format!("ran {ticks} ticks")
                        }
                    };
                    Outcome { ok: true, result: Value::String(text), tick: Some(self.tick), halted: None }
                }
                "net_status" if self.mute == Some(peer) => Outcome::err("unknown tool `net_status`"),
                "net_status" if peer == HOST => {
                    let desyncs = u64::from(self.desync_at.is_some_and(|at| self.tick >= at));
                    Outcome::ok(json!({"peers": [
                        {"player": 1, "rtt_ms": 301.0, "loss": 0.02, "resyncs": 0, "inputs": {"late": 0}, "desyncs": desyncs},
                        {"player": 2, "rtt_ms": 299.0, "loss": 0.021, "resyncs": 0, "inputs": {"late": 1}, "desyncs": 0}
                    ]}))
                }
                "net_status" => Outcome::ok(json!({"stage": "Playing", "rtt_ms": 300.0, "loss": 0.02, "rebases": 0, "desyncs": 0,
                                                    "prediction": {"error_mean_m": 0.01, "error_max_m": 0.2, "snaps": 0}})),
                "regen" | "save_game" => Outcome::ok("ok"),
                "light_attack" => Outcome::err("not enough stamina: 10/20 (ready in 0.2s)"),
                _ => Outcome::ok("ok"),
            })
        }
    }

    #[test]
    fn every_member_plays_the_host_steps_and_the_plan_is_kept() {
        let plan = SoakPlan { seed: 7, ticks: 600, max_step: 30, regions: vec!["wilds".into()], regen_every: 200, save_every: 300, report_every: 300, tick_hz: 60.0 };
        let mut stand = Pretend::new(None);
        let mut lines = Vec::new();
        let report = run(&mut stand, &plan, &mut |l| lines.push(l.to_owned())).unwrap();
        assert!(report.passed(), "{report}");
        assert_eq!((report.from_tick, report.to_tick), (0, 600), "exactly the planned ticks: {report}");
        assert_eq!(stand.count("host", "regen"), 2, "at 200 and 400, not at 600: the soak is over");
        assert_eq!(report.regens, 2);
        assert_eq!(stand.count("host", "save_game"), 1);
        assert!(stand.calls.iter().filter(|(_, t, _)| t == "step").all(|(p, _, _)| p == HOST), "only the host steps a stand");
        // c2 was found dead once and healed by the host; afterwards it played like the others.
        assert_eq!(report.members[2].revived, 1);
        assert_eq!(report.members[0].inputs, report.rounds);
        assert_eq!(report.members[2].inputs, report.rounds - 1);
        assert!(report.members.iter().all(|m| m.refused <= m.inputs));
        assert_eq!(lines, report.log, "progress lines are printed as they come");
        assert!(lines.iter().any(|l| l.starts_with("tick ") && l.contains("desyncs 0")), "{lines:?}");
        assert!(report.to_string().ends_with("1 saves"), "{report}");
        assert!(report.members[1].net.starts_with("client Playing: rtt 300 ms, loss 2.0%"), "{}", report.members[1].net);
        assert!(report.members[0].net.contains("p1 rtt 301 ms loss 2.0%"), "{}", report.members[0].net);

        // The same seed on the same stand makes the same calls.
        let mut again = Pretend::new(None);
        run(&mut again, &plan, &mut |_| {}).unwrap();
        assert_eq!(again.calls, stand.calls);
    }

    #[test]
    fn a_schedule_finer_than_a_step_is_kept_to_the_tick() {
        // Steps of up to 30 ticks, a region change every 25 and a save every 40: no step may run
        // over a due point, or one would be lost.
        let plan = SoakPlan { seed: 9, ticks: 600, max_step: 30, regions: vec!["wilds".into()], regen_every: 25, save_every: 40, ..SoakPlan::default() };
        let mut stand = Pretend::new(None);
        let report = run(&mut stand, &plan, &mut |_| {}).unwrap();
        assert_eq!((report.regens, report.saves), (23, 14), "every due point before tick 600: {report}");
        let at: Vec<u64> = report
            .log
            .iter()
            .filter_map(|l| l.strip_prefix("tick ")?.split_once(':')?.0.parse().ok())
            .collect();
        assert_eq!(at, (1..=23).map(|k| k * 25).collect::<Vec<u64>>(), "each region change on its tick");
    }

    #[test]
    fn a_desync_stops_the_soak_at_once_and_fails_it() {
        let plan = SoakPlan { seed: 3, ticks: 6000, ..SoakPlan::default() };
        let mut stand = Pretend::new(Some(250));
        let report = run(&mut stand, &plan, &mut |_| {}).unwrap();
        assert!(!report.passed());
        assert_eq!(report.to_tick, 250);
        assert_eq!(report.stopped.as_deref(), Some("net_desync"));
        assert_eq!(report.members[0].desyncs, Some(1), "the host's count is asked at the end");
        assert!(report.to_string().starts_with("soak seed 3") && report.to_string().contains("FAIL soak seed 3"), "{report}");
    }

    #[test]
    fn a_member_that_cannot_say_how_it_went_fails_the_soak() {
        let plan = SoakPlan { seed: 5, ticks: 120, ..SoakPlan::default() };
        let mut stand = Pretend::new(None);
        stand.mute = Some("c1");
        let report = run(&mut stand, &plan, &mut |_| {}).unwrap();
        assert!(report.stopped.is_none(), "it played every tick: {report}");
        assert!(!report.passed(), "{report}");
        assert_eq!(report.members[1].desyncs, None);
        assert_eq!(report.members[1].net, "net_status refused: unknown tool `net_status`");
        let text = report.to_string();
        assert!(text.contains("  c1: ") && text.contains("desyncs ?; net_status refused"), "{text}");
        assert!(text.contains("FAIL soak seed 5") && text.ends_with("; no net_status from c1"), "{text}");
    }

    #[test]
    fn a_plan_past_the_last_tick_is_refused_before_anything_is_played() {
        let plan = SoakPlan { ticks: u64::MAX, regen_every: u64::MAX, save_every: u64::MAX, report_every: u64::MAX, ..SoakPlan::default() };
        let mut stand = Pretend::new(None);
        stand.tick = 10;
        let err = run(&mut stand, &plan, &mut |_| {}).unwrap_err();
        assert!(err.starts_with(&format!("a soak of {} ticks from tick 10 ends past the last tick", u64::MAX)), "{err}");
        assert!(stand.calls.is_empty(), "nothing was called: {:?}", stand.calls);

        // A plan that fits: its schedule points past the last tick saturate there and never come.
        let plan = SoakPlan { ticks: 90, ..plan };
        let mut stand = Pretend::new(None);
        stand.tick = 1;
        let report = run(&mut stand, &plan, &mut |_| {}).unwrap();
        assert_eq!((report.from_tick, report.to_tick, report.saves, report.regens), (1, 91, 0, 0), "{report}");
    }

    #[test]
    fn a_refused_scheduled_call_is_an_error_that_names_it() {
        struct NoRegion(Pretend);
        impl Driver for NoRegion {
            fn call(&mut self, a: &str, t: &str, v: Value) -> Result<Outcome, String> {
                self.0.call(a, t, v)
            }
            fn session(&mut self, a: &str, c: &SessionConfig) -> Result<Outcome, String> {
                self.0.session(a, c)
            }
            fn status(&mut self) -> Result<Status, String> {
                self.0.status()
            }
            fn journal_since(&mut self, after: u64) -> Result<JournalRead, String> {
                self.0.journal_since(after)
            }
            fn peers(&mut self) -> Result<Vec<String>, String> {
                self.0.peers()
            }
            fn call_on(&mut self, p: &str, a: &str, t: &str, v: Value) -> Result<Outcome, String> {
                if t == "regen" { Ok(Outcome::err("unknown region `wild`; did you mean `wilds`?")) } else { self.0.call_on(p, a, t, v) }
            }
        }
        let plan = SoakPlan { regions: vec!["wild".into()], regen_every: 10, ..SoakPlan::default() };
        let err = run(&mut NoRegion(Pretend::new(None)), &plan, &mut |_| {}).unwrap_err();
        assert!(err.starts_with("regen {\"region\":\"wild\""), "{err}");
        assert!(err.ends_with("was refused: unknown region `wild`; did you mean `wilds`?"), "{err}");
    }

    #[test]
    fn only_a_desync_or_a_halt_ends_a_soak_early() {
        assert_eq!(stopped_early("ran 30 ticks (0 → 30)"), None);
        assert_eq!(stopped_early("ran 9 ticks (0 → 9), stopped: 20 s wall-clock budget"), None);
        assert_eq!(stopped_early("ran 9 ticks (0 → 9), stopped: net_desync").as_deref(), Some("net_desync"));
        let halted = "ran 3 ticks (0 → 3), stopped: gameplay system x panicked at tick 3: boom\nmore";
        assert_eq!(stopped_early(halted).as_deref(), Some("gameplay system x panicked at tick 3: boom"));
        assert_eq!(game_time(108_000, 60.0), "30:00 of game time");
        assert_eq!(game_time(90, 30.0), "0:03 of game time");
    }
}
