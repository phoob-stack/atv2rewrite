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
<<<<<<< HEAD
| `atv-game` | The game (binary `atv2`): the original front end (title, main menu, rider / ATV / track select, loading screen) into a race with the original HUD (rev gauge, laps, times, lap meter, 3-2-1-GO countdown, checkpoint and wrong-way banners, pause and results menus); a drivable quad with its rider on any track, chase camera, keyboard and gamepad input. |
| `atv-hd` | HD texture packs: upscales the game's own textures with an AI upscaler (Real-ESRGAN) on the player's machine into a local cache the game prefers when present. |
=======
| `atv-game` | The game (binary `atv2`): the original front end (title, main menu, rider / ATV / track select, loading screen) into a race with the original HUD (rev gauge, laps, times, lap meter, 3-2-1-GO countdown, checkpoint and wrong-way banners, pause and results menus); Single Race, Time Trial, Arcade and Open Freestyle modes; tricks with the original stunt animations; a drivable quad with its rider on any track, chase camera, keyboard and gamepad input. |
>>>>>>> origin/main
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
cargo run --release -p atv-tool -- race --track Dunes1 --riders 4 --laps 2   # AI-only race, prints lap times
```

| Action | Keyboard | Gamepad |
|---|---|---|
| Throttle | W / Up | RT or A |
| Brake / reverse | S / Down | LT or X |
| Steer | A / D | Left stick |
| Lean back / forward | I / K | Left stick up/down |
| Jump (preload) | Space | RB or B |
| Speed boost | Shift | Y (on the ground) |
| Kick | E | LB (on the ground) |
| Respot | R | |
| Restart | Backspace | |
| Pause | Escape | Start |
| Tricks (in the air) | 1 / 2 / 3 / 4 + direction | X / Y / LB / RB + left stick or D-pad |

## Game modes

- **Single Race**: pick rider, ATV, track and laps, then race five computer
  riders (`--opponents N` for 0-7). They drive the original game's AI along
  each track's racing line. The lap meter shows everyone's place, and the
  results show where you finished.
- **Time Trial**: the same without opponents, against the clock.
- **Career**: the Amateur, Qualifying and Professional championships of
  `Setup/Career.xml`, five rounds each against seven computer riders, with
  race and trick points, skill points, gold medals and ATV unlocks; a top-3
  finish opens the next. Professional opens **Custom**, as in the original:
  after rider and ATV, Choose Your Tracks lists the fifteen championship
  tracks (A adds the highlighted one as the next round or takes it out,
  Right then Up/Down sets its laps from 1 to 20, Y or Continue goes on),
  and Point Distribution shares each place's race and trick points out
  between the race and the trick ranking in 5% steps (Left/Right). Custom
  rounds earn skill points but no medals.
- **Arcade**: the 16 rounds of the disc's `Setup/Arcade.xml` back to back.
  The clock starts at the track's arcade time (40 s) and the arcade
  checkpoints and the line add more. Win a round (finish first, in time)
  for the next one; each round allows three attempts, and losing the third
  sends Arcade back to round 1 (the disc's "You need to finish in first
  place to move onto the next round" and "Attempts: %d/3"). Seven computer riders race each round, as in the
  original (its arcade mode seats 1 player and 7 AI).
  The next round is saved after each one (`arcade` in career.cfg, the
  original's `mArcadeProgress`); choosing Arcade then offers to continue
  from it or start a new game.
- **Open Freestyle**: roam any track for points against a time limit (1, 3,
  5, 10, 15 or 30 minutes, or unlimited) with the disc's Bronze, Silver and
  Gold targets. Points come from tricks, multi-trick combos, flips and spins,
  big air, wheelies, endos and bicycles, the Stadium trick boxes (Cage Jump,
  Bus Leap, Hoopa...) and the first visit to each of the track's spots.
- **Multiplayer**: split-screen Single Race for two to four players (two
  split top and bottom, three or four take a quarter each), with computer
  riders making the field up to four. Each player picks a rider (one
  another player has is greyed out) and an ATV in turn. Player 1 drives
  with WASD (lean R/F, preload Space, boost left Shift, kick E) or the first pad, player 2 with the
  arrow keys (lean Numpad 8/5, preload right Shift, boost Numpad 0, kick Numpad 6) or the second pad, and
  players 3 and 4 with the third and fourth pads. Once the first player is
  home the others get a minute to finish. Freestyle Battle puts the same
  players on a track against a time limit with no computer riders; the
  most points wins. In split screen player 1's trick buttons are 1-4 and
  player 2's are Numpad 1-4. The multiplayer Championship is the original's
  custom championship for the same players: they choose its tracks and
  laps on Choose Your Tracks (and the points split too, once Custom is
  open in the career), computer riders make the field up to four, and
  each round scores the race and the trick ranking as a career round
  does.

Tricks are the 28 from `Setup/Tricks.xml`: hold a trick button (or two for
the level 5-7 tricks: 1+2, 2+3, 3+4) and a direction while airborne. Keep
holding at the peak for extra points; land before the trick's recovery and
you bail and lose the jump. Different tricks in one jump multiply. As in the
original, each trick has a level (four per level) and you can only pull the
ones up to your skill level, which rises with career and challenge skill
points: a new player has the four level 1 tricks on button 1. Split-screen
players 2-4 get their rider's own skill rating. The **Trick Book** (Y or T
on rider select, track select and the career status page) shows every trick
with its level, points and buttons, lit on the pad picture, while the rider
performs it; tricks above your level are greyed with the skill points still
needed. Computer riders throw tricks too, as the original's AI does: more
than 1.75 m off the ground they pick a random trick of their skill rating
that fits the air left, and chain another while it lasts. In a career their
trick scores count against yours for the trick points.

**Speed boost**, as in the original (Tricks.xml `SpeedBoost`): every
point you bank fills the boost meter, the disc in the small dial above the
speed (2500 points fill it). Hold the boost button to spend it: full
throttle and 2.5 times the drive, for up to ten seconds of a full meter. A
crash loses a fifth of it. Computer riders fill theirs with their tricks
and spend it when it's full.

**Kicks**, as in the original: the kick button kicks out at whoever rides
alongside (towards the nearest rider's side). A kick that connects steals
up to a fifth of their boost and takes some of their strength: all of it
if they're weaker than you (each rider's `mStrength`), three quarters if
equal, half if stronger. The kick that takes the last of it knocks them
off and steals all their boost. Strength comes back in two seconds.
Computer riders with an aggression of 3 or more kick anyone alongside.

Menus: arrow keys or D-pad to move, Enter or A to select, Escape or B to go
back; left/right changes the lap count (or Open Freestyle's time limit) on
track select.

The quad respots automatically when it is upside down or falls off the world.
Other options: `--frontend` (show the menus even with `--track`, which
then preselects), `--screen title|main|rider|atv|track|tricks` (open on that
front-end screen), `--mode race|timetrial|arcade|freestyle` with
`--minutes N` (freestyle limit, 0 = unlimited) or `--round N` (arcade round),
`--opponents N` (computer riders in a single race, default 5),
`--players N` (split screen for N players, in a single race or freestyle),
`--autodrive` (autopilot chases checkpoints and throws tricks), `--camera
x,y,z,tx,ty,tz` (free-fly camera in game coordinates: WASD, Q/E, Shift, right
mouse to look), `--screenshot out.png --after 8` (saves a frame and exits),
`--resolution 1920x1080`, `--fullscreen` (borderless, desktop resolution).

Tracks: Dunes1-3, Forest1-3, Glacier1-3, Industrial1-3, Swamp1-3, Stadium1-2.
Quads: ATC, Bandit, Blaze, Bullet, Challenger, Charger, Dimension, Nitro,
Rocket, Skyline, Storm.
Riders: Male1-18, Female1-6 (the folders under `Characters`).

Linux build dependencies: `libasound2-dev libudev-dev libxkbcommon-dev
libwayland-dev` (standard Bevy requirements).

## HD textures (AI upscaled)

The menus, HUD and fonts, and optionally the track, quad and rider
textures, can be upscaled with [Real-ESRGAN](https://github.com/xinntao/Real-ESRGAN)
(the `realesrgan-ncnn-vulkan` build, which runs on any Vulkan GPU with no
Python). The upscaled pack is made on your PC from your own disc and kept in
your local data folder (`%LOCALAPPDATA%\atv2\data\hd` on Windows,
`~/.local/share/atv2/hd` on Linux). It is never part of this repository.

1. Download `realesrgan-ncnn-vulkan` for your OS from the
   [Real-ESRGAN releases](https://github.com/xinntao/Real-ESRGAN/releases)
   and unzip it next to `atv2-launcher` (its own folder is fine).
2. In the launcher, press **Build HD pack**. Tick "Also upscale tracks,
   quads and riders" for the 3D textures too (slower, a few hundred MB).
3. Play. The game uses the pack whenever it is there; untick "Use HD
   textures" (or pass `--no-hd`) to play with the originals.

Menu and HUD art is upscaled 4x by the model and stored at 2x, uncompressed
where it has transparency so text and edges stay crisp; tiling 3D textures
are upscaled with their edges wrapped (so they still tile seamlessly) and
re-compressed to DXT with mipmaps. A stopped build resumes where it left off.

From the command line:

```
cargo run --release -p atv-tool -- hd --upscaler /path/to/realesrgan-ncnn-vulkan            # menus and HUD
cargo run --release -p atv-tool -- hd --world --upscaler /path/to/realesrgan-ncnn-vulkan    # and 3D textures
cargo run --release -p atv-game -- --hd /some/pack/folder                                   # use another pack folder
```

## Format notes

See `docs/formats.md` for everything reverse-engineered so far, including
what is still unknown.
