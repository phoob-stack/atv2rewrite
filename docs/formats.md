# ATV Quad Power Racing 2 (Xbox): disc and file-format catalog

Step 1 of the PC rewrite plan. Everything here was read from the player's own disc image
(`ATV 2 Rewrite/Xbox - ATV Quad Power Racing 2.bin`). No game data is copied into this
project; the scripts in `/mnt/project-files/tools/disc/` regenerate it from the image in ~20 s.

## How to unpack

```
python3 tools/disc/xiso.py  "<image>.bin" disc/            # Xbox filesystem -> disc/
python3 tools/disc/bark.py  disc/ATV.bah > bark.txt          # archive index
python3 tools/disc/extract.py bark.txt disc/ATV.bad assets/  # 1,931 files, ~190 MB
```

## Layer 1: disc image

- Plain 2048-byte-sector XISO (no video partition, image is only 158 MB). The `.cue` is a
  single MODE1/2048 track, so the `.bin` is the ISO directly.
- Header `MICROSOFT*XBOX*MEDIA` at 0x10000; root dir sector/size at +20.
- Directory entries are binary trees: `u16 left, u16 right (in dwords), u32 sector,
  u32 size, u8 attr (0x10 = dir), u8 namelen, name`.

| Path | What it is |
|---|---|
| `default.xbe` | Game executable. The reference for reverse engineering logic, physics and any format detail we can't infer. |
| `user.xml` | Debug/boot config left on disc (track, laps, AI count, editor mode). Useful for testing. |
| `ATV.bah` / `ATV.bad` | "Bark" archive: index + data, holds almost all game content. |
| `Music/*.wav` | 19 music sections, RIFF WAV, Xbox IMA ADPCM (format 0x69), 44.1 kHz, stereo except the mono `Stadium.wav`. 36-byte blocks per channel decode to 65 samples. **Decoded** by `wav.rs`, sample-exact with ffmpeg; `Setup/Sound.xml` says which play on each environment. |
| `Movies/*.xmv` | Acclaim/Climax intro videos (Xbox XMV, ffmpeg can decode). |
| `Fonts/blimey.ttf` | Standard TrueType font. |
| `Sound/dsstdfx.bin` | Xbox DSP effects image. Not needed on PC. |

## Layer 2: Bark archive (`ATV.bah` + `ATV.bad`)

Index (`.bah`, little-endian):
- `"Bark 1.06\0\0\0"`, then `u32 4, u32 0, u32 17 (root subdir count), u32 0, u32 0x7d0200`.
- Directory record: `u32 namelen, u32 nfiles, u32 nsubdirs, u32 0, name[namelen]`, then
  its files, then its subdirs (recursive).
- File record: `u32 namelen, u32 offset, u32 stored_size, u32 raw_size, u32 hash, name`.
  Offsets are contiguous into `.bad`. The hash is a content hash (identical files share
  it) but it is not plain CRC32; not needed for loading.

Each blob in `.bad`: `u32 method, u32 raw_size, data`.
- method 0: stored.
- method 1: classic Okumura LZSS. Flag byte read LSB-first, 1 = literal. Match = 2 bytes:
  `pos = b0 | (b1 & 0xF0) << 4`, `len = (b1 & 0x0F) + 3`. 4 KB ring buffer, zero-filled,
  write position starts at 0xFEE. Verified: all 1,931 files decode to their exact sizes,
  and text files decode cleanly.

## Layer 3: content formats

Counts are files in the archive (Animations 212, Cars 290, Characters 98, FrontEnd 284,
Levels 856, shaders 75, other 116).

| Ext | Count | Magic | Contents | Status |
|---|---|---|---|---|
| `.bog` | 811 | `BOG 1.01` | Texture. 785 wrap a standard DDS (551 DXT1, 234 DXT5); 26 are raw: format 32 = swizzled ARGB8 (shadow/bump maps), format 64 = swizzled 8-bit with a 256-entry palette (sky domes, tyre overlays). | **Decoded, all 811.** |
| `.rom` | 330 | `ROM 1.26` | Models: vehicle parts, wheels, riders, props, sky domes. Triangle meshes plus bicubic Bezier patches. | **Decoded, all parse exactly.** See below. |
| `.shm` | 141 | `ROM 1.26` | Shadow/simplified mesh twin of each `.rom`, same layout. | **Decoded.** |
| `.row` | 17 | `ROW 2.25` | Track world: materials, locators (start grid, checkpoints, respots, water boxes), lights, prop instances, sprites, meshes and the road surface as Bezier patches. | **Mostly decoded.** Spatial tree between mesh blocks still unknown. |
| `.kdt` | 17 | none | The original's track collision: `u32` triangle count, then 0x5C-byte triangles (face plane, three edge planes, padded bbox, `u32` surface id into `Setup/Materials.xml` order), then a spatial tree. Layout in `kdt.rs`. | **Triangles decoded, all 17**; their surfaces give the rewrite's collision grip and drag. Tree not read. |
| `.hsc` | 35 | none | Knock-over prop hulls (cones, signs, barriers, planks): `u32` hull count; per hull a `u32` plane count and planes (normal, d; inside where n·p + d <= 0); then a `u32` sphere count and spheres (centre, radius), read at `0x24c10`. The signs are 6-plane boxes, one per part. | Layout known; not used: the rewrite knocks `FREE` props over using an upright cylinder from each model's bounding box. |
| `.aid` | 17 | `AID 1.02` | AI racing line: a graph of waypoints with the drivable corridor, speed limits, respot and jump flags. | **Decoded, all 17.** See below. |
| `.nin` | 17 | none (`0xCDAB0DF0` markers) | Per-track node/trigger data. | To do. |
| `.viz` | 17 | none | Visibility / camera zones. | To do. |
| `.cam` | 17 | `CAM v1.00` | Tiny camera header. | To do. |
| `.roo` | 1 | `OBJECT` | Object placement. | To do. |
| `Levels/*/World/*.xml` | | XML | Object placements (cameras, triggers, positions, matrices). | Readable text. |
| `.bas` | 48 | `BAS 1.02` | Rider skeleton in its bind (T-) pose. | **Decoded.** See below. |
| `.bac` | 121 | `BAC 1.03` | Rider animations (riding leans, celebrations, crashes, idle). | **Decoded** (117; the four `*_bike` stunt files and `punch_floor` have another layout). |
| `.cuban` | 91 | `CUBAN 1.02` | Compressed or curve animations (crashes). | To do. |
| `.skel` | 1 | text | Rider skeleton definition (31 bones, mirror pairs, two-bone limbs). | Mirror pairs read. |
| `.pts` | 11 | `POINT 1.00` | Named vehicle hard-points (wheel centres etc.). | Simple, to do. |
| `.params` | 12 | text | Vehicle physics tuning (torque, gearing, preload, air resistance). | Readable text. |
| `.bed` | 35 | binary | Sprite sheets for `.bog` texture pages: HUD, front end, particles, bitmap fonts. | **Decoded, all 35.** See below. |
| `.ink` | 45 | `0x00CEFDAB` | Static prop collision mesh: `u32` triangle count, then triangles of three `f32[3]` corners in model space. A placed prop whose model has one (rocks, trees, buildings, vehicles, tyre stacks, the stadium gates) collides as it. | **Decoded, all 45**; used for prop collision. |
| `.xwb` | 9 | `WBND` | XACT v1 wave banks: engine, wheel and impact sounds (`Vehicles`), rider voices, menu effects (`SpotFX`), the stadium crowd and one ambient loop per environment. All mono PCM. Layout in `xwb.rs`. | **Decoded, all 9**; engine, wheel, ambient and crowd sounds play. |
| `.asm` / pixel shaders | 75 | text | Xbox vertex/pixel shader source. A reference for rewriting effects in WGSL. | Readable text. |
| `Setup/*.xml` | 13 | XML | Game rules: tracks, career, arcade, freestyle, tricks, riders, vehicles, cameras, materials, sound, front-end screens. | Read by `setup.rs` (flat entries). |
| `Strings/*.txt` | 2 | text | UI strings (English, plus a joke "l33t" language). | Readable text. |

## Content inventory

- **Tracks:** 17 races across 6 environments (Dunes 1-3, Forest 1-3, Glacier 1-3,
  Industrial 1-3, Swamp 1-3, Stadium 1-2), plus shared per-environment model/texture sets.
- **Quads:** ATC, Bandit, Blaze, Bullet, Challenger, Charger, Dimension, Nitro, Rocket,
  Skyline, Storm. Each is built from parts (body, handlebars, wishbones, mudguards, trail
  arm, shock, exhaust) as separate `.rom` files, so suspension can be animated per part.
- **Riders:** 6 female and 18 male character models sharing one skeleton.
- The engine is Climax's multi-platform engine: the data also contains GameCube and PS2
  controller art, so formats are platform-neutral apart from the textures.

## ROM 1.26 models (decoded in step 2)

```
0x00 "ROM 1.26\0" + 3 pad
0x0C u32 materials, locators, points, sprites, 0, nodes
0x24 i32 -1, -1, f32 -1, bbox_min[3], bbox_max[3]
0x48 materials  0x8C each (name[32] = texture name, rest is editor junk)
     locators   0x7C each: name[64], pos, scale, 3x3 rotation
     points     0x2C each: name[32], pos  (wheel centres, pivots, lights...)
     sprites    0x68 each: kind[32] ("faceme"), material[32], 10 floats
     nodes:     i32 header[5], u32 tris, u32 patches, u32 0, u32 verts,
                i32 -1 x3, u32 idx[tris*3], vertex[verts], patch[patches]
vertex (56 B): pos3, normal3, uv2, 4 floats (overlay uv2 + 2 unused),
     u32 ARGB, u32 bone (riders; 0 elsewhere)
patch (672 B): u32 uv_full, uv2_full, uv3_full; X[16], Y[16], Z[16]
     (4x4 Bezier control points, SoA); u32 0; uv, uv2, uv3 nets at words
     52, 84, 116 (16 pairs, corners-first like track patches; only the 4
     corners are valid when the flag is 0); u32 colour[4] at word 148;
     i32 bone[16] at word 152 (one per control point in riders, -1
     elsewhere).
```
Quad paint atlases (`Tpage*`) and `GEN_Tyre` keep a gloss mask in alpha
(shiny plastic opaque, seat and rubber clear; the tyre tread pattern);
it is not coverage. `GEN_components` alpha is real cut-out coverage.
Node header[1] is the base material, header[2] the overlay material (mud,
tyre dirt), header[0] probably a render group. Vertex colours are baked
light with 0x80 = 1.0 (Xbox MODULATE2X); vertex alpha drives effects, not
coverage. Quads are assembled from parts placed at the body's named points
(`handlebarscentre`, `flsuspcentre`, `rwheelcentre`...); wheels live in
`Cars/Generic` and the right front wheel is the left one mirrored.

The overlay layers are dirt that builds up during a race:
- `GEN_quadmud_01` on the quads, `CH_quadmud_01` on the riders, and
  `UNI_TyreOverlay` on the tyres.
- `Tracks.xml` swaps in the sand or snow versions per track:
  `mVehicleDirtTexture` for the dirt, and `mTyreType` (`Road`, `Sand`,
  `Snow`) for the `road_`, `sand_` or `snow_` wheel models and the tyre
  palette (`UNI_TyreOverlay_{Mud,Dust,Snow}Palette` colours, under
  `UNI_TyreOverlay`'s alpha).
- The `ShinyThingWithDirt` pixel shader mixes the dirt in by its texture
  alpha × vertex alpha × the lighting alpha. The vertex shader takes that
  last value from the ambient colour constant (`c5.a`), which is how the
  game feeds in the dirt level. Dirt also cuts the specular.
- Rider vertex alpha is all zero and `CH_quadsnow_01` has no alpha. The
  port gives rider dirt the mud texture's coverage.
- The port's dirt level grows with distance driven on the ground: full
  after 1.5 km. That rate is a guess.

## Riders: BAS skeletons, BAC animations

```
BAS: "BAS 1.02\0" + 3 pad, u32 bones (31), then 32 B per bone:
     f32 offset[3] (from parent), f32 quat[4] (x,y,z,w; identity),
     u32 child_count  -- bones are depth-first, so counts give the tree
BAC: "BAC 1.03\0" + 3 pad, u32 bones, u32 frames, then bone-major keys,
     28 B each: f32 pos[3], f32 quat[4] (x,y,z,w), parent-relative
```
Tree: 0 pelvis; 1-3 spine; 4-6 neck/head; 7-14 left arm (7 clavicle,
8 upper arm, 9 forearm, 10 wrist, 11-12 hand, 13-14 shoulder); 15-22 the
right arm likewise; 23-26 right leg (hip, knee, ankle, toe); 27-30 left
leg. Left is -X. All riders share the tree and differ in bone lengths;
the mesh is in the bind pose (a T-pose around the pelvis) and rigidly
skinned: one bone per vertex or patch control point. A BAC root key is in
quad-model space (every quad has the same saddle, handle and footpeg
points, so the riding clips fit them all); the riding clips
(`Generic/Bac/leen_*`, `lean_side_right`, `turn_right1`, `powerslide`,
`landing`) start from the seated pose and end at a full lean.

## ROW 2.25 track worlds (mostly decoded in step 2)

```
0x00 "ROW 2.25\0" + pad, u32 fog colour
0x10 u32 materials, 0, locators, points, lights, instances, sprites, 0,
     effects, unknown
0x38 materials 0x8C; locators 0x7C; points 0x2C; lights 0x4C (name[32],
     type, 0, pos, dir, rgb 0..255); instances 0x80 (flags[32] "FREE" =
     knockable, model[32] "name.clm" -> name.rom, pos, scale, 3x3, ARGB
     tint); sprites 0x68; then effect records (sea, river, mist) and a
     spatial tree interleaved with mesh blocks.
mesh block: u32 flags, i32 material, i32 material2, i32, u32 tris,
     u32 verts, u32 patches, u32 0, i32 x3, idx, vertex (52 B, no pad),
     patch (604 B, colours at word 147, no adjacency tail)
vertex:     pos, normal, uv, uv2, 2 floats, u32 ARGB
patch:      u32 uv_full, uv2_full, uv3_full; X[16], Y[16], Z[16];
            three 16-pair UV nets at words 51, 83 and 115; ARGB[4] at 147
```
Mesh flags: 0x1 has patches, 0x2 alpha-tested (foliage, fences), 0x2000
water (and some shadow decals, with 0x400), 0x4000 foam/wake, 0x8000 baked shadow decal. Material -1 is
invisible collision-only geometry. `material2` is an overlay layer (grass
strips, tyre tracks, the checker on inflatable arches) drawn over the base
with the second UV set, weighted by vertex alpha.

Patch UV nets list the four corners first (control points 0, 12, 15, 3),
then the inner points of the 0-12 edge, the two inner columns, and the
inner points of the 3-15 edge. A net whose `*_full` flag is 0 holds only
its corners; the rest of the record is stale memory. Corner colours
follow the same corner order. Shadow decal textures (`*_shad`, `*shadow*`)
mark shadow as bright texels on black, or in alpha.
Locators named `Start1`..`Start8` are the 2.5 m grid boxes; `Checkpoint N`,
`Respot`, `waterbox` and weather zones (`snow .. mist .. tint ..`) are boxes
too. `Startpoint 1` is the start/finish line and is checkpoint 1: 0x65c50
reads the number after the first ten letters of either name, so a lap runs
from the line through `Checkpoint 2` .. `Checkpoint N` and back to it (no
track has a `Checkpoint 1`). Each is passed when the quad's hull touches
the far (+Z) face of its box (0x64440, 0x64ac0), anywhere across it; a few
numbers have two boxes (Swamp 1's 6 on two routes) and either counts. The
rewrite also takes a checkpoint within 25 m of its middle, and computer
riders only use that. A `Respot` box (any locator whose name starts
`Respot`, any case, 0x65c50) is a zone a racer is put back from the moment
they are in it (0x910f0, 0x652d0); Dunes, Forest and Glacier 2 and 3 have
them (Forest 1 and 3 eleven each). A `waterbox` (matched case-insensitively, 0x101030) is deep water:
its top, `pos.y + scale.y / 2`, is the surface, and anything over its
footprint (`scale.x` by `scale.z` along the first and third rotation
rows) counts, however far down. Forest's tops match its water meshes to
0.1 m. Elsewhere they are placed by hand: measured at each box's centre,
Dunes' and Swamp's run from 4 m over the water mesh to 13 m under it, and
Stadium 2's pool box is 0.8 m under the water you see. Mesh blocks are
currently found by validating candidate headers, which recovers 87-98% of
each file's bytes; decoding the tree will make it exact and will give the
visibility structure.

## BED sprite sheets and the 2D art

```
u8 1, u8 1, source TGA name (NUL-terminated)
u16 width, u16 height      the source TGA's size in pixels
u16 count
count x { u16 n, n x (i32 x, i32 y), name (NUL-terminated), i16 link }
```

Coordinates are normalised to the page, 0..100000 across its width and
height, so they hold whatever size the `.bog` was saved at. Named entries
are 4-point rectangles; `link` points at an unnamed 1-point entry holding
the sprite's pivot (the rev needle's hub, a glyph's baseline origin) or is
-1. Fonts name each glyph by its character code in decimal (`65` = `A`,
Latin-1 for accents), every glyph linked to its baseline origin.

What the UI uses:

| Page | Contents |
|---|---|
| `Sprites/HUDTPAGE` | Rev gauge: `DIAL` (pivot = hub), `NEEDLE` (pivot = its hub end), `PRELOAD` arc drawn into the `DIAL_PRELOAD` slot, `BOOST` pie into `DIAL_BOOST` (filled clockwise from 12 o'clock by the boost meter, its alpha throbbing as the sine of a phase that runs at 2 rad/s and wraps at pi; 0xa2a00 / 0xa2ee0), `WEIRD_DOT` hub cap. Dial face reads 1-8 (x1000 rpm) from -134 deg to +21 deg, and KPH. `HUDTPAGE_b` is a larger, unused gauge with no sheet. |
| `Sprites/321go`, `Sprites/positions` | Countdown digits and GO; position balls. The sheet names P1-P8 but only P1-P4 have art; the P5-P8 cells are empty. |
| `Sprites/Position_meter` | Lap progress line, chequered flag, markers and the CHECKPOINT banner. No sheet; regions measured from the page (see `ui.rs`). |
| `Fonts/*` | Bitmap fonts: `MgrammeCombined` (outlined capitals, menus), `MgrammeSmall`, `kim36` (chrome mixed case), and `MgrammeInside` / `MgrammeOutline`, the fill and the outline of `MgrammeCombined` as white masks with the same layout. The executable's font table (0x1ced6c) holds `Font32`, `MgrammeInside`, `MgrammeOutline` and `MgrammeSmall`; text at a scale above 1 is drawn as the outline in one colour under the fill in another (0x44170, 0x47910). `Fonts/blimey.ttf` on the disc is a TrueType font. |
| `FrontEnd/NewDesign` | Menu selection bar, attribute bars, selection square, arrows, medals, panels. |
| `FrontEnd/BG/*` | Full-screen backgrounds; `Setup/Frontend.xml` names each screen's (`BACKGROUND0`) and its title (`mTitleText`). |
| `FrontEnd/RiderMugs` | 24 portraits, 8 per row of 64 px, in `Setup/Riders.xml` order. |
| `FrontEnd/Track_*_LARGE` | Track previews, also the race loading screen's picture. `LoadingOverlay` (an 8x256 strip) is that screen's bands; `FELoading1` is a plain loading screen. |

`Strings/Strings_ENG.txt` is one string per line with no keys: the keys are
in the executable (575 of them from 0x1cf9a8), and a string's id there is
its 0-based line in the file (0x9f is `Dunes1`, "Lagoon"). The UI mostly
uses the same English labels directly; the loading screen and the
challenges read lines from the file.

## AID 1.02 racing lines

`Levels/<Track>/World/<Track>.aid`, read by the retail loader at `0xf24b0`
(base class) and `0x640e0` (the AI's fields). Header: `"AID 1.02"` padded to
12 bytes, `u32` node count, `u32` record size (152); records start at 0x14.

| Offset | Type | Field |
|---|---|---|
| 0x00 | char[32] | name (empty; the tail is stale editor memory) |
| 0x20 | f32[3] | position (game frame) |
| 0x2C | f32[3] | forward (direction of travel) |
| 0x38 | f32[3] | up |
| 0x44 | f32 | left corridor edge along the node's right vector (negative) |
| 0x48 | f32 | right corridor edge (positive) |
| 0x4C | f32 | unused, a stale copy of the left edge |
| 0x50 | i32[4] | previous nodes (-1 = none) |
| 0x60 | i32[4] | next nodes; two at a split around a shortcut |
| 0x70 | f32 | speed limit, m/s (0 = none) |
| 0x74 | u32 | flags: bit 0 = a respot may use this node (clear on jumps and in water) |
| 0x78 | u8 | jump boost, 0-100: how hard the AI hops off this node |

Dunes 1's main loop (following first links) is 2425 m, the `mLength`
`Setup/Tracks.xml` gives it. Some alternative
routes pass up to 150 m wide of a checkpoint locator (Swamp 1 misses
checkpoint 4), so the rewrite lets a computer rider's lap skip one
checkpoint. The driver that follows the lines is in `atv-sim/src/ai.rs`,
with the original's addresses.

The line also keeps watch over the racers (`atv-game/src/respot.rs`).
Each checkpoint box takes the stretch of line nearest the middle of its
face, and each node is given the checkpoint it leads to: walking back
along first links from each checkpoint's stretch to another's, box by box
in locator order, then nodes off those walks taking the checkpoint of the
node they lead to (0x65230, 0x642b0). Every frame a racer's stretch is
found near the last one (0x659c0), or over the whole line when they are
three corridor widths out (0x64a40), by sideways distance alongside a
stretch or distance to its nearer end (0x64310). Then, in races only (not
freestyle or challenges, game types 4, 5, 8 and 9) and with the rider on
the quad (0x910f0, 0x908f0):

- More than 10 m outside the corridor for 5 s puts the racer back, with
  "Return To Track - n" from the first second (n the seconds left, rounded
  up; strings 0xe0, 0xdd, 0xe3, shown by 0x902f0). The original measures
  from the corridor's middle but against its edges (0x8fba0), so a
  lopsided corridor reaches further on its wide side.
- A stretch whose ends lead to neither the next checkpoint nor the one
  passed last, for 4 s, puts the racer back, with "Missed Checkpoint - n"
  from half a second.
- Facing back down the stretch for 2 s shows "Wrong Way" (no respot).

The respots and the two countdowns need `mRespotsOn` (1 in the disc's
`user.xml`) and the track's `mAllowsForcedRespots` (all but the Stadium).
A respot puts the racer on the line at the furthest point they reached
on a stretch inside one checkpoint's run whose two nodes allow a respot,
not the next checkpoint's own stretch (0x90400, 0x8fd30), on the nearer
ground over or under it, facing down the line; before the first such
point, back down the line to the nearest node that allows one. Every
respot in a race goes there, after a crash, stuck upside down or on the
rider's say-so too (0x90ac0, run half a second into the fade 0x25f20).
With any other racer within 10 m of the point (0x1c3160, squared), racer
i of n goes i/n of the way across the corridor from its left edge, so
player 1, the first, goes to the left edge. Asked for, a respot first
fades the racer's model out over half a second (state 1 at racer +0x2c,
alpha twice the time left, set as the model's colour at +0x106c), puts it
back (state 2), then blinks it back in over three seconds (state 3: alpha
`(sin(34π × t/6 + 3π/2) + 1) / 2`, 0x26050 ends it); asking again before
it's over does nothing, and the respot counts as it's asked for (racer
+0xac). The racer's shadow fades out with it and back in steadily over
the three seconds. The rewrite only watches player 1, and draws its quad
and rider only while that alpha is at least a half, since its textures
keep gloss in their alpha; it has no racer shadows.

The original paces its computer riders (0x69950, 0x6a3f0, 0x6a970): "pace
cars" lap the track at `mAIBestLapTime`..`mAIWorstLapTime` from
`Setup/Tracks.xml` (one car per three racers, plus one), the riders ranked
by ability, technique and their quad's top speed, acceleration and
handling (0x25ab0) follow the cars best first, and a rider's target speeds
scale by `1 - lead / 100 m`, kept to 0.5-1.5. A car with a player near it
is pulled up to 600 m towards them. The cars set off level with the
best-rated rider's place on the grid (0x6a0c0), and the rewrite counts
distance from the start/finish line (the first checkpoint), not node 0,
which on Glacier 2 is 900 m round the lap. Passing a jump node at 40 mph
or more, a computer rider releases a preload of strength `boost / 100`
times how well its travel lines up with the line (0x66380, 0x66460,
0x5a9e0). The rewrite adds two respot rules: a rider respotted three
times in a row without getting past the same spot is put past it (the
frozen creeks on Glacier 2 and 3 catch riders that come up to the jumps
too slow), and one that gets no further round the lap for 20 s is
respotted. It also lets a rider run at full pace for the last second
(at least 10 m) before a jump node, so slowed riders still clear the
jumps; with no such window Swamp 2 riders fall short often, with a 4 s
one they lap Swamp 2 in 124-138 s against the original's 157-163. The rider skill setup (0x66200, applied
at 0x681d0) makes an AI quad's brake (and handbrake) pedal instant, scales
`SIDEFORCEMAX` by `1.15 (1 + Ability/10)`, sets `TORQUESLIDESCALE` to
`0.75 / (1 + Ability/10)` and `FORCEPOINT` to 0; steering and engine are
left alone.

## Game-mode rules (Setup/*.xml)

- `Arcade.xml`: `RoundN TrackName=` in order (16 rounds, Dunes 1 twice).
  The original races fifteen: `ArcadeSM` (0x95e70) brings up
  `Arcade_Complete` after the fifteenth win (0x96070).
- The profile's championship (`CareerData`, saved at 0x11cb0):
  `mChampionshipType` (Amateur, Qualifying, Professional, Custom),
  `mRound`, `mTrackCount`, `Track%i`/`TrackLaps%i` (17 slots), then
  `ChampInfo%i` per rider (0x119f0: `VehicleType`, `RiderType`,
  `ChampionshipPoints`, `Rank`, `LastRacePos`). The profile has only this
  one: Multiplayer Championship (`CustomChampSM`, 0x97850) resets it and
  makes it Custom, and the Custom status page never saves by itself; the
  profile holds it until its next save. Career's Continue (enabled by
  0x121a0 for any type with a track left) restores the first rider's
  rider and ATV (0x12470) and races the rest as computer riders.
- Arcade's field (0x1dd200, saved in the profile as `mRound`, `Player%d`
  and `mDropOuts%d` at 0x11470): a new game seats the player at the back
  (0x11530). After a win the grid is the finishing order and the last three
  are out (0x115b0); three riders replace them at the back, picked by
  0x9c090 from the computer riders ranked by skill rating: the one at the
  next round's difficulty (round / 14, times 1.4 below 0.6, 0x11130), then
  one below, one above and so on, skipping riders in the field and those
  who went out in the last two rounds. `Arcade_Status` (page 0xb, 0x38ee0)
  lists 0xc8 "Grid Positions" as `%d. %s` (the player and new riders in
  their own colours) and 0x19c "Ranking" (the certificate a rider's skill
  rating reaches, 0x88470), then `%s: %s` 0x16f "Out" rows.
  `Tracks.xml` gives each track's `mArcadeStartTime` (40 s everywhere) and
  `mAIBestLapTime`/`mAIWorstLapTime`. An arcade round is one lap
  (`ArcadeSM`, 0x95e70, sets the lap count to 1). The time-extension gates
  are the checkpoints named with seconds after `Arcade`
  (`Checkpoint 3 Arcade 30`, 10 to 30 s): 0x65c50 reads them with `atol`
  into the checkpoint's +0x134/+0x138, and 0x648f0 adds them to the clock
  (`player+0xa4`, which starts at `mArcadeStartTime`, 0x7e610) once per
  pass, for the local player when not thrown off (0x27620). Every locator
  counts, so Swamp 2's doubled checkpoints give twice. The start line
  gives nothing. The original finds `Arcade` by case (0xe4900), which
  misses Dunes 3's and Glacier 3's `arcade 30` and Forest 3's `Acade 30`
  and leaves rounds 11, 12 and 15 too short to finish; the rewrite counts
  them.
- `Freestyle.xml`: `OpenFSRecords/TimeNMinute(s)` and `TimeUnlimited` each
  hold Bronze/Silver/Gold `Record=` scores for Open Freestyle; `FSChallenge`
  has `Challenge_1..12`, each with `mBronze/Silver/GoldBonus` and
  Bronze/Silver/Gold `Record=` times in seconds (lower is better), and
  `Unlockable0..11` (`mRider="1"`) give a rider for gold in challenge K+1
  (the bare `<Unlockable>` is ignored). All twelve run on Stadium 2: the
  course of challenge N is the `checkpoint` locators N*100..N*100+39 in
  `Stadium2.row` (lower-case, sometimes with a trailing space or words
  after the number, `checkpoint 600 endo box`; 1003 is there twice and
  either counts), passed in order (0x90bb0) and won at the last
  (0x99d50). A checkpoint is passed when the quad's hull touches the far
  (+Z) face of its box, `pos ± sx/2·R0 ± sy/2·R1 + sz/2·R2` (0x64440,
  0x64ac0). Every challenge starts at `start1` (0x86e00); the `startN`
  locators go unused. The clock counts up from the start with no limit
  (0x265a0), and the first respot fails the run (`player+0xac`, counted
  by 0x25f20 and checked by 0x803c0). The challenge goal (vtable
  0x1c40c0) ignores tricks, so the bicycle and endo challenges only want
  their gates, and the cone penalty never applies: 0x9ad90 gives props of
  type 0x11 a 5 s penalty (`prop+0x454`, taken once by 0x23750), but the
  `FREE n` cones are type 0x19.
  Props whose flags carry numbers after the first word (`FREE 1` cones,
  `GEN_Start_gate 1 2 3 5 6`, `GEN_stasignfloor 2 3`) are only out in those
  challenges; `GEN_stasignfloor` has no model on the disc. Challenge 12
  ("burst all the balloons") has no checkpoints or balloons placed. The
  original's medal test is strict (`time < record`), and a better medal
  pays its bonus once per tier climbed. The texts are `CHALLENGE_TEXT_N`
  in `Strings/Strings_ENG.txt`, which holds values only, in sorted key
  order (lines 92-103 for `_1`, `_10`, `_11`, `_12`, `_2`..`_9`).
  A challenge (game type 5) has no camera intro: the race start (0x801c0)
  instead opens `ID_FS_INTRO` (0x55bf0, entered by 0x7fcc0 when
  `mRaceIntro` is set and it isn't a replay) with the HUD hidden: the
  challenge's name (`Challenge_N` through the string table, 0x9ac60), its
  `CHALLENGE_TEXT_N` in a text box (0x9ac90) and a "Press Start" button
  (string 0x17c). The countdown follows once it closes (0x7fd70). The
  rewrite keeps the chase camera behind the waiting quad meanwhile; the
  original hands the view to the (empty, on Stadium 2) intro cameras.
- `Tricks.xml`: one element per trick with `mControlCombo="KeyAHeld,
  StickUp"` (buttons D, A, B, E for levels 1-4, `KeyCombo1/3/2` for 5-7),
  `mScorePoints`, `mHoldPoints`, `mTotalTrickTimeSec`, `mTrickEndTimeSec`
  and an `AnimData` child naming the stunt clip (`Anim Stunt Seat Grab`). The
  clips are `Animations/Stunts/Bac/*.bac` (31 bones, rider space, same as
  the riding clips); most names match with spaces dropped, the rest are
  Scarecrow = `crucifixtion`, Scissors = `leg_splt`, Spin Bike = `bike_spin`,
  Reverse Spin = `reverse_spin_grab`, Yogi = `Y`. `*_bike.bac` (1 bone) move
  the quad for the tricks that spin it. `Wheelie`, `Bicycle` and `Endo` give
  ground-trick points; `SpeedBoost` is the race boost tricks earn: a
  landed combo adds points x `TrickScoreMod` (0.0004) to a 0..1 meter
  (0x62040 -> 0x1b830); while the boost input (input slot 11) is held and
  the meter isn't empty the drive is multiplied by `BoostTorqueMod` (2.5,
  the physics' drive factor at +0x7e4) and the meter drains `BoostTimeMod`
  (0.1) a second (0x1b750); holding it also opens the throttle fully
  (0x1a9a0). A crash takes `CrashLoss` (0.2); `AttackLoss` and
  `AttackAwardAIMod` are for kicks: the kick (rider anim state 5, the
  `Kick` clip, mirrored for the left; 0x27eb0) lands on its 12th frame
  (0x2dc50) on any rider within 1 m of the kicker's position + (±1.2, 0,
  0.3) in its frame, and the victim (0x2a200) loses strength (1 if its
  `mStrength` is below the kicker's, 0.75 if equal, 0.5 if above; strength
  refills over Riders.xml's root `mStrengthRestoreTime`, 0x2cca0). It
  hands over min(meter, `AttackLoss`) of boost and flinches
  (`Hit_from_right`, mirrored for a kick from the left), or at no
  strength left is knocked off (crash types 3/4) and hands over all of
  it. The AI (0x630f0) kicks when its `mAggression` x 0.2 > 0.5 and a
  rider is within 2.2 m more than 45 degrees off its heading.
  `mLevel` (1-7, four tricks each) is the skill level a rider needs: the
  original refuses a trick above it (0x18439). A player's level comes from
  the profile's skill points (`Career.xml` `SkillLevelPoints`), another
  player's rider's from its `Riders.xml` `mSkillRating` (0x25590); Training
  is level 1. The trick ids, the Trick Book's order, are the xbe's name table
  at 0x1d05d0 (Scissors = 0 .. Surfer_Flip = 27). The book's pad picture is
  `FrontEnd/TrickPad_XBox`: `PAD`, `BUTTON0-3` the d-pad's up, down, left
  and right, `BUTTON4-7` Y, B, A and X, `BUTTON8/9` the triggers on the top
  view, and the `BUTTON` and `SHOULDER1` glows that light them.
  `FrontEnd/TrickPics` is a placeholder (one picture repeated).
- `TrickBoxes.xml`: per track, `TBOXn Name Points LinkedBox`. Only the
  stadiums have real ones; they are the `TBOXn` locator boxes in the
  `.row` (scale = half size), several boxes per id for hoops. Linked boxes
  are the two ends of a gap.
- `VehicleCameras.xml`: the follow cameras the Options menu offers,
  `FollowNear` (CAM_FOLLOW_CLOSE), `FollowFar` and `FollowVeryClose`, plus
  the replay mounts (`MountOnFront`, `MountOnWheel`, ...) and the
  lead-back cameras. Each follow camera has a `PosOffset` and a
  `LookOffset` (vehicle frame, Z forward) and `On/OffGroundPos/RotStiffness`.
  The children repeat `X`/`Y`/`Z`, so they are read with `setup::child`
  rather than flattened. Where the offsets are measured from is not known:
  taken from the centre of mass they look down on the quad at 20-37
  degrees and lose the horizon, so the rewrite aims 1 m above it. The
  rewrite eases the offset (not the position) at the position stiffness,
  so the camera lags turns but not speed, and turns the view at the
  rotation stiffness. Very Close sits at the rider's head, so the rider
  is hidden in it. `CameraDirector.xml` lists the replay director's
  camera sets.
- Options pages (`Frontend.xml` `Options*` screens): Audio has Music and
  Sound Effects volume as 16-segment bars (`FrontEnd/SoundBar`: `BAR_ON`
  over `BAR_OFF`); Camera picks one of the three follow cameras; Control
  Config shows `FrontEnd/BigPad_Xbox` (top and front views of the Duke pad;
  `BigPad_Overlay_Xbox` holds the leader lines for its labels) and a
  Vibration tick box (`FrontEnd/checkbox`: `OFF`, `ON`). Replays,
  Language and Credits pages exist too; the disc has no credits text.
  Profiles opens Available Profiles (see Profiles below).
- `Career.xml`: root attributes `mMaxSkillForAmateur` (7),
  `mNumUnlockables` and `mTimePercentage` (1.3); `SkillLevelPoints`
  (`SkillLevel1..7`), `CertificateSkillLevels` (Amateur 0, Professional 4,
  Superstar 7, Unknown 8), `PositionPoints` and `TrickPoints`
  (`Position0..7`, both 25/20/16/10/8/5/1/0), `TrackOrder` with an
  `Amateur`/`Open`/`Professional` child of `Round0..4` track names, and
  `RiderAvailability` with one child per rider (`Dana_Creech`) holding 0/1
  for `Amateur`, `Open`, `Professional` and `Custom`. `Unlockable*` entries
  with `mATV="1"` free the ATV in `mData` at `mCondition` gold medals.
  `Attrib_*` tables look unused. From the career code in `default.xbe`:
  each race of 8 ranks the field by finishing order and by trick score and
  adds both points tables; the player's trick score goes onto their skill
  points twice (once, capped at `SkillLevel[mMaxSkillForAmateur]`, in
  Amateur); a rider not home by `mTimePercentage` of the winner's time
  doesn't finish; a top-3 championship finish opens the next one
  (Amateur, then Open shown as Qualifying, Professional, Custom). The
  status screens are `Career_Status_Amateur`, `_Pro` and `_Pro2`, and
  `_Custom` (Current Standings).
- Custom championship (`CustomChampSM`, 0x97850; `mCustomUnlocked` in the
  profile's `Unlockables`): rider and ATV, then `Career_Custom_Tracks`
  (0x426a0 builds it, 0x42ea0 takes input) lists the race tracks the
  profile has unlocked, with a round and laps column. A adds the highlighted track as the next
  round (0x42530) or takes it out and moves the later rounds up (0x42570);
  Right on an added track edits its laps, Up/Down 1 to 20, and a new track
  takes the last laps set (1 at first). Continue needs one track and hands
  the rounds in order to the championship (0x42d80, 0x11730).
  `Career_Custom_Points_Distrib` (0x43210, 0x425d0, 0x43680) holds one
  value, 0 to 100 in steps of 5 (50 at first): each place's
  `PositionPoints` + `TrickPoints` total goes that share to Bonus Points
  (the trick ranking) and the rest to Race Points. It draws `PointsTpage`
  `HBAR8` (eight bars) with a `BLOB1` slider on each place's bar. The
  page is skipped while `mCustomUnlocked` is clear, which only matters
  for multiplayer, whose Championship runs the same state machine with
  two to four players (track choice included). Custom fields come from
  `RiderAvailability`'s `Custom` column. `Career_Custom_Rider_AI_Change`
  (Opponent Difficulty) is built but never opened.
- Multiplayer (no setup file; from `Frontend.xml`, the strings and the
  front-end code): Main menu `MULTIPLAYER` opens `Multiplayer_New_Game`
  (Single Race, Freestyle Battle, Championship), then
  `Multiplayer_Num_Players` (two to four), then rider and ATV select per
  player (`Multiplayer_SingleRace_Rider`/`_ATV`/`_Track`, and `_Freestyle_`
  and `_Career_` variants) with a `PLAYER n` banner;
  `RIDER_LOCKED_MULTIPLAYER` refuses a rider another player has. Two
  players split the screen top and bottom, three or four take quarters.
  `WAITING_FOR_REST` ("Waiting For Other Competitors") shows on a finished
  player's view.
  The multiplayer Championship has rider and ATV screens
  (`Multiplayer_Career_Rider`/`_ATV`), then runs the custom championship's
  state machine (`ID_MULTIPLAYER_MODE_CHAMPIONSHIP` starts `CustomChampSM`
  at 0x98520): Choose Your Tracks, and Point Distribution once Custom is
  open (see the custom championship above).
- Unlocks (the profile's `Unlockables`, saved at 0x12ed0): bit fields
  `mTracksUnlocked` (17 tracks: the 15 race tracks, then Stadium 1 and 2),
  `mRidersUnlocked` (24) and `mVehiclesUnlocked` (11), set at 0x12fa0,
  0x12fe0 and 0x13020. `Tracks.xml` `mAvailable="1"` marks the tracks open
  from the start (Dunes 1, Forest 1, Swamp 1); `Riders.xml` and
  `VehicleSetup.xml` `mAvailability` is `Unlocked`, `LockedHuman` (the ten
  pros; Bullet, Challenger, Nitro, Skyline, Storm), `LockedNeverAI`
  (Devil Girl, Troll) or `LockedBoth` (ATC). A track opens when an Arcade
  or Career round's status page comes up on it (0x95f60, 0x97050), and
  an Arcade round won opens the next round's track with a "New Track"
  message (0x7e290; the results pop-ups at 0x53da0 also say "New ATV",
  "New Rider", "Rider Ranking Now" and "Skill Level Increased To %d").
  Track select (`ID_TRACK_SELECT_PAGE`, Single Race and Time Trial) shows
  a locked track with string 0x203 and won't start it; Choose Your Tracks
  lists only open tracks. Rider select hides riders 22 and 23 (the
  secret pair) while locked and shows the others with 0x1ac ("Earn gold
  medals in freestyle challenge to unlock pro riders"); ATV select gives
  0x1a ("Earn %d more gold medals in career mode") or, with no medal count
  for it, 0x1c. Freestyle.xml's rider rewards list Jasmin Plante twice and
  never Jeremiah Jones, so he only comes with a cheat.
- Cheats (0x1cef14, names to bits; applied at 0x18f20 and kept in the
  profile as `mCheat`): typed as a new profile's name instead of creating
  one. `ALLOFEM` 0x7e (all below), `ALLRIDERS` 0x2, `ALLBIKES` 0x4,
  `ALLTRACKS` 0x8, `ALLTRICKS` 0x10 (skill points to the maximum),
  `ALLCHAMPS` 0x20 (Qualifying, Professional and Custom open), `ALLCHALS`
  0x40 (all twelve challenges open) and `NOHUD` 0x80. Each shows the
  `ID_CHEAT_PAGE` box ("Cheat" and its own string, 0x6f-0x75).
- Profiles (ten slots, 0x16340; the frontend state machines at 0x9b430
  on): `Profile_Select` (`ID_PROFILE_SELECTION_PAGE`, page 0x2b) shows
  `FrontEnd/ProfileNumbers` (1-10, eight 64-texel tiles a row) as a picture
  bar, empty slots greyed, the name or 0xa3 "Empty Profile", and Skill
  Level, Skill Points, Ranking, Championship, Arcade (of 15) and Challenge
  (of 12); A is 0x1c2 "Select" or 0x89 "Create Profile". With no profile
  loaded the game first says 0x164 ("There have been no player profiles
  loaded...") in `ID_PROFILE_MSGBOX`. `Profile_Rider`
  (`ID_CREATEPROFILE_PAGE`, 0x2c) offers the twelve `Unlocked` riders,
  refusing one another profile has (0x1ae); `Profile_Name`
  (`ID_NAMEPROFILE_PAGE`, 0x2d) is `ID_NAME_ENTRY` on
  `FrontEnd/nameentrypanel` (`NAMEPANEL` keyboard, `SMALLNAMEPANEL` name
  box): rows `ABCDEFGHIJKLMN`, `OPQRSTUVWXYZ` and `1234567890-"*`, then Del,
  Space and Ok, up to 12 characters (0x51390). Arcade and Career have no
  rider select (no `ID_RIDER_SELECT_PAGE` for `Arcade_Rider`): they ride
  the profile's rider. Options lists Profiles between Replays and Credits.
- The race menu (0x4a5c0, `ID_RACEPAUSEDPAGE`): 0x1e Audio, 0x40 Camera,
  0x83 Controller, 0x1a8 Return to Race (focused as it opens), 0x1a7
  Restart Race (0x1a5 Restart Challenge, 0x1a6 Restart in freestyle) and
  0x187 Quit Race. The first three open `ID_PAUSE_SUBPAGE` pages (0x4a500,
  0x4a530, 0x4a560); going back returns to the item that opened them
  (0x4aa40, 0x491b0).

## Particles (Setup/Materials.xml, Sprites/Particles)

`Setup/Materials.xml` has one entry per ground type (28, read by 0x782e0
into 0x58-byte records). Besides grip it says what a wheel throws up:
`WheelDustType`/`WheelDustProb`/`WheelDustColour` (puffs off a wheel
turning fast), `WheelParticleType` (`Dirt`, `Gravel` or `Snow`, the chunk
sprites `Dirt0..2`, `Gravel0..3` or `Dust0`), `WheelParticleProb`,
`WheelParticleDustProb`, `WheelParticleColour`, `WheelParticleSize`.
Colours are stored as bytes and scaled by 1/128 when used, so 0.5 draws a
sprite as it is.

The particle manager (set up at 0x8e440, run by 0x8e380) keeps five pools
on `Sprites/Particles`: sparks (50, 0.3 s), chunks (600, 1.5 s, gravity
10, bouncing 0.9 off the ground they left), exhaust puffs (40, 0.3 s),
dust (80, 1 s) and slow smoke (240, 4 s). Each frame it rolls, per wheel,
a chance from the wheel's spray `1.8 v - tread speed x forward`, scaled
from nothing at 5 mph to full at 30 mph (0x8bf40), and per wheel on the
ground a dust chance from `min(speed / 50 mph, wheel rad/s / 100)^2`
(0x8b900). The exhaust (0x8b330) puffs at low speed and, while boosting
(the quad's flag at +0x1174), bursts dark smoke that thins out over a
second and spits three sparks a frame (0x8ada0). Computer riders' quads
throw half as much, and nothing is thrown past 50 m from the camera.
Wheels in water throw nothing.

Splashes (0xe6520, tuned by the `Splash_*` values at 0x1d35c4) go off
where a quad or a thrown rider comes down into deep water faster than
0.08 m a frame, on the water's surface. Strength is `min(1, 5 × drop per
frame)`. 32 `Splash` sprites (a pool of 64, life 2 s times
`(strength + 1) / 2`, gravity 6) leave from a ring up to 0.75 × the
radius out (the quad's reach to its outer wheel edges; 0.5 m is assumed
for a rider), going 1.2 m/s outwards plus 3 × the body's motion, and up at
`(3 + rand 4) × strength`. Each is 0.3-0.4 m, grows 1.5-2 m/s, spins up to
0.3 rad/s and fades out over 2 s from half opacity (`Splash Alpha Fade
In` sits in zeroed memory at 0x1e4474, so there's no fade in). Four rings follow at
the centre (`Ripple`, half-size 0.25 × radius, growing 1.5 m/s, at 0,
0.11, 0.22 and 0.33 s, opacities 0.25, 0.5, 0.375, 0.25), and 16 small
ones (growing 0.6 m/s, opacity 0.4) appear 0.3-1.3 s later up to
`8 × strength × radius` away. Ripples (0x1072c0, a pool of 256, updated
by 0x106f00 and drawn flat by 0x107010) wait out their delay, then grow
for 2 s; how they fade isn't worked out, so they fade evenly. A
rider treading water sends out three rings (growing 1.5 m/s, opacity 0.4,
0.05 s apart; 0xe69e0 kind 1) each time the loop starts again. The
original also rings a quad's wheels where they cross the surface (kind
0), but only while the quad is a metre under it, which a sunk quad hardly
ever is; the rewrite leaves that out.

## Replays and their cameras

The original records only the human riders' processed pad input: three
bytes a tick (steering X and Y in sevenths, throttle and brake, the
trick it detected, a few buttons) behind a `[00][u32 tick]` timestamp,
into a 256 KiB buffer (0xf6a10, packed by 0x87190), which fills after
about six and a half minutes. Playback feeds the bytes back in place of
the pad and runs the whole race again, AI riders included. There is no
rewind. The replay page (0x4ab00, handler 0x84400) has five icons: start
again, pause, play, save (`REPLAY0` in the save game `ATVREP`, 0x16870)
and back to the results. The results page offers "Replay" only to a lone
human rider (0x3f260). "Save Replay" and "Watch Replay" are unused
strings, and the main menu's replay loader (0x98870) is never reached.

`Setup/CameraDirector.xml` (loaded at 0x6f120) holds three styles
(`Realistic`, `Custom`, `AllCameras`; the default is `mDefaultStyle`). Each
has a `PrimaryCamera` and secondaries with an `mCameraType` and
`mSwapThreshold` (seconds). Only `mNumCameras - 1` secondaries are read,
so AllCameras' last one (On Front) never loads, and the list is built
backwards. The director (0x72be0, run by 0x72e50) cuts when the shot has
run 3 s and longer than the threshold of the camera it started with (in
practice always the primary's 8 s, since that pointer is never updated),
with the rider on all four wheels and not thrown off. Cuts alternate
between the primary and a secondary found by stepping 1 to 3 places along
the list (0x6ede0). A replay always follows the player's rider.
Once a lone human rider has finished, the race screen (0x82cd0) runs the
same director on the live race behind the results, and there it may move
to another rider once the current one has been watched 4 s (0x72d00):
first a computer rider from a scored table at 0x1deb58 (0x729f0, the
lowest value among the entries in play; not decoded), otherwise one at
random, and only if two checks at 0x27620 pass (taken here as neither
rider being thrown off). Moving resets the shot.
`mInactiveSwapThreshold` and `mPlayerSwapThreshold` are loaded but unused.

The camera types (table 0x1d0990, set up by 0x72ab0) are: 0 Trackside, 1
Follow High (offset 5, 15, 5 at stiffness 0.5), 2 Orbit Player (starts
1.5 m behind and up, or where the last camera was if within 10 m; the
pad's sticks turn it), 3 Follow Far Behind (`VehicleCameras.xml`
FollowFar), 4 Two Rider (10 m beyond the rider and 2.5 m up, on the far
side from the other rider), 5 Wheel (MountOnWheel), 6 Handlebars
(MountOnHandlebars), 7 On Front (MountOnFront) and 8 Low Behind
(MountLowBehind). The vehicle cameras are indexed by the name table at
0x1d09f4 (Front, Back, Handlebars, Wheel, LowBehind, Near, Far, LeadBack
Near/Far, VeryClose), not in the XML's order.

Trackside cameras come from the track's `World/<Track>.xml`: objects with
`ManipulatorType="Camera"` (`GUID`, `Position`, `CameraType`,
`CameraZoom`) and `ManipulatorType="Camera Box"` (`Position`, a `Matrix`
of rows, `Scale` as half-extents, and `NextGUID` naming the camera).
Each frame the first box holding the rider (`|row_i . (p - centre)| <
Scale_i`, 0x733f0) picks the camera (0x6b1b0). Outside every box the shot
drops to the quad's last vehicle camera until the next cut. The cameras
stand still and aim at the rider (0x73510): `CameraType` 0 as they are, 1
zooming so that `zoom = max(1, tan(fov/2) / 0.7 x f x distance / rider
size)`, with `f` picked in 0.4 to 0.75 at each camera, 2 the same with a
hand-held wobble (an aim error of up to 0.1 pi / zoom springing back at k
25, and a log-zoom kick of up to 0.2 at k 20), and 3 the wobble at
`CameraZoom`. Zoom divides the view's tangents (0x6d720). Type 4 cameras
(start and flyby paths) and `Camera Focus Point` objects are not
trackside cameras. `HeatHaze` is forced on in Dunes and Swamp.

`.cam` (`CAM v1.00`, a count, then 44-byte records ending in a position)
is named by 0x7b8e0 but nothing reads it. `.viz` holds visibility zones
for drawing (`u32 1, u32 0, u32 count`, then 64-byte `{id, centre, 3x3,
half-extents}` and per-zone lists, read by 0xca430), not cameras.

## The attract demo

`user.xml` on the disc sets `mAttractModeTime="30"` (the default too;
read at 0x18640 into the settings at 0x1dbb50) and `mDemoMaxTime="300"`
(0x1dbb7c). Press Start's page (0x98390) starts the demo state machine
(`DemoSM`, 0x98140, once via 0x540e0) when it has been up that long and
nothing has been pressed for as long. Its setup (0x97f90) picks a random
track whose `Tracks.xml` entry has `mAvailableForDemo="1"` (every track but
the Stadiums; `mAvailable` is not checked), or with `mDemoTrackRotate` steps
through them three at a time. It seats `min(mMaxPlayers, 8)` riders, all
computer-driven, with AI skill between 0.5 and 1 (0x9c2a0), and sets
`mHasCamera` on the first rider and, a third of the time, the second as
well, and then half of those times the third and fourth too: one, two or
four views. The race screen turns the TV director on (0x829c0) and opens
`ID_DEMOPAGE` (0x80ba0, built by 0x3c980): the big logo
(`Frontend_ATVLogoBIG`), "Press Start" (string 0x17c), the track's names
as "%s - %s" and, in some builds, "Demo (%d)" counting the demos. The demo
ends on a button press (0x80c30 takes buttons 0 and 2 of its input
events), when any rider finishes, or after `mDemoMaxTime` (0x80d20), and
the front end comes back.

The race (0x86530) gives each rider with `mHasCamera` a view of its own
(0x83460): two split top and bottom (left and right when 0x1dbc60, set
from the console's widescreen video flags, is 1) and four take a quarter
each, as in split screen. The demo page's enter (0x80ba0) turns the
director on for the first view only; the others keep their riders' race
cameras. The rewrite gives the extra views to the computer riders after
player 1's seat on the grid, always top and bottom for two, and hears
their quads as it hears split-screen players' (each view a listener).

## The race intro

The intro state (0x1c1b2c; enter 0x7dd80, update 0x7e750) runs before the
countdown when `mRaceIntro` is set (`user.xml`, 0x1dbb68) and the race
isn't a replay; it plays in the attract demo too. It switches off every
view but player 1's, makes that one fullscreen (0x83010), hides the HUD
(0xa4600) and hands the view to the intro cameras (0x82fa0). When the last
of them is done the player's camera comes back (0x83730) and the countdown
state follows; nothing skips it.

The intro cameras are the `World/<Track>.xml` `Camera` objects with
`CameraType` 4 to 6 (0x21460 sends them to 0x73180; all on the disc are
4, "P: Intro", 0x6d290, field of view 45°). They go in a table by `ID`
(ten slots, 0x72fb0), and the intro plays them from ID 0 up, skipping
empty slots (0x6ac70: Glacier 1 has IDs 0, 2 and 3), cutting to the next
once a shot is done (0x6aae0, 0x6cbc0). A camera's `NextGUID` names a `Path
Node` object: `NumNodes`, the first node at `Position` and the rest at
`NodePos1`... The path is resampled every `mFixedNodeSep` metres (5;
adjusted to divide the length evenly) and a spline fitted through those
points (0x77e00, 0x77fb0). Each shot (0x6c590 sets it up, 0x6cf00 runs it)
starts from its camera's own `Position` at rest and gains 6.704 m/s every
second up to 44.694 m/s (100 mph). First it runs a cubic Hermite from
there to the path's start, both tangents the path's starting direction
times the straight distance d between them, with its parameter moving at
speed/d (0x6c890); past the end the overshoot carries on along the path,
which is then followed by distance until it ends (0x6ca30). With
`NextGUID` 0 the camera holds still for 10 seconds; with a path that isn't
there the shot ends at once. All the while it looks at the `Camera Focus
Point` whose `mIndex` is its `FocusIndex` (`FocusIndex` 0: none, and the
camera keeps its `Matrix`, `Row2` forward). Focus points are built like
the cameras' movers (0x6cc30, with `mTrackVelFrac` scaling the top speed)
and could follow paths of their own (0x6cfd0), but none on the disc have
one. Every track has intro cameras except Swamp 3 and Stadium 2; the
whole intro runs from about 5 s (Glacier 3, Forest 2) to about 22 s
(Dunes 2, Industrial 1).

## The race loading screen

0x57d40 builds it. Its picture is the track's `Track_<track>_LARGE` over the
whole screen with `LoadingOverlay` stretched on top: a navy band to 73 (in
640x480 screen units), see-through to 238, a light grey band to 276, a dark
grey one to 390 and navy again from 403. On it go "%s : %s" with the area's
and the track's names (`Industrial` reads "Works"), centred on y 50 at scale
0.65; a racing tip's title on 258 at 0.6, black outlined in grey 0.7; the
tip's text in grey 0.7 outlined in black, wrapped in a box from x 32 to 608
whose top is 276, at 0.4 (0.3 for the `LONGTIP`s); and "Loading %s Game"
(0x108) with the game type's name (0x86410) on 430 at 0.4. The caption and
the mode line are blue (0.2, 0.4, 0.8) outlined in white. The tip (0x57bf0)
is one of the first 13 of a table of 14: `TIP_1` to `TIP_9` under `TIP`,
`LONGTIP_1` to `_3` under `LONGTIP` and `HINT_1` and `_2` under `HINT`, so
`HINT_2` never shows. A text's cap cell is 0.1035 of the font page times its
scale times 1.385 (0x479a0) in units where the screen is 2 tall, about 24.8
screen units per unit of scale; its glyphs are 0.9 times that wide across a
screen 2 wide (+0x38), so 1.2 times wider than their texels on a 4:3 screen.
The screen also lists the field: "Competitors" and "ATV" headers at x 150
and 330 on y 100, then a row per racer from 118, 15 apart, computer riders
in yellow and players in orange. The rewrite leaves the list out, since
outside Career and Arcade it picks the computer riders only as the race
spawns them.

## Track records

Each track (the 17 of the track enum at 0x1d0db0) has five records, at +0xec
to +0x14c of the track object, 0x18 bytes each: the value, the holder's name
(+4), and the rider and vehicle as indices (+0x14, +0x15). In order: Best Lap
(string 0xac), Longest Jump (0x10d), Biggest Air (0xcf), Best Wheelie (0x10e)
and Best Bicycle (0x10b) (0x4cd60). `Tracks.xml` gives their starting values
as each track's `Fastest_Lap`, `Longest_Jump`, `Highest_Jump`,
`Longest_Wheelie` and `Longest_Bicycle` (`Record`, `Name`, `RiderType` naming
a `Riders.xml` entry, `VehicleType`): 360 s, 10 m, 10 m, 5 s and 2 s on every
track, so the first lap anyone finishes is a record. The system save (0x12840)
keeps them in a `Track_Records` section, a tag per track holding
`TrackRecord%d` entries (0x13fe0); resetting copies the setup's back (0x13f60).
They show as a time, "%.1fM" or "%.1fS", or long as "%.1f metres" and
"%.1f Seconds" (0x4cdf0).

The results page (0x3ef60) calls 0x14660, which goes through the racers in
order and, for each human (vfunc +0x3c), takes their stunt stats: biggest air
(+0xf4) above record 2, longest jump (+0xec) above 1, bicycle (+0xc0) above
4, wheelie (+0xbc) above 3, and the best lap below record 0 except on track
0xf and 0x10 (Stadium 1 and 2). A record beaten takes the value, the rider
and vehicle, and the profile's name (DAT_001d86a0 +4), or with more than one
human that controller's player name (0x88820); a later racer can beat it
again. After the results, each human with records gets the record page
(0x4e050, `ID_RECORD_PAGE`): "Track record broken!" (0x15e), "Track record
broken by player %d!" in split screen (0x15d), or "Stadium record ..."
(0x15b, 0x15a) for game types 4, 5, 8 and 9 (the freestyle ones and
training), a row per record with its long value, then "Please enter your
name" (0x176) and the name entry, holding the name the record was set
with. OK (`ID_NAME_ENTRY_OK`, 0x4e160) writes the name into each of that
player's records. The state machine skips it for challenges and training
(game types 5 and 9, 0x8006e). It comes when the results' button is
pressed (0x7fe40 starts the record state for player 0), one player after
another (0x80890), and then the game carries on.

The stunt manager measures the jumps (0x61a00, run from 0x62b20 while the
racer isn't crashed). With no wheel touching it takes off: the quad's x and z
then, and the height reset. When a wheel touches again the length (+0xf0) is
the distance across the ground from there. In the air the height (+0xf8) is
the most of the vehicle's +0x1168, which 0x1b6c0 sets to the ground's
distance straight down from the body within 50 m (`FLT_MAX` past that, and
then it's left out). With all four wheels down, the longest jump and biggest
air take the length and height if they're more. A wheelie, bicycle or endo
that scores (a second or more, 0x614f0) sets +0xbc, +0xc0 or +0xc4 to its
time if that's more.

The track select (0x519d0) shows the highlighted track's records on three
panels at the screen's left edge (x -1), 0.6 wide and 0.3 tall, placed at
y 0.15, -0.15 and -0.45 less 0.15 (0x51920, 0x4cc50), each a box in
(0.15, 0.27, 0.27)
holding the holder's mug (`FrontEnd/RiderMugs`), the record's name, its long
value and the holder's name, fitted to the box (0x4d360, 0x4ced0). Each has
all five records in turn (0x4dfa0): the first from the best lap, the second
from the longest jump and the third from the biggest air. Every 2.5 s
(+0xe4) a panel turns over to its next one, the second and third starting
0.1 and 0.2 s on (+0xe8, 0x4e760). The rewrite stacks them right of the
track's picture, since its track list is on the left.

The rewrite keeps the records in `records.cfg` in the config folder and
lists those broken among the results too. Its results and race menu are one
screen, so the record page comes when an item on it is picked, and that item
goes ahead after the last page. A record is first set with the profile's
name, or the rider's when there's no profile or in split screen. That name
stands if the box is left as it is or emptied. The rewrite also leaves out a
jump that ends in a crash or a respot.

Open Freestyle's medal targets are records as well. The system save's
records hold, for each of the seven time limits, three entries of the same
0x18-byte shape, best first (0x17e70: +4, 0x18 each, 0x48 a limit):
Gold, Silver and Bronze. They start from `Freestyle.xml`'s `OpenFSRecords`
(`Time1Minute` to `Time30Minutes` and `TimeUnlimited`, each with a
`GoldRecord`, `SilverRecord` and `BronzeRecord` like the track records';
the defaults are the same people on every limit). A session's HUD shows
them as its targets (0xa0e60, 0xa1230: "Gold", "Silver", "Bronze"; none for
training). At the results of Open Freestyle and Freestyle Battle (game
types 4 and 8, 0x7ff60), 0x18090 gives each human the first entry their
score is more than, the best score first and a second human on the same
entry put on the next (off the end past Bronze); 0x182d0 then moves the
entries below down one, dropping the last, and writes the human in
(0x14450). Each goes on the record page as "Gold Medal Score", "Silver
Medal Score" or "Bronze Medal Score" (strings 199, 0x1c6 and 0x36, kinds
1 to 3, 0x4d8d0) with the score. The Open Freestyle page lists the time
limit's three with the medal, "%d Points" (0x178) and the holder's name
(0x56ca0, 0x560c0). The rewrite saves them in `records.cfg` too, as
`freestyle <minutes or unlimited> <gold|silver|bronze> = ...` lines once a
limit's have changed.

The freestyle HUD (0x9fe50, update 0x9ff60, draw 0xa0b40) shows "Time"
(string 0xe2) only in a challenge or with a time limit (time index not 6),
and "Score" (0xe1) in game types 4 and 8. The score on it rolls up: each
1/30 s, while it is under the session's score, it gains a random amount
(0xaac50) below 5% of the gap, or below 3 when that is more, and is held
at the real score. The rewrite takes the same steps per 1/30 s of time
(the original takes at most one a frame).

The race HUD (0xa4990) lays its panels out in a 0..2 square per viewport
(0xa4800 from the left, 0xa4870 from the right, 0xa48e0 measured from the
top), with the rev gauge 0.7 across, 0.12 in from the bottom right corner.
Top left, 0.1 in and 0.15 down, a panel 0.64 by 0.264 (0xa1230, draw
0xa1930, update 0xa1510 every 1/30 s) holds rows at y 0.3, -0.2 and -0.7
of its own -1..1 square, each a label right-aligned in x -1..0.3 and a
figure from x 0.4 (the first figure 1.2 times the 0.8 size). In a race
(not Time Trial) the rows are "Position" (string 0xde: "1st" to "8th",
0x193 to 0x19a, from the racer's place, +0x25c), "Score" (0xe1: the
player's trick score rolling up as the freestyle score does; in a career
round, game types 3 and 7, "%ld (%s)" with the player's place by trick
score among the field, sorted again every second by 0xa1130) and, with
more than one racer and one viewport, "Lead" (0xdc): "%im" of the second
racer's distance to go less the player's when the player leads, else the
leader's less the player's, cut towards zero. The distance to go (racer
+0x13c, 0x904e0) is measured round the AI line to the next checkpoint and,
with whole laps still to ride, on to the finish; the running order
(0x901f0, 0x8fa60) is laps, then checkpoints, then the least distance to
go, and DAT_001deaf4 onwards holds the racers in place order. The lead
figure takes the labels' colours when the player leads. In freestyle
types 4, 5 and 8 the panel lists three medal rows instead (draw 0xa1930):
a `Frontend/SmallMedals` icon (`GOLDMEDAL`, `SILVERMEDAL`, `BRONZEMEDAL`,
0.75 of the panel high and 0.45 of that wide) at x -0.9 and a figure at
x -0.45, the rows at y 0, -0.9 and -1.8. In types 4 and 8 the figures come
from the session's live list (0x18090): the time limit's three records
with each human's score put in the first place it beats (strictly), the
best score first, the rest moving down and the last dropping off. The row
holding the player (+0xac) takes the figures' colours and the others the
labels'; that row shows the rolling score, except in Gold's place, where
0xa1510 leaves the list's own (real) score. In a challenge (type 5) the
rows are the challenge's record times, and the lit row is the best medal
the clock is still inside (0xa1040). ATV Academy (type 9) shows only
"Score". Top right, 0.1 in and 0.15 down, 0.8 by the gauge's
width (0.7): Time Trial's panel, freestyle's (above), or a race's (0x9f840,
draw 0xa0100): "Current" (0xd8, the lap being ridden), "Best" (0xd6) and
"Lap" (0xdb, "%ld/%ld"), labels in x -1..0 and figures from 0.1; with
more than one viewport only the lap. All these panels draw in
`MgrammeSmall`, labels shaded from (0.47, 0.67, 0.67) down to (0.25,
0.54, 0.54) and figures from (1.0, 0.51, 0.3) down to (0.55, 0.13, 0.13).
The rewrite draws the race panels' rows with its own sizes and fonts in
those top colours, and takes the lead from the computer riders' distance
round the racing line. Its freestyle panels follow the same layout: the
medal list top left (the Academy's score instead), "Time" and "Score" top
right; the old rewrite-only lines for the next medal target and the spots
found are gone, as the original has neither.

## Time Trial's ghost

Only Time Trial (game type 1) makes the ghost object (0x9dd60, 0x110 bytes,
at DAT_001deb44), with two 0x32000-byte streams: the lap being recorded
(+0xc4) and the ghost being played. It starts from the track's lap record
(0x9d930, through 0x17e60), with the ghost's own lap time (+0x70) at 9e30
for none yet. Every start (0x84ce0, then 0x9e110) copies the saved ghost
into playback, starts recording the human (0x9da50), creates two ghost
racers (0x9dcd0 through 0x85e70; vtable 0x1b7948), one on the saved ghost's
rider and vehicle (+0x68, +0x6c) and one on the player's, and starts
playback (0x9dbe0); the ghost shows only once there is one (+0x14).
Loading a ghost racer (0x26240) sets flags on the model and texture
managers (DAT_001e229c +0x198, DAT_001e2298 +0x1b4) so it gets its own
copies of its models and textures. Every frame (0x9de20) records the
player while the stream has room and moves the ghost on (vfunc +0x64); a
ghost with nothing left to play is put 200 m down.

When a lap is done, the Time Trial HUD (0x9fbd0) calls 0x9e120: a lap
faster than the ghost's makes this lap's recording the ghost (0x9dc50: the
player's rider and vehicle, changed +0x1c, there is a ghost +0x14), and
either way 0x9e090 starts recording and playback again from the lap's
start. The first lap's recording starts on the grid. At the end (0x9d9b0)
a best lap under the track's lap record becomes the record, and the ghost
is saved with the game (0x9d890, 0x9df80). The HUD (0x9fa60) shows
"Record" (string 0xdf: the track's lap record, lowered as soon as a lap
beats it), "Best" (0xd6), "Current" (0xd8) and a list of laps (0x9fbd0
every 1/30 s, drawn by 0xa0660). The list holds four times: each finished
lap goes in slot min(laps, 4) - 1, the lap being ridden in slot
min(laps, 3), and past four laps the list moves up one, so it shows the
last three laps and the one being ridden (the original overwrites lap 4
with the lap in progress before it moves up, so lap 4 never shows). Rows
step down half a line from y 0.3 in the HUD box: Record, Best, then
"Current" beside the first lap row. Labels use the colour pair
0x1df850/0x1df834 (grey-teal and dark teal), times 0x1df844/0x1df840
(orange and dark red), and the lap being ridden yellow for both.

The rewrite records what is drawn, as its replays do: the quad, its
wheels, the rider's seat and every joint, 30 times a second. The ghost is
drawn at 40% opacity (how the original draws it isn't decoded) and kept per
track in `ghosts/<track>.ghost` in the config folder: `ATV2GHST`, version
1, the lap time, the rider and quad (each a u16 length and the name), the
joint count (u16) and frame count (u32), then per frame the lap time, the
quad's place and turn, the seat's, each wheel's height and turn, the
pelvis's place and each joint's turn (places as f32, turns as four i16
fractions of 32767). The rewrite's Time Trial times are TIME, RECORD,
BEST and the lap list with CURRENT beside its first row; the lap being
ridden is yellow and the earlier laps white, like the rewrite's other
times.

## Still unknown

the `.kdt` spatial tree, `.nin`, `.viz`, `.cuban` (compressed animation), the names of
Vehicles bank sounds 0 and 17–31 and of the rider voices,
the ROW spatial tree and effect records, and the material record fields.

## Sound

- `Setup/Sound.xml` nests, unlike the other setup files (`sound.rs` reads
  it): root mix levels (`mPlayerEngineVol` 0.25, `mAmbientVol` 0.3,
  `mMusicVol` 1.0, `mSurfaceVol` 1.0, `mEnginePitchZero` 0.6,
  `mEnginePitchSpan` 1.2), then per-environment `Surfaces` (wheel sample,
  type, volume, pitch per material), `Music` (an optional `IntroSection`
  then `TrackSection`s), `Ambient` and `Crowd` banks. `DefaultTrack` is
  the fallback for each.
- Wave bank format word: low 4 bits always 2, sample rate in bits 4–30,
  bit 31 set for 16-bit signed and clear for 8-bit unsigned samples. No
  entry has a loop region.
- Vehicles bank: 1 is the engine "off throttle" loop and 2 the "on
  throttle" loop; the name table at `0x1d2838` gives 3–16 (`Gravel1`–`3`,
  `Rumble1`–`4`, `Water1`–`3`, `Twigs1`–`2`, `Screech1`–`2`).
- Engine update (`0x93c60`), with r = revs / max revs and t = throttle,
  on the ground: on-loop volume `clamp((t + r) * 0.7, 0.2, 1)` and pitch
  `mEnginePitchZero + mEnginePitchSpan * r`; off-loop volume
  `clamp(1 - on, 0.3, 1)` and pitch `0.85 + 1.8 * r`. Another branch
  (a vehicle value at `+0x2b0` below 0.6) changes the mix and isn't
  ported.
- Banks load into slots: 0 Vehicles, 1 Riders, 2 SpotFX, 3 the track's
  ambient or crowd bank. One-shots go through `0x92e30` (bank, index,
  volume, pitch).
- SpotFX (3 clips): 0 the menu cursor moving, 1 select (and, it seems,
  back), 2 a pop-up panel opening or closing; all at
  `mFrontEndSpotFXVol`, pitch 1. The rewrite plays 0 and 1 on the front
  end and in-race menus, and 2 as the pause menu opens and closes.
- Riders (34 clips): `0x94690` plays one when a thrown rider's body hits
  something with an impulse over 2.5 while going faster than 2.2347, at
  most once a second unless the hit is twice as hard as the last. Female
  riders (`mIsMale` 0) draw from 0-13 (table `0x1d28f4`), male from 14-32
  (`0x1d2930`), re-rolling the last one played; volume and pitch 1. Clip
  33 is never used.
- Quad knocks: `0x94480` plays Vehicles clips from the table
  `[25, 29, 30, 17, 18, 23]` (`0x1d28c0`) when the vehicle's impact value
  passes 0.3, with a 0.25 s gap unless twice as hard: the first 3 up to
  impact 8, the first 4 up to 14, all 6 above; volume `impact / 8` (or
  `/ 14`) `+ 0.7` capped at 1, halved for some hit directions; pitch
  `1 + rand(0.5)`. The rewrite uses the chassis' closing speed over 2 m/s
  for the impact.
- Sound sets (`0x94ef0`, vtable `0x1c34d4`): each quad that makes sound
  gets a set of parts, each with its own update: engine (`0x93c60`),
  knocks (`0x94480`), rider voice (`0x94690`) and wheel noise (`0x93f10`,
  one voice per wheel, positioned at it). `0x92ff0` gives every player a
  full set, with all four wheel voices for one or two players and wheel 0
  only for three or four; computer riders get engine-only sets, as many
  as `4 - players`, and `0x92c80` hands them each frame to the riders
  nearest view 1's listener.
- Listeners: one per view (`0x93310` copies each view camera's position
  and axes). A 3D voice is placed against the listener nearest to it
  (`0x76180` -> `0x75e50`, then `0x73f70` makes the position and velocity
  relative to it), so each split-screen player hears their own quad as
  player 1 does.
- How loud other quads are: DirectSound's 3D distance model isn't decoded;
  the rewrite gives them the player's mix scaled by `(8 / (d + 8))²`, d
  being the metres from the nearest player's quad, and gives computer
  riders wheel noise and crash sounds too.
- Water: the track's water meshes (materials named `*water*`: `FO_water`,
  `DU_water`, `Water_24bit`...) aren't solid; the quad drives on the bed
  below. The rewrite keeps their level triangles as a separate surface to
  measure wading depth against: wheels under it play the `Shallow_Water`
  surface sound (`Deep_Water`'s pitch past 0.4 m) instead of the ground's,
  and wash the dirt off. Mesh flag 0x2000 is not a water flag on its own:
  Forest's shadow decals carry it too. Deep water (crashes) goes by the
  `waterbox` locators instead, as the original does.
