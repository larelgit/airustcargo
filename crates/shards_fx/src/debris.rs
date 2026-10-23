//! Debris (F9): a particle that lands. A chunk of an effect with `collide: true` is no longer a
//! function of its age: it is stepped, `STEP` seconds at a time on the simulation's clock, a ray
//! per step against the world's colliders. It falls, hits a surface, bounces (`restitution`),
//! loses speed to the surface (`friction`), slides, and comes to rest lying on it; with
//! `rest: UntilCleared` it then stays until it is cleared or recycled.
//!
//! The state is pure numbers here and the world is behind [`Ground`] (a ray), so the whole
//! motion is tested without Bevy's physics. Nothing of it is the simulation's: the numbers are
//! picture only, and nothing here reads or writes a gameplay component. A particle is stepped
//! with the same fixed `STEP` whatever the frame rate, so a frame that comes late catches up to
//! the same place, and the shown pose is interpolated between the last two steps.

use bevy::prelude::*;

/// The time one step covers (s).
pub const STEP: f64 = 1.0 / 60.0;
/// How far above a surface a sliding particle looks for the surface again (m).
const SEAT: f32 = 0.04;
/// How far below its feet a sliding particle still follows the ground (m): a step down.
const FOLLOW: f32 = 0.15;
/// A particle in contact slower than this, on a surface this flat or flatter, is at rest (m/s).
const REST_SPEED: f32 = 0.1;
/// A bounce slower than this (m/s) is no bounce: the particle stays on the surface and slides.
const SETTLE_SPEED: f32 = 0.6;
/// The least `normal.y` of a surface that holds a particle at rest (a floor, a shallow slope).
const FLAT: f32 = 0.5;
/// A particle that has bounced this many times stops bouncing, whatever its speed.
const MAX_BOUNCES: u8 = 32;
/// How fast a particle in contact turns to lie flat (1/s).
const LIE_DOWN: f32 = 12.0;
/// How fast its tumble dies away in contact (1/s).
const SPIN_DAMP: f32 = 8.0;

/// Where a ray met the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub point: Vec3,
    /// The surface's unit normal, facing the ray's origin.
    pub normal: Vec3,
}

/// The world as a particle meets it: the nearest surface along a ray, if there is one within `max`.
pub trait Ground {
    fn cast(&mut self, from: Vec3, dir: Vec3, max: f32) -> Option<Surface>;
}

/// What moves a particle: an effect's numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Physics {
    /// m/s² along Y.
    pub gravity: f32,
    /// 1/s: the speed decays as e^(−drag·t).
    pub drag: f32,
    /// The share of its speed along the normal a bounce keeps.
    pub restitution: f32,
    /// How much of the speed along a surface a bounce takes, and how hard a sliding particle is held back.
    pub friction: f32,
}

impl Physics {
    /// The numbers an effect moves its particles by.
    pub fn of(def: &crate::def::EffectDef) -> Self {
        Self { gravity: def.gravity, drag: def.drag, restitution: def.restitution, friction: def.friction }
    }
}

/// A stepped particle: where it is now and a step ago, how it is turned, and whether it lies still.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Debris {
    /// The clock (s) the state `to` is for.
    pub t: f64,
    /// The place a step ago and now.
    pub from: Vec3,
    pub to: Vec3,
    pub velocity: Vec3,
    /// How it was turned a step ago and now.
    pub turn_from: Quat,
    pub turn_to: Quat,
    pub spin_axis: Vec3,
    /// Radians a second.
    pub spin: f32,
    /// The turn about the surface's normal it lies down with (radians).
    pub yaw: f32,
    /// The normal of the surface it lies or slides on, while it does.
    pub contact: Option<Vec3>,
    /// It has come to rest: it is not stepped again.
    pub rested: bool,
    /// It keeps lying until it is cleared (`rest: UntilCleared`), however old it gets.
    pub keep: bool,
    pub bounces: u8,
}

impl Debris {
    /// A particle leaving `at` with `velocity` at clock `born`.
    pub fn new(at: Vec3, velocity: Vec3, born: f64, spin_axis: Vec3, spin: f32, yaw: f32, keep: bool) -> Self {
        Self {
            t: born,
            from: at,
            to: at,
            velocity,
            turn_from: Quat::IDENTITY,
            turn_to: Quat::IDENTITY,
            spin_axis,
            spin,
            yaw,
            contact: None,
            rested: false,
            keep,
            bounces: 0,
        }
    }

    /// Steps it until its state is at or past clock `now`, a ray or two per step, at most until `rays`
    /// has counted `budget` (the steps it could not take are taken by a later frame). `radius` is the
    /// distance from its middle to its surface at a clock: the size the particle has then, halved.
    pub fn advance(
        &mut self,
        now: f64,
        physics: &Physics,
        radius: &dyn Fn(f64) -> f32,
        ground: &mut dyn Ground,
        rays: &mut u32,
        budget: u32,
    ) {
        while !self.rested && self.t < now && *rays < budget {
            self.step(physics, radius(self.t + STEP).max(0.0), ground, rays);
        }
    }

    /// Where it is and how it is turned at clock `now`, between its last two steps.
    pub fn shown(&self, now: f64) -> (Vec3, Quat) {
        let k = (((now - (self.t - STEP)) / STEP) as f32).clamp(0.0, 1.0);
        (self.from.lerp(self.to, k), self.turn_from.slerp(self.turn_to, k))
    }

    fn step(&mut self, p: &Physics, r: f32, ground: &mut dyn Ground, rays: &mut u32) {
        let h = STEP as f32;
        self.from = self.to;
        self.turn_from = self.turn_to;
        self.t += STEP;
        match self.contact {
            Some(normal) => self.slide(normal, p, r, h, ground, rays),
            None => self.fly(p, r, h, ground, rays),
        }
        self.turn(h);
    }

    /// One step through the air: gravity and drag, then a ray along the way.
    fn fly(&mut self, p: &Physics, r: f32, h: f32, ground: &mut dyn Ground, rays: &mut u32) {
        self.velocity.y += p.gravity * h;
        if p.drag > 0.0 {
            self.velocity *= ops::exp(-p.drag * h);
        }
        let delta = self.velocity * h;
        match self.cast_along(delta, r, ground, rays) {
            Some(hit) => self.land(hit, p, r),
            None => self.to += delta,
        }
    }

    /// One step along a surface: held back by friction, pulled along it by gravity, kept on it by a
    /// ray down, and stopped by whatever stands in the way.
    fn slide(&mut self, n: Vec3, p: &Physics, r: f32, h: f32, ground: &mut dyn Ground, rays: &mut u32) {
        let g = Vec3::Y * p.gravity;
        self.velocity -= n * self.velocity.dot(n);
        self.velocity += (g - n * g.dot(n)) * h;
        let load = (-g.dot(n)).max(0.0);
        let speed = self.velocity.length();
        if speed > 1e-6 {
            self.velocity *= (speed - p.friction * load * h).max(0.0) / speed;
        }
        if p.drag > 0.0 {
            self.velocity *= ops::exp(-p.drag * h);
        }
        if n.y >= FLAT && self.velocity.length() < REST_SPEED {
            self.rest(n);
            return;
        }
        let delta = self.velocity * h;
        if let Some(hit) = self.cast_along(delta, r, ground, rays) {
            // Something stands in the way: an edge, a wall.
            self.contact = None;
            self.land(hit, p, r);
            return;
        }
        let next = self.to + delta;
        *rays += 1;
        match ground.cast(next + n * (r + SEAT), -n, 2.0 * r + 2.0 * SEAT + FOLLOW) {
            Some(under) => {
                self.to = under.point + under.normal * r;
                self.velocity -= under.normal * self.velocity.dot(under.normal);
                self.contact = Some(under.normal);
            }
            // The ground ends here: it falls.
            None => {
                self.to = next;
                self.contact = None;
            }
        }
    }

    /// The first surface along `delta` from where it is, a radius `r` short of it: one ray.
    fn cast_along(&self, delta: Vec3, r: f32, ground: &mut dyn Ground, rays: &mut u32) -> Option<Surface> {
        let length = delta.length();
        if length <= 1e-5 {
            return None;
        }
        *rays += 1;
        ground.cast(self.to, delta / length, length + r).filter(|hit| hit.normal.dot(delta) < 0.0)
    }

    /// Met `hit`: sits on it (its middle a radius `r` off), and bounces or settles.
    fn land(&mut self, hit: Surface, p: &Physics, r: f32) {
        let n = hit.normal;
        self.to = hit.point + n * r;
        let vn = self.velocity.dot(n);
        if vn >= 0.0 {
            return;
        }
        let vt = self.velocity - n * vn;
        let away = -vn * p.restitution;
        let lost = p.friction * (1.0 + p.restitution) * -vn;
        let along = vt.length();
        let vt = if along > lost { vt * (1.0 - lost / along) } else { Vec3::ZERO };
        self.spin *= 0.6;
        self.bounces = self.bounces.saturating_add(1);
        if away < SETTLE_SPEED || self.bounces >= MAX_BOUNCES {
            // Settles: from here it slides along the surface. A ceiling does not hold it: it drops.
            self.velocity = vt;
            if n.y > -0.05 {
                self.contact = Some(n);
            }
        } else {
            self.velocity = vt + n * away;
        }
    }

    fn rest(&mut self, n: Vec3) {
        self.rested = true;
        self.velocity = Vec3::ZERO;
        self.spin = 0.0;
        self.from = self.to;
        self.turn_to = lying(n, self.yaw);
        self.turn_from = self.turn_to;
    }

    /// Turns it: tumbling in the air, laying down in contact.
    fn turn(&mut self, h: f32) {
        match self.contact {
            Some(n) if !self.rested => {
                self.turn_to = self.turn_from.slerp(lying(n, self.yaw), 1.0 - ops::exp(-LIE_DOWN * h)).normalize();
                self.spin *= ops::exp(-SPIN_DAMP * h);
            }
            None if self.spin != 0.0 => {
                self.turn_to = (Quat::from_axis_angle(self.spin_axis, self.spin * h) * self.turn_from).normalize();
            }
            _ => {}
        }
    }
}

/// How a particle lies on a surface of normal `n`: its up along the normal, turned by `yaw` about it.
fn lying(n: Vec3, yaw: f32) -> Quat {
    (Quat::from_rotation_arc(Vec3::Y, n) * Quat::from_rotation_y(yaw)).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A floor at `floor` (ending at `floor_until` along x), a wall at x = `wall` and a ceiling, all optional.
    struct Room {
        floor: f32,
        floor_until: f32,
        wall: Option<f32>,
        ceiling: Option<f32>,
    }

    impl Room {
        fn floor() -> Self {
            Self { floor: 0.0, floor_until: f32::MAX, wall: None, ceiling: None }
        }
    }

    impl Ground for Room {
        fn cast(&mut self, from: Vec3, dir: Vec3, max: f32) -> Option<Surface> {
            let mut best: Option<(f32, Surface)> = None;
            let mut consider = |t: f32, normal: Vec3| {
                if (0.0..=max).contains(&t) && best.is_none_or(|(b, _)| t < b) {
                    best = Some((t, Surface { point: from + dir * t, normal }));
                }
            };
            if dir.y < -1e-6 {
                let t = (self.floor - from.y) / dir.y;
                if (from + dir * t).x <= self.floor_until {
                    consider(t, Vec3::Y);
                }
            }
            if let Some(x) = self.wall.filter(|_| dir.x > 1e-6) {
                consider((x - from.x) / dir.x, Vec3::NEG_X);
            }
            if let Some(y) = self.ceiling.filter(|_| dir.y > 1e-6) {
                consider((y - from.y) / dir.y, Vec3::NEG_Y);
            }
            best.map(|(_, s)| s)
        }
    }

    const R: f32 = 0.05;

    fn chunk(at: Vec3, velocity: Vec3) -> Debris {
        Debris::new(at, velocity, 0.0, Vec3::X, 4.0, 0.7, true)
    }

    fn physics(restitution: f32, friction: f32) -> Physics {
        Physics { gravity: -9.8, drag: 0.0, restitution, friction }
    }

    /// Runs `d` to clock `until` in one go, with no limit on the rays.
    fn run(d: &mut Debris, p: &Physics, room: &mut Room, until: f64) -> u32 {
        let mut rays = 0;
        d.advance(until, p, &|_| R, room, &mut rays, u32::MAX);
        rays
    }

    #[test]
    fn a_chunk_bounces_lower_each_time_and_comes_to_rest_lying_on_the_floor() {
        let (p, mut room) = (physics(0.5, 0.6), Room::floor());
        let mut d = chunk(Vec3::new(0.0, 2.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        // The apex after each landing falls with the square of the restitution.
        let mut apexes = Vec::new();
        let mut top = 0.0f32;
        let mut rays = 0;
        while !d.rested && d.t < 20.0 {
            let before = d.velocity.y;
            d.advance(d.t + STEP, &p, &|_| R, &mut room, &mut rays, u32::MAX);
            if before > 0.0 && d.velocity.y <= 0.0 {
                apexes.push(top);
                top = 0.0;
            }
            if d.velocity.y > 0.0 {
                top = top.max(d.to.y - R);
            }
        }
        assert!(d.rested, "it settles within 20 s, at {} after {} bounces", d.t, d.bounces);
        assert!(d.bounces >= 2);
        assert!(apexes.len() >= 2 && apexes.windows(2).all(|w| w[1] < w[0]), "each bounce lower than the one before: {apexes:?}");
        assert!((d.to.y - R).abs() < 1e-4, "its middle a radius above the floor: {}", d.to.y);
        let up = d.turn_to * Vec3::Y;
        assert!(up.abs_diff_eq(Vec3::Y, 1e-3), "lying flat: {up}");
        // At rest it is not stepped again, and not shown anywhere else.
        let at = d;
        assert_eq!(run(&mut d, &p, &mut room, 60.0), 0);
        assert_eq!(d, at);
        assert_eq!(d.shown(60.0).0, at.to);
    }

    #[test]
    fn a_fast_chunk_does_not_go_through_the_floor() {
        let (p, mut room) = (physics(0.3, 0.5), Room::floor());
        for speed in [5.0, 30.0, 120.0, 600.0] {
            let mut d = chunk(Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, -speed, 0.0));
            let mut rays = 0;
            while d.t < 6.0 && !d.rested {
                d.advance(d.t + STEP, &p, &|_| R, &mut room, &mut rays, u32::MAX);
                assert!(d.to.y >= R - 1e-3, "{speed} m/s: under the floor at {} (t {})", d.to.y, d.t);
            }
        }
    }

    #[test]
    fn friction_stops_a_slide_sooner() {
        let slid = |friction: f32| {
            let mut d = chunk(Vec3::new(0.0, 0.5, 0.0), Vec3::new(4.0, 0.0, 0.0));
            run(&mut d, &physics(0.2, friction), &mut Room::floor(), 30.0);
            assert!(d.rested, "friction {friction}: at rest");
            d.to.x
        };
        let (rough, smooth) = (slid(0.9), slid(0.05));
        assert!(rough < smooth, "{rough} against {smooth}");
        assert!(rough > 0.5, "it still travelled: {rough}");
    }

    #[test]
    fn a_wall_throws_it_back_and_a_ceiling_lets_it_drop() {
        let p = physics(0.6, 0.3);
        let mut room = Room { floor: 0.0, floor_until: f32::MAX, wall: Some(3.0), ceiling: Some(2.0) };
        let mut d = chunk(Vec3::new(0.0, 1.0, 0.0), Vec3::new(12.0, 0.0, 0.0));
        let mut rays = 0;
        let mut turned = false;
        while d.t < 3.0 && !d.rested {
            d.advance(d.t + STEP, &p, &|_| R, &mut room, &mut rays, u32::MAX);
            assert!(d.to.x <= 3.0 - R + 1e-3, "through the wall at {}", d.to.x);
            turned |= d.velocity.x < -1.0;
        }
        assert!(turned, "it came back");
        // Thrown up, it hits the ceiling and does not stay there.
        let mut d = chunk(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 12.0, 0.0));
        let mut touched = false;
        while d.t < 6.0 && !d.rested {
            d.advance(d.t + STEP, &p, &|_| R, &mut room, &mut rays, u32::MAX);
            assert!(d.to.y <= 2.0 - R + 1e-3, "through the ceiling at {}", d.to.y);
            touched |= d.to.y > 2.0 - 2.0 * R;
        }
        assert!(touched && d.rested && (d.to.y - R).abs() < 1e-3, "it hit the ceiling, fell and lies on the floor: {}", d.to.y);
    }

    #[test]
    fn a_slide_that_runs_out_of_floor_falls() {
        let p = physics(0.2, 0.2);
        let mut room = Room { floor: 0.0, floor_until: 2.0, wall: None, ceiling: None };
        let mut d = chunk(Vec3::new(0.0, 0.3, 0.0), Vec3::new(5.0, 0.0, 0.0));
        run(&mut d, &p, &mut room, 3.0);
        assert!(d.to.x > 2.0 && d.to.y < -1.0, "off the edge and still falling: {}", d.to);
        assert!(!d.rested && d.contact.is_none());
    }

    #[test]
    fn frames_that_come_late_catch_up_to_the_same_place() {
        let p = physics(0.45, 0.5);
        let one = {
            let mut d = chunk(Vec3::new(0.0, 2.0, 0.0), Vec3::new(2.0, 1.0, -1.0));
            run(&mut d, &p, &mut Room::floor(), 8.0);
            d
        };
        // Frames of 7, 16 and 90 ms, and every fourth one allowed 5 rays only.
        let mut d = chunk(Vec3::new(0.0, 2.0, 0.0), Vec3::new(2.0, 1.0, -1.0));
        let mut room = Room::floor();
        let (mut now, mut frame) = (0.0, 0usize);
        while now < 8.0 {
            now += [0.007, 0.016, 0.090][frame % 3];
            let mut rays = 0;
            d.advance(now, &p, &|_| R, &mut room, &mut rays, if frame % 4 == 0 { 5 } else { u32::MAX });
            frame += 1;
        }
        let mut rays = 0;
        d.advance(8.0, &p, &|_| R, &mut room, &mut rays, u32::MAX);
        assert_eq!((d.to, d.rested, d.bounces), (one.to, one.rested, one.bounces));
        // A budget that is spent leaves the rest of the steps for the next frame.
        let mut d = chunk(Vec3::new(0.0, 2.0, 0.0), Vec3::ZERO);
        let mut rays = 0;
        d.advance(5.0, &p, &|_| R, &mut room, &mut rays, 10);
        assert!((10..=12).contains(&rays) && d.t < 1.0, "{rays} rays, at {}", d.t);
    }

    #[test]
    fn a_shown_pose_lies_between_the_last_two_steps() {
        let p = physics(0.5, 0.5);
        let mut d = chunk(Vec3::new(0.0, 5.0, 0.0), Vec3::ZERO);
        run(&mut d, &p, &mut Room::floor(), 0.5);
        let (a, b) = (d.from, d.to);
        assert!(b.y < a.y);
        let mid = d.shown(d.t - STEP * 0.5).0;
        assert!(mid.abs_diff_eq((a + b) * 0.5, 1e-4), "{mid}");
        assert_eq!(d.shown(d.t + 5.0).0, b, "ahead of its steps it is where it last was");
    }
}
