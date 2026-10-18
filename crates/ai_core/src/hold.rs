//! Holds that are not the simulation's: the picture and the real-time clock waiting for a
//! moment (a hit stop), without a single tick being lost or added.
//!
//! - [`ClockHold`]: the fixed clock stands still for some **real** time. The virtual clock's
//!   relative speed is forced to 0 (not paused: a pause is the player's, the pause menu's, the
//!   gate's, and it mutes sound), so `Time<Fixed>` gathers nothing and no tick is due; when the
//!   hold ends the speed comes back and the next frame counts a normal frame's time, with no
//!   catch-up. A hold consumes **zero** ticks: replays, snapshots and gameplay never see it.
//!   [`sim::speed`](crate::sim::speed) reports the speed the player chose, and
//!   [`sim::set_speed`](crate::sim::set_speed) during a hold changes that speed, not the hold.
//!   A hold lasts the time asked for plus up to about two frames (it starts from the frame in which it is asked and is checked at the start of the next frame,
//!   before the frame's time is counted).
//! - [`PresentationHolds`]: bodies (by `AiId`) whose picture stands still for some real time while
//!   the clock runs: what a co-op machine does, since no machine may stop the others' clock. The
//!   animation bridge reads it. Never a component on an entity with an `AiId`: replays and
//!   snapshots would hash it.
//!
//! Both are measured on `Time<Real>`. Neither is part of the simulation: they are never saved in a
//! snapshot or recorded in a replay.

use std::collections::BTreeMap;
use std::time::Duration;

use bevy::prelude::*;

use crate::id::AiId;
use crate::sim::SimTick;

/// A finished clock hold, for `ai/status` and tools: how long it really lasted and how many
/// simulation ticks ran inside it (0 when it works).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeldSpan {
    /// `Time<Real>` when the clock stopped, and when it ran again.
    pub from: Duration,
    pub to: Duration,
    /// Ticks that ran between the two.
    pub ticks_inside: u64,
}

impl HeldSpan {
    /// How long the clock stood still.
    pub fn real(&self) -> Duration {
        self.to.saturating_sub(self.from)
    }
}

/// The fixed clock held for real time (see the module docs). Asked with [`request`](Self::request),
/// applied by [`apply_hold`] in `First`.
#[derive(Resource, Debug, Default)]
pub struct ClockHold {
    /// `Time<Real>` the hold ends at; overlapping requests merge by taking the later end.
    until: Option<Duration>,
    /// The speed the player chose, while the clock is held.
    player_speed: f32,
    /// The virtual clock's speed is forced to 0 now; where it stopped.
    began: Option<(Duration, u64)>,
    /// Holds that have begun, and the real time they held the clock in all.
    pub count: u64,
    pub total: Duration,
    /// The newest finished hold.
    pub last: Option<HeldSpan>,
    /// Ticks that ran inside any finished hold (stays 0 when the hold works).
    pub ticks_inside: u64,
}

impl ClockHold {
    /// Holds the clock until `Time<Real>` reaches `until` (a hold already asked for that lasts
    /// longer stays: the later end wins).
    pub fn request(&mut self, until: Duration) {
        self.until = Some(self.until.map_or(until, |old| old.max(until)));
    }

    /// The clock is forced to stand still now.
    pub fn is_held(&self) -> bool {
        self.began.is_some()
    }

    /// Asked for, or held: the clock stands still or is about to.
    pub fn is_pending(&self) -> bool {
        self.until.is_some()
    }

    /// `Time<Real>` the hold ends at.
    pub fn until(&self) -> Option<Duration> {
        self.until
    }

    /// Ticks that ran since the clock stopped, while it is held (0 when the hold works).
    pub fn ticks_since_held(&self, tick: u64) -> Option<u64> {
        self.began.map(|(_, began)| tick.saturating_sub(began))
    }

    /// The speed the player chose while the clock is held.
    pub fn player_speed(&self) -> Option<f32> {
        self.is_held().then_some(self.player_speed)
    }

    /// Sets the player's speed while held; returns whether it was held.
    pub(crate) fn set_player_speed(&mut self, speed: f32) -> bool {
        if self.is_held() {
            self.player_speed = speed;
        }
        self.is_held()
    }
}

/// Ends a hold now: the speed comes back. Called by `step` (a stepped game never holds) and when
/// the clock is paused.
pub fn cancel_hold(world: &mut World) {
    let tick = world.get_resource::<SimTick>().map_or(0, |t| t.0);
    let now = real_now(world);
    let Some(mut hold) = world.get_resource_mut::<ClockHold>() else { return };
    let (held, speed) = (hold.is_held(), hold.player_speed);
    release(&mut hold, tick, now);
    if held {
        world.resource_mut::<Time<Virtual>>().set_relative_speed(speed);
    }
}

fn real_now(world: &World) -> Duration {
    world.get_resource::<Time<Real>>().map_or(Duration::ZERO, Time::elapsed)
}

/// Marks the hold finished at `now`, tick `tick`.
fn release(hold: &mut ClockHold, tick: u64, now: Duration) {
    hold.until = None;
    if let Some((from, began_tick)) = hold.began.take() {
        let span = HeldSpan { from, to: now, ticks_inside: tick.saturating_sub(began_tick) };
        hold.total += span.real();
        hold.ticks_inside += span.ticks_inside;
        hold.last = Some(span);
    }
}

/// `First`, after the gate and before this frame's time is counted: while a hold is due the
/// virtual clock's speed is 0; when it ends the speed comes back. A paused clock ends the hold
/// (nothing runs anyway; the pause is not the hold's).
pub fn apply_hold(mut hold: ResMut<ClockHold>, real: Res<Time<Real>>, tick: Option<Res<SimTick>>, mut virt: ResMut<Time<Virtual>>) {
    let now = real.elapsed();
    let ticks = tick.map_or(0, |t| t.0);
    let due = hold.until.is_some_and(|until| now < until) && !virt.is_paused();
    match (due, hold.began.is_some()) {
        (true, false) => {
            hold.player_speed = virt.relative_speed();
            virt.set_relative_speed(0.0);
            hold.began = Some((now, ticks));
            hold.count += 1;
        }
        (true, true) => {
            // Something set the speed behind the hold's back (not through `sim::set_speed`):
            // that is the player's speed now.
            if virt.relative_speed() != 0.0 {
                hold.player_speed = virt.relative_speed();
                virt.set_relative_speed(0.0);
            }
        }
        (false, true) => {
            virt.set_relative_speed(hold.player_speed);
            release(&mut hold, ticks, now);
        }
        (false, false) => hold.until = None,
    }
}

/// One body's picture hold: when it was asked for and when it ends (`Time<Real>`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    from: Duration,
    until: Duration,
}

/// A held body's drawn pose, kept for the hold and the catch-up after it.
#[derive(Clone, Copy, Debug)]
struct Frozen {
    /// Where the body stood still.
    pose: Transform,
    span: Span,
    /// What [`PresentationHolds::drawn`] returned last, where a new hold during a catch-up starts.
    last: Transform,
}

impl Frozen {
    /// The catch-up lasts as long as the hold did.
    fn catch_up_ends(&self) -> Duration {
        self.span.until + self.span.until.saturating_sub(self.span.from)
    }
}

/// Bodies whose picture is held, by `AiId`, until a `Time<Real>`.
///
/// A held body is drawn standing still (the animation bridge pauses its player;
/// [`drawn`](Self::drawn) freezes its position and rotation) and then catches up to where it
/// really is over as long as the hold lasted. The simulation never sees any of it.
#[derive(Resource, Debug, Default)]
pub struct PresentationHolds {
    spans: BTreeMap<AiId, Span>,
    frozen: BTreeMap<AiId, Frozen>,
    /// Holds asked for so far.
    pub count: u64,
}

impl PresentationHolds {
    /// Holds `id`'s picture from `from` (the real time it is asked at) until `until` (the later
    /// end wins when it is held already).
    pub fn hold(&mut self, id: AiId, from: Duration, until: Duration) {
        let span = self.spans.entry(id).or_insert(Span { from, until });
        if from >= span.until {
            // The old hold was over: a new one.
            *span = Span { from, until };
        }
        span.until = span.until.max(until);
        self.count += 1;
    }

    /// Whether `id`'s picture is held at `now`.
    pub fn is_held(&self, id: AiId, now: Duration) -> bool {
        self.spans.get(&id).is_some_and(|span| now < span.until)
    }

    /// The bodies held at `now`, in id order, with the real time they are held until.
    pub fn held(&self, now: Duration) -> impl Iterator<Item = (AiId, Duration)> + '_ {
        self.spans.iter().filter(move |(_, span)| now < span.until).map(|(id, span)| (*id, span.until))
    }

    /// Forgets the holds that are over, and the catch-ups that are done.
    pub fn prune(&mut self, now: Duration) {
        self.spans.retain(|_, span| now < span.until);
        self.frozen.retain(|_, f| now < f.catch_up_ends());
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty() && self.frozen.is_empty()
    }

    /// Where `id`'s body is drawn at `now` when it really is at `live`: standing where it was
    /// drawn when the hold began for as long as the hold lasts, then blended (linearly, rotation
    /// by slerp, the way render interpolation blends) from there to `live` over as long as the
    /// hold lasted, so it catches up without a jump. A body never held is drawn at `live`. Call
    /// it once per frame per body, where the body's drawn pose is made; scale is always `live`'s.
    pub fn drawn(&mut self, id: AiId, now: Duration, live: Transform) -> Transform {
        if let Some(span) = self.spans.get(&id).copied().filter(|span| now < span.until) {
            let frozen = self.frozen.entry(id).or_insert(Frozen { pose: live, span, last: live });
            if frozen.span.until <= now {
                // A new hold while the last one was catching up: stand still where it was drawn.
                *frozen = Frozen { pose: frozen.last, span, last: frozen.last };
            }
            frozen.span.until = span.until;
            let pose = Transform { scale: live.scale, ..frozen.pose };
            frozen.last = pose;
            return pose;
        }
        let Some(frozen) = self.frozen.get_mut(&id) else { return live };
        let length = frozen.span.until.saturating_sub(frozen.span.from).as_secs_f32();
        let t = if length <= 0.0 { 1.0 } else { now.saturating_sub(frozen.span.until).as_secs_f32() / length };
        if t >= 1.0 {
            self.frozen.remove(&id);
            return live;
        }
        let drawn = Transform {
            translation: frozen.pose.translation.lerp(live.translation, t),
            rotation: frozen.pose.rotation.slerp(live.rotation, t),
            scale: live.scale,
        };
        frozen.last = drawn;
        drawn
    }
}

/// `First`: forgets the picture holds that are over.
pub fn prune_holds(mut holds: ResMut<PresentationHolds>, real: Res<Time<Real>>) {
    if !holds.is_empty() {
        holds.prune(real.elapsed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{self, advance_tick};
    use bevy::time::{TimePlugin, TimeSystems, TimeUpdateStrategy};

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(TimePlugin)
            .init_resource::<SimTick>()
            .init_resource::<ClockHold>()
            .init_resource::<PresentationHolds>()
            .insert_resource(Time::<Fixed>::from_hz(60.0))
            // One fixed step of real time per frame: one tick per frame while nothing is held.
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 60.0)))
            .add_systems(FixedFirst, advance_tick)
            .add_systems(First, (apply_hold, prune_holds).before(TimeSystems));
        app.finish();
        app.cleanup();
        // The first update only starts the clock.
        app.update();
        app
    }

    fn ticks(app: &App) -> u64 {
        app.world().resource::<SimTick>().0
    }

    fn real(app: &App) -> Duration {
        app.world().resource::<Time<Real>>().elapsed()
    }

    #[test]
    fn a_hold_runs_no_tick_and_the_speed_stays_the_players() {
        let mut app = app();
        sim::set_speed(app.world_mut(), 2.0);
        for _ in 0..3 {
            app.update();
        }
        let before = ticks(&app);
        assert!(before >= 3, "frames tick before the hold: {before}");

        // 6 frames of real time (100 ms), asked for from the end of the current frame.
        let until = real(&app) + Duration::from_millis(100);
        app.world_mut().resource_mut::<ClockHold>().request(until);
        for _ in 0..4 {
            app.update();
            assert_eq!(ticks(&app), before, "held: no tick");
            assert_eq!(app.world().resource::<ClockHold>().ticks_since_held(ticks(&app)), Some(0), "none since it stopped");
            assert_eq!(sim::speed(app.world()), 2.0, "the player's speed is what is reported");
            assert!(!sim::is_paused(app.world()), "a hold is not a pause");
        }
        assert!(app.world().resource::<ClockHold>().is_held());
        // A speed set during the hold is the player's, applied after it.
        sim::set_speed(app.world_mut(), 1.0);
        assert_eq!(sim::speed(app.world()), 1.0);
        assert_eq!(app.world().resource::<Time<Virtual>>().relative_speed(), 0.0);
        for _ in 0..10 {
            app.update();
        }
        let hold = app.world().resource::<ClockHold>();
        assert!(!hold.is_held() && !hold.is_pending());
        assert_eq!(app.world().resource::<Time<Virtual>>().relative_speed(), 1.0, "the speed set during the hold came back");
        let span = hold.last.expect("a finished hold");
        assert_eq!((hold.count, span.ticks_inside, hold.ticks_inside), (1, 0, 0), "{span:?}");
        // The hold lasted what was asked, plus under a frame.
        assert!(span.real() >= Duration::from_millis(80) && span.real() < Duration::from_millis(130), "{span:?}");
        // After it the clock counts one tick per frame again, with no catch-up burst.
        let after = ticks(&app);
        app.update();
        app.update();
        assert_eq!(ticks(&app), after + 2);
    }

    #[test]
    fn overlapping_requests_keep_the_later_end() {
        let mut hold = ClockHold::default();
        hold.request(Duration::from_millis(200));
        hold.request(Duration::from_millis(120));
        assert_eq!(hold.until(), Some(Duration::from_millis(200)));
        hold.request(Duration::from_millis(300));
        assert_eq!(hold.until(), Some(Duration::from_millis(300)));
    }

    #[test]
    fn a_pause_and_a_step_end_the_hold_and_the_pause_stays_the_players() {
        let mut app = app();
        let until = real(&app) + Duration::from_secs(10);
        app.world_mut().resource_mut::<ClockHold>().request(until);
        app.update();
        assert!(app.world().resource::<ClockHold>().is_held());
        sim::pause(app.world_mut());
        app.update();
        assert!(!app.world().resource::<ClockHold>().is_held(), "paused: not held");
        assert!(sim::is_paused(app.world()));
        sim::resume(app.world_mut());
        let before = ticks(&app);
        app.update();
        assert_eq!(ticks(&app), before + 1, "the speed is back");

        // A step cancels a hold that is running.
        let until = real(&app) + Duration::from_secs(10);
        app.world_mut().resource_mut::<ClockHold>().request(until);
        app.update();
        assert!(app.world().resource::<ClockHold>().is_held());
        let report = sim::step(app.world_mut(), 3, Duration::from_secs(5), |_| None).unwrap();
        assert_eq!(report.ticks(), 3);
        assert!(!app.world().resource::<ClockHold>().is_pending());
        assert_eq!(app.world().resource::<Time<Virtual>>().relative_speed(), 1.0);
    }

    #[test]
    fn picture_holds_merge_by_the_later_end_and_expire() {
        let mut app = app();
        let now = real(&app);
        let (a, b) = (AiId(5), AiId(2));
        {
            let mut holds = app.world_mut().resource_mut::<PresentationHolds>();
            holds.hold(a, now, now + Duration::from_millis(100));
            holds.hold(a, now, now + Duration::from_millis(40));
            holds.hold(b, now, now + Duration::from_millis(40));
            assert!(holds.is_held(a, now) && holds.is_held(b, now));
            assert_eq!(holds.held(now).map(|(id, _)| id).collect::<Vec<_>>(), [b, a], "in id order");
        }
        for _ in 0..4 {
            app.update();
        }
        let now = real(&app);
        let holds = app.world().resource::<PresentationHolds>();
        assert!(holds.is_held(a, now) && !holds.is_held(b, now), "b ended at 40 ms, a at 100 ms");
        for _ in 0..6 {
            app.update();
        }
        assert!(app.world().resource::<PresentationHolds>().is_empty());
    }

    fn at(x: f32) -> Transform {
        Transform::from_xyz(x, 0.0, 0.0)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_held_body_is_drawn_where_it_stood_then_blended_to_the_live_pose_over_the_hold() {
        let mut holds = PresentationHolds::default();
        let id = AiId(3);
        assert_eq!(holds.drawn(id, ms(0), at(9.0)), at(9.0), "never held: live");
        holds.hold(id, ms(1000), ms(1100));
        // Held: the pose of the first frame, whatever the live one does (the scale is the live one's).
        assert_eq!(holds.drawn(id, ms(1000), at(1.0)), at(1.0));
        assert_eq!(holds.drawn(id, ms(1060), at(5.0)), at(1.0));
        let scaled = Transform { scale: Vec3::splat(2.0), ..at(7.0) };
        assert_eq!(holds.drawn(id, ms(1099), scaled).translation, Vec3::X);
        assert_eq!(holds.drawn(id, ms(1099), scaled).scale, Vec3::splat(2.0));
        // The catch-up lasts as long as the hold did: half way at 1150, there at 1200.
        assert!((holds.drawn(id, ms(1100), at(5.0)).translation.x - 1.0).abs() < 1e-5, "starts where it stood");
        assert!((holds.drawn(id, ms(1150), at(5.0)).translation.x - 3.0).abs() < 1e-4);
        assert_eq!(holds.drawn(id, ms(1200), at(5.0)), at(5.0));
        holds.prune(ms(1200));
        assert!(holds.is_empty(), "forgotten once done");
        assert_eq!(holds.drawn(id, ms(1300), at(8.0)), at(8.0));
    }

    #[test]
    fn a_hold_during_a_catch_up_stands_where_the_body_was_drawn_and_rotation_takes_the_short_way() {
        let mut holds = PresentationHolds::default();
        let id = AiId(1);
        holds.hold(id, ms(0), ms(100));
        holds.drawn(id, ms(0), at(0.0));
        let half = holds.drawn(id, ms(150), at(4.0));
        assert!((half.translation.x - 2.0).abs() < 1e-4);
        holds.hold(id, ms(150), ms(250));
        assert!((holds.drawn(id, ms(160), at(9.0)).translation.x - 2.0).abs() < 1e-4, "frozen where it was drawn");
        // Turned a quarter: half way is an eighth.
        let mut holds = PresentationHolds::default();
        holds.hold(id, ms(0), ms(100));
        holds.drawn(id, ms(0), Transform::IDENTITY);
        let turned = Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let mid = holds.drawn(id, ms(150), turned);
        assert!(mid.rotation.angle_between(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)) < 1e-4);
    }
}
