# Sprite Atlas Packer Spec Completion TODO

Source spec: `~/git/openclaw/docs/sprite-atlas-packer-spec-2026-06-11.md`

## Done And Verified

- [x] Tauri/Rust + React/Vite app styled like `SeamlessImageEdit`.
- [x] Headless CLI pack mode with WebP default, PNG/both toggle, POT/NPOT, padding, trim, max size, rotation opt-in.
- [x] TexturePacker JSON Array compatible `atlas.json` with `animations[]`, pivots, `duration_ms`, `rotated`, trim metadata, top-left y-down coordinates.
- [x] PNG/JPG/WebP/GIF first-frame inputs, EXIF orientation handling, metadata-stripped outputs.
- [x] Optional `.ase`/`.aseprite` import through local Aseprite CLI.
- [x] Deterministic packing, golden fixture, trim recomposition, no-bleed sampling, Phaser/Pixi shape compatibility harness.
- [x] Packing algorithms: MaxRects default plus shelf, guillotine, quadtree.
- [x] Project file load/save and schema validation.
- [x] Reverse sheet slicer: grid auto-detect plus alpha connected components.
- [x] Previewer: generated atlas + sheet rendering, thumbnails, range selection, onion skin, loop, ping-pong, FPS override, arrow stepping.
- [x] GIF export, including selected preview range, using `gifski` when available and built-in GIF fallback.
- [x] Comparison atlas preview.
- [x] CLI watch mode and GUI hot-reload preview toggle.
- [x] AI Agent Control API with bearer auth, discovery file, pack/slice/state/atlas endpoints, bundled agent skill.
- [x] Near-duplicate dedup aliases and linked-channel sibling sheets.
- [x] Stable pivot/anchor option for trimmed frames.
- [x] Aseprite-flavored JSON sidecar.
- [x] Godot native `SpriteFrames` export artifact.
- [x] Unity editor import script artifact.
- [x] WebM preview export from the GUI previewer.
- [x] Paired CLI workflow with `SeamlessImageEdit` documented and verified against the sibling app's headless CLI flags.
- [x] Shared `image-prep` crate explicitly deferred: `SpriteAtlasPacker` and `SeamlessImageEdit` are separate repos, so extraction should happen in a future shared workspace/package rather than being invented locally here.
- [x] Live hot-reload server change-push/WebSocket for games via `/api/v1/events?token=...`.

## Spec Completion Status

Required in-repo spec work is implemented. Keep the exploratory backlog below separate from v1 completion.

## Explicitly Exploratory / Not Required To Ship V1

- [ ] Multi-scale variant export (`--scales`).
- [ ] Repack visual diff.
- [ ] Waste heatmap and deeper packing hints.
- [ ] Timing curve editor.
- [ ] Phone/QR preview.

## Environment-Limited Verification

- [ ] `cargo check --features desktop` and `npm run tauri:build` need Linux desktop build deps (`pkg-config`, DBus/WebKitGTK dev packages). Core Rust tests and frontend build pass without them.
