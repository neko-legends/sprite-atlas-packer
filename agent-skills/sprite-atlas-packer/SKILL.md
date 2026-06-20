---
name: sprite-atlas-packer
description: Control SpriteAtlasPacker through its local HTTP API for packing sprite folders, slicing existing sheets, and inspecting the latest atlas output.
tags: [sprite-atlas, tauri, game-dev, texturepacker, remote-control]
---

# Sprite Atlas Packer Control Skill

Use this skill when an agent needs to drive a running SpriteAtlasPacker control server instead of shelling out for one isolated pack command.

## Start Or Discover

Human or agent starts the server:

```bash
cargo run --manifest-path src-tauri/Cargo.toml -- \
  --headless \
  --serve \
  --port 9877 \
  --token sap-local
```

The server writes a discovery file:

```text
~/.config/sprite-atlas-packer/control-api.json
```

It contains `host`, `port`, `token`, and `baseUrl`.
It also includes `eventsUrl` for live atlas-change notifications over WebSocket.

## Auth

Every API call requires:

```text
Authorization: Bearer <token>
```

## Endpoints

Base URL:

```text
http://<host>:9877/api/v1
```

### State

```bash
curl -s \
  -H "Authorization: Bearer sap-local" \
  http://127.0.0.1:9877/api/v1/state
```

Returns app version, last pack result, last slice result, last atlas, and last sheet path.

### Pack

```bash
curl -s -X POST \
  -H "Authorization: Bearer sap-local" \
  -H "Content-Type: application/json" \
  --data-binary @pack.json \
  http://127.0.0.1:9877/api/v1/pack
```

`pack.json`:

```json
{
  "input": "/absolute/path/to/raw-sprites",
  "output": "/absolute/path/to/build/atlas",
  "format": "webp",
  "packingStrategy": "maxrects",
  "pot": true,
  "padding": 2,
  "maxSize": 2048,
  "rotate": false,
  "trim": true,
  "stablePivot": true,
  "dedupThreshold": 1,
  "linkedChannels": ["_n"],
  "godotSpriteframes": false,
  "unityImporter": false,
  "asepriteJson": false,
  "animationRegex": "^(?P<name>.+)_(?P<index>\\d+)$"
}
```

Only `input` and `output` are required. Defaults match the app: WebP, MaxRects, POT, padding 2, trim on.
Set `maxSize` to cap both atlas dimensions; omit it for the default 16384px ceiling.
Leave `rotate` false unless the user explicitly wants rotated TexturePacker frames.
Set `packingStrategy` to `"guillotine"` for deterministic binary-split packing, `"quadtree"` for binary-tree leaf splitting on many small sprites, or `"shelf"` only when you want simple row packing for quick/debug layouts.
Set `stablePivot` when trimmed animation frames should keep their original canvas-center anchor.
Set `dedupThreshold` to `0` for exact duplicate aliasing or a small value like `1` for held frames with tiny pixel deltas.
Set `linkedChannels` to suffixes such as `["_n", "_e", "_m"]` when normal, emissive, or mask images should be packed into sibling sheets with the same frame rectangles as the color atlas.
Set `godotSpriteframes` or `unityImporter` when the workflow needs native Godot/Unity helper artifacts. Keep `rotate` false for these exports.

### Slice Existing Sheet

```bash
curl -s -X POST \
  -H "Authorization: Bearer sap-local" \
  -H "Content-Type: application/json" \
  --data-binary @slice.json \
  http://127.0.0.1:9877/api/v1/slice
```

`slice.json`:

```json
{
  "input": "/absolute/path/to/sheet.png",
  "output": "/absolute/path/to/build/sliced",
  "mode": "grid",
  "cellWidth": 32,
  "cellHeight": 32,
  "prefix": "frame",
  "format": "png"
}
```

Use `"mode": "alpha"` for connected-component slicing.

For AI-generated sheets, set `normalizeGenerated` and provide the expected
source grid cell size plus the desired normalized output frame size. The source
cell size may be approximate; generated mode rounds a 1254px sheet with 314px
cells to a 4x4 grid instead of dropping the last row/column. If `cellWidth` and
`cellHeight` are omitted, generated mode infers common layouts such as 4x4 or
4x3 from the sheet shape. Prefer transparent PNG from the image model. If the
model needs a visible separator/matte, request a thin dark neutral gray
separator/outline such as `#2b2f36`, not green chroma. Generated normalization
removes dark neutral matte connected to sheet or cell edges, then strips green
spill only as a fallback cleanup.

For walking/running sheets, do not ask for generic "walking frames." Request one
coherent loop in left-to-right, top-to-bottom order:

- 16 frames: contact, in-between, down, in-between, passing, in-between, up, in-between, opposite contact, in-between, opposite down, in-between, opposite passing, in-between, opposite up, final in-between before frame 1.
- 8 frames: contact, down, passing, up, opposite contact, opposite down, opposite passing, opposite up.
- Keep the same camera, scale, silhouette, palette, full body, fixed frame box, stable bottom/root anchor, and no repeated first frame at the end.

```json
{
  "input": "/absolute/path/to/generated_walk_sheet.png",
  "output": "/absolute/path/to/build/normalized",
  "mode": "grid",
  "cellWidth": 256,
  "cellHeight": 256,
  "frameWidth": 256,
  "frameHeight": 256,
  "prefix": "hero_walk",
  "format": "png",
  "normalizeGenerated": true,
  "asepriteJson": true
}
```

This emits a cleaned `sheet.png` or `sheet.webp`, normalized `frames/*.png`,
`atlas.json`, and `spritesheet_normalization.json`. The normalization manifest
includes a `continuity` section with loop diagnostics; if it warns about random
pose jumps or a bad last-to-first transition, reject the sheet and regenerate
with stricter walk-cycle guidance. Use the emitted `frames` folder as
`/api/v1/pack` input when the user wants a tighter production atlas.

### Assemble Agent-Generated Loop Frames

This is an agent-facing animation pipeline beside the original packer. Use it
when an image model cannot keep all 8/16 cells coherent in one large sheet.
Generate the frames one image at a time, save them into one folder, and name
them in playback order:

```text
000_contact.png
001_inbetween.png
002_down.png
003_inbetween.png
004_passing.png
...
015_final_inbetween_before_contact.png
```

Then call the tool API:

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

The endpoint sorts filenames, removes transparent/dark-gray/green matte
artifacts, applies one global uniform scale, anchors every frame to the same
root position, writes the assembled sheet/frames/atlas, and emits continuity
warnings in `spritesheet_normalization.json`.

### Animation Job API

Use this stricter workflow when the user wants a polished walking loop rather
than a quick assembled sheet.

1. Start a job:

```bash
curl -s -X POST \
  -H "Authorization: Bearer sap-local" \
  -H "Content-Type: application/json" \
  -d '{"output":"/absolute/path/to/jobs/cat_walk","action":"cat_walk","frameCount":16,"columns":4,"frameWidth":256,"frameHeight":256,"prefix":"cat_walk"}' \
  http://127.0.0.1:9877/api/v1/animation-job/start
```

2. For each frame in `animation_job.json`, use its `prompt` and
`poseGuidePath` when generating the image. The prompt contract is also the
validator contract: keep the root/ground stable, make planted front/hind paws
readable, preserve paw spacing, lock style/palette, and avoid a duplicate tail
frame. Submit each candidate:

```bash
curl -s -X POST \
  -H "Authorization: Bearer sap-local" \
  -H "Content-Type: application/json" \
  -d '{"jobDir":"/absolute/path/to/jobs/cat_walk","frameIndex":0,"sourcePath":"/absolute/path/to/generated/000.png"}' \
  http://127.0.0.1:9877/api/v1/animation-job/submit-frame
```

3. If `accepted` is false, regenerate only that frame using the returned
`errors`, `warnings`, same pose guide, locked identity reference, and neighboring
accepted frames as references. The report includes foreground `style` stats and
lower-body `gait` stats, so use those to distinguish style drift from a frame
that is simply not walking. Do not assemble failed candidates unless the user
explicitly asks for a debug draft.

4. Assemble only after every frame is accepted:

```bash
curl -s -X POST \
  -H "Authorization: Bearer sap-local" \
  -H "Content-Type: application/json" \
  -d '{"jobDir":"/absolute/path/to/jobs/cat_walk","format":"png","asepriteJson":true}' \
  http://127.0.0.1:9877/api/v1/animation-job/assemble
```

5. Export the preview with `/api/v1/export-gif` and inspect
`spritesheet_normalization.json`. If continuity warnings remain, regenerate the
specific bad tail/neighbor frames and assemble again.

### Export Animation Preview

Use the tool API to export a preview GIF from the atlas produced by `/api/v1/slice`
or `/api/v1/pack`:

```bash
curl -s -X POST \
  -H "Authorization: Bearer sap-local" \
  -H "Content-Type: application/json" \
  --data-binary @export-gif.json \
  http://127.0.0.1:9877/api/v1/export-gif
```

`export-gif.json`:

```json
{
  "atlasPath": "/absolute/path/to/build/normalized/atlas.json",
  "sheetPath": "/absolute/path/to/build/normalized/sheet.png",
  "outputPath": "/absolute/path/to/build/normalized/preview.gif",
  "animation": "hero_walk",
  "fpsOverride": 8.0
}
```

### Latest Atlas

```bash
curl -s \
  -H "Authorization: Bearer sap-local" \
  http://127.0.0.1:9877/api/v1/atlas
```

Returns the latest `atlas` JSON plus `sheetPath` and `outputPath`.

### Live Events

Browser/game clients can subscribe to:

```text
ws://127.0.0.1:9877/api/v1/events?token=sap-local
```

Each successful `/pack` or `/slice` call broadcasts a JSON text frame with `type`, `atlas`, `sheetPath`, `outputPath`, and `wrote`.

## Agent Rules

- Prefer absolute paths in request bodies.
- Call `/api/v1/state` before doing work to confirm the server and token.
- After pack or slice, call `/api/v1/atlas` if you need to inspect generated metadata.
- Use `eventsUrl` when driving a running game/editor that should hot-reload after pack or slice.
- If the server is unreachable, fall back to the headless CLI command.
- Do not overwrite an output folder unless the user’s requested workflow implies it.
