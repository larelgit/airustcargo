//! The table of the state machine, row by row, with hand-made observations.

use std::collections::BTreeMap;

use super::*;
use crate::def::{MemberGroup, PullBack, SpawnAt, StartDef, WaveDef};

const DT: f32 = 1.0 / 60.0;

fn group() -> MemberGroup {
    MemberGroup {
        prefab: "grunt".into(),
        count: 1,
        up_to: None,
        at: SpawnAt::Offsets(vec![Vec3::ZERO]),
        face: Default::default(),
        home: Default::default(),
        leash: None,
        avoid_behind: None,
        overrides: BTreeMap::new(),
    }
}

fn wave(delay: f32, placed: bool) -> WaveDef {
    WaveDef { delay, placed, warn: None, members: vec![group()] }
}

fn def(waves: Vec<WaveDef>) -> EncounterDef {
    EncounterDef {
        radius: 20.0,
        start: StartDef::Manual,
        waves,
        tags: vec![],
        budget: None,
        allowed: vec![],
        forbidden: vec![],
        rules: None,
        resume_after: 2.0,
        reset_after: Some(30.0),
        on_reset: OnReset::Restart,
        pull_back: PullBack { heal: true, state: None, deadline: 6.0 },
        lock: None,
        once: false,
        net_always: false,
        blocks_join: false,
    }
}

fn id(n: u32) -> AiId {
    AiId(n)
}

fn run_with(members: &[(u32, u32, bool)]) -> Run {
    Run {
        key: "world/test".into(),
        encounter: "test".into(),
        members: members.iter().map(|(i, w, c)| Member { id: id(*i), wave: *w, counted: *c }).collect(),
        ..default()
    }
}

/// What a tick shows: two players, `standing` of them not out, near; members out as listed.
struct World3 {
    standing: u32,
    near: bool,
    start_met: bool,
    out: Vec<u32>,
    disturbed: Vec<u32>,
    all_home: bool,
}

impl Default for World3 {
    fn default() -> Self {
        Self { standing: 2, near: true, start_met: false, out: vec![], disturbed: vec![], all_home: true }
    }
}

impl World3 {
    fn obs(&self, run: &Run) -> Obs {
        Obs {
            dt: DT,
            participants: 2,
            standing: self.standing,
            near: self.near,
            start_met: self.start_met,
            members: run
                .members
                .iter()
                .map(|m| MemberObs { out: self.out.contains(&m.id.0), disturbed: self.disturbed.contains(&m.id.0) })
                .collect(),
            all_home: self.all_home,
        }
    }
}

fn tick(run: &mut Run, def: &EncounterDef, world: &World3) -> Vec<Fx> {
    let mut fx = Vec::new();
    advance(run, def, &world.obs(run), &mut fx);
    fx
}

fn spawned(fx: &[Fx]) -> Option<u32> {
    fx.iter().find_map(|f| match f {
        Fx::Spawn { wave } => Some(*wave),
        _ => None,
    })
}

fn has(fx: &[Fx], test: impl Fn(&Fx) -> bool) -> bool {
    fx.iter().any(test)
}

/// Runs ticks with no change in the world until `want` holds of the effects; the number of ticks
/// run (1 is the first tick after the one that set the timer).
fn ticks_until(run: &mut Run, def: &EncounterDef, world: &World3, want: impl Fn(&Fx) -> bool, limit: u32) -> u32 {
    for n in 1..=limit {
        let fx = tick(run, def, world);
        if has(&fx, &want) {
            return n;
        }
    }
    panic!("not reached in {limit} ticks; state {:?}", run.state);
}

#[test]
fn ticks_follow_the_timer_rule() {
    assert_eq!(ticks(1.2, DT), 72);
    assert_eq!(ticks(2.5, DT), 150);
    assert_eq!(ticks(2.0, DT), 120);
    assert_eq!(ticks(30.0, DT), 1800);
    assert_eq!(ticks(6.0, DT), 360);
    assert_eq!(ticks(0.0, DT), 0);
    assert_eq!(ticks(-1.0, DT), 0);
    assert_eq!(ticks(f32::NAN, DT), 0);
    // A hair above a whole number of ticks is the whole number (the engine's epsilon).
    assert_eq!(ticks(1.2 + 1e-5, DT), 72);
    assert_eq!(ticks(1.2 + 1e-3, DT), 73);
}

#[test]
fn the_first_wave_spawns_72_ticks_after_the_start_and_the_next_150_after_a_clear() {
    let def = def(vec![wave(1.2, false), wave(2.5, false)]);
    let mut run = run_with(&[]);
    run.state = RunState::Armed;
    // Tick N: the start.
    let fx = tick(&mut run, &def, &World3 { start_met: true, ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Started(_))));
    let warned = fx.iter().find_map(|f| match f {
        Fx::Warned(w) => Some(w.clone()),
        _ => None,
    });
    assert_eq!((warned.as_ref().map(|w| w.wave), warned.map(|w| w.delay)), (Some(1), Some(1.2)));
    assert!(matches!(run.state, RunState::Between { .. }));
    // Ticks N+1 ..: the spawn is at N+72.
    let n = ticks_until(&mut run, &def, &World3::default(), |f| matches!(f, Fx::Spawn { wave: 1 }), 200);
    assert_eq!(n, 72, "a timer set in tick N expires at the check of tick N + 72");
    assert_eq!(run.state, RunState::Running);
    assert_eq!(run.wave, 1);

    // The wave's only member (id 5, as the step would list it) dies in tick M: cleared in M, next
    // wave 150 ticks later.
    run.members.push(Member { id: id(5), wave: 1, counted: true });
    let fx = tick(&mut run, &def, &World3 { out: vec![5], ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Cleared(c) if c.wave == 1)));
    assert!(has(&fx, |f| matches!(f, Fx::Warned(w) if w.wave == 2 && w.delay == 2.5)));
    let n = ticks_until(&mut run, &def, &World3::default(), |f| matches!(f, Fx::Spawn { wave: 2 }), 400);
    assert_eq!(n, 150);
}

#[test]
fn the_resume_spawns_120_ticks_after_someone_stands() {
    let def = def(vec![wave(1.2, false), wave(2.5, false)]);
    let mut run = run_with(&[(5, 1, true)]);
    run.wave = 1;
    run.state = RunState::Running;
    // Everyone falls: wipe in wave 1 (spawned): the wave goes back to 0.
    let fx = tick(&mut run, &def, &World3 { standing: 0, ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Wiped(w) if w.rewound && w.wave == 0)));
    assert!(has(&fx, |f| matches!(f, Fx::Despawn(ids) if ids == &vec![id(5)])));
    assert_eq!(run.state, RunState::Wiped { resume: None, pulled_back: false });
    assert!(run.members.is_empty());
    // Nobody stands for a while: nothing happens.
    for _ in 0..10 {
        assert!(tick(&mut run, &def, &World3 { standing: 0, ..default() }).is_empty());
    }
    // Tick R: someone stands. The pending wave (1) is warned at once.
    let fx = tick(&mut run, &def, &World3::default());
    assert!(has(&fx, |f| matches!(f, Fx::Resumed(r) if r.wave == 1 && r.delay == 2.0)));
    assert!(has(&fx, |f| matches!(f, Fx::Warned(w) if w.wave == 1)));
    let n = ticks_until(&mut run, &def, &World3::default(), |f| matches!(f, Fx::Spawn { wave: 1 }), 400);
    assert_eq!(n, 120, "resume_after 2 s");
    assert_eq!((run.wave, &run.state), (1, &RunState::Running));
}

#[test]
fn a_placed_pack_killed_before_the_start_is_cleared_and_won_instead_of_panicking() {
    // Finding 1: one placed wave, every member out (or none ever alive) before the start.
    let def = def(vec![wave(0.0, true)]);
    let mut run = run_with(&[(7, 1, true), (8, 1, true)]);
    run.wave = 1;
    run.state = RunState::Armed;
    let fx = tick(&mut run, &def, &World3 { start_met: true, out: vec![7, 8], ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Started(_))));
    assert_eq!(run.state, RunState::Running, "the clear path runs; waves[1] does not exist");
    let fx = tick(&mut run, &def, &World3 { out: vec![7, 8], ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Cleared(c) if c.wave == 1)));
    assert!(has(&fx, |f| matches!(f, Fx::Completed(_))));
    assert_eq!(run.state, RunState::Won);
    assert!(run.members.is_empty(), "the dead are pruned");

    // A load that spawned nothing: the run has no entries at all.
    let mut run = run_with(&[]);
    run.wave = 1;
    run.state = RunState::Armed;
    tick(&mut run, &def, &World3 { start_met: true, ..default() });
    assert_eq!(run.state, RunState::Running);
    let fx = tick(&mut run, &def, &World3::default());
    assert!(has(&fx, |f| matches!(f, Fx::Completed(_))));
    assert_eq!(run.state, RunState::Won);
}

#[test]
fn a_placed_pack_killed_before_the_start_goes_on_to_the_next_wave_when_there_is_one() {
    let def = def(vec![wave(0.0, true), wave(1.0, false)]);
    let mut run = run_with(&[(7, 1, true)]);
    run.wave = 1;
    run.state = RunState::Armed;
    tick(&mut run, &def, &World3 { start_met: true, out: vec![7], ..default() });
    assert_eq!(run.state, RunState::Running);
    let fx = tick(&mut run, &def, &World3 { out: vec![7], ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Cleared(_))));
    assert!(matches!(run.state, RunState::Between { .. }));
    let n = ticks_until(&mut run, &def, &World3::default(), |f| matches!(f, Fx::Spawn { wave: 2 }), 100);
    assert_eq!(n, 60);
}

#[test]
fn a_wipe_in_a_spawned_wave_after_a_placed_wave_keeps_the_pending_wave() {
    // Finding 2: a placed first wave and spawned waves after it. A wipe in spawned wave 2 sets
    // wave = 1, which is also what a placed wave with survivors has: the flag, not the number,
    // says what the resume does.
    let def = def(vec![wave(0.0, true), wave(1.0, false)]);
    let mut run = run_with(&[(9, 2, true)]);
    run.wave = 2;
    run.state = RunState::Running;
    tick(&mut run, &def, &World3 { standing: 0, ..default() });
    assert_eq!(run.wave, 1);
    assert_eq!(run.state, RunState::Wiped { resume: None, pulled_back: false });
    tick(&mut run, &def, &World3::default());
    let n = ticks_until(&mut run, &def, &World3::default(), |f| matches!(f, Fx::Spawn { wave: 2 }), 400);
    assert_eq!(n, 120, "the pending wave 2 comes back resume_after after someone returns");
    assert_eq!((run.wave, &run.state), (2, &RunState::Running));
}

#[test]
fn a_wipe_of_the_placed_wave_pulls_it_back_and_the_resume_goes_idle() {
    let def = def(vec![wave(0.0, true), wave(1.0, false)]);
    let mut run = run_with(&[(3, 1, true), (4, 1, true)]);
    run.wave = 1;
    run.state = RunState::Running;
    let fx = tick(&mut run, &def, &World3 { standing: 0, out: vec![4], ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::PullBack(ids) if ids == &vec![id(3)])), "only the survivor is pulled back");
    assert!(has(&fx, |f| matches!(f, Fx::Wiped(w) if !w.rewound && !w.between && w.wave == 1)));
    assert_eq!(run.state, RunState::Wiped { resume: None, pulled_back: true });
    assert_eq!(run.members.len(), 2, "the placed wave's entries stay");
    tick(&mut run, &def, &World3::default());
    let mut idle = false;
    for _ in 0..130 {
        let fx = tick(&mut run, &def, &World3::default());
        assert!(spawned(&fx).is_none(), "nothing spawns: the fight starts again by its start condition");
        if run.state == RunState::Idle {
            idle = true;
            break;
        }
    }
    assert!(idle);
}

#[test]
fn a_wipe_between_waves_rewinds_nothing_and_the_pending_wave_comes_after_the_resume() {
    let def = def(vec![wave(1.0, false), wave(1.0, false), wave(1.0, false)]);
    let mut run = run_with(&[]);
    run.wave = 1;
    run.state = RunState::Between { left: 30 };
    let fx = tick(&mut run, &def, &World3 { standing: 0, ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Wiped(w) if w.between && !w.rewound && w.wave == 1)));
    assert_eq!(run.wave, 1);
    tick(&mut run, &def, &World3::default());
    let n = ticks_until(&mut run, &def, &World3::default(), |f| matches!(f, Fx::Spawn { wave: 2 }), 400);
    assert_eq!(n, 120);
}

#[test]
fn clearing_beats_wiping_on_one_tick_and_the_wipe_comes_the_next_tick() {
    let def = def(vec![wave(1.0, false), wave(1.0, false)]);
    let mut run = run_with(&[(5, 1, true)]);
    run.wave = 1;
    run.state = RunState::Running;
    // The last counted member and the last participant fall on the same tick.
    let fx = tick(&mut run, &def, &World3 { standing: 0, out: vec![5], ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Cleared(_))));
    assert!(!has(&fx, |f| matches!(f, Fx::Wiped(_))), "one transition per tick");
    assert!(matches!(run.state, RunState::Between { .. }));
    // Next tick: the wipe is checked against Between and rewinds nothing.
    let fx = tick(&mut run, &def, &World3 { standing: 0, ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Wiped(w) if w.between && !w.rewound)));
    assert_eq!(run.wave, 1);

    // The last wave: a won fight is won; the wipe never happens.
    let one = self::def(vec![wave(1.0, false)]);
    let mut run = run_with(&[(5, 1, true)]);
    run.wave = 1;
    run.state = RunState::Running;
    let fx = tick(&mut run, &one, &World3 { standing: 0, out: vec![5], ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Completed(_))));
    assert_eq!(run.state, RunState::Won);
    assert!(tick(&mut run, &one, &World3 { standing: 0, ..default() }).is_empty());
}

#[test]
fn a_wipe_drops_the_dead_entries_of_the_rewound_wave_and_the_adds() {
    let def = def(vec![wave(1.0, false), wave(1.0, false)]);
    // Wave 2: 20 and 21 counted (21 is a corpse in the world), 30 an add.
    let mut run = run_with(&[(20, 2, true), (21, 2, true), (30, 0, false)]);
    run.wave = 2;
    run.state = RunState::Running;
    let fx = tick(&mut run, &def, &World3 { standing: 0, out: vec![21], ..default() });
    assert!(run.members.is_empty(), "{:?}", run.members);
    assert!(has(&fx, |f| matches!(f, Fx::Despawn(ids) if ids == &vec![id(30)])), "the adds are despawned");
    assert!(has(&fx, |f| matches!(f, Fx::Despawn(ids) if ids == &vec![id(20)])), "the living member is despawned");
    assert!(has(&fx, |f| matches!(f, Fx::Retire(ids) if ids == &vec![id(21)])), "the corpse loses its marker");
    assert_eq!(run.wave, 1);
}

#[test]
fn a_clear_prunes_the_out_entries_and_a_win_releases_the_adds() {
    let def = def(vec![wave(1.0, false)]);
    let mut run = run_with(&[(1, 1, true), (2, 1, true), (3, 0, false)]);
    run.wave = 1;
    run.state = RunState::Running;
    let fx = tick(&mut run, &def, &World3 { out: vec![1, 2], ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Retire(ids) if ids == &vec![id(1), id(2)])));
    assert!(has(&fx, |f| matches!(f, Fx::Release(ids) if ids == &vec![id(3)])));
    assert!(run.members.is_empty());
    assert_eq!(run.state, RunState::Won);
}

#[test]
fn an_add_never_holds_a_wave_open() {
    let def = def(vec![wave(1.0, false), wave(1.0, false)]);
    let mut run = run_with(&[(1, 1, true), (3, 0, false)]);
    run.wave = 1;
    run.state = RunState::Running;
    let fx = tick(&mut run, &def, &World3 { out: vec![1], ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Cleared(_))));
    assert_eq!(run.members.len(), 1, "the living add stays listed through the clear");
}

#[test]
fn the_absence_reset_is_tick_exact_in_every_fighting_state() {
    for state in [RunState::Running, RunState::Between { left: 100_000 }, RunState::Wiped { resume: None, pulled_back: false }] {
        let def = def(vec![wave(1.0, false)]);
        let mut run = run_with(&[(1, 1, true)]);
        run.wave = 1;
        run.state = state.clone();
        // A wiped run has nobody standing; the others have people, far away.
        let standing = if matches!(state, RunState::Wiped { .. }) { 0 } else { 2 };
        let world = World3 { near: false, standing, ..default() };
        let n = ticks_until(&mut run, &def, &world, |f| matches!(f, Fx::Reset(r) if r.why == ResetWhy::Absent), 2000);
        assert_eq!(n, 1800, "{state:?}: the 1800th tick with nobody near");
        // `Resetting` was set in this tick and counted down in it.
        assert_eq!(run.state, RunState::Resetting { left: 359 }, "{state:?}");
    }
}

#[test]
fn someone_coming_back_near_restarts_the_absence_clock() {
    let def = def(vec![wave(1.0, false)]);
    let mut run = run_with(&[(1, 1, true)]);
    run.wave = 1;
    run.state = RunState::Running;
    for _ in 0..1000 {
        tick(&mut run, &def, &World3 { near: false, ..default() });
    }
    assert_eq!(run.absent, 1000);
    tick(&mut run, &def, &World3::default());
    assert_eq!(run.absent, 0);
    let n = ticks_until(&mut run, &def, &World3 { near: false, ..default() }, |f| matches!(f, Fx::Reset(_)), 2000);
    assert_eq!(n, 1800);
}

#[test]
fn a_resume_in_progress_resets_for_absence_like_the_other_fighting_states() {
    // Finding 9: Wiped { Some } had no absence row.
    let def = def(vec![wave(1.0, false)]);
    let mut run = run_with(&[]);
    run.state = RunState::Wiped { resume: Some(100_000), pulled_back: false };
    let n = ticks_until(&mut run, &def, &World3 { near: false, ..default() }, |f| matches!(f, Fx::Reset(r) if r.why == ResetWhy::Absent), 2000);
    assert_eq!(n, 1800);
}

#[test]
fn a_reset_restarts_or_keeps_the_cleared_waves() {
    let waves = vec![wave(0.0, true), wave(1.0, false), wave(1.0, false)];
    for (on_reset, want) in [(OnReset::Restart, 1), (OnReset::KeepCleared, 2)] {
        let mut def = def(waves.clone());
        def.on_reset = on_reset;
        // Running wave 3 (spawned): the placed wave and wave 2 are cleared.
        let mut run = run_with(&[(40, 3, true), (99, 0, false)]);
        run.wave = 3;
        run.state = RunState::Running;
        let obs = World3::default().obs(&run);
        reset(&mut run, &def, &obs, ResetWhy::Dev, &mut Vec::new());
        assert_eq!(run.wave, want, "{on_reset:?}");
        assert!(matches!(run.state, RunState::Resetting { left: 360 }));
        assert!(run.members.is_empty());
    }
}

#[test]
fn a_reset_pulls_the_placed_survivors_back_and_despawns_the_rest() {
    let mut def = def(vec![wave(0.0, true), wave(1.0, false)]);
    def.on_reset = OnReset::Restart;
    let mut run = run_with(&[(3, 1, true), (4, 1, true), (50, 2, true), (51, 0, false)]);
    run.wave = 2;
    run.state = RunState::Running;
    let mut fx = Vec::new();
    let world = World3 { out: vec![4], ..default() };
    let obs = world.obs(&run);
    reset(&mut run, &def, &obs, ResetWhy::Absent, &mut fx);
    assert!(has(&fx, |f| matches!(f, Fx::PullBack(ids) if ids == &vec![id(3)])));
    assert!(has(&fx, |f| matches!(f, Fx::Despawn(ids) if ids == &vec![id(50), id(51)])));
    assert_eq!(run.wave, 1);
    assert_eq!(run.members.iter().map(|m| m.id.0).collect::<Vec<_>>(), vec![3, 4]);
}

#[test]
fn resetting_ends_when_everyone_is_home_or_the_deadline_runs_out() {
    let def = def(vec![wave(1.0, false)]);
    let mut run = run_with(&[]);
    run.state = RunState::Resetting { left: 5 };
    let fx = tick(&mut run, &def, &World3 { all_home: true, ..default() });
    assert!(fx.is_empty());
    assert_eq!(run.state, RunState::Idle);

    let mut run = run_with(&[]);
    run.state = RunState::Resetting { left: 360 - 1 };
    let walking = World3 { all_home: false, ..default() };
    let n = ticks_until(&mut run, &def, &walking, |f| matches!(f, Fx::PlaceHome), 400);
    assert_eq!(n, 360, "the stragglers are placed home 6 s after the reset tick");
    assert_eq!(run.state, RunState::Idle);
}

#[test]
fn idle_waits_for_the_start_condition_to_be_false_once() {
    let def = def(vec![wave(1.0, false)]);
    let mut run = run_with(&[]);
    for _ in 0..5 {
        tick(&mut run, &def, &World3 { start_met: true, ..default() });
        assert_eq!(run.state, RunState::Idle, "a run made under a player's feet does not start");
    }
    tick(&mut run, &def, &World3 { start_met: false, ..default() });
    assert_eq!(run.state, RunState::Armed);
    let fx = tick(&mut run, &def, &World3 { start_met: true, ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Started(_))));
}

#[test]
fn an_armed_placed_pack_is_pulled_into_the_fight_when_a_member_is_out_hurt_or_hunting() {
    // Finding 8: a pack can be fought and kited before its start condition holds.
    let def = def(vec![wave(0.0, true)]);
    for disturbed in [vec![1], vec![2]] {
        let mut run = run_with(&[(1, 1, true), (2, 1, true)]);
        run.wave = 1;
        run.state = RunState::Armed;
        assert!(tick(&mut run, &def, &World3::default()).is_empty());
        assert_eq!(run.state, RunState::Armed);
        let fx = tick(&mut run, &def, &World3 { disturbed, ..default() });
        assert!(has(&fx, |f| matches!(f, Fx::Started(_))));
        assert_eq!(run.state, RunState::Running);
    }
    // Not for a spawned wave: nothing stands before the start.
    let def2 = self::def(vec![wave(1.0, false)]);
    let mut run = run_with(&[(1, 1, true)]);
    run.state = RunState::Armed;
    assert!(tick(&mut run, &def2, &World3 { disturbed: vec![1], ..default() }).is_empty());
}

#[test]
fn a_won_encounter_that_is_once_is_remembered() {
    let mut def = def(vec![wave(1.0, false)]);
    def.once = true;
    let mut run = run_with(&[(1, 1, true)]);
    run.wave = 1;
    run.state = RunState::Running;
    let fx = tick(&mut run, &def, &World3 { out: vec![1], ..default() });
    assert!(has(&fx, |f| matches!(f, Fx::Remember)));
}

#[test]
fn the_machine_never_indexes_past_the_waves() {
    // Every state at every wave number, with every observation shape: no panic.
    let def = def(vec![wave(1.0, true), wave(1.0, false)]);
    let states = [
        RunState::Idle,
        RunState::Armed,
        RunState::Running,
        RunState::Between { left: 0 },
        RunState::Wiped { resume: None, pulled_back: false },
        RunState::Wiped { resume: Some(0), pulled_back: true },
        RunState::Wiped { resume: Some(0), pulled_back: false },
        RunState::Resetting { left: 0 },
        RunState::Won,
    ];
    for state in states {
        for wave in 0..=3 {
            for standing in [0, 2] {
                for start_met in [false, true] {
                    for members in [vec![], vec![(1, 1, true)], vec![(1, 2, true), (2, 0, false)]] {
                        let mut run = run_with(&members);
                        run.state = state.clone();
                        run.wave = wave;
                        tick(&mut run, &def, &World3 { standing, start_met, ..default() });
                        tick(&mut run, &def, &World3 { standing, start_met, out: vec![1, 2], ..default() });
                    }
                }
            }
        }
    }
}
