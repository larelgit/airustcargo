//! The in-process stand: a host and its clients as apps of one process, on a [`MemoryHub`],
//! driven tick by tick so that a run is the same every time.
//!
//! One stand tick is one host tick: the host runs one, each connected client as many as its
//! clock says (0, 1 or 2), then the hub's clock moves one tick on. While any member loads by
//! itself (a client building the world from the seed, a host changing region) the hub's clock
//! stands still and nobody ticks: how long loading takes on this machine changes nothing.
//!
//! `step`, `pause`, `resume` and `speed` on a member are deferred ([`StepRoute::Deferred`]):
//! [`Stand::call`] runs them as stand ticks and answers like the tool would.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ai_core::builtin::{apply_clock, StepWatch};
use ai_core::sim::{self, ClockAction, DrivenClock, StepEnd, StepPoll, StepReport};
use ai_core::tools::finished;
use ai_core::invariants::Violation;
use ai_core::{call_tool, CallOutcome, SimGate, StepQueue, StepRoute, SystemFailure};
use bevy::prelude::*;
use serde_json::Value;
use shards_core::EngineState;

use crate::client::{self, NetClient};
use crate::conditioner::LinkConditions;
use crate::hub::{LinkId, MemoryHub};
use crate::session::{NetClock, NetEndpoint};
use crate::transport::PeerId;

/// Gives `app` an endpoint on `hub` named `name` (a client: `host` is its host's peer).
/// Call it after building the app and before its first update. Returns its peer id.
pub fn attach(app: &mut App, hub: &MemoryHub, name: &str, host: Option<PeerId>) -> PeerId {
    let transport = hub.endpoint(name);
    let id = transport.id();
    app.insert_resource(NetEndpoint { transport: Box::new(transport), clock: NetClock::Hub(hub.clone()), host });
    id
}

/// Why a member stopped in a stand tick: a failed gameplay system or a violated invariant.
#[derive(Debug, Clone)]
pub enum MemberHalt {
    Failed(SystemFailure),
    Violated(Violation),
}

impl MemberHalt {
    pub fn headline(&self) -> String {
        match self {
            MemberHalt::Failed(f) => f.headline(),
            MemberHalt::Violated(v) => v.headline(),
        }
    }

    /// How a step that hit `halts` ends: failures first, as [`sim::step`] reports them.
    fn step_end(halts: Vec<MemberHalt>) -> StepEnd {
        let (mut failed, mut violated) = (Vec::new(), Vec::new());
        for halt in halts {
            match halt {
                MemberHalt::Failed(f) => failed.push(f),
                MemberHalt::Violated(v) => violated.push(v),
            }
        }
        if failed.is_empty() {
            StepEnd::InvariantViolated(violated)
        } else {
            StepEnd::SystemFailed(failed)
        }
    }
}

/// One app of a stand.
pub struct Member {
    pub name: String,
    pub app: App,
    /// Its link to the host on the hub (a client's).
    pub link: Option<LinkId>,
    queue: Arc<Mutex<StepQueue>>,
}

pub struct Stand {
    pub hub: MemoryHub,
    pub members: Vec<Member>,
    /// Stand ticks run (the hub's clock is this many ticks).
    pub iteration: u64,
    tick_ms: f64,
    /// Running in real time (`resume`), at `speed`: for a driver with a clock (`--stand`).
    /// What every member's `ai/status`, `observe` and replies say (see [`DrivenClock`]).
    pub running: bool,
    pub speed: f32,
    /// How long a member may load by itself.
    pub load_timeout: Duration,
    /// Real time not yet run as ticks, in ticks, and when it was counted ([`Stand::serve`]).
    owed: f64,
    counted: Option<Instant>,
}

/// Ticks [`Stand::serve`] runs at most per frame to catch up with the clock.
pub const MAX_CATCH_UP: u64 = 8;

impl Stand {
    /// A stand of apps attached to `hub` ([`attach`]), member 0 the host, built and finished.
    pub fn new(hub: MemoryHub, apps: Vec<(String, App)>) -> Self {
        let mut members = Vec::new();
        let mut tick_ms = 1000.0 / 60.0;
        for (i, (name, mut app)) in apps.into_iter().enumerate() {
            let queue = Arc::new(Mutex::new(StepQueue::default()));
            app.insert_resource(StepRoute::Deferred(queue.clone()));
            if i == 0 {
                tick_ms = app.world().get_resource::<Time<Fixed>>().map_or(tick_ms, |t| t.timestep().as_secs_f64() * 1000.0);
            }
            members.push(Member { name, app, link: None, queue });
        }
        let stand =
            Self { hub, members, iteration: 0, tick_ms, running: false, speed: 1.0, load_timeout: Duration::from_secs(30), owed: 0.0, counted: None };
        stand.sync_clock();
        stand
    }

    /// Tells every member how the stand runs their ticks (`running`, `speed`): their virtual
    /// clocks stay paused, the stand runs the ticks.
    pub fn sync_clock(&self) {
        let clock = DrivenClock { running: self.running, speed: self.speed };
        for member in &self.members {
            if let Ok(mut queue) = member.queue.lock() {
                queue.set_driven(clock);
            }
        }
    }

    /// Adds a member (a client joining later, on hub link `link`); returns its index.
    pub fn add(&mut self, name: &str, mut app: App, link: Option<LinkId>) -> usize {
        let queue = Arc::new(Mutex::new(StepQueue::default()));
        app.insert_resource(StepRoute::Deferred(queue.clone()));
        self.members.push(Member { name: name.to_owned(), app, link, queue });
        self.sync_clock();
        self.members.len() - 1
    }

    /// Changes the conditions of member `member`'s link to the host, both ways.
    pub fn set_link(&self, member: usize, conditions: LinkConditions) -> Result<(), String> {
        let link = self.members[member].link.ok_or_else(|| format!("{} has no link to the host", self.members[member].name))?;
        self.hub.set_link_conditions(link, conditions, conditions);
        Ok(())
    }

    pub fn world(&self, member: usize) -> &World {
        self.members[member].app.world()
    }

    pub fn world_mut(&mut self, member: usize) -> &mut World {
        self.members[member].app.world_mut()
    }

    pub fn app(&mut self, member: usize) -> &mut App {
        &mut self.members[member].app
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    fn advance_hub(&mut self) {
        self.iteration += 1;
        self.hub.set_time((self.iteration as f64 * self.tick_ms).round() as u64);
    }

    /// Whether member `i` is loading by itself.
    fn loading(&self, i: usize) -> bool {
        let world = self.world(i);
        if world.contains_resource::<NetClient>() {
            return client::loading_locally(world);
        }
        let state = world.get_resource::<State<EngineState>>().map(|s| *s.get());
        state != Some(EngineState::InGame) || world.get_resource::<SimGate>().is_some_and(SimGate::is_held)
    }

    /// One frame of every member (`update`): loading goes on, the session talks.
    pub fn frame(&mut self) {
        for member in &mut self.members {
            ai_core::activate(member.app.world());
            member.app.update();
        }
    }

    /// Frames until the host has loaded and every client is connected (or refused): the hub's
    /// clock moves only in frames where nobody loads by itself.
    pub fn settle(&mut self) -> Result<(), String> {
        let started = Instant::now();
        loop {
            // While anyone loads by itself only they get frames: what the others would receive
            // meanwhile depends on how fast this machine loads.
            let loading: Vec<usize> = (0..self.len()).filter(|i| self.loading(*i)).collect();
            if loading.is_empty() {
                self.frame();
            } else {
                for i in &loading {
                    let app = &mut self.members[*i].app;
                    ai_core::activate(app.world());
                    app.update();
                }
            }
            let local = (0..self.len()).any(|i| self.loading(i));
            let joined = self.members.iter().skip(1).all(|m| {
                let stage = m.app.world().get_resource::<NetClient>().map(|c| c.stage.clone());
                matches!(stage, None | Some(client::ClientStage::Connected | client::ClientStage::Ended(_)))
            });
            if !local && joined {
                return Ok(());
            }
            if local {
                std::thread::sleep(Duration::from_millis(1));
            } else {
                self.advance_hub();
            }
            if started.elapsed() > self.load_timeout {
                let waiting: Vec<String> = self
                    .members
                    .iter()
                    .map(|m| {
                        let stage = m.app.world().get_resource::<NetClient>().map(|c| format!("{:?}", c.stage)).unwrap_or_else(|| "host".into());
                        format!("{} {stage} ({})", m.name, ai_core::sim::gate_message(m.app.world()).unwrap_or_default())
                    })
                    .collect();
                return Err(format!("the stand did not settle within {:?}: {}", self.load_timeout, waiting.join("; ")));
            }
        }
    }

    /// Waits (frames, no ticks, the hub's clock still) while any member loads by itself.
    fn wait_local(&mut self) -> Result<(), String> {
        let started = Instant::now();
        loop {
            let loading: Vec<usize> = (0..self.len()).filter(|i| self.loading(*i)).collect();
            if loading.is_empty() {
                return Ok(());
            }
            for i in &loading {
                let app = &mut self.members[*i].app;
                ai_core::activate(app.world());
                app.update();
            }
            std::thread::sleep(Duration::from_millis(1));
            if started.elapsed() > self.load_timeout {
                let names: Vec<String> = loading.iter().map(|i| self.members[*i].name.clone()).collect();
                return Err(format!("{} still loading after {:?}", names.join(", "), self.load_timeout));
            }
        }
    }

    /// One stand tick. Returns the gameplay failures it caused, by member.
    pub fn tick(&mut self) -> Result<Vec<(usize, MemberHalt)>, String> {
        self.wait_local()?;
        let mut failed = Vec::new();
        for (i, member) in self.members.iter_mut().enumerate() {
            let world = member.app.world_mut();
            ai_core::activate(world);
            let ticks = match world.get_resource::<NetClient>().map(|c| c.stage == client::ClientStage::Connected) {
                // Not playing yet: it only talks (the pump runs in a frame without a tick).
                Some(false) => 0,
                Some(true) => client::clock_ticks(world),
                None => 1,
            };
            if ticks == 0 {
                // Held back (joining, or its clock says wait): it still hears the host.
                member.app.update();
                continue;
            }
            let world = member.app.world_mut();
            for _ in 0..ticks {
                sim::run_tick(world)?;
            }
            ai_core::failures::collect(world);
            failed.extend(ai_core::failures::take_unreported(world).into_iter().map(|f| (i, MemberHalt::Failed(f))));
            failed.extend(ai_core::invariants::take_unreported(world).into_iter().map(|v| (i, MemberHalt::Violated(v))));
        }
        self.advance_hub();
        Ok(failed)
    }

    /// `ticks` stand ticks; panics on a gameplay failure.
    pub fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            match self.tick() {
                Ok(failed) if failed.is_empty() => {}
                Ok(halted) => panic!("gameplay halted: {:?}", halted.iter().map(|(i, h)| format!("{}: {}", self.members[*i].name, h.headline())).collect::<Vec<_>>()),
                Err(e) => panic!("{e}"),
            }
        }
    }

    /// Calls `tool` on member `member` as agent `dev`; a deferred `step` or clock change runs
    /// here and its answer comes back as the call's.
    pub fn call(&mut self, member: usize, tool: &str, args: Value) -> CallOutcome {
        self.call_as(member, "dev", tool, args)
    }

    pub fn call_as(&mut self, member: usize, agent: &str, tool: &str, args: Value) -> CallOutcome {
        let out = call_tool(self.world_mut(member), agent, tool, args);
        let Some(id) = out.result.get("pending").and_then(Value::as_u64) else { return out };
        self.drive();
        let queue = self.members[member].queue.clone();
        let polled = queue.lock().map(|q| q.poll(id)).unwrap_or(StepPoll::Unknown);
        match polled {
            StepPoll::Done(outcome) => outcome,
            _ => out,
        }
    }

    /// Runs every deferred request of every member.
    pub fn drive(&mut self) {
        for m in 0..self.len() {
            let queue = self.members[m].queue.clone();
            let clock = queue.lock().map(|mut q| q.take_clock()).unwrap_or_default();
            for request in clock {
                let result = match request.action {
                    ClockAction::Pause => {
                        self.running = false;
                        Ok(format!("paused at tick {}", sim::tick(self.world(m))))
                    }
                    ClockAction::Resume => {
                        self.running = true;
                        Ok(format!("running from tick {}", sim::tick(self.world(m))))
                    }
                    // Refused as a game of its own refuses it (an infinity), the stand untouched.
                    ClockAction::Speed(x) if !x.is_finite() => apply_clock(self.world_mut(m), ClockAction::Speed(x)),
                    ClockAction::Speed(x) => {
                        // As `sim::set_speed`: no negative speed (it would owe ticks forever).
                        self.speed = x.max(0.0);
                        // The answer reads the stand's clock: it says the new speed.
                        self.sync_clock();
                        let speed = self.speed;
                        apply_clock(self.world_mut(m), ClockAction::Speed(speed))
                    }
                };
                self.sync_clock();
                let outcome = finished(self.world(m), request.action.tool(), result.map(Value::String));
                if let Ok(mut q) = queue.lock() {
                    q.complete(request.id, outcome);
                }
            }
            let steps = queue.lock().map(|mut q| q.take_steps()).unwrap_or_default();
            for request in steps {
                // A step pauses, as `sim::step` does: after it, time stands where it stopped.
                self.running = false;
                self.sync_clock();
                let started = Instant::now();
                let from_tick = sim::tick(self.world(m));
                let watch = StepWatch::begin(self.world_mut(m), &request);
                ai_core::perf::begin_span(self.world_mut(m));
                let mut end = StepEnd::Completed;
                for _ in 0..request.ticks {
                    match self.tick() {
                        Ok(halted) if halted.is_empty() => {}
                        Ok(halted) => {
                            end = MemberHalt::step_end(halted.into_iter().map(|(_, h)| h).collect());
                            break;
                        }
                        Err(e) => {
                            end = StepEnd::Condition(e);
                            break;
                        }
                    }
                    let world = self.world_mut(m);
                    ai_core::activate(world);
                    if let Some(reason) = watch.check(world) {
                        end = StepEnd::Condition(reason);
                        break;
                    }
                    if started.elapsed() > Duration::from_secs(ai_core::builtin::STEP_WALL_SECS) {
                        end = StepEnd::WallClock;
                        break;
                    }
                }
                let world = self.world_mut(m);
                let timing = ai_core::perf::span(world);
                let report = StepReport { from_tick, to_tick: sim::tick(world), end, wall: started.elapsed(), timing };
                let outcome = watch.finish(world, &report);
                if let Ok(mut q) = queue.lock() {
                    q.complete(request.id, outcome);
                }
            }
        }
    }

    /// One frame of a stand driven from outside in real time (`--stand N`): every member
    /// answers its requests, deferred steps and clock changes run, and while the stand runs
    /// (`resume`) it ticks to keep up with the clock at its speed. A gameplay failure pauses
    /// it (the failing member journals it), as a game in real time pauses. Returns whether a
    /// member is exiting.
    pub fn serve(&mut self) -> bool {
        self.sync_clock();
        self.frame();
        self.drive();
        let now = Instant::now();
        if self.running {
            let since = self.counted.map_or(Duration::ZERO, |t| now.duration_since(t));
            self.owed += since.as_secs_f64() * 1000.0 * self.speed as f64 / self.tick_ms;
            let due = (self.owed.floor() as u64).min(MAX_CATCH_UP);
            // Behind by more than a frame's catch-up: the rest is dropped, not run later.
            self.owed = if self.owed >= (MAX_CATCH_UP + 1) as f64 { 0.0 } else { self.owed - due as f64 };
            for _ in 0..due {
                match self.tick() {
                    Ok(failed) if failed.is_empty() => {}
                    Ok(halted) => {
                        for (i, halt) in halted {
                            let member = &mut self.members[i];
                            let text = format!("stand paused: {}", halt.headline());
                            ai_core::journal::emit(member.app.world_mut(), ai_core::journal::JournalKind::Error, text);
                        }
                        self.running = false;
                        break;
                    }
                    Err(e) => {
                        warn!("stand: {e}");
                        self.running = false;
                        break;
                    }
                }
            }
        } else {
            self.owed = 0.0;
        }
        self.counted = Some(now);
        // Only the host ends the stand: a client that exits (`--exit-with-host`, kicked) has
        // left the session, the others play on.
        self.members.first().is_some_and(|host| host.app.should_exit().is_some())
    }

    /// Ends the stand as a host quitting ends a session: the host's exit hooks (the clients
    /// are told, get their characters), the hub's clock running until that has arrived over
    /// links with latency and loss (at most `max_ms` of hub time), then the clients' exit
    /// hooks. Returns how the host exits.
    pub fn shut_down(&mut self, max_ms: u64) -> AppExit {
        let Some(host) = self.members.first_mut() else { return AppExit::Success };
        // What a hook logs goes to its own member's journal.
        ai_core::activate(host.app.world());
        shards_core::run_exit_hooks(host.app.world_mut());
        let until = self.hub.now() + max_ms;
        loop {
            self.frame();
            let told = self.members.iter().skip(1).all(|m| {
                m.app.world().get_resource::<NetClient>().is_none_or(|c| matches!(c.stage, client::ClientStage::Ended(_)))
            });
            if (told && self.hub.in_transit() == 0) || self.hub.now() >= until {
                break;
            }
            self.advance_hub();
        }
        for member in self.members.iter_mut().skip(1) {
            ai_core::activate(member.app.world());
            shards_core::run_exit_hooks(member.app.world_mut());
        }
        self.members[0].app.should_exit().unwrap_or(AppExit::Success)
    }

    /// Member `name`'s index.
    pub fn index(&self, name: &str) -> Option<usize> {
        self.members.iter().position(|m| m.name == name)
    }
}
