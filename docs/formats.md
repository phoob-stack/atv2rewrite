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
| `.kdt` | 17 | none | Per-track spatial data, likely collision tree. | To do. |
| `.hsc` | 35 | none | Knock-over prop hulls (cones, signs, barriers, planks): `u32` hull count; per hull a `u32` plane count and planes (normal, d; inside where n·p + d <= 0); then a `u32` sphere count and spheres (centre, radius), read at `0x24c10`. The signs are 6-plane boxes, one per part. | Layout known; not used yet (needs props that move). |
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
water, 0x4000 foam/wake, 0x8000 baked shadow decal. Material -1 is
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
too. Mesh blocks are currently found by validating candidate headers, which
recovers 87-98% of each file's bytes; decoding the tree will make it exact
and will give the visibility structure.

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
| `Sprites/HUDTPAGE` | Rev gauge: `DIAL` (pivot = hub), `NEEDLE` (pivot = its hub end), `PRELOAD` arc drawn into the `DIAL_PRELOAD` slot, `BOOST` pie into `DIAL_BOOST`, `WEIRD_DOT` hub cap. Dial face reads 1-8 (x1000 rpm) from -134 deg to +21 deg, and KPH. `HUDTPAGE_b` is a larger, unused gauge with no sheet. |
| `Sprites/321go`, `Sprites/positions` | Countdown digits and GO; position balls. The sheet names P1-P8 but only P1-P4 have art; the P5-P8 cells are empty. |
| `Sprites/Position_meter` | Lap progress line, chequered flag, markers and the CHECKPOINT banner. No sheet; regions measured from the page (see `ui.rs`). |
| `Fonts/*` | Bitmap fonts: `MgrammeCombined` (outlined capitals, menus), `MgrammeSmall`, `kim36` (chrome mixed case), and outline/inner variants. `Fonts/blimey.ttf` on the disc is a TrueType font. |
| `FrontEnd/NewDesign` | Menu selection bar, attribute bars, selection square, arrows, medals, panels. |
| `FrontEnd/BG/*` | Full-screen backgrounds; `Setup/Frontend.xml` names each screen's (`BACKGROUND0`) and its title (`mTitleText`). |
| `FrontEnd/RiderMugs` | 24 portraits, 8 per row of 64 px, in `Setup/Riders.xml` order. |
| `FrontEnd/Track_*_LARGE` | Track previews; `FELoading1` is the loading screen. |

`Strings/Strings_ENG.txt` is one string per line with no keys (the keys are
in the executable), so the UI uses the same English labels directly.

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

## Game-mode rules (Setup/*.xml)

- `Arcade.xml`: `RoundN TrackName=` in order (16 rounds, Dunes 1 twice).
  `Tracks.xml` gives each track's `mArcadeStartTime` (40 s everywhere) and
  `mAIBestLapTime`/`mAIWorstLapTime`. Tracks mark their time-extension gates
  with extra `Checkpoint N arcade` locators (Forest 3 spells it `acade`) at
  the same spot as `Checkpoint N`. The disc gives no seconds per gate; the
  rewrite shares the slowest AI lap time out over the gates by how much of
  the lap each covers.
- `Freestyle.xml`: `OpenFSRecords/TimeNMinute(s)` and `TimeUnlimited` each
  hold Bronze/Silver/Gold `Record=` scores for Open Freestyle; `FSChallenge`
  has `Challenge_1..12`, each with `mBronze/Silver/GoldBonus` and
  Bronze/Silver/Gold `Record=` times in seconds (lower is better), and
  `Unlockable0..11` (`mRider="1"`) give a rider for gold in challenge K+1
  (the bare `<Unlockable>` is ignored). All twelve run on Stadium 2: the
  course of challenge N is the `checkpoint` locators N*100..N*100+39 in
  `Stadium2.row` (lower-case, sometimes with a trailing space or words
  after the number, `checkpoint 600 endo box`), and `startN` is its start.
  Props whose flags carry numbers after the first word (`FREE 1` cones,
  `GEN_Start_gate 1 2 3 5 6`, `GEN_stasignfloor 2 3`) are only out in those
  challenges; `GEN_stasignfloor` has no model on the disc. Challenge 12
  ("burst all the balloons") has no checkpoints or balloons placed. The
  original's medal test is strict (`time < record`), and a better medal
  pays its bonus once per tier climbed. The texts are `CHALLENGE_TEXT_N`
  in `Strings/Strings_ENG.txt`, which holds values only, in sorted key
  order (lines 92-103 for `_1`, `_10`, `_11`, `_12`, `_2`..`_9`).
- `Tricks.xml`: one element per trick with `mControlCombo="KeyAHeld,
  StickUp"` (buttons D, A, B, E for levels 1-4, `KeyCombo1/3/2` for 5-7),
  `mScorePoints`, `mHoldPoints`, `mTotalTrickTimeSec`, `mTrickEndTimeSec`
  and an `AnimData` child naming the stunt clip (`Anim Stunt Seat Grab`). The
  clips are `Animations/Stunts/Bac/*.bac` (31 bones, rider space, same as
  the riding clips); most names match with spaces dropped, the rest are
  Scarecrow = `crucifixtion`, Scissors = `leg_splt`, Spin Bike = `bike_spin`,
  Reverse Spin = `reverse_spin_grab`, Yogi = `Y`. `*_bike.bac` (1 bone) move
  the quad for the tricks that spin it. `Wheelie`, `Bicycle` and `Endo` give
  ground-trick points; `SpeedBoost` is the race boost tricks earn.
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
  Vibration tick box (`FrontEnd/checkbox`: `OFF`, `ON`). Replays, Profiles,
  Language and Credits pages exist too; the disc has no credits text.
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
  status screens are `Career_Status_Amateur`, `_Pro` and `_Pro2`.
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

## Still unknown

`.kdt` (likely collision tree), `.nin`, `.viz`, `.cuban` (compressed animation), the names of
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
