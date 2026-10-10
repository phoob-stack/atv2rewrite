# Vehicle physics

`atv-sim` is a port of the original handling code in `default.xbe`, not a
fresh model. The constants come from each quad's `.params` file; anything a
file leaves out falls back to the engine's built-in defaults. Addresses below are virtual addresses in the retail XBE, for
anyone checking the port against a disassembly.

## Model

* Mass is 1, so forces are accelerations. Gravity is 25 m/s².
* The game frame is left handed (X right, Y up, +Z forward). The port runs in
  game coordinates internally and mirrors (negates Z) when it exposes state
  and when it raycasts the track.
* Wheels are spring struts along the body's up axis. They only hold the quad
  up. Drive, braking, cornering and grip are applied to the body as a whole in
  a ground frame built from the wheel contacts.
* Explicit Euler at 120 Hz with up to 32 substeps per frame, which matches
  the original's minimum timestep.

## Where things came from

| Original | Port (`vehicle.rs`) | What it does |
|---|---|---|
| `0x5cd10` (ATV update) | `substep` | Per-step order of everything below |
| `0x1a9a0` | `controls` | Pedals, reverse on held brake, auto brake, steer and lean rates |
| `0xbbc80` | `approach` | Rate limited move toward a target |
| wheel vtable `0x1c60b4` | `wheel_contacts`, `springs` | Strut raycasts and spring/damper forces |
| `0x5bd10` | `ground_frame`, `ground_forces` | Slides, wheelspin, drive, brakes, cornering, side grip, drag |
| `0x5d030`, `0xbc800`, `0xb53e0` | `wheel_contacts`, `ground_frame` | Surface grip and drag under each tyre, averaged over the wheels down (`surface.rs`) |
| `0x5a650` | end of `ground_forces` | Torque plus the force it implies at the application point |
| ATV vtable `0x1bf708` | `wheelies`, `air_forces` | Wheelie/endo torque, air steer and lean, air gravity scale |
| ATV update | `preload_update`, `landing_stabiliser` | Preload hop and landing pitch damping |
| Dyne vtable `0x1c5788` | `integrate`, `post_step` | Integration, gearbox, engine revs |
| `0xb5bf0`, `0x59aa0` | `params.rs` | `.params` parser and curve sampling |

## Crashes

The game decides when the player crashes (`atv-game/src/crash.rs`), using
the original's rules:

* **Impact** (`0x1a5f0`). A chassis sphere hits something. The hit counts
  as `speed into the surface × (0.5 − 0.5·up·n) × ((forward·n + 1)/2 + 1)`.
  If that is over 5 (10 with three or more wheels down), the quad crashes.
  So a hard landing on the back or side crashes, and so does a fast hit
  into a wall.
* **Rollover**. Tipped past `up.y < 0.25` with its body touching, the quad
  crashes within a few steps (+1.2 a step against a limit of 6). With one
  wheel or none down and the body clear, it crashes after
  `(0.5 − 0.5·up.y)` per second adds up to 6.
* **Crash start** (`0x28ce0`). The way the quad went over (nose, back, or
  a side) and the cause pick the clip:
  * Rollover: `fall_front_endo`, `fall_wheelie` / `fall_high_wheelie`, or
    `fall_onto_back`.
  * Impact: `head_over_heels` (speed² ≤ 800) or `crash_front_1`,
    `fall_wheelie` / `fall_high_wheelie`, or `fall_side_A`.
  * A right-side fall plays the clip mirrored.
* **Water** (sensors `0xd7f60`, set up with -1 m at `0x1edd0` and
  `0x2d510`; the rider at `0x2cca0` and `0x2ca40`; the respot at `0x26e30`).
  Deep water is the track's `waterbox` boxes (see formats.md), measured
  from the quad's centre of mass:
  * Coming down faster than 0.08 m a frame (4.8 m/s) less than 3 m over
    a box's top, the seated rider braces with `Plunge`.
  * With its centre 1 m under the top, the quad is in the water: the rider
    comes off (crash type 5) and treads water (`Tread_water`: frames 2-46
    once, then 46-68 over and over, `0x2bbb0` case 6, each loop sending out
    three rings), and the quad is dragged by `-10 × velocity` and
    `-5 × spin` (`0x5ba70`).
  * A thrown rider who goes 1 m under a top treads water too.
  * Going in falling faster than 4.8 m/s splashes (see formats.md,
    Particles), for the quad and for a thrown rider alike.
  * 1.5 s after the quad or the rider went in, the original starts its
    respot (`0x25f20`), which fades for 0.5 s; here the quad is put back
    at 2 s.
  * The floating rider is pulled up by `50 × (0.3 − h − 0.5 × rising
    speed)` and slowed at 25 a second (`0x2ca40`). The original measures
    `h` from the box top and only pulls a body a metre under it; here `h`
    is the height over the water you can see, which floats the pelvis
    0.2 m under it.
  * While the rider treads water the follow camera stops above the water
    and turns to watch (the original's camera here isn't worked out).
  * Only player 1 crashes into water so far; computer riders and players
    2-4 drive on along the bed.
  * The original treads water even when only the quad went in, the rider
    still on the bank; the rewrite keeps such a rider lying where they
    fell, with the same respot.
* **Getting up** (`0x2bbb0`). After going head over heels the rider rolls
  about on the back (`movement_on_back_*`). After a face-first crash the
  rider pushes up (`push_up*`, `reach_out`). Then the quad is put back at
  the last safe spot.
* Crash clip ids are the table at `0x1d1b00`: a `u32` id, then a 56-byte
  path. They run from 22 (`head_over_heels`) to 46 (`run_over_back`).

The rider's pelvis rides a point mass. It leaves the seat with the quad's
velocity plus a small hop, falls under gravity, and slides to a stop on the
track collision. The clip's bone rotations are layered on top of it.

These are approximations:

* The clips play at 30 frames a second.
* The original's frame ranges and per-clip speeds aren't copied.
* The original's rider body is a full physics object, not a point mass.

A landing after more than half a second in the air plays the `landing`
clip once over the riding pose.

## Surfaces

The original collides with the track's `.kdt` triangles, each naming a
surface in `Setup/Materials.xml`. The rewrite collides with the `.row`
render meshes, so on load each render triangle takes the surface of the
`.kdt` triangle under its centre (they match to a few centimetres). Each
tyre on the ground takes its surface's `Friction` as grip (a tenth of it on
faces over 45 degrees from the strut) and its `Damping` as drag; the quad
averages both over the tyres down. Grip blends every handling pair between
its grippy and slippery value, and keeps its last value in the air. Drag
adds to air resistance against ground speed squared. Props have full grip
and no drag.

Damping is what slows a quad: Water and Deep_Water 30, Dirt_Bank 20,
Mud_Bank and Sand_Bank 15, Loose_Sand and Snow_Bank 10, Loose_Dirt and
Loose_Mud 5. Plain Mud has none, only less grip (0.45).

## Not ported yet

* Turbo, the bicycle trick, and the bumpy-terrain wheel offset.
* The original's own body collision spheres. `body_collision` uses five
  hand-placed spheres instead, answered the way the original answers a
  contact: a position fix plus an impulse at the contact point (`0xad9b0`).
* Only the automatic gearbox (mode 0).
* Knock-over props (cones, signs, barriers, crates) collide as upright
  cylinders, not through their `.hsc` hulls. They are bodies of mass 0.05
  and hardness 0.3 (`0x2e100` over the defaults at `0xada50`) against the
  quad's 1 and 0.2, so a hit costs the quad `1.06 × 0.05 / 1.05` (about
  5%) of its closing speed, as the original's body contact (`0xb29d0`,
  `0xb1ca0`) gives; how they tumble is the rewrite's own. Static props
  collide through their `.ink` meshes.
* Crash parts of the original that aren't ported yet:
  * water crashes for computer riders and players 2-4 (their quads do
    splash)
  * hits from other riders (causes 3 and 4)
  * the rider's own head contact (cause 2)
  * computer riders crashing (they are respotted as before)

## Tests

`crates/atv-sim/tests/handling.rs` drives a quad on a flat plane. The tests
need real tuning files, so they only run when `ATV2_PARAMS` points at an
extracted `Cars` folder and pass trivially otherwise:

```
ATV2_PARAMS=/path/to/extracted/Cars cargo test -p atv-sim
```
