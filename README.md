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

## Local Control API

Agents can drive a running local server with bearer-token auth. In the desktop
app, use the `Remote` panel to enable AI Agent Remote Control for the current
session; it starts off, then shows the local address and generated token after
you enable it. The same API can also be started from the CLI:

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
