# Sprite Atlas Packer

Local sprite atlas packer for 2D game pipelines. The current build includes a
headless Rust CLI plus a Tauri/React GUI shell styled to match the other
forPublic apps. It packs loose PNG/JPG/WebP/GIF images into a deterministic
TexturePacker JSON-Array compatible atlas plus a `sheet.webp`, `sheet.png`, or
both, then previews animations from the same atlas JSON that gets written.
PNG/JPG/WebP/GIF inputs are decoded directly. `.ase`/`.aseprite` inputs are
accepted through the local Aseprite CLI, exporting frames to temporary PNGs
before packing; set `SPRITE_ATLAS_PACKER_ASEPRITE_BIN` if the binary is not
named `aseprite` on your PATH.

![Video to packed sprite animation preview](demo-cat-walk/video-tool-output/cat_walk_video_preview_slow_7fps.gif)

```bash
cargo run --manifest-path src-tauri/Cargo.toml -- \
  --headless \
  --input ./raw-sprites \
  --output ./build/atlas \
  --algo maxrects \
  --format webp \
  --pot \
  --padding 2 \
  --max-size 2048 \
  --trim \
  --stable-pivot \
  --dedup-threshold 1 \
  --linked-channels _n,_e,_m \
  --animation-regex '^(?P<name>.+)_(?P<index>\d+)$' \
  --aseprite-json \
  --godot-spriteframes \
  --unity-importer
```

Project files can pin a batch:

```json
{
  "sourceFolder": "./raw-sprites",
  "outputPath": "./build/atlas",
  "format": "webp",
  "pot": true,
  "padding": 2,
  "maxSize": 2048,
  "rotate": false,
  "trim": true,
  "stablePivot": true,
  "dedupThreshold": 1,
  "linkedChannels": ["_n", "_e", "_m"],
  "godotSpriteframes": true,
  "unityImporter": true,
  "packingStrategy": "maxrects",
  "animationRegex": "^(?P<name>.+)_(?P<index>\\d+)$",
  "animations": [
    {
      "name": "hero_idle",
      "frames": ["hero_idle_0", "hero_idle_1"],
      "loop": true,
      "fpsHint": 8.3,
      "pivot": { "x": 0.5, "y": 0.5 },
      "durationMs": 120,
      "frameOverrides": {
        "hero_idle_1": {
          "pivot": { "x": 0.48, "y": 0.54 },
          "durationMs": 160
        }
      }
    }
  ]
}
```

Project files can be checked against `schemas/atlas-project.schema.json`.

```bash
cargo run --manifest-path src-tauri/Cargo.toml -- \
  --headless \
  --project hero.atlasproj.json \
  --watch
```

Watch mode repacks when supported source images change and skips rewrites when
the source snapshot is unchanged. In the desktop GUI, enable **Hot reload
preview** in the Source panel to repack and refresh the preview automatically
when the source content or pack settings change.

`animationRegex` can override the default final-numeric-token grouping. Use a
named `name` capture and optional `index` capture. Explicit project
`animations` override inferred groups and can pin frame order, loop state, FPS,
pivot, and frame duration. Use `frameOverrides` inside an animation to adjust a
single frame's pivot or `durationMs` while keeping the animation-wide defaults
for the rest of the sequence.

`stablePivot` computes each trimmed frame's pivot from the original untrimmed
canvas center. This keeps frame animations anchored consistently after trimming
while still allowing explicit project animation pivots to override it.

`dedupThreshold` aliases duplicate or near-duplicate frames to one packed rect
when their dimensions and trim bounds match. `0` means exact duplicates only;
higher values allow small per-channel deltas while preserving every frame name
in `atlas.json`.

`linkedChannels` packs sibling channel images into matching atlas sheets. For a
source frame like `hero_idle_0.png`, suffix `_n` looks for
`hero_idle_0_n.png` beside it and writes `sheet_n.webp`, `sheet_n.png`, or both
using the same packed frame rectangles as the color atlas. Missing channel
images leave transparent pixels in the linked sheet.

`--algo` accepts `maxrects` (default, best fit), `guillotine` (texture-budget
friendly binary splits), `quadtree` (binary-tree leaf splitting for many small
sprites), or `shelf` (simple deterministic row packing for quick/debug
layouts). Project files store this as `packingStrategy`.

`--max-size` caps the atlas width and height, defaulting to 16384.

`--rotate` allows non-square sprites to be stored rotated 90 degrees when that
helps them fit; it is off by default for importer compatibility.

## Video to Sprite Animation

The desktop GUI includes a **Video** panel for turning a short walk-cycle clip
into a packed sprite animation. Choose an MP4/WebM/MOV/M4V, pick a 4x4, 5x5, or
custom grid, set the frame size, FPS, and optional start/end trim, then run
**Make Sprite Sheet**. The app samples the video into ordered PNG frames, packs
them through the generated-frame normalizer, and writes a transparent-frame
`sheet.png`, `atlas.json`, `aseprite.json`, `frames/*.png`, and an animated GIF
preview.

The packed output keeps transparent canvas space around each pose, so the atlas
is ready for game animation import. If the source video already has alpha or a
transparent/clean keyed subject, that transparency is preserved through the PNG
frame and atlas pipeline.

## Local Control API

Agents can drive a running local server with bearer-token auth. In the desktop
app, the `Remote` panel shows the AI Agent Control checkbox, local address, and
generated token. The current desktop build starts the local control API
automatically, and the checkbox can disable or re-enable it for the session.
The same API can also be started from the CLI:

```bash
cargo run --manifest-path src-tauri/Cargo.toml -- \
  --headless \
  --serve \
  --port 9877 \
  --token sap-local
```

The server writes `~/.config/sprite-atlas-packer/control-api.json` and exposes:

- `GET /api/v1/state`
- `POST /api/v1/pack`
- `POST /api/v1/slice`
- `POST /api/v1/animation-job/start`
- `POST /api/v1/animation-job/submit-frame`
- `POST /api/v1/animation-job/assemble`
- `POST /api/v1/assemble-generated-loop`
- `POST /api/v1/export-gif`
- `GET /api/v1/atlas`
- `WebSocket /api/v1/events?token=<token>`

See `agent-skills/sprite-atlas-packer/SKILL.md` for request shapes and agent
usage notes.

Games and tools can connect to the WebSocket events URL from the discovery file
to receive JSON `{ "type": "pack" | "slice", ... }` notifications whenever the
control API produces a new atlas. HTTP requests still use `Authorization:
Bearer <token>`; the WebSocket endpoint also accepts the token query parameter
because browser WebSocket clients cannot set custom auth headers.

Reverse mode slices an existing sheet back into loose frames plus atlas JSON:

```bash
cargo run --manifest-path src-tauri/Cargo.toml -- \
  --headless \
  --input ./downloaded-sheet.png \
  --output ./build/sliced \
  --slice grid \
  --cell 32x32 \
  --prefix hero_run \
  --format png
```

```bash
cargo run --manifest-path src-tauri/Cargo.toml -- \
  --headless \
  --input ./irregular-sheet.png \
  --output ./build/sliced \
  --slice alpha \
  --prefix prop \
  --format png
```

Grid mode accepts an explicit cell size or attempts to infer cells from
transparent gutters. Alpha mode detects connected non-transparent components.

For AI/generated spritesheets, reverse mode can clean generated mattes,
auto-refine grid cell boundaries from visible sprite clusters, normalize every
frame with one global scale, and place the result on stable transparent frame
canvases before writing the normal sheet, frames, and atlas. Prefer transparent
PNG from image generation; if the model needs visible separators, use a thin
dark neutral gray separator or outline instead of green chroma. Generated
normalization strips dark neutral matte connected to sheet/cell edges and
cleans green spill as a fallback.

```bash
cargo run --manifest-path src-tauri/Cargo.toml -- \
  --headless \
  --input ./generated/hero_walk_raw.png \
  --output ./build/normalized \
  --slice grid \
  --cell 256x256 \
  --prefix hero_walk \
  --format png \
  --normalize-generated \
  --frame-size 256x256
```

This writes `spritesheet_normalization.json` beside `atlas.json` with the
detected source cells, normalized frame boxes, root anchors, global scale,
continuity diagnostics, and policy strings. For walking loops, warnings about
random adjacent pose jumps or a poor last-to-first transition mean the generated
sheet should be regenerated with explicit gait poses. The output `frames/`
folder can be packed again with normal pack mode when you want a tighter
production atlas.

For agent-driven frame-at-a-time generation, keep the original packer workflow
separate and use the control API to assemble generated loop frames from a
folder. Name the images in the intended playback order, such as
`000_contact.png`, `001_inbetween.png`, and so on:

```bash
curl -s -X POST \
  -H "Authorization: Bearer sap-local" \
  -H "Content-Type: application/json" \
  --data-binary @assemble-loop.json \
  http://127.0.0.1:9877/api/v1/assemble-generated-loop
```

`assemble-loop.json`:

```json
{
  "input": "/absolute/path/to/generated/ordered_frames",
  "output": "/absolute/path/to/build/assembled_walk",
  "columns": 4,
  "frameWidth": 256,
  "frameHeight": 256,
  "prefix": "hero_walk",
  "format": "png",
  "asepriteJson": true
}
```

This endpoint normalizes each separate generated frame with the same matte
cleanup, global scale, root anchoring, and continuity diagnostics used by
generated-sheet slicing, then writes the assembled sheet and atlas.

For stricter agent-driven animation, use animation jobs. A job creates the
frame plan and SVG pose guides first, then accepts or rejects one generated
frame at a time before assembly:

```bash
curl -s -X POST \
  -H "Authorization: Bearer sap-local" \
  -H "Content-Type: application/json" \
  -d '{"output":"/absolute/path/to/jobs/cat_walk","action":"cat_walk","frameCount":16,"columns":4,"frameWidth":256,"frameHeight":256,"prefix":"cat_walk"}' \
  http://127.0.0.1:9877/api/v1/animation-job/start
```

The response includes `animation_job.json`, `pose_guides/*.svg`, and frame
prompts. The prompts describe the same checks the API applies: stable root,
stable ground line, visible front/hind paw contacts, readable paw spacing,
locked palette/style, and no duplicate tail frame. Generate frame images one at
a time, then submit each candidate:

```bash
curl -s -X POST \
  -H "Authorization: Bearer sap-local" \
  -H "Content-Type: application/json" \
  -d '{"jobDir":"/absolute/path/to/jobs/cat_walk","frameIndex":0,"sourcePath":"/absolute/path/to/generated/000.png"}' \
  http://127.0.0.1:9877/api/v1/animation-job/submit-frame
```

If the validation response is not accepted, regenerate that frame using the
reported errors/warnings and the same pose guide. The validation report includes
foreground `style` stats and lower-body `gait` stats so agents can see whether a
frame failed because it changed identity/style or because it is not a readable
walk pose. When every frame is accepted:

```bash
curl -s -X POST \
  -H "Authorization: Bearer sap-local" \
  -H "Content-Type: application/json" \
  -d '{"jobDir":"/absolute/path/to/jobs/cat_walk","format":"png","asepriteJson":true}' \
  http://127.0.0.1:9877/api/v1/animation-job/assemble
```

The previewer scrubs the same `atlas.json` that gets written, with thumbnail
timelines, onion-skin, loop/ping-pong playback, arrow-key stepping, FPS override,
and start/end frame range controls. It can export the selected animation range
to GIF from `atlas.json` plus the emitted sheet, using `gifski` when that
executable is available and falling back to the built-in GIF encoder otherwise.
The GUI can also record the selected preview range to WebM through the browser's
`MediaRecorder` support.

The GUI previewer can also load a second `atlas.json` as a comparison atlas.
It resolves the sheet from that atlas's `meta.image` beside the JSON file and
shows a second animated timeline for before/after repack review.

## Sister App Workflow

`SeamlessImageEdit` can feed this packer directly through the two headless CLIs:

```bash
cargo run --manifest-path ../SeamlessImageEdit/src-tauri/Cargo.toml -- \
  --headless \
  --mode tile \
  --strategy seam-cut \
  --format webp \
  --output-dir ./build/seamless \
  --suffix _seamless \
  --overwrite \
  ./raw-sprites

cargo run --manifest-path src-tauri/Cargo.toml -- \
  --headless \
  --input ./build/seamless \
  --output ./build/atlas \
  --format webp \
  --trim \
  --animation-regex '^(?P<name>.+)_(?P<index>\d+)(?:_seamless)?$'
```

Use `--recursive` on the SeamlessImageEdit command when source sprites are
nested. A one-click "Send to Atlas" action belongs in the sibling app, but this
repo's CLI and project files are ready for the shared pipeline.

`--aseprite-json` writes an optional `aseprite.json` sidecar with Aseprite-style
`duration` fields and `meta.frameTags`, while keeping `atlas.json` as the
canonical TexturePacker JSON Array output.

`--godot-spriteframes` writes `godot_spriteframes.tres`, a Godot `SpriteFrames`
resource that references the emitted sheet and atlas regions. `--unity-importer`
writes `SpriteAtlasPackerImporter.cs`, an Editor helper that can apply the
generated `atlas.json` sprite rectangles to the selected Unity texture. These
native helper exports require non-rotated frames; keep `--rotate` off for them.

See `~/git/openclaw/docs/sprite-atlas-packer-spec-2026-06-11.md` for the locked
product spec.

## GUI

```bash
npm install
npm run dev
```

The web shell runs on `http://127.0.0.1:1450/`. For the desktop app, install the
normal Linux Tauri system dependencies (`pkg-config`, DBus/WebKitGTK development
packages, etc.) and run:

```bash
npm run tauri:dev
```

The desktop-only Rust dependencies are behind the `desktop` feature so core CLI
tests can run without those OS packages.

## Verification

```bash
npm run build
npm run test:compat
cargo test --manifest-path src-tauri/Cargo.toml
```

`npm run test:compat` generates fixture PNGs, packs them with the real CLI,
validates the TexturePacker JSON Array shape expected by Phaser's atlas parser,
checks a lossless projection into Pixi's documented spritesheet data shape, and
validates a representative `.atlasproj.json` against
`schemas/atlas-project.schema.json`.

Rust tests also include a committed golden atlas fixture under
`tests/golden/basic/`, a deterministic sheet hash check, trim recomposition, and
bilinear edge sampling to guard against color bleed from neighboring frames.
