//! Handling checks on a flat plane. They need the game's own tuning files,
//! so they only run when `ATV2_PARAMS` points at an extracted `Cars` folder
//! (e.g. from `atv-tool extract`); otherwise they pass without checking.

use std::sync::Arc;

use atv_formats::hsc::BodyShape;
use atv_sim::body::Body;
use atv_sim::{Input, TrackCollision, Vehicle, VehicleParams};
use glam::{Quat, Vec3};

fn params() -> Option<VehicleParams> {
    let dir = std::path::PathBuf::from(std::env::var_os("ATV2_PARAMS")?);
    let generic = std::fs::read_to_string(dir.join("Generic/Generic.params")).ok()?;
    let quad = std::fs::read_to_string(dir.join("Bandit/Bandit.params")).ok();
    let mut p = VehicleParams::from_files(&generic, quad.as_deref());
    // The quads' collision shape, when the folder has it.
    if let Some(hsc) = std::fs::read(dir.join("Generic/Generic.hsc")).ok().and_then(|b| BodyShape::parse(&b).ok()) {
        p.body = Arc::new(Body::from_hsc(&hsc));
    }
    Some(p)
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

/// Flat ground plus extra triangles.
fn flat_with(extra: Vec<[Vec3; 3]>) -> TrackCollision {
    let s = 2000.0;
    let mut tris = vec![
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
    ];
    tris.extend(extra);
    TrackCollision::new(tris, 8.0)
}

/// A quad (two triangles) from four corners.
fn quad(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> [[Vec3; 3]; 2] {
    [[a, b, c], [a, c, d]]
}

#[test]
fn lands_a_kicker_on_its_wheels() {
    let Some(p) = params() else { return };
    // A 1.5 m kicker 8 m wide across the -Z direction (forward), with a
    // sheer back face.
    let (w, z0, z1, h) = (4.0, -40.0, -46.0, 1.5);
    let mut extra = Vec::new();
    extra.extend(quad(
        Vec3::new(-w, 0.0, z0),
        Vec3::new(w, 0.0, z0),
        Vec3::new(w, h, z1),
        Vec3::new(-w, h, z1),
    ));
    extra.extend(quad(
        Vec3::new(-w, h, z1),
        Vec3::new(w, h, z1),
        Vec3::new(w, 0.0, z1),
        Vec3::new(-w, 0.0, z1),
    ));
    let world = flat_with(extra);
    let mut v = Vehicle::new(p, Vec3::new(0.0, 0.6, 40.0), Quat::IDENTITY);
    run(&mut v, &world, Input::default(), 1.0);
    let go = Input {
        throttle: 1.0,
        ..Default::default()
    };
    let (mut air, mut longest, mut landed_at) = (0.0f32, 0.0f32, None);
    let mut lowest = f32::MAX;
    for i in 0..(10.0 * 60.0) as usize {
        v.step(&go, 1.0 / 60.0, &world);
        let t = i as f32 / 60.0;
        assert!(v.up().y > 0.0, "flipped over at t={t:.2}");
        if v.position.z < z1 - 2.0 {
            lowest = lowest.min(v.position.y);
        }
        if v.grounded() {
            if air > 0.3 && landed_at.is_none() {
                landed_at = Some(t);
                println!(
                    "landed after {air:.2}s at {:.0} mph, nose {:.2}",
                    v.speed_mph(),
                    v.forward().y
                );
            }
            air = 0.0;
        } else {
            air += 1.0 / 60.0;
            longest = longest.max(air);
        }
        if landed_at.is_some_and(|l| t > l + 2.0) {
            break;
        }
    }
    println!("longest air {longest:.2}s, lowest centre of mass after the kicker {lowest:.2}");
    // Gravity is 2.8x while rising (INAIRGRAVUPSCALE), so plain ramp jumps
    // are short; preload is what gives big air.
    assert!(longest > 0.3, "barely left the ground");
    assert!(landed_at.is_some(), "never landed");
    assert!(lowest > 0.1, "chassis went into the ground");
    assert!(v.up().y > 0.95 && v.wheels.iter().all(|w| w.contact));
}

#[test]
fn survives_a_drop_without_pogoing() {
    let Some(p) = params() else { return };
    let world = flat();
    let mut v = Vehicle::new(p, Vec3::new(0.0, 6.0, 0.0), Quat::IDENTITY);
    let mut hit = false;
    let mut rebound = 0.0f32;
    let mut lowest = f32::MAX;
    for _ in 0..(4.0 * 60.0) as usize {
        v.step(&Input::default(), 1.0 / 60.0, &world);
        lowest = lowest.min(v.position.y);
        if v.grounded() {
            hit = true;
        }
        if hit {
            rebound = rebound.max(v.position.y);
        }
    }
    println!(
        "lowest {lowest:.2}, highest after touchdown {rebound:.2}, rest {:.2}",
        v.position.y
    );
    assert!(lowest > 0.1, "chassis went into the ground");
    assert!(rebound < 0.8, "bounced back up to {rebound:.2}");
    assert!(v.up().y > 0.99 && v.velocity.length() < 0.1);
}

#[test]
fn does_not_drive_through_walls() {
    let Some(p) = params() else { return };
    let z = -30.0;
    let wall = quad(
        Vec3::new(-10.0, 0.0, z),
        Vec3::new(10.0, 0.0, z),
        Vec3::new(10.0, 3.0, z),
        Vec3::new(-10.0, 3.0, z),
    );
    let world = flat_with(wall.to_vec());
    let mut v = Vehicle::new(p, Vec3::new(0.0, 0.6, 60.0), Quat::IDENTITY);
    let go = Input {
        throttle: 1.0,
        ..Default::default()
    };
    let mut fastest = 0.0f32;
    for _ in 0..(8.0 * 60.0) as usize {
        v.step(&go, 1.0 / 60.0, &world);
        fastest = fastest.max(v.speed_mph());
        assert!(v.position.y < 2.0, "climbed the wall");
    }
    println!(
        "hit the wall at up to {fastest:.0} mph, stopped at z={:.2}",
        v.position.z
    );
    assert!(fastest > 50.0);
    assert!(v.position.z > z, "went through the wall");
}

#[test]
fn boost_pulls_harder() {
    let Some((v, world)) = settled() else {
        return;
    };
    let full = Input {
        throttle: 1.0,
        ..Default::default()
    };
    let speed_after = |scale: f32| {
        let mut v = v.clone();
        v.drive_scale = scale;
        run(&mut v, &world, full, 3.0);
        v.speed_mph()
    };
    let (plain, boosted) = (speed_after(1.0), speed_after(2.5));
    println!("after 3 s: {plain:.1} mph, boosted {boosted:.1} mph");
    assert!(boosted > plain + 5.0);
}

/// Retail only: revving on the grid holds the quad in place, and letting
/// go with the revs in the turbo window gives a faster start.
#[test]
fn turbo_start_from_the_grid() {
    let Some((_, world)) = settled() else { return };
    let launch = |rev: f32| {
        let (mut v, _) = settled().unwrap();
        v.hold_on_grid();
        let start = v.position;
        let revving = Input {
            throttle: rev,
            ..Default::default()
        };
        run(&mut v, &world, revving, 2.0);
        let moved = (v.position - start).with_y(0.0).length();
        v.start_race();
        let turbo = v.turbo();
        let full = Input {
            throttle: 1.0,
            ..Default::default()
        };
        run(&mut v, &world, full, 1.0);
        (moved, turbo, v.speed_mph(), v.revs)
    };
    if !params().unwrap().retail {
        return;
    }
    let (moved, turbo, mph, _) = launch(0.75);
    let (moved_flat, turbo_flat, mph_flat, _) = launch(1.0);
    println!("turbo {turbo} {mph:.1} mph, flat out {turbo_flat} {mph_flat:.1} mph, moved {moved:.3} {moved_flat:.3}");
    assert!(moved < 0.01 && moved_flat < 0.01, "crept off the grid");
    assert!(turbo && !turbo_flat);
    assert!(mph > mph_flat + 2.0);
}

/// Drawn between steps at a monitor's rate, a falling quad moves on every
/// frame and by a steadily growing amount: no frame repeats the last pose
/// and none jumps two steps.
#[test]
fn drawn_pose_moves_every_frame() {
    let Some((mut v, world)) = settled() else { return };
    v.reset(Vec3::new(0.0, 20.0, 0.0), Quat::IDENTITY);
    // As Bevy's fixed loop: 120 Hz steps, frames drawn at 144 Hz.
    let (step, frame) = (1.0 / 120.0, 1.0 / 144.0);
    let mut spare = 0.0;
    let mut heights = Vec::new();
    for _ in 0..100 {
        spare += frame;
        while spare >= step {
            spare -= step;
            v.step(&Input::default(), step, &world);
        }
        heights.push(v.drawn(spare).0.y);
    }
    let drops: Vec<f32> = heights.windows(2).map(|w| w[0] - w[1]).collect();
    // A frame's drop grows by at most a tick's gain in speed (falling
    // gravity 25 x 1.5 for 1/60 s) over a frame: 4.3 mm. Showing the
    // stepped pose instead drops 0 and then twice as far.
    for d in drops.windows(2).skip(10) {
        assert!(d[0] > 0.0 && d[1] > 0.0, "a frame stood still: {drops:?}");
        assert!((d[1] - d[0]).abs() < 0.0045, "uneven frames {d:?}: {drops:?}");
    }
}

#[test]
fn sides_stay_out_of_a_wall() {
    let Some(p) = params() else { return };
    // A wall along the quad's right (+X in the sim frame), 1 m away.
    let x = 1.0;
    let wall = quad(Vec3::new(x, 0.0, -50.0), Vec3::new(x, 0.0, 50.0), Vec3::new(x, 3.0, 50.0), Vec3::new(x, 3.0, -50.0));
    let world = flat_with(wall.to_vec());
    let mut v = Vehicle::new(p, Vec3::new(0.0, 0.6, 0.0), Quat::IDENTITY);
    run(&mut v, &world, Input::default(), 1.0);
    // Slammed sideways into it.
    v.nudge(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0));
    let mut closest = f32::MAX;
    for _ in 0..120 {
        v.step(&Input::default(), 1.0 / 60.0, &world);
        closest = closest.min(x - v.position.x);
    }
    println!("closest {closest:.2} m from the wall, settled {:.2} m, up {:.2}", x - v.position.x, v.up().y);
    // The body is about 1.1 m wide; its side stops at the wall.
    assert!(closest > 0.45 && closest < 0.7, "centre {closest:.2} m from the wall");
}

/// Two quads level on the ground, `b` at `at` from `a`.
fn pair(at: Vec3) -> Option<(Vehicle, Vehicle, TrackCollision)> {
    let world = flat();
    let mut a = Vehicle::new(params()?, Vec3::new(0.0, 0.6, 0.0), Quat::IDENTITY);
    let mut b = Vehicle::new(params()?, Vec3::new(0.0, 0.6, 0.0) + at, Quat::IDENTITY);
    run(&mut a, &world, Input::default(), 1.0);
    run(&mut b, &world, Input::default(), 1.0);
    Some((a, b, world))
}

fn run_pair(a: &mut Vehicle, b: &mut Vehicle, world: &TrackCollision, input: Input, secs: f32, mut each: impl FnMut(&Vehicle, &Vehicle)) {
    for _ in 0..(secs * 60.0) as usize {
        a.step(&input, 1.0 / 60.0, world);
        b.step(&Input::default(), 1.0 / 60.0, world);
        atv_sim::ai::bump(a, b);
        each(a, b);
    }
}

#[test]
fn quads_side_by_side_touch_at_their_sides() {
    let Some((mut a, mut b, world)) = pair(Vec3::X * 1.8) else { return };
    // `a` slides into `b`.
    a.nudge(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0));
    let mut closest = f32::MAX;
    run_pair(&mut a, &mut b, &world, Input::default(), 2.0, |a, b| closest = closest.min(b.position.x - a.position.x));
    println!("closest {closest:.2} m apart, b moved {:.2} m", b.position.x - 1.8);
    // They meet at their sides (wheels 0.75 m out from each middle), not
    // 2 m apart nor through each other, and `b` is shoved along.
    assert!(closest > 1.0 && closest < 1.6, "closest {closest:.2}");
    assert!(b.position.x > 1.85);
}

#[test]
fn quads_bump_with_little_bounce() {
    // `b` parked 6 m ahead of `a` (forward is -Z), `a` drives into it.
    let Some((mut a, mut b, world)) = pair(Vec3::NEG_Z * 6.0) else { return };
    let go = Input { throttle: 1.0, ..Default::default() };
    let (mut hit, mut before) = (None, 0.0);
    let mut closest = f32::MAX;
    run_pair(&mut a, &mut b, &world, go, 2.0, |a, b| {
        closest = closest.min(a.position.z - b.position.z);
        if hit.is_none() {
            if b.velocity.z < -1.0 {
                hit = Some((a.velocity.z, b.velocity.z));
            } else {
                before = a.velocity.z;
            }
        }
    });
    let (va, vb) = hit.expect("never touched");
    println!("closest {closest:.2} m; a hit at {before:.2} m/s, then a {va:.2} m/s, b {vb:.2} m/s");
    // Nose to tail about 1.5 m apart, and hardness 0.2: `b` is pushed on
    // without flying off ahead.
    assert!(closest > 1.2 && closest < 2.0, "closest {closest:.2}");
    assert!(vb - va > -1.0 * va.abs() * 0.35, "bounced apart: a {va:.2} b {vb:.2}");
}

#[test]
fn a_shown_quad_is_drawn_where_it_was_told() {
    let Some((mut v, world)) = settled() else { return };
    let mut puppet = Vehicle::new(v.params.clone(), Vec3::new(50.0, 0.6, 0.0), Quat::IDENTITY);
    let input = Input { throttle: 1.0, steer: 0.5, ..Input::default() };
    puppet.show(&v.pose(), 1.0 / 60.0);
    for _ in 0..120 {
        let before = v.pose().position;
        v.step(&input, 1.0 / 60.0, &world);
        let now = v.pose().position;
        puppet.show(&v.pose(), 1.0 / 60.0);
        // From the pose shown before to the new one.
        assert!(puppet.drawn(0.0).0.distance(before) < 1e-3);
        assert!(puppet.drawn(1.0 / 120.0).0.distance(before.lerp(now, 0.5)) < 1e-3);
        assert!(puppet.drawn(1.0 / 60.0).0.distance(now) < 1e-3);
    }
    let (pose, shown) = (v.pose(), puppet.pose());
    assert!(shown.position.distance(pose.position) < 1e-4 && shown.rotation.angle_between(pose.rotation) < 1e-4);
    assert!(shown.velocity.distance(pose.velocity) < 1e-4 && shown.angular_velocity.distance(pose.angular_velocity) < 1e-4);
    assert_eq!((shown.gear, shown.wheels.len()), (pose.gear, pose.wheels.len()));
    for (a, b) in shown.wheels.iter().zip(&pose.wheels) {
        assert!((a.compression - b.compression).abs() < 1e-4 && (a.spin - b.spin).abs() < 1e-4 && a.contact == b.contact);
    }
    assert!((puppet.speed_mph() - v.speed_mph()).abs() < 1e-3 && puppet.grounded() == v.grounded());
}

#[test]
fn the_game_plays_by_the_last_ticks_pose() {
    let Some((mut v, world)) = settled() else { return };
    let input = Input { throttle: 1.0, ..Input::default() };
    for _ in 0..90 {
        // A 120 Hz caller: every other step runs a tick.
        v.step(&input, 1.0 / 120.0, &world);
        let pose = v.pose();
        assert_eq!(v.position, pose.position);
        assert!(v.rotation.dot(pose.rotation).abs() > 1.0 - 1e-6);
    }
    // Drawn just after a step, the quad is still on its way there.
    assert!(v.drawn(0.0).0.distance(v.position) > 0.0);
}
