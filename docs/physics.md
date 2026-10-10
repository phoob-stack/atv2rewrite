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
| `0x5a650` | end of `ground_forces` | Torque plus the force it implies at the application point |
| ATV vtable `0x1bf708` | `wheelies`, `air_forces` | Wheelie/endo torque, air steer and lean, air gravity scale |
| ATV update | `preload_update`, `landing_stabiliser` | Preload hop and landing pitch damping |
| Dyne vtable `0x1c5788` | `integrate`, `post_step` | Integration, gearbox, engine revs |
| `0xb5bf0`, `0x59aa0` | `params.rs` | `.params` parser and curve sampling |

## Not ported yet

* Surface materials: grip is fixed at 1 and surface drag at 0.
* Turbo, the bicycle trick, and the bumpy-terrain wheel offset.
* The original's own body collision spheres. `body_collision` uses five
  hand-placed spheres instead, answered the way the original answers a
  contact: a position fix plus an impulse at the contact point (`0xad9b0`).
* Only the automatic gearbox (mode 0).

## Tests

`crates/atv-sim/tests/handling.rs` drives a quad on a flat plane. The tests
need real tuning files, so they only run when `ATV2_PARAMS` points at an
extracted `Cars` folder and pass trivially otherwise:

```
ATV2_PARAMS=/path/to/extracted/Cars cargo test -p atv-sim
```
