# Vehicle physics

`atv-sim` is a port of the original handling code in `default.xbe`, not a
fresh model. The constants come from each quad's `.params` file; anything a
file leaves out falls back to the engine's built-in defaults. Addresses below are virtual addresses in the prototype
disc's XBE (built 2002-10-02), for anyone checking the port against a
disassembly; "Retail" lists what the USA retail XBE (2002-12-22) changes.

## Model

* Mass is 1, so forces are accelerations. Gravity is 25 m/s².
* The game frame is left handed (X right, Y up, +Z forward). The port runs in
  game coordinates internally and mirrors (negates Z) when it exposes state
  and when it raycasts the track.
* Wheels are spring struts along the body's up axis. They only hold the quad
  up. Drive, braking, cornering and grip are applied to the body as a whole in
  a ground frame built from the wheel contacts.
* Explicit Euler in fixed 1/60 s ticks, as the original: its game loop
  (0x84f20) steps the world at the display rate (60 Hz on NTSC, from
  0x19db40), at most 0.1 s per frame. Time short of a tick carries over, and
  the pose shown in between blends the last two ticks. A finer step changes
  how the suspension throws the quad off ramps and through water: at 120 Hz
  computer riders fell into Forest 2's river 19 times in a 3-lap race, at
  60 Hz none did.

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

## Retail

The retail disc's tuning goes with retail handling code, so the port picks
the code by the data: `.params` with `ENGINEREVRATEREVVING` (retail's
Generic has it, the prototype's doesn't) set `VehicleParams::retail`. On the
prototype's files nothing below applies. Retail addresses:

| Retail | Port | What changed from the prototype |
|---|---|---|
| `0x6f750` (ATV update) | `substep` | Rider lean, wheelies and the pre-load are off on the grid |
| `0x6e0b0` (ATV vtable `0x1bb1e8` slot 18) | `controls` | Runs the Dyne's smoothing (`0xda3f0`) and then its own copy, so pedals and steering move twice per tick |
| `0x6cfa0` | `wheelies` | Torque scale lerps WHEELIETORQUE0MPH..100MPH by speed, times the 0 mph value again |
| `0x6e6e0` | `ground_forces` | Wheelspin only above 3/4 throttle; side grip × ground normal.y²; reversing yaw damping 0.8 (was 0.4); CONSTMASSTORQUE only going forward; turbo drive |
| `0x6db90` | `preload_update` | The hop also spins: (lean − throttle + brake) × PRELOADVERTSPINMAG about right, steer × PRELOADHORIZSPINMAG about up |
| `0x6d1b0` | `stabiliser` | On one or two wheels, `speed × BICYCLETORQUE × (ground side.y − right.y − BICYCLEDAMPING × roll rate)` about the forward axis, at the front right wheel (the bicycle trick's resting state; the trick itself isn't ported) |
| `0x6e2e0` | `post_step` | ENGINEREVRATENORMAL/REVVING are RPM/s slew limits on the target revs; revs then ease toward it at 50/s (`0x6e540`) |
| `0x6e470`, `0x6d490`, `0x6d570` | `set_grid`, `start_race` | Held on the grid (only the springs move it, Dyne `0xd9190`) the rider can rev. Letting go with the revs between MINTURBOREVSCALE and MAXTURBOREVSCALE of the range gives a turbo start: drive × DRIVEFORCETURBOSCALE for MAXTURBOTIME seconds |

Unchanged: air forces and gravity (`0x6de40`, `0x6d5d0`), landing
stabiliser (`0x6ce20`), integrator (`0xd1970`), surface grip (`0xdfc20`),
pedals and reverse (`0x1c150`), brakes and cornering. Most ATV fields sit 4
bytes later than in the prototype (in-air gravity `0x80c` vs `0x808`).

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

The original collides with the track's `.kdt` triangles (0x5d030), each
naming a surface in `Setup/Materials.xml`, and so does the rewrite. The
`.row` render meshes mostly match them to a few centimetres, but not
everywhere: Industrial 2 and 3 have a 2-3 m trench across the track that
the `.kdt` bridges, where riders on the render meshes crashed and respotted
(47 and 51 respots in a 6-rider 3-lap race, now 24 and 13). Under deep
water the `.kdt` has no floor at all, so render triangles fill in there,
with no surface. Each
tyre on the ground takes its surface's `Friction` as grip (a tenth of it on
faces over 45 degrees from the strut) and its `Damping` as drag; the quad
averages both over the tyres down. Grip blends every handling pair between
its grippy and slippery value, and keeps its last value in the air. Drag
adds to air resistance against ground speed squared. Props have full grip
and no drag.

Damping is what slows a quad: Water and Deep_Water 30, Dirt_Bank 20,
Mud_Bank and Sand_Bank 15, Loose_Sand and Snow_Bank 10, Loose_Dirt and
Loose_Mud 5. Plain Mud has none, only less grip (0.45).

## Player tuning

The launcher's Config tab writes `tuning.cfg` (`atv_sim::tuning`). The game
reads it for every race (`atv-game/src/tuning.rs`) and builds each quad's
`VehicleParams` as usual, then:

* per-quad lines (`Bandit.MASS = 1.3`) replace the first number of that key
  after the quad's own file. `FRONT.`/`REAR.` wheel keys set the wheels in
  front of or behind the centre. `AIRGRAVITYSCALE` wins over `INAIRGRAVSCALE`.
* percentages scale the parsed values on every quad: `gravity` scales
  `VehicleParams::gravity` (the engine's 25 m/s², on the ground and in the
  air), `air_gravity` the in-air gravity scale, and so on.

Computer riders keep the game's tuning unless `computer_riders = true`.
`atv-tool` ignores the file.

## Not ported yet

* The bicycle trick and the bumpy-terrain wheel offset.
* The original's own body collision spheres. `body_collision` uses five
  hand-placed spheres instead, answered the way the original answers a
  contact: a position fix plus an impulse at the contact point (`0xad9b0`).
* Only the automatic gearbox (mode 0).
* Knock-over props (cones, signs, deckchairs, umbrellas) collide with the
  quads by their `.hsc` shapes as the original's do (a sphere inside the
  other body's hull, `0xb3da0`, or touching one of its spheres, `0xb29d0`),
  in the fixed physics tick, but the quads themselves don't collide by
  `Generic.hsc` with the track or each other yet. They are bodies of mass 0.05
  and hardness 0.3 (`0x2e100` over the defaults at `0xada50`) against the
  quad's 1 and 0.2, so a hit costs the quad `1.06 × 0.05 / 1.05` (about
  5%) of its closing speed, as the original's body contact (`0xb29d0`,
  `0xb1ca0`) gives; how they tumble (a rigid body landing on its hull
  corners and spheres) is the rewrite's own. Static props
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
