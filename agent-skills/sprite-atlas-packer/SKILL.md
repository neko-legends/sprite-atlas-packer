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
