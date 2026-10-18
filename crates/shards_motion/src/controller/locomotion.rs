//! One character, one tick: the movement modes and the transitions between them.
//!
//! Velocities change over the tick and positions move by the average of the old and
//! the new velocity: exact for constant acceleration, so a jump peaks at `jump_height`
//! whatever the tick rate.

use bevy::prelude::*;

use super::mover::{clip, flat, Mover, Slide, Touch};
use super::params::{CharacterParams, MANTLE_SLACK};
use super::{LocomotionInput, Mantle, MotionMode, MotionState};
use crate::modifiers::MoveModifiers;

/// Landing is noticed this far above the ground (m).
const LAND_PROBE: f32 = 0.05;
/// A swimmer stands up where the water is this much shallower than `swim_depth` (m).
const SHALLOW_MARGIN: f32 = 0.1;
/// How fast a swimmer drifts to the floating height (1/s of the height error).
const FLOAT_RATE: f32 = 4.0;
/// A ledge is climbed when the input points at it within 60°.
const PUSH_COS: f32 = 0.5;
/// A jump in water climbs out onto a ledge this close in front of the body (m).
pub(crate) const CLIMB_OUT_REACH: f32 = 0.3;

/// What happened, for the events the system triggers afterwards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Note {
    Mode(MotionMode, MotionMode),
    Jumped,
    Landed { fall_height: f32, impact_speed: f32, tier: u8, jumped: bool },
    Mantled { height: f32 },
    Entered(Entity),
    Exited(Entity),
}

pub(crate) struct Step<'a, 'w> {
    pub mover: &'a Mover<'w>,
    pub params: &'a CharacterParams,
    /// What slows, caps or roots the character this tick.
    pub modifiers: &'a MoveModifiers,
    /// This tick's input, `jump` and `external` included (the component's are consumed).
    pub input: &'a LocomotionInput,
    pub state: &'a mut MotionState,
    pub dt: f32,
    /// Where the character faces, on the ground plane.
    pub facing: Vec3,
    /// The fall this tick continues began with a jump (tracked with landing tiers only).
    pub jumped_flight: bool,
    pub notes: Vec<Note>,
}

impl Step<'_, '_> {
    /// Runs the tick and returns the new feet position.
    pub fn run(&mut self, feet: Vec3) -> Vec3 {
        if self.state.placed.is_some_and(|at| at != feet) {
            self.restart(feet);
        }
        if self.input.jump {
            self.state.jump_buffer_left = self.params.jump_buffer;
        }
        // A climb follows a path checked when it started; everything else first gets
        // out of whatever it was pushed into.
        let climbing = self.state.mode == MotionMode::Mantle;
        let feet = if climbing { feet } else { self.mover.depenetrate(feet) };
        let feet = match self.state.mode {
            MotionMode::Mantle => self.mantle(feet),
            MotionMode::Ground => self.ground(feet),
            MotionMode::Air | MotionMode::Glide => self.air(feet),
            MotionMode::Water => self.water(feet),
        };
        let feet = if self.state.mode == MotionMode::Mantle { feet } else { self.mover.depenetrate(feet) };
        let feet = self.surroundings(feet);
        let dt = self.dt;
        let s = &mut *self.state;
        s.jump_buffer_left = (s.jump_buffer_left - dt).max(0.0);
        if s.mode != MotionMode::Ground {
            s.coyote_left = (s.coyote_left - dt).max(0.0);
        }
        feet
    }

    /// Someone else moved the body (a teleport, a respawn): the speed, the fall and the
    /// climb belonged to where it was. Standing if there is ground under the feet, else
    /// falling from here, from rest.
    fn restart(&mut self, feet: Vec3) {
        let standing = self.mover.ground(feet, self.params.snap_distance).is_some_and(|g| g.walkable);
        let s = &mut *self.state;
        s.velocity = Vec3::ZERO;
        s.mantle = None;
        s.fall_from_y = None;
        s.coyote_left = 0.0;
        s.grounded = false;
        s.ground_normal = Vec3::Y;
        self.set_mode(if standing { MotionMode::Ground } else { MotionMode::Air });
    }

    fn set_mode(&mut self, to: MotionMode) {
        let from = self.state.mode;
        if from != to {
            self.state.mode = to;
            self.notes.push(Note::Mode(from, to));
        }
    }

    /// Movement input on the ground plane, nothing while an action owns the body.
    fn wish(&self) -> Vec3 {
        if self.input.locked { Vec3::ZERO } else { flat(self.input.move_dir).clamp_length_max(1.0) }
    }

    // --- ground ---------------------------------------------------------------------

    fn ground(&mut self, feet: Vec3) -> Vec3 {
        let p = self.params;
        if self.state.jump_buffer_left > 0.0 && !self.input.locked {
            return self.jump(feet);
        }
        let wish = self.wish();
        let (speed, sprinting) = self.modifiers.ground_speed(p, wish.length(), self.input.sprint);
        self.state.sprinting = sprinting;
        let target = wish.normalize_or_zero() * speed;
        let v0 = flat(self.state.velocity);
        let braking = if p.brake_only_without_input {
            wish == Vec3::ZERO
        } else {
            target.length_squared() < v0.length_squared()
        };
        let rate = if braking { self.deceleration() } else { self.acceleration() };
        let mut v = step_toward(v0, target, rate * self.dt, p.axis_stepping);
        let delta = along_ground((v0 + v) * 0.5 * self.dt, self.state.ground_normal) + self.input.external;
        let (moved, stepped) = self.walk(feet, delta);
        for touch in moved.touches.iter().filter(|t| !t.walkable) {
            v = clip(v, touch.plane);
        }
        self.state.velocity = v;
        if !stepped
            && let Some(wall) = moved.wall()
            && let Some(feet) = self.try_mantle(moved.feet, &wall, p.step_height, true, None, p.mantle_height)
        {
            return feet;
        }
        self.stick(moved.feet)
    }

    /// Slide along the ground; when something steep blocks the way, also try stepping
    /// over it and keep whichever got further. `true`: the step won.
    fn walk(&self, feet: Vec3, delta: Vec3) -> (Slide, bool) {
        let plain = self.mover.slide(feet, delta, true);
        if plain.touches.iter().all(|t| t.walkable) {
            return (plain, false);
        }
        let want = flat(delta);
        let Some(dir) = want.try_normalize() else {
            return (plain, false);
        };
        let Some(stepped) = self.mover.step_up(feet, want, self.params.step_height) else {
            return (plain, false);
        };
        let progress = |s: &Slide| flat(s.feet - feet).dot(dir);
        if progress(&stepped) > progress(&plain) + 1e-4 { (stepped, true) } else { (plain, false) }
    }

    /// Keeps the feet on walkable ground within snap distance, or starts falling.
    fn stick(&mut self, feet: Vec3) -> Vec3 {
        match self.mover.ground(feet, self.params.snap_distance) {
            Some(ground) if ground.walkable => {
                let s = &mut *self.state;
                s.grounded = true;
                s.ground_normal = ground.normal;
                s.coyote_left = self.params.coyote_time;
                s.velocity.y = 0.0;
                feet - Vec3::Y * ground.drop
            }
            _ => {
                let s = &mut *self.state;
                s.grounded = false;
                s.ground_normal = Vec3::Y;
                s.fall_from_y = Some(feet.y);
                s.velocity.y = s.velocity.y.min(0.0);
                self.set_mode(MotionMode::Air);
                feet
            }
        }
    }

    fn jump(&mut self, feet: Vec3) -> Vec3 {
        let s = &mut *self.state;
        s.velocity.y = self.params.jump_speed();
        s.jump_buffer_left = 0.0;
        s.coyote_left = 0.0;
        s.grounded = false;
        s.ground_normal = Vec3::Y;
        s.fall_from_y = Some(feet.y);
        self.notes.push(Note::Jumped);
        self.set_mode(MotionMode::Air);
        self.air(feet)
    }

    // --- air and glide ----------------------------------------------------------------

    fn air(&mut self, feet: Vec3) -> Vec3 {
        let p = self.params;
        let locked = self.input.locked;
        if self.state.mode == MotionMode::Air
            && self.state.jump_buffer_left > 0.0
            && self.state.coyote_left > 0.0
            && !locked
        {
            return self.jump(feet);
        }
        let glide = self.input.glide && !locked;
        if self.state.mode == MotionMode::Air && glide && self.state.velocity.y < 0.0 {
            self.set_mode(MotionMode::Glide);
        } else if self.state.mode == MotionMode::Glide && !glide {
            self.set_mode(MotionMode::Air);
        }

        let v0 = self.state.velocity;
        let (h0, dt) = (flat(v0), self.dt);
        let wish = self.wish();
        let (h, vy) = if self.state.mode == MotionMode::Glide {
            let dir = wish.try_normalize().or(h0.try_normalize()).unwrap_or(self.facing);
            let h = approach(h0, dir * p.glide_speed, p.glide_accel * dt);
            let vy = match self.state.updraft {
                Some(updraft) => (v0.y + updraft.x * dt).min(updraft.y),
                None => (v0.y - p.gravity * dt).max(-p.glide_fall_speed),
            };
            (h, vy)
        } else {
            let h = match wish.try_normalize() {
                Some(dir) => {
                    let (speed, _) = self.modifiers.ground_speed(p, wish.length(), self.input.sprint);
                    step_toward(h0, dir * speed, self.acceleration() * p.air_control * dt, p.axis_stepping)
                }
                // No input: the air keeps its speed unless the params brake it.
                None if p.air_deceleration > 0.0 => {
                    let rate = self.modifiers.deceleration.unwrap_or(p.air_deceleration);
                    step_toward(h0, Vec3::ZERO, rate * dt, p.axis_stepping)
                }
                None => h0,
            };
            (h, (v0.y - p.gravity * dt).max(-p.terminal_speed))
        };
        let mut v = h + Vec3::Y * vy;
        let moved = self.mover.slide(feet, (v0 + v) * 0.5 * dt + self.input.external, false);
        for touch in &moved.touches {
            v = clip(v, touch.normal);
        }
        self.state.velocity = v;
        // A lip no higher than a step is no ledge: the fall ends on the tread below it,
        // and the walk steps up.
        if let Some(wall) = moved.wall()
            && let Some(feet) = self.try_mantle(moved.feet, &wall, p.step_height, true, None, p.mantle_height)
        {
            return feet;
        }

        let feet = moved.feet;
        let s = &mut *self.state;
        s.fall_from_y = Some(s.fall_from_y.map_or(feet.y, |top| top.max(feet.y)));
        if vy <= 0.0
            && let Some(ground) = self.mover.ground(feet, LAND_PROBE)
            && ground.walkable
        {
            // The bottom of water deep enough to swim in: a fast fall can pass the whole
            // float depth in one tick. That is a swim, not a landing.
            if self.mover.water(feet).is_some_and(|surface| self.depth(feet, surface) > p.swim_depth) {
                self.enter_water();
                return feet;
            }
            let feet = feet - Vec3::Y * ground.drop;
            let s = &mut *self.state;
            let fall_height = s.fall_from_y.take().map_or(0.0, |top| (top - feet.y).max(0.0));
            s.velocity.y = 0.0;
            s.grounded = true;
            s.ground_normal = ground.normal;
            s.coyote_left = p.coyote_time;
            // Shallows break a fall: no landing is reported where the water is deeper than the limit.
            let wet = p.landing_wade_limit.is_some_and(|limit| {
                self.mover.water(feet).is_some_and(|surface| surface - feet.y > limit)
            });
            if !wet {
                let jumped = self.jumped_flight;
                let tier = p.landing_tier(-vy, jumped);
                self.notes.push(Note::Landed { fall_height, impact_speed: -vy, tier, jumped });
            }
            self.set_mode(MotionMode::Ground);
            return feet;
        }
        feet
    }

    // --- water ----------------------------------------------------------------------

    fn water(&mut self, feet: Vec3) -> Vec3 {
        let p = self.params;
        let Some(surface) = self.state.water_surface else {
            self.state.fall_from_y = Some(feet.y);
            self.set_mode(MotionMode::Air);
            return self.air(feet);
        };
        // Jump in water: climb out where there is a ledge ahead.
        if self.state.jump_buffer_left > 0.0 && !self.input.locked {
            let dir = self.wish().try_normalize().unwrap_or(self.facing);
            // A surface-relative window when the params have one, else the ordinary reach.
            let reach = p.climb_out_max_height.map_or(p.mantle_height, |limit| {
                (surface + limit + self.mover.skin - feet.y).max(0.0)
            });
            if let Some(wall) = self.wall_ahead(feet, dir)
                && let Some(feet) = self.try_mantle(feet, &wall, 0.0, false, None, reach)
            {
                self.state.jump_buffer_left = 0.0;
                return feet;
            }
        }
        let v0 = self.state.velocity;
        let wish = self.wish();
        let fast = p.swim_fast_speed > 0.0
            && self.input.sprint
            && !self.modifiers.no_sprint
            && wish != Vec3::ZERO;
        let top = self.modifiers.swim_speed(if fast { p.swim_fast_speed } else { p.swim_speed });
        let rate = swim_rate(p, wish != Vec3::ZERO);
        let h = step_toward(flat(v0), wish * top, rate * self.dt, p.axis_stepping);
        let float_y = surface - p.float_depth;
        let vy = ((float_y - feet.y) * FLOAT_RATE).clamp(-p.swim_speed, p.swim_speed);
        let mut v = h + Vec3::Y * vy;
        let moved = self.mover.slide(feet, (v0 + v) * 0.5 * self.dt + self.input.external, false);
        for touch in &moved.touches {
            v = clip(v, touch.normal);
        }
        self.state.velocity = v;
        self.state.sprinting = fast;
        // Pushing against a ledge climbs it by itself up to the auto height above the surface
        // (when the params have one); a higher ledge needs the jump above.
        let auto = p.climb_out_auto_height.map(|height| surface + height);
        if let Some(wall) = moved.wall()
            && let Some(feet) = self.try_mantle(moved.feet, &wall, 0.0, true, auto, p.mantle_height)
        {
            return feet;
        }
        moved.feet
    }

    /// Water, updrafts and volume events at the new position; water changes the mode.
    fn surroundings(&mut self, mut feet: Vec3) -> Vec3 {
        let p = self.params;
        let around = self.mover.surroundings(feet);
        for volume in &self.state.volumes {
            if !around.volumes.contains(volume) {
                self.notes.push(Note::Exited(*volume));
            }
        }
        for volume in &around.volumes {
            if !self.state.volumes.contains(volume) {
                self.notes.push(Note::Entered(*volume));
            }
        }
        self.state.volumes = around.volumes;
        self.state.water_surface = around.water;
        self.state.updraft = around.updraft.map(|(accel, max)| Vec2::new(accel, max));

        let deep = |feet: Vec3, surface: f32| self.depth(feet, surface);
        match (self.state.mode, around.water) {
            (MotionMode::Ground, Some(surface)) if deep(feet, surface) > p.swim_depth => self.enter_water(),
            (MotionMode::Air | MotionMode::Glide, Some(surface))
                if feet.y <= surface - p.float_depth && deep(feet, surface) > p.swim_depth =>
            {
                self.enter_water()
            }
            (MotionMode::Water, None) => {
                self.state.fall_from_y = Some(feet.y);
                self.set_mode(MotionMode::Air);
            }
            (MotionMode::Water, Some(surface)) if deep(feet, surface) < p.swim_depth - SHALLOW_MARGIN => {
                // Shallow enough to stand: down onto the bottom, or fall to it.
                let reach = p.snap_distance + (p.swim_depth - p.float_depth);
                match self.mover.ground(feet, reach) {
                    Some(ground) if ground.walkable => {
                        feet -= Vec3::Y * ground.drop;
                        let s = &mut *self.state;
                        s.grounded = true;
                        s.ground_normal = ground.normal;
                        s.velocity.y = 0.0;
                        self.set_mode(MotionMode::Ground);
                    }
                    _ => {
                        self.state.fall_from_y = Some(feet.y);
                        self.set_mode(MotionMode::Air);
                    }
                }
            }
            _ => {}
        }
        feet
    }

    /// How deep the water with this surface is at the feet; anything a meter past
    /// `swim_depth` counts as that.
    fn depth(&self, feet: Vec3, surface: f32) -> f32 {
        self.mover.depth(feet, surface, self.params.swim_depth + 1.0)
    }

    fn enter_water(&mut self) {
        let s = &mut *self.state;
        s.grounded = false;
        s.ground_normal = Vec3::Y;
        s.fall_from_y = None;
        s.coyote_left = 0.0;
        s.sprinting = false;
        s.velocity.y = s.velocity.y.max(-self.params.swim_speed);
        self.set_mode(MotionMode::Water);
    }

    // --- mantle ---------------------------------------------------------------------

    fn mantle(&mut self, feet: Vec3) -> Vec3 {
        let Some(mut climb) = self.state.mantle else {
            self.set_mode(MotionMode::Ground);
            return self.stick(feet);
        };
        climb.t = (climb.t + self.dt / climb.duration.max(1e-3)).min(1.0);
        let pos = climb.position();
        if climb.t >= 1.0 {
            self.state.mantle = None;
            self.state.velocity = Vec3::ZERO;
            self.set_mode(MotionMode::Ground);
            return self.stick(pos);
        }
        self.state.velocity = (pos - feet) / self.dt;
        self.state.mantle = Some(climb);
        pos
    }

    /// Climbs onto the obstacle behind `wall` if its top is more than `min_height` and at
    /// most `reach` above the feet (the params' `mantle_height`, or a surface-relative limit
    /// turned into a height over the feet) and the capsule fits up there. `push`: only
    /// when the input points at the wall; `top_limit`: and only if the ledge top (world height)
    /// is no higher than that.
    fn try_mantle(
        &mut self,
        feet: Vec3,
        wall: &Touch,
        min_height: f32,
        push: bool,
        top_limit: Option<f32>,
        reach: f32,
    ) -> Option<Vec3> {
        if self.input.locked {
            return None;
        }
        let p = self.params;
        let dir = (-flat(wall.normal)).try_normalize()?;
        if push && self.wish().normalize_or_zero().dot(dir) < PUSH_COS {
            return None;
        }
        let m = self.mover;
        let top = m.obstacle_top(feet, wall.point, dir, reach)?;
        let rest = top + m.skin;
        let height = rest - feet.y;
        if height <= min_height || height > reach + MANTLE_SLACK || top_limit.is_some_and(|limit| top > limit) {
            return None;
        }
        // Prefer standing fully on top; a narrow top takes a shorter step.
        let rise = (height + 0.1).min(reach + 0.1);
        let (to, corner) = [2.0, 1.0, 0.5].into_iter().find_map(|reach| {
            let to = m.ledge(feet, dir, rise, m.radius * reach)?;
            if (to.y - rest).abs() > 0.05 {
                return None;
            }
            Some((to, m.climb_corner(feet, to)?))
        })?;
        let s = &mut *self.state;
        s.mantle = Some(Mantle { from: feet, corner, to, t: 0.0, duration: p.mantle_time });
        s.velocity = Vec3::ZERO;
        s.grounded = false;
        s.coyote_left = 0.0;
        s.fall_from_y = None;
        self.notes.push(Note::Mantled { height: to.y - feet.y });
        self.set_mode(MotionMode::Mantle);
        Some(feet)
    }

    /// Speed-up rate of the ground and air: the modifiers' override, else the params'.
    fn acceleration(&self) -> f32 {
        self.modifiers.acceleration.unwrap_or(self.params.acceleration)
    }

    /// Slow-down rate on the ground: the modifiers' override, else the params'.
    fn deceleration(&self) -> f32 {
        self.modifiers.deceleration.unwrap_or(self.params.deceleration)
    }

    /// A steep surface right in front (within a hand's reach), as a slide would touch it.
    fn wall_ahead(&self, feet: Vec3, dir: Vec3) -> Option<Touch> {
        let hit = self.mover.cast(feet, dir, self.mover.skin + CLIMB_OUT_REACH)?;
        (!self.mover.walkable(hit.normal)).then_some(Touch {
            plane: hit.normal,
            normal: hit.normal,
            point: hit.point,
            walkable: false,
        })
    }
}

impl Mantle {
    /// Where the climb is at `t`: straight up to `corner`, then over to `to`, at an even
    /// speed along the path.
    pub fn position(&self) -> Vec3 {
        let up = (self.corner.y - self.from.y).max(0.0);
        let over = (self.to - self.corner).length();
        let total = up + over;
        if total <= f32::EPSILON {
            return self.to;
        }
        let along = self.t.clamp(0.0, 1.0) * total;
        if along <= up {
            self.from + Vec3::Y * along
        } else {
            self.corner + (self.to - self.corner) * ((along - up) / over.max(f32::EPSILON))
        }
    }
}

/// How fast a swimmer changes its speed: toward an input (`has_input`) or to rest.
fn swim_rate(p: &CharacterParams, has_input: bool) -> f32 {
    let speed_up = if p.swim_acceleration > 0.0 { p.swim_acceleration } else { p.acceleration };
    if has_input || p.swim_deceleration <= 0.0 { speed_up } else { p.swim_deceleration }
}

/// `approach`, or (`per_axis`) each horizontal axis toward its own target by at most `max_step`
/// (`y` stays as it is).
fn step_toward(v: Vec3, target: Vec3, max_step: f32, per_axis: bool) -> Vec3 {
    if per_axis {
        Vec3::new(move_toward(v.x, target.x, max_step), v.y, move_toward(v.z, target.z, max_step))
    } else {
        approach(v, target, max_step)
    }
}

fn move_toward(from: f32, to: f32, max_step: f32) -> f32 {
    if (to - from).abs() <= max_step { to } else { from + (to - from).signum() * max_step }
}

/// Moves `v` toward `target` by at most `max_step`.
fn approach(v: Vec3, target: Vec3, max_step: f32) -> Vec3 {
    let diff = target - v;
    let len = diff.length();
    if len <= max_step || len <= f32::EPSILON { target } else { v + diff * (max_step / len) }
}

/// A horizontal displacement along the ground plane: same horizontal distance, up or
/// down the slope.
fn along_ground(d: Vec3, normal: Vec3) -> Vec3 {
    if normal.y < 1e-3 {
        return d;
    }
    Vec3::new(d.x, -(normal.x * d.x + normal.z * d.z) / normal.y, d.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approach_stops_at_target() {
        assert_eq!(approach(Vec3::ZERO, Vec3::X * 5.0, 1.0), Vec3::X);
        assert_eq!(approach(Vec3::X * 4.5, Vec3::X * 5.0, 1.0), Vec3::X * 5.0);
    }

    #[test]
    fn slope_projection_keeps_horizontal_distance() {
        let normal = Vec3::new(-1.0, 1.0, 0.0).normalize(); // 45° rising toward +x
        let d = along_ground(Vec3::X, normal);
        assert!((d.y - 1.0).abs() < 1e-5 && d.x == 1.0, "{d}");
    }

    #[test]
    fn mantle_path_goes_up_then_over() {
        let m = Mantle {
            from: Vec3::ZERO,
            corner: Vec3::Y * 1.0,
            to: Vec3::new(1.0, 1.0, 0.0),
            t: 0.25,
            duration: 1.0,
        };
        assert!((m.position() - Vec3::Y * 0.5).length() < 1e-6);
        assert!((Mantle { t: 0.75, ..m }.position() - Vec3::new(0.5, 1.0, 0.0)).length() < 1e-6);
        assert_eq!(Mantle { t: 1.0, ..m }.position(), m.to);
    }
}
