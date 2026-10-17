//! The simulation's float math must give the same bits on every OS (docs/manual.md, "Time and determinism").
//!
//! `bevy::math::ops` takes `sin`, `exp`, `powf` and the rest from the pure-Rust `libm` crate when Bevy's `libm` feature
//! is on (`[workspace.dependencies]` of the root `Cargo.toml`); without it, it calls std, that is the platform's C
//! math library, and MSVC, glibc and macOS differ in the last bit of many results. `libm` has no platform code, so
//! the numbers below are the same on Windows, Linux and macOS. The inputs were picked where the platform's math
//! gave another bit than `libm` on the machine these were written on (a Windows one, MSVC's UCRT), so a feature change
//! that brings the platform's math back fails here, not in a replay on somebody else's machine.
//!
//! A change of the pinned `libm` version can move a bit too: that is a change of the game's numbers like any other
//! (the reference replays are recorded again), and these pins are recorded again with them.

use bevy::math::ops;
use bevy::prelude::*;

fn bits(x: u32) -> f32 {
    f32::from_bits(x)
}

#[test]
fn ops_results_are_the_same_bits_on_every_os() {
    // (what, got, the bits libm gives)
    let table: [(&str, f32, u32); 22] = [
        ("sin(50.514313)", ops::sin(bits(1112149672)), 1048325714),
        ("cos(-0.18344116)", ops::cos(bits(3191592960)), 1065071725),
        ("tan(-0.51375353)", ops::tan(bits(3204679002)), 3205526981),
        ("asin(-0.92895794)", ops::asin(bits(3211644976)), 3214444019),
        ("acos(0.93166983)", ops::acos(bits(1064206826)), 1052663400),
        ("atan(-2.6333504)", ops::atan(bits(3223881936)), 3214580626),
        ("exp(13.595493)", ops::exp(bits(1096386340)), 1229188240),
        ("exp2(-7.24772)", ops::exp2(bits(3236425042)), 1003985961),
        ("ln(45.661602)", ops::ln(bits(1110877563)), 1081380732),
        ("log2(43.534313)", ops::log2(bits(1110319907)), 1085158890),
        ("log10(736.7048)", ops::log10(bits(1144532251)), 1077379516),
        ("cbrt(-874.76447)", ops::cbrt(bits(3294277869)), 3239642449),
        ("tanh(0.55717945)", ops::tanh(bits(1057923920)), 1057063286),
        ("sinh(-0.016908169)", ops::sinh(bits(3163194112)), 3163194544),
        ("cosh(3.512126)", ops::cosh(bits(1080084140)), 1099313785),
        ("exp_m1(0.79495907)", ops::exp_m1(bits(1061913200)), 1067151317),
        ("ln_1p(73.19743)", ops::ln_1p(bits(1116890390)), 1082773690),
        ("atan2", ops::atan2(bits(3238492569), bits(1090200014)), 3209872991),
        ("hypot", ops::hypot(bits(3234533324), bits(1083832863)), 1090442962),
        ("powf", ops::powf(bits(1091226476), bits(1062109228)), 1085722052),
        // And the ones every math library agrees on, so that the table also fails if `ops` is wrong altogether.
        ("sin(1)", ops::sin(1.0), 0x3f576aa4),
        ("atan(1) = pi/4", ops::atan(1.0), 0x3f490fdb),
    ];
    let wrong: Vec<String> = table
        .iter()
        .filter(|(_, got, want)| got.to_bits() != *want)
        .map(|(what, got, want)| format!("{what}: got {:#010x}, libm gives {want:#010x}", got.to_bits()))
        .collect();
    assert!(
        wrong.is_empty(),
        "`bevy::math::ops` does not give libm's bits, so a platform math library is back (is Bevy's `libm` feature still on?):\n{}",
        wrong.join("\n")
    );
}

#[test]
fn sin_cos_is_sin_and_cos() {
    let x = bits(1112149672);
    let (s, c) = ops::sin_cos(x);
    assert_eq!((s.to_bits(), c.to_bits()), (ops::sin(x).to_bits(), ops::cos(x).to_bits()));
}

#[test]
fn glam_takes_its_math_from_the_same_libm() {
    // `Quat::from_rotation_y`, `Vec2::to_angle` and the rest call glam's own math module: with glam's `libm` feature
    // (it comes with Bevy's) that is `libm`, the same functions as `ops`, otherwise the platform's. The angles are
    // inputs on which the two differ on a machine with the platform's math.
    let half = bits(1112149672); // 50.514313 rad, sin differs
    let q = Quat::from_rotation_y(half * 2.0);
    assert_eq!(q.y.to_bits(), ops::sin(half).to_bits(), "Quat::from_rotation_y: sin");
    assert_eq!(q.w.to_bits(), ops::cos(half).to_bits(), "Quat::from_rotation_y: cos");

    let (y, x) = (bits(3238492569), bits(1090200014));
    assert_eq!(Vec2::new(x, y).to_angle().to_bits(), ops::atan2(y, x).to_bits(), "Vec2::to_angle: atan2");
}
