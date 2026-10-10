# ATV Quad Power Racing 2: PC rewrite

A from-scratch PC version of the 2003 Xbox game, written in Rust with Bevy.
**No game data is included or distributed.** Everything is loaded at runtime
from the player's own copy of the game: the Xbox disc image (`.iso`/`.bin`)
or a folder the disc was extracted to.

## Crates

| Crate | What it does |
|---|---|
| `atv-formats` | Engine-free readers: Xbox disc filesystem, "Bark" archive + LZSS, BOG textures (DXT1/DXT5, swizzled ARGB, 8-bit palettized), ROM 1.26 models (meshes + Bezier patches), ROW 2.25 track worlds. |
| `atv-tool` | Command line: `list`, `extract <dir>`, `check` (parses every texture, model and track). |
| `atv-viewer` | Bevy viewer: loads a track with its props and sky, and a fully assembled quad on the start grid. |

## Running

```
# Point at your own disc image (or extracted folder)
export ATV2_GAME="/path/to/ATV Quad Power Racing 2.bin"

cargo run --release -p atv-tool -- check
cargo run --release -p atv-viewer -- --track Dunes1 --quad Bandit
```

Viewer controls: WASD to fly, Q/E down/up, Shift for speed, hold the right
mouse button to look around. Other options: `--camera x,y,z,tx,ty,tz` (game
coordinates), `--screenshot out.png` (saves a frame and exits).

Tracks: Dunes1-3, Forest1-3, Glacier1-3, Industrial1-3, Swamp1-3, Stadium1-2.
Quads: ATC, Bandit, Blaze, Bullet, Challenger, Charger, Dimension, Nitro,
Rocket, Skyline, Storm.

Linux build dependencies: `libasound2-dev libudev-dev libxkbcommon-dev
libwayland-dev` (standard Bevy requirements).

## Format notes

See `docs/formats.md` for everything reverse-engineered so far, including
what is still unknown.
