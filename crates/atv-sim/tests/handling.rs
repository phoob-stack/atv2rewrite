//! Handling checks on a flat plane. They need the game's own tuning files,
//! so they only run when `ATV2_PARAMS` points at an extracted `Cars` folder
//! (e.g. from `atv-tool extract`); otherwise they pass without checking.

use atv_sim::{Input, TrackCollision, Vehicle, VehicleParams};
use glam::{Quat, Vec3};

fn params() -> Option<VehicleParams> {
    let dir = std::path::PathBuf::from(std::env::var_os("ATV2_PARAMS")?);
    let generic = std::fs::read_to_string(dir.join("Generic/Generic.params")).ok()?;
    let quad = std::fs::read_to_string(dir.join("Bandit/Bandit.params")).ok();
    Some(VehicleParams::from_files(&generic, quad.as_deref()))
}

fn flat() -> TrackCollision {
    let s = 2000.0;
    TrackCollision::new(
        vec![
            [
                Vec3::new(-s, 0.0, -s),
                Vec3::new(s, 0.0, -s),
                Vec3::new(s, 0.0, s),
            ],
            [
                Vec3::new(-s, 0.0, -s),
                Vec3::new(s, 0.0, s),
                Vec3::new(-s, 0.0, s),
            ],
        ],
        64.0,
    )
}

fn run(v: &mut Vehicle, world: &TrackCollision, input: Input, secs: f32) {
    for _ in 0..(secs * 60.0) as usize {
        v.step(&input, 1.0 / 60.0, world);
    }
}

fn settled() -> Option<(Vehicle, TrackCollision)> {
    let world = flat();
    let mut v = Vehicle::new(params()?, Vec3::new(0.0, 0.6, 0.0), Quat::IDENTITY);
    run(&mut v, &world, Input::default(), 2.0);
    Some((v, world))
}

#[test]
fn settles_level_on_all_four_wheels() {
    let Some((v, _)) = settled() else { return };
    println!("rest height {:.3} up {:?}", v.position.y, v.up());
    assert!(v.wheels.iter().all(|w| w.contact));
    assert!(v.up().y > 0.99);
    assert!(v.velocity.length() < 0.05, "still moving: {:?}", v.velocity);
}

#[test]
fn accelerates_and_tops_out() {
    let Some((mut v, world)) = settled() else {
        return;
    };
    let full = Input {
        throttle: 1.0,
        ..Default::default()
    };
    let mut t60 = None;
    for i in 0..(30 * 60) {
        v.step(&full, 1.0 / 60.0, &world);
        if t60.is_none() && v.speed_mph() >= 60.0 {
            t60 = Some(i as f32 / 60.0);
        }
    }
    println!(
        "0-60 {t60:?}s, top {:.1} mph in gear {}",
        v.speed_mph(),
        v.gear
    );
    assert!(t60.is_some());
    assert!(
        v.forward().x.abs() < 0.05,
        "pulled off line: {:?}",
        v.forward()
    );
}

#[test]
fn steers_the_right_way() {
    let Some((mut v, world)) = settled() else {
        return;
    };
    run(
        &mut v,
        &world,
        Input {
            throttle: 1.0,
            ..Default::default()
        },
        3.0,
    );
    run(
        &mut v,
        &world,
        Input {
            throttle: 1.0,
            steer: 1.0,
            ..Default::default()
        },
        1.0,
    );
    println!(
        "after right turn: fwd {:?} mph {:.1}",
        v.forward(),
        v.speed_mph()
    );
    // Sim frame: forward starts at -Z; turning right swings it toward +X.
    assert!(v.forward().x > 0.3);
    assert!(v.up().y > 0.9);
}

#[test]
fn brakes_then_reverses() {
    let Some((mut v, world)) = settled() else {
        return;
    };
    run(
        &mut v,
        &world,
        Input {
            throttle: 1.0,
            ..Default::default()
        },
        3.0,
    );
    run(
        &mut v,
        &world,
        Input {
            brake: 1.0,
            ..Default::default()
        },
        6.0,
    );
    println!("reversing at {:.1} mph gear {}", v.speed_mph(), v.gear);
    assert!(v.speed_mph() < -3.0);
    assert_eq!(v.gear, 0);
}

#[test]
fn leaning_back_pulls_a_wheelie() {
    let Some((mut v, world)) = settled() else {
        return;
    };
    let mut max_pitch: f32 = 0.0;
    for _ in 0..120 {
        v.step(
            &Input {
                throttle: 1.0,
                lean: -1.0,
                ..Default::default()
            },
            1.0 / 60.0,
            &world,
        );
        max_pitch = max_pitch.max(v.forward().y);
    }
    println!("wheelie pitch {max_pitch:.2}");
    assert!(max_pitch > 0.1);
}

#[test]
fn preload_hops() {
    let Some((mut v, world)) = settled() else {
        return;
    };
    run(
        &mut v,
        &world,
        Input {
            throttle: 1.0,
            ..Default::default()
        },
        3.0,
    );
    run(
        &mut v,
        &world,
        Input {
            throttle: 1.0,
            jump: true,
            ..Default::default()
        },
        1.0,
    );
    let y0 = v.position.y;
    let mut peak = y0;
    let mut air = 0;
    for _ in 0..90 {
        v.step(
            &Input {
                throttle: 1.0,
                ..Default::default()
            },
            1.0 / 60.0,
            &world,
        );
        peak = peak.max(v.position.y);
        air += usize::from(!v.grounded());
    }
    println!(
        "hop height {:.2} m, {air} frames in the air at {:.0} mph",
        peak - y0,
        v.speed_mph()
    );
    assert!(peak - y0 > 0.2);
}

#[test]
fn leans_in_the_air() {
    let Some((mut v, world)) = settled() else {
        return;
    };
    v.reset(Vec3::new(0.0, 30.0, 0.0), Quat::IDENTITY);
    run(
        &mut v,
        &world,
        Input {
            lean: 1.0,
            ..Default::default()
        },
        0.5,
    );
    println!("nose after leaning forward in the air: {:?}", v.forward());
    assert!(v.forward().y < -0.1);
}
