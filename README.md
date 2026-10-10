# ATV Quad Power Racing 2: PC rewrite

A from-scratch PC version of the 2003 Xbox game, written in Rust with Bevy.
**No game data is included or distributed.** Everything is loaded at runtime
from the player's own copy of the game: the Xbox disc image (`.iso`/`.bin`)
or a folder the disc was extracted to.

## Crates

| Crate | What it does |
|---|---|
| `atv-formats` | Engine-free readers: Xbox disc filesystem, "Bark" archive + LZSS, BOG textures (DXT1/DXT5, swizzled ARGB, 8-bit palettized), BED sprite sheets and bitmap fonts, ROM 1.26 models (meshes + Bezier patches), ROW 2.25 track worlds, `Setup/*.xml` rule files. |
| `atv-sim` | Engine-free vehicle physics ported from the original game (see `docs/physics.md`): reads the game's `.params` tuning files, spring-strut wheels, body-level ground forces, auto gearbox, wheelies, preload jumps, air control, and track collision built from the track's meshes and patches. |
| `atv-tool` | Command line: `list`, `extract <dir>`, `check` (parses every texture, model and track), `sim` (headless autopilot lap test). |
| `atv-game` | The game (binary `atv2`): the original front end (title, main menu, rider / ATV / track select, loading screen) into a race with the original HUD (rev gauge, laps, times, lap meter, 3-2-1-GO countdown, checkpoint and wrong-way banners, pause and results menus); a drivable quad with its rider on any track, chase camera, keyboard and gamepad input. |
| `atv-launcher` | The launcher (binary `atv2-launcher`): pick your disc image or extracted folder once, choose track, quad, resolution and fullscreen, press Play. |

## Running

The easy way is the launcher. Build both binaries so they sit side by side,
then run the launcher:

```
cargo build --release -p atv-game -p atv-launcher
./target/release/atv2-launcher
```

![Launcher](docs/launcher.png)

It checks your game data with the same loader the game uses, lists the tracks
and quads it finds, remembers your choices, and starts `atv2` for you. If the
game fails to start, its last error lines show in the launcher.

From the command line:

```
# Point at your own disc image (or extracted folder)
export ATV2_GAME="/path/to/ATV Quad Power Racing 2.bin"

cargo run --release -p atv-tool -- check
cargo run --release -p atv-game                     # front end
cargo run --release -p atv-game -- --track Dunes1 --quad Bandit --rider Male1 --laps 3   # straight into a race
cargo run --release -p atv-tool -- sim --track Dunes1 --quad Bandit --seconds 120
```

| Action | Keyboard | Gamepad |
|---|---|---|
| Throttle | W / Up | RT or A |
| Brake / reverse | S / Down | LT or X |
| Steer | A / D | Left stick |
| Lean back / forward | I / K | Left stick up/down |
| Jump (preload) | Space | RB or B |
| Respot | R | |
| Restart | Backspace | |
| Pause | Escape | Start |

Menus: arrow keys or D-pad to move, Enter or A to select, Escape or B to go
back; left/right changes the lap count on track select.

The quad respots automatically when it is upside down or falls off the world.
Other options: `--frontend` (show the menus even with `--track`, which
then preselects), `--screen title|main|rider|atv|track` (open on that
front-end screen), `--autodrive` (autopilot chases checkpoints), `--camera
x,y,z,tx,ty,tz` (free-fly camera in game coordinates: WASD, Q/E, Shift, right
mouse to look), `--screenshot out.png --after 8` (saves a frame and exits),
`--resolution 1920x1080`, `--fullscreen` (borderless, desktop resolution).

Tracks: Dunes1-3, Forest1-3, Glacier1-3, Industrial1-3, Swamp1-3, Stadium1-2.
Quads: ATC, Bandit, Blaze, Bullet, Challenger, Charger, Dimension, Nitro,
Rocket, Skyline, Storm.
Riders: Male1-18, Female1-6 (the folders under `Characters`).

Linux build dependencies: `libasound2-dev libudev-dev libxkbcommon-dev
libwayland-dev` (standard Bevy requirements).

## Format notes

See `docs/formats.md` for everything reverse-engineered so far, including
what is still unknown.
