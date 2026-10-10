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
| `Music/*.wav` | 18 music tracks, RIFF WAV, Xbox IMA ADPCM (format 0x69), 44.1 kHz stereo. ffmpeg decodes them. |
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
| `.hsc` | 35 | none | Per-prop collision hulls (plane lists). | To do. |
| `.aid` | 17 | `AID 1.02` | AI racing line / waypoints. | To do. |
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
| `.bed` | 35 | binary | Sprite-sheet layouts for `.bog` texture pages (UI, particles). | To do. |
| `.ink` | 45 | `0x00CEFDAB` | Extra model data (vertex positions), likely low-detail or env-map geometry. | To do. |
| `.xwb` | 9 | `WBND` | XACT wave banks (sound effects, rider voices). Known format; samples are likely Xbox ADPCM. | To do. |
| `.asm` / pixel shaders | 75 | text | Xbox vertex/pixel shader source. A reference for rewriting effects in WGSL. | Readable text. |
| `Setup/*.xml` | 13 | XML | Game rules: tracks, career, arcade, freestyle, tricks, riders, vehicles, cameras, materials, sound. | Readable text. |
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

## Still unknown

`.kdt` (likely collision tree), `.hsc` (prop collision hulls), `.aid` (AI
lines), `.nin`, `.viz`, `.cuban` (compressed animation), `.xwb` (sound),
the ROW spatial tree and effect records, and the material record fields.
