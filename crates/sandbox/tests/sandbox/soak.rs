//! `shards soak` in process: the seeded agents of `shards::script::soak` play a host and two
//! clients on links of 150 ms one way with 2 % loss (E7's conditions), and the host changes the
//! region and saves on the way. No member may count a desync. The full half hour, E7's
//! acceptance, is `#[ignore]`d here: CI plays it against a live stand (`shards soak --minutes 30`).

use shards::script::soak::{self, SoakPlan, SoakReport};
use shards::testing::StandDriver;

use super::net::*;

/// Ticks of a minute at the sandbox's 60 Hz.
const MINUTE: u64 = 3600;

fn soak_on_a_lossy_stand(plan: &SoakPlan) -> SoakReport {
    let mut stand = stand_with(2, lossy());
    soak::run(&mut StandDriver { stand: &mut stand }, plan, &mut |_| {}).unwrap()
}

#[test]
fn two_minutes_of_three_players_on_lossy_links_stay_in_sync() {
    let plan = SoakPlan {
        seed: 7,
        ticks: 2 * MINUTE,
        regions: vec!["wilds".into()],
        regen_every: MINUTE,
        save_every: 45 * 60,
        report_every: MINUTE,
        ..SoakPlan::default()
    };
    let report = soak_on_a_lossy_stand(&plan);
    assert!(report.passed(), "{report}");
    // It played the whole plan: the region changed once (every client was sent the new world) and
    // everything was saved twice (every client was sent its character).
    assert_eq!((report.ticks(), report.regens, report.saves), (2 * MINUTE, 1, 2), "{report}");
    let members: Vec<&str> = report.members.iter().map(|m| m.member.as_str()).collect();
    assert_eq!(members, ["host", "c1", "c2"]);
    for member in &report.members {
        // Most inputs are taken (a refusal is play too: no stamina, busy), and each member's session
        // answered at the end.
        assert!(member.inputs > 100 && member.refused < member.inputs / 2, "{report}");
        assert!(!member.net.starts_with("net_status:"), "{report}");
    }
}

#[test]
#[ignore = "half an hour of game time (E7's acceptance), minutes of wall time: cargo test -p sandbox --test sandbox -- --ignored half_an_hour"]
fn half_an_hour_of_three_players_on_lossy_links_stays_in_sync() {
    let plan = SoakPlan {
        seed: 30,
        ticks: 30 * MINUTE,
        regions: vec!["wilds".into()],
        regen_every: 5 * MINUTE,
        save_every: 3 * MINUTE,
        report_every: 5 * MINUTE,
        ..SoakPlan::default()
    };
    let report = soak_on_a_lossy_stand(&plan);
    assert!(report.passed(), "{report}");
    assert_eq!(report.ticks(), 30 * MINUTE, "{report}");
}
