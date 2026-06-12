import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open } from '@tauri-apps/plugin-dialog'
import { save } from '@tauri-apps/plugin-dialog'
import {
  BoxSelect,
  CheckCircle2,
  FileDown,
  FileJson,
  FolderOpen,
  Grid3X3,
  Image as ImageIcon,
  Layers,
  Loader2,
  Pause,
  Package,
  Play,
  Radio,
  RefreshCw,
  Ruler,
  Save,
  SkipBack,
  SkipForward,
  Sparkles,
  TriangleAlert,
  Upload,
  Video,
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

type OutputFormat = 'webp' | 'png' | 'both'
type PackingStrategy = 'maxrects' | 'shelf' | 'guillotine' | 'quadtree'

type PackOptions = {
  input: string
  output: string
  format: OutputFormat
  packingStrategy: PackingStrategy
  pot: boolean
  padding: number
  maxSize: number
  rotate: boolean
  trim: boolean
  dryRun: boolean
  asepriteJson: boolean
  godotSpriteframes: boolean
  unityImporter: boolean
  animationRegex?: string | null
  stablePivot: boolean
  dedupThreshold?: number | null
  linkedChannels: string[]
}

type ProjectOptions = Omit<PackOptions, 'dryRun'>

type SliceMode = 'grid' | 'alpha'

type SliceOptions = {
  input: string
  output: string
  format: OutputFormat
  mode: SliceMode
  cellWidth: number
  cellHeight: number
  prefix: string
  dryRun: boolean
  asepriteJson: boolean
}

type PackReport = {
  inputCount: number
  atlasWidth: number
  atlasHeight: number
  packingStrategy: string
  usedPixels: number
  wastePercent: number
  frames: string[]
  animations: string[]
  wrote: string[]
  aliases: AliasReport[]
  atlas: Atlas
}

type AliasReport = {
  frame: string
  target: string
}

type SliceReport = {
  frameCount: number
  sheetWidth: number
  sheetHeight: number
  frames: string[]
  wrote: string[]
  atlas: Atlas
}

type GifExportReport = {
  outputPath: string
  animation: string
  frameCount: number
  encoder: string
}

type AtlasPreviewLoad = {
  atlasPath: string
  sheetPath: string
  sheetDataUrl: string
  atlas: Atlas
}

type AgentControlInfo = {
  enabled: boolean
  host: string
  port: number
  token: string
  baseUrl: string
  discoveryFile: string
}

type Atlas = {
  meta: {
    image: string
    size: Size
  }
  frames: AtlasFrame[]
  animations?: AtlasAnimation[]
}

type AtlasFrame = {
  filename: string
  frame: Rect
  rotated: boolean
  trimmed: boolean
  spriteSourceSize: Rect
  sourceSize: Size
  pivot: { x: number; y: number }
  duration_ms?: number
}

type AtlasAnimation = {
  name: string
  frames: string[]
  loop: boolean
  fps_hint: number
}

type Rect = {
  x: number
  y: number
  w: number
  h: number
}

type Size = {
  w: number
  h: number
}

const SETTINGS_KEY = 'spriteAtlasPacker.settings.v1'

const defaultOptions: PackOptions = {
  input: '',
  output: '',
  format: 'webp',
  packingStrategy: 'maxrects',
  pot: true,
  padding: 2,
  maxSize: 16384,
  rotate: false,
  trim: true,
  dryRun: false,
  asepriteJson: false,
  godotSpriteframes: false,
  unityImporter: false,
  animationRegex: '',
  stablePivot: false,
  dedupThreshold: null,
  linkedChannels: [],
}

function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

function classNames(...items: Array<string | false | null | undefined>): string {
  return items.filter(Boolean).join(' ')
}

function compactPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean)
  if (parts.length <= 3) return path || 'None'
  const separator = path.includes('\\') ? '\\' : '/'
  return `...${separator}${parts.slice(-3).join(separator)}`
}

function fileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path
}

function normalizeLinkedChannels(value: string): string[] {
  return Array.from(
    new Set(
      value
        .split(',')
        .map((item) => item.trim())
        .filter(Boolean)
        .map((item) => (item.startsWith('_') ? item : `_${item}`)),
    ),
  ).sort()
}

function loadOptions(): PackOptions {
  try {
    const raw = window.localStorage.getItem(SETTINGS_KEY)
    if (!raw) return defaultOptions
    const parsed = JSON.parse(raw) as Partial<PackOptions>
    return {
      input: typeof parsed.input === 'string' ? parsed.input : '',
      output: typeof parsed.output === 'string' ? parsed.output : '',
      format: parsed.format === 'png' || parsed.format === 'both' || parsed.format === 'webp' ? parsed.format : 'webp',
      packingStrategy:
        parsed.packingStrategy === 'shelf' ||
        parsed.packingStrategy === 'guillotine' ||
        parsed.packingStrategy === 'quadtree' ||
        parsed.packingStrategy === 'maxrects'
          ? parsed.packingStrategy
          : 'maxrects',
      pot: typeof parsed.pot === 'boolean' ? parsed.pot : true,
      padding: typeof parsed.padding === 'number' ? Math.max(0, Math.min(16, parsed.padding)) : 2,
      maxSize: typeof parsed.maxSize === 'number' ? Math.max(1, Math.min(16384, parsed.maxSize)) : 16384,
      rotate: typeof parsed.rotate === 'boolean' ? parsed.rotate : false,
      trim: typeof parsed.trim === 'boolean' ? parsed.trim : true,
      dryRun: false,
      asepriteJson: typeof parsed.asepriteJson === 'boolean' ? parsed.asepriteJson : false,
      godotSpriteframes: typeof parsed.godotSpriteframes === 'boolean' ? parsed.godotSpriteframes : false,
      unityImporter: typeof parsed.unityImporter === 'boolean' ? parsed.unityImporter : false,
      animationRegex: typeof parsed.animationRegex === 'string' ? parsed.animationRegex : '',
      stablePivot: typeof parsed.stablePivot === 'boolean' ? parsed.stablePivot : false,
      dedupThreshold:
        typeof parsed.dedupThreshold === 'number' && Number.isFinite(parsed.dedupThreshold)
          ? Math.max(0, Math.min(255, parsed.dedupThreshold))
          : null,
      linkedChannels: Array.isArray(parsed.linkedChannels)
        ? parsed.linkedChannels.filter((item): item is string => typeof item === 'string')
        : [],
    }
  } catch {
    return defaultOptions
  }
}

function outputImageName(format: OutputFormat): string {
  return format === 'png' ? 'sheet.png' : 'sheet.webp'
}

function joinPath(folder: string, name: string): string {
  if (!folder) return name
  const separator = folder.includes('\\') ? '\\' : '/'
  return `${folder.replace(/[\\/]+$/, '')}${separator}${name}`
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value))
}

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms))
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image()
    image.onload = () => resolve(image)
    image.onerror = () => reject(new Error('Could not load sheet image for export'))
    image.src = src
  })
}

function drawAtlasFrame(context: CanvasRenderingContext2D, image: HTMLImageElement, frame: AtlasFrame, width: number, height: number) {
  context.clearRect(0, 0, width, height)
  context.imageSmoothingEnabled = false
  const scale = Math.min(width / Math.max(1, frame.sourceSize.w), height / Math.max(1, frame.sourceSize.h))
  const offsetX = Math.floor((width - frame.sourceSize.w * scale) / 2)
  const offsetY = Math.floor((height - frame.sourceSize.h * scale) / 2)
  const dx = offsetX + frame.spriteSourceSize.x * scale
  const dy = offsetY + frame.spriteSourceSize.y * scale
  const dw = frame.spriteSourceSize.w * scale
  const dh = frame.spriteSourceSize.h * scale
  if (frame.rotated) {
    context.save()
    context.translate(dx, dy + dh)
    context.rotate(-Math.PI / 2)
    context.drawImage(image, frame.frame.x, frame.frame.y, frame.frame.w, frame.frame.h, 0, 0, dh, dw)
    context.restore()
  } else {
    context.drawImage(image, frame.frame.x, frame.frame.y, frame.frame.w, frame.frame.h, dx, dy, dw, dh)
  }
}

function webmMimeType(): string {
  if (typeof MediaRecorder === 'undefined') return ''
  return ['video/webm;codecs=vp9', 'video/webm;codecs=vp8', 'video/webm'].find((type) => MediaRecorder.isTypeSupported(type)) ?? ''
}

async function renderWebmPreview(atlas: Atlas, sheetSrc: string, animationName: string, fpsOverride: number, frameStart: number, frameEnd: number): Promise<Blob> {
  if (typeof MediaRecorder === 'undefined') throw new Error('WebM export is not supported in this browser')
  const mimeType = webmMimeType()
  if (!mimeType) throw new Error('WebM export is not supported in this browser')

  const frameByName = new Map(atlas.frames.map((frame) => [frame.filename, frame]))
  const animation = atlas.animations?.find((item) => item.name === animationName) ?? atlas.animations?.[0] ?? {
    name: 'frames',
    frames: atlas.frames.map((frame) => frame.filename),
    loop: true,
    fps_hint: 8,
  }
  const allFrames = animation.frames.map((name) => frameByName.get(name)).filter((frame): frame is AtlasFrame => Boolean(frame))
  const start = clamp(Math.min(frameStart, frameEnd), 0, Math.max(0, allFrames.length - 1))
  const end = clamp(Math.max(frameStart, frameEnd), 0, Math.max(0, allFrames.length - 1))
  const frames = allFrames.slice(start, end + 1)
  if (!frames.length) throw new Error('No frames available for WebM export')

  const image = await loadImage(sheetSrc)
  const canvas = document.createElement('canvas')
  canvas.width = Math.max(1, ...frames.map((frame) => frame.sourceSize.w))
  canvas.height = Math.max(1, ...frames.map((frame) => frame.sourceSize.h))
  const context = canvas.getContext('2d')
  if (!context) throw new Error('Could not create WebM export canvas')
  if (!canvas.captureStream) throw new Error('WebM export is not supported in this browser')

  const fps = clamp(Math.round(fpsOverride > 0 ? fpsOverride : animation.fps_hint || 8), 1, 60)
  const stream = canvas.captureStream(fps)
  const chunks: BlobPart[] = []
  const recorder = new MediaRecorder(stream, { mimeType })

  const done = new Promise<Blob>((resolve, reject) => {
    recorder.ondataavailable = (event) => {
      if (event.data.size > 0) chunks.push(event.data)
    }
    recorder.onerror = (event) => reject((event as Event & { error?: DOMException }).error ?? new Error('WebM recording failed'))
    recorder.onstop = () => resolve(new Blob(chunks, { type: mimeType }))
  })

  recorder.start()
  for (const frame of frames) {
    drawAtlasFrame(context, image, frame, canvas.width, canvas.height)
    const duration = fpsOverride > 0 ? 1000 / fpsOverride : frame.duration_ms ?? 1000 / fps
    await wait(Math.max(30, duration))
  }
  recorder.stop()
  stream.getTracks().forEach((track) => track.stop())
  return done
}

function downloadBlob(blob: Blob, fileName: string) {
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = fileName
  anchor.click()
  URL.revokeObjectURL(url)
}

function AtlasPreview({
  atlas,
  sheetSrc,
  title = 'Animation Preview',
  onExportGif,
  onExportWebm,
}: {
  atlas: Atlas
  sheetSrc: string
  title?: string
  onExportGif?: (animation: string, fpsOverride: number, frameStart: number, frameEnd: number) => void
  onExportWebm?: (animation: string, fpsOverride: number, frameStart: number, frameEnd: number) => void
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const imageRef = useRef<HTMLImageElement | null>(null)
  const [imageReady, setImageReady] = useState(false)
  const [animationName, setAnimationName] = useState(atlas.animations?.[0]?.name ?? '')
  const [frameIndex, setFrameIndex] = useState(0)
  const [playing, setPlaying] = useState(false)
  const [loop, setLoop] = useState(true)
  const [pingPong, setPingPong] = useState(false)
  const [onionSkin, setOnionSkin] = useState(false)
  const [fpsOverride, setFpsOverride] = useState(0)
  const [rangeStart, setRangeStart] = useState(0)
  const [rangeEnd, setRangeEnd] = useState(0)
  const directionRef = useRef(1)

  const frameByName = useMemo(() => new Map(atlas.frames.map((frame) => [frame.filename, frame])), [atlas.frames])
  const animations = useMemo(() => {
    if (atlas.animations?.length) return atlas.animations
    return [{ name: 'frames', frames: atlas.frames.map((frame) => frame.filename), loop: true, fps_hint: 8 }]
  }, [atlas])
  const activeAnimation = animations.find((animation) => animation.name === animationName) ?? animations[0]
  const activeFrames = activeAnimation.frames.map((name) => frameByName.get(name)).filter((frame): frame is AtlasFrame => Boolean(frame))
  const maxFrameIndex = Math.max(0, activeFrames.length - 1)
  const rangeStartIndex = clamp(Math.min(rangeStart, rangeEnd), 0, maxFrameIndex)
  const rangeEndIndex = clamp(Math.max(rangeStart, rangeEnd), 0, maxFrameIndex)
  const currentFrame = activeFrames[clamp(frameIndex, rangeStartIndex, rangeEndIndex)]
  const fps = fpsOverride > 0 ? fpsOverride : activeAnimation.fps_hint || 8

  useEffect(() => {
    setAnimationName(animations[0]?.name ?? '')
    setFrameIndex(0)
    setPlaying(false)
  }, [animations])

  useEffect(() => {
    const maxIndex = Math.max(0, activeFrames.length - 1)
    setRangeStart(0)
    setRangeEnd(maxIndex)
    setFrameIndex(0)
    directionRef.current = 1
  }, [activeAnimation.name, activeFrames.length])

  useEffect(() => {
    setFrameIndex((current) => clamp(current, rangeStartIndex, rangeEndIndex))
  }, [rangeEndIndex, rangeStartIndex])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null
      const tagName = target?.tagName.toLowerCase()
      if (tagName === 'input' || tagName === 'select' || tagName === 'textarea') return
      if (event.key === 'ArrowLeft') {
        event.preventDefault()
        step(-1)
      }
      if (event.key === 'ArrowRight') {
        event.preventDefault()
        step(1)
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [activeFrames.length])

  useEffect(() => {
    setImageReady(false)
    const image = new Image()
    image.onload = () => {
      imageRef.current = image
      setImageReady(true)
    }
    image.src = sheetSrc
    return () => {
      imageRef.current = null
    }
  }, [sheetSrc])

  useEffect(() => {
    if (!playing || activeFrames.length <= 1) return
    const frame = currentFrame
    const duration = fpsOverride > 0 ? 1000 / fpsOverride : frame?.duration_ms ?? 1000 / fps
    const timeout = window.setTimeout(() => {
      setFrameIndex((current) => {
        const next = current + directionRef.current
        if (next >= rangeStartIndex && next <= rangeEndIndex) return next
        if (pingPong) {
          directionRef.current *= -1
          return clamp(current + directionRef.current, rangeStartIndex, rangeEndIndex)
        }
        if (loop || activeAnimation.loop) return rangeStartIndex
        setPlaying(false)
        return current
      })
    }, Math.max(30, duration))
    return () => window.clearTimeout(timeout)
  }, [activeAnimation.loop, activeFrames.length, currentFrame, fps, fpsOverride, loop, pingPong, playing, rangeEndIndex, rangeStartIndex])

  useEffect(() => {
    const canvas = canvasRef.current
    const image = imageRef.current
    if (!canvas || !imageReady || !image || !currentFrame) return
    const context = canvas.getContext('2d')
    if (!context) return

    const maxWidth = 520
    const maxHeight = 360
    const source = currentFrame.sourceSize
    const scale = Math.min(maxWidth / Math.max(1, source.w), maxHeight / Math.max(1, source.h), 5)
    const cssWidth = Math.max(180, Math.ceil(source.w * scale))
    const cssHeight = Math.max(160, Math.ceil(source.h * scale))
    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2)
    canvas.width = Math.round(cssWidth * pixelRatio)
    canvas.height = Math.round(cssHeight * pixelRatio)
    canvas.style.width = `${cssWidth}px`
    canvas.style.height = `${cssHeight}px`
    context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0)
    context.imageSmoothingEnabled = false
    context.clearRect(0, 0, cssWidth, cssHeight)

    const drawFrame = (frame: AtlasFrame, alpha: number, offsetX: number) => {
      context.save()
      context.globalAlpha = alpha
      const scaleX = cssWidth / Math.max(1, frame.sourceSize.w)
      const scaleY = cssHeight / Math.max(1, frame.sourceSize.h)
      const dx = frame.spriteSourceSize.x * scaleX + offsetX
      const dy = frame.spriteSourceSize.y * scaleY
      const dw = frame.spriteSourceSize.w * scaleX
      const dh = frame.spriteSourceSize.h * scaleY
      if (frame.rotated) {
        context.translate(dx, dy + dh)
        context.rotate(-Math.PI / 2)
        context.drawImage(image, frame.frame.x, frame.frame.y, frame.frame.w, frame.frame.h, 0, 0, dh, dw)
      } else {
        context.drawImage(image, frame.frame.x, frame.frame.y, frame.frame.w, frame.frame.h, dx, dy, dw, dh)
      }
      context.restore()
    }

    if (onionSkin && activeFrames.length > 1) {
      const previous = activeFrames[clamp(frameIndex - 1, 0, activeFrames.length - 1)]
      const next = activeFrames[clamp(frameIndex + 1, 0, activeFrames.length - 1)]
      if (previous && previous !== currentFrame) drawFrame(previous, 0.32, -4)
      if (next && next !== currentFrame) drawFrame(next, 0.26, 4)
    }
    drawFrame(currentFrame, 1, 0)
  }, [activeFrames, currentFrame, frameIndex, imageReady, onionSkin])

  function step(delta: number) {
    setPlaying(false)
    setFrameIndex((current) => clamp(current + delta, rangeStartIndex, rangeEndIndex))
  }

  return (
    <section className="preview-panel animator-panel">
      <div className="preview-head">
        <div className="preview-title">
          <strong>{title}</strong>
          <small>{currentFrame?.filename ?? 'No frame selected'}</small>
        </div>
        <select
          className="animation-select"
          value={activeAnimation.name}
          onChange={(event) => {
            directionRef.current = 1
            setAnimationName(event.currentTarget.value)
            setFrameIndex(0)
          }}
        >
          {animations.map((animation) => (
            <option key={animation.name} value={animation.name}>
              {animation.name}
            </option>
          ))}
        </select>
      </div>

      <div className="animation-stage">
        <canvas ref={canvasRef} />
      </div>

      <div className="transport-row">
        <button className="icon-button compact" type="button" onClick={() => step(-1)} title="Previous frame">
          <SkipBack size={15} />
        </button>
        <button className="icon-button compact" type="button" onClick={() => setPlaying((current) => !current)} title="Play or pause">
          {playing ? <Pause size={15} /> : <Play size={15} />}
        </button>
        <button className="icon-button compact" type="button" onClick={() => step(1)} title="Next frame">
          <SkipForward size={15} />
        </button>
        <label className="mini-toggle">
          <input type="checkbox" checked={loop} onChange={(event) => setLoop(event.currentTarget.checked)} />
          <span>Loop</span>
        </label>
        <label className="mini-toggle">
          <input type="checkbox" checked={pingPong} onChange={(event) => setPingPong(event.currentTarget.checked)} />
          <span>Ping-pong</span>
        </label>
        <label className="mini-toggle">
          <input type="checkbox" checked={onionSkin} onChange={(event) => setOnionSkin(event.currentTarget.checked)} />
          <span>Onion</span>
        </label>
        <label className="fps-field">
          <span>FPS</span>
          <input
            min={0}
            max={60}
            step={1}
            type="number"
            value={fpsOverride}
            onChange={(event) => setFpsOverride(clamp(Number(event.currentTarget.value) || 0, 0, 60))}
          />
        </label>
        <button
          className="icon-button compact"
          type="button"
          onClick={() => onExportGif?.(activeAnimation.name, fpsOverride, rangeStartIndex, rangeEndIndex)}
          title="Export GIF"
          disabled={!onExportGif}
        >
          <FileDown size={15} />
        </button>
        <button
          className="icon-button compact"
          type="button"
          onClick={() => onExportWebm?.(activeAnimation.name, fpsOverride, rangeStartIndex, rangeEndIndex)}
          title="Export WebM"
          disabled={!onExportWebm}
        >
          <Video size={15} />
        </button>
      </div>

      <input
        aria-label="Animation playhead"
        className="playhead-slider"
        disabled={activeFrames.length <= 1}
        max={rangeEndIndex}
        min={rangeStartIndex}
        step={1}
        type="range"
        value={clamp(frameIndex, rangeStartIndex, rangeEndIndex)}
        onChange={(event) => {
          setPlaying(false)
          setFrameIndex(clamp(Number(event.currentTarget.value) || 0, rangeStartIndex, rangeEndIndex))
        }}
      />

      <div className="range-row">
        <label className="fps-field">
          <span>Start</span>
          <input
            min={0}
            max={rangeEndIndex}
            type="number"
            value={rangeStartIndex}
            onChange={(event) => {
              setPlaying(false)
              const value = clamp(Number(event.currentTarget.value) || 0, 0, rangeEndIndex)
              setRangeStart(value)
              setFrameIndex((current) => clamp(current, value, rangeEndIndex))
            }}
          />
        </label>
        <label className="fps-field">
          <span>End</span>
          <input
            min={rangeStartIndex}
            max={maxFrameIndex}
            type="number"
            value={rangeEndIndex}
            onChange={(event) => {
              setPlaying(false)
              const value = clamp(Number(event.currentTarget.value) || 0, rangeStartIndex, maxFrameIndex)
              setRangeEnd(value)
              setFrameIndex((current) => clamp(current, rangeStartIndex, value))
            }}
          />
        </label>
      </div>

      <div className="timeline-strip">
        {activeFrames.map((frame, index) => (
          <FrameThumb
            active={index === frameIndex}
            frame={frame}
            image={imageRef.current}
            imageReady={imageReady}
            selected={index >= rangeStartIndex && index <= rangeEndIndex}
            key={frame.filename}
            onClick={() => {
              setPlaying(false)
              setFrameIndex(index)
            }}
          />
        ))}
      </div>
    </section>
  )
}

function FrameThumb({
  active,
  frame,
  image,
  imageReady,
  selected,
  onClick,
}: {
  active: boolean
  frame: AtlasFrame
  image: HTMLImageElement | null
  imageReady: boolean
  selected: boolean
  onClick: () => void
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || !image || !imageReady) return
    const context = canvas.getContext('2d')
    if (!context) return
    const size = 54
    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2)
    canvas.width = size * pixelRatio
    canvas.height = size * pixelRatio
    context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0)
    context.imageSmoothingEnabled = false
    context.clearRect(0, 0, size, size)
    const scale = Math.min(size / Math.max(1, frame.sourceSize.w), size / Math.max(1, frame.sourceSize.h))
    const dx = (size - frame.sourceSize.w * scale) / 2 + frame.spriteSourceSize.x * scale
    const dy = (size - frame.sourceSize.h * scale) / 2 + frame.spriteSourceSize.y * scale
    const dw = frame.spriteSourceSize.w * scale
    const dh = frame.spriteSourceSize.h * scale
    context.save()
    if (frame.rotated) {
      context.translate(dx, dy + dh)
      context.rotate(-Math.PI / 2)
      context.drawImage(image, frame.frame.x, frame.frame.y, frame.frame.w, frame.frame.h, 0, 0, dh, dw)
    } else {
      context.drawImage(image, frame.frame.x, frame.frame.y, frame.frame.w, frame.frame.h, dx, dy, dw, dh)
    }
    context.restore()
  }, [frame, image, imageReady])

  return (
    <button className={classNames('timeline-thumb', selected && 'selected', active && 'active')} type="button" onClick={onClick} title={frame.filename}>
      <canvas ref={canvasRef} />
      <span>{frame.filename}</span>
    </button>
  )
}

function App() {
  const [options, setOptions] = useState<PackOptions>(loadOptions)
  const [sliceOptions, setSliceOptions] = useState<SliceOptions>({
    input: '',
    output: '',
    format: 'png',
    mode: 'grid',
    cellWidth: 32,
    cellHeight: 32,
    prefix: 'slice',
    dryRun: false,
    asepriteJson: false,
  })
  const [report, setReport] = useState<PackReport | null>(null)
  const [sliceReport, setSliceReport] = useState<SliceReport | null>(null)
  const [notice, setNotice] = useState('Ready')
  const [busy, setBusy] = useState(false)
  const [dragging, setDragging] = useState(false)
  const [sheetPreview, setSheetPreview] = useState<string>('')
  const [comparison, setComparison] = useState<AtlasPreviewLoad | null>(null)
  const [agentControl, setAgentControl] = useState<AgentControlInfo | null>(null)
  const [hotReload, setHotReload] = useState(false)
  const [hotReloadSignature, setHotReloadSignature] = useState('')
  const [log, setLog] = useState<string[]>([])

  const canRun = Boolean(options.input.trim()) && Boolean(options.output.trim()) && !busy
  const atlasSize = report ? `${report.atlasWidth} x ${report.atlasHeight}` : '0 x 0'
  const frameCount = report?.frames.length ?? 0
  const animationCount = report?.animations.length ?? 0
  const waste = report ? `${report.wastePercent.toFixed(1)}%` : '0.0%'
  const packSignature = useMemo(() => JSON.stringify({ ...options, dryRun: false }), [options])

  const pushLog = useCallback((message: string) => {
    setLog((current) => [message, ...current].slice(0, 80))
  }, [])

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      window.localStorage.setItem(SETTINGS_KEY, JSON.stringify({ ...options, dryRun: false }))
    }, 150)
    return () => window.clearTimeout(timeout)
  }, [options])

  useEffect(() => {
    if (!isTauriRuntime()) return
    void refreshAgentControl()
  }, [])

  useEffect(() => {
    if (!isTauriRuntime()) return
    const cleanupDrag = getCurrentWindow().onDragDropEvent((event) => {
      if (event.payload.type === 'enter' || event.payload.type === 'over') {
        setDragging(true)
      }
      if (event.payload.type === 'leave') {
        setDragging(false)
      }
      if (event.payload.type === 'drop') {
        setDragging(false)
        const folder = event.payload.paths[0]
        if (folder) {
          setOptions((current) => ({ ...current, input: folder }))
          setNotice(`Input set to ${fileName(folder)}`)
          pushLog(`Input folder: ${folder}`)
        }
      }
    })
    return () => {
      void cleanupDrag.then((unlisten) => unlisten())
    }
  }, [pushLog])

  useEffect(() => {
    if (!hotReload || !isTauriRuntime() || !options.input || !options.output) return
    let cancelled = false
    let running = false
    const tick = async () => {
      if (running || busy) return
      running = true
      try {
        const snapshot = await invoke<string>('source_snapshot_gui', { input: options.input })
        const signature = `${packSignature}\n${snapshot}`
        if (!cancelled && signature !== hotReloadSignature) {
          await runHotReloadPack(signature)
        }
      } catch (error) {
        const message = String(error)
        if (!cancelled) {
          setNotice(message)
          pushLog(`Hot reload failed: ${message}`)
        }
      } finally {
        running = false
      }
    }
    void tick()
    const interval = window.setInterval(() => void tick(), 1500)
    return () => {
      cancelled = true
      window.clearInterval(interval)
    }
  }, [busy, hotReload, hotReloadSignature, options.input, options.output, packSignature, pushLog])

  async function refreshAgentControl() {
    if (!isTauriRuntime()) return
    try {
      setAgentControl(await invoke<AgentControlInfo>('agent_control_status_gui'))
    } catch (error) {
      pushLog(`Remote control status failed: ${String(error)}`)
    }
  }

  async function toggleAgentControl() {
    if (!isTauriRuntime()) {
      setNotice('Desktop runtime required for remote control')
      return
    }
    try {
      const next = await invoke<AgentControlInfo>(agentControl?.enabled ? 'stop_agent_control_gui' : 'start_agent_control_gui')
      setAgentControl(next)
      setNotice(next.enabled ? `Remote control enabled on ${next.baseUrl}` : 'Remote control disabled')
      pushLog(next.enabled ? `Remote control enabled: ${next.baseUrl}` : 'Remote control disabled.')
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`Remote control failed: ${message}`)
    }
  }

  async function copyAgentToken() {
    if (!agentControl?.token) return
    try {
      await window.navigator.clipboard.writeText(agentControl.token)
      setNotice('Remote token copied')
    } catch {
      setNotice('Unable to copy token')
    }
  }

  async function chooseInput() {
    if (!isTauriRuntime()) {
      setNotice('Desktop runtime required for folder picking')
      return
    }
    const selected = await open({ directory: true, multiple: false, defaultPath: options.input || undefined })
    if (typeof selected === 'string') {
      setOptions((current) => ({ ...current, input: selected }))
      setNotice(`Input set to ${fileName(selected)}`)
      pushLog(`Input folder: ${selected}`)
    }
  }

  async function chooseOutput() {
    if (!isTauriRuntime()) {
      setNotice('Desktop runtime required for folder picking')
      return
    }
    const selected = await open({ directory: true, multiple: false, defaultPath: options.output || undefined })
    if (typeof selected === 'string') {
      setOptions((current) => ({ ...current, output: selected }))
      setNotice(`Output set to ${fileName(selected)}`)
      pushLog(`Output folder: ${selected}`)
    }
  }

  async function chooseSheet() {
    if (!isTauriRuntime()) {
      setNotice('Desktop runtime required for file picking')
      return
    }
    const selected = await open({
      directory: false,
      multiple: false,
      filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif'] }],
    })
    if (typeof selected === 'string') {
      setSliceOptions((current) => ({ ...current, input: selected, output: current.output || options.output }))
      setNotice(`Sheet set to ${fileName(selected)}`)
      pushLog(`Sheet source: ${selected}`)
    }
  }

  async function runPack(dryRun: boolean) {
    if (!canRun && !dryRun) {
      setNotice('Choose input and output folders')
      return
    }
    if (!options.input || (!options.output && !dryRun)) {
      setNotice('Choose input and output folders')
      return
    }
    setBusy(true)
    setSheetPreview('')
    setNotice(dryRun ? 'Planning atlas...' : 'Packing atlas...')
    try {
      const nextReport = await invoke<PackReport>('pack_atlas_gui', {
        options: { ...options, dryRun },
      })
      setReport(nextReport)
      setSliceReport(null)
      const summary = `${nextReport.inputCount} sprites, ${nextReport.atlasWidth} x ${nextReport.atlasHeight}, ${nextReport.wastePercent.toFixed(1)}% waste`
      setNotice(dryRun ? `Dry run: ${summary}` : `Packed: ${summary}`)
      pushLog(`${dryRun ? 'Dry run' : 'Packed'} ${summary}.`)
      if (!dryRun) {
        const preview = await invoke<string>('preview_image_data_url', {
          path: joinPath(options.output, outputImageName(options.format)),
        })
        setSheetPreview(preview)
      }
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`Error: ${message}`)
    } finally {
      setBusy(false)
    }
  }

  async function runHotReloadPack(signature: string) {
    setBusy(true)
    setSheetPreview('')
    setNotice('Hot-reloading atlas...')
    try {
      const nextReport = await invoke<PackReport>('pack_atlas_gui', {
        options: { ...options, dryRun: false },
      })
      setReport(nextReport)
      setSliceReport(null)
      const summary = `${nextReport.inputCount} sprites, ${nextReport.atlasWidth} x ${nextReport.atlasHeight}, ${nextReport.wastePercent.toFixed(1)}% waste`
      setNotice(`Hot reload: ${summary}`)
      pushLog(`Hot reload packed ${summary}.`)
      const preview = await invoke<string>('preview_image_data_url', {
        path: joinPath(options.output, outputImageName(options.format)),
      })
      setSheetPreview(preview)
      setHotReloadSignature(signature)
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`Hot reload error: ${message}`)
    } finally {
      setBusy(false)
    }
  }

  async function runSlice(dryRun: boolean) {
    const output = sliceOptions.output || options.output
    if (!sliceOptions.input || (!output && !dryRun)) {
      setNotice('Choose a sheet image and output folder')
      return
    }
    setBusy(true)
    setSheetPreview('')
    setNotice(dryRun ? 'Planning sheet slice...' : 'Slicing sheet...')
    try {
      const nextReport = await invoke<SliceReport>('slice_sheet_gui', {
        options: { ...sliceOptions, output, dryRun },
      })
      setSliceReport(nextReport)
      setReport(null)
      const summary = `${nextReport.frameCount} frames from ${nextReport.sheetWidth} x ${nextReport.sheetHeight}`
      setNotice(dryRun ? `Slice dry run: ${summary}` : `Sliced: ${summary}`)
      pushLog(`${dryRun ? 'Slice dry run' : 'Sliced'} ${summary}.`)
      if (!dryRun) {
        const preview = await invoke<string>('preview_image_data_url', {
          path: joinPath(output, outputImageName(sliceOptions.format)),
        })
        setSheetPreview(preview)
      }
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`Slice error: ${message}`)
    } finally {
      setBusy(false)
    }
  }

  async function openOutput() {
    if (!isTauriRuntime() || !options.output) return
    try {
      await invoke('open_path', { path: options.output })
    } catch (error) {
      setNotice(String(error))
    }
  }

  async function loadProject() {
    if (!isTauriRuntime()) {
      setNotice('Desktop runtime required for project files')
      return
    }
    const selected = await open({
      directory: false,
      multiple: false,
      filters: [{ name: 'Atlas Project', extensions: ['atlasproj.json', 'json'] }],
    })
    if (typeof selected !== 'string') return
    try {
      const project = await invoke<ProjectOptions>('load_project_gui', { path: selected })
      setOptions({ ...project, dryRun: false })
      setReport(null)
      setSheetPreview('')
      setNotice(`Loaded ${fileName(selected)}`)
      pushLog(`Loaded project: ${selected}`)
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`Project load failed: ${message}`)
    }
  }

  async function saveProject() {
    if (!isTauriRuntime()) {
      setNotice('Desktop runtime required for project files')
      return
    }
    const selected = await save({
      defaultPath: 'atlas.atlasproj.json',
      filters: [{ name: 'Atlas Project', extensions: ['json'] }],
    })
    if (typeof selected !== 'string') return
    try {
      const { dryRun: _dryRun, ...projectOptions } = options
      await invoke('save_project_gui', { path: selected, options: projectOptions })
      setNotice(`Saved ${fileName(selected)}`)
      pushLog(`Saved project: ${selected}`)
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`Project save failed: ${message}`)
    }
  }

  async function loadComparisonAtlas() {
    if (!isTauriRuntime()) {
      setNotice('Desktop runtime required for comparison atlases')
      return
    }
    const selected = await open({
      directory: false,
      multiple: false,
      filters: [{ name: 'Atlas JSON', extensions: ['json'] }],
    })
    if (typeof selected !== 'string') return
    try {
      const loaded = await invoke<AtlasPreviewLoad>('load_atlas_preview_gui', { path: selected })
      setComparison(loaded)
      setNotice(`Comparison loaded: ${fileName(selected)}`)
      pushLog(`Loaded comparison atlas: ${selected}`)
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`Comparison load failed: ${message}`)
    }
  }

  async function exportGif(animation: string, fpsOverride: number, frameStart: number, frameEnd: number) {
    if (!isTauriRuntime()) {
      setNotice('Desktop runtime required for GIF export')
      return
    }
    const outputFolder = report ? options.output : sliceReport ? sliceOptions.output || options.output : ''
    const sheetName = report ? outputImageName(options.format) : sliceReport ? outputImageName(sliceOptions.format) : ''
    if (!outputFolder || !sheetName) {
      setNotice('Pack or slice an atlas before exporting')
      return
    }
    const selected = await save({
      defaultPath: `${animation || 'animation'}.gif`,
      filters: [{ name: 'GIF', extensions: ['gif'] }],
    })
    if (typeof selected !== 'string') return
    try {
      const result = await invoke<GifExportReport>('export_gif_gui', {
        options: {
          atlasPath: joinPath(outputFolder, 'atlas.json'),
          sheetPath: joinPath(outputFolder, sheetName),
          outputPath: selected,
          animation,
          fpsOverride,
          frameStart,
          frameEnd,
        },
      })
      setNotice(`Exported ${fileName(result.outputPath)} (${result.frameCount} frames, ${result.encoder})`)
      pushLog(`Exported GIF ${result.outputPath} with ${result.encoder}.`)
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`GIF export failed: ${message}`)
    }
  }

  async function exportWebm(animation: string, fpsOverride: number, frameStart: number, frameEnd: number) {
    const atlas = report?.atlas ?? sliceReport?.atlas
    if (!atlas || !sheetPreview) {
      setNotice('Pack or slice an atlas before exporting')
      return
    }
    let selected = ''
    if (isTauriRuntime()) {
      const saved = await save({
        defaultPath: `${animation || 'animation'}.webm`,
        filters: [{ name: 'WebM', extensions: ['webm'] }],
      })
      if (typeof saved !== 'string') return
      selected = saved
    }
    try {
      const blob = await renderWebmPreview(atlas, sheetPreview, animation, fpsOverride, frameStart, frameEnd)
      if (selected) {
        const bytes = Array.from(new Uint8Array(await blob.arrayBuffer()))
        await invoke('write_binary_file_gui', { path: selected, bytes })
        setNotice(`Exported ${fileName(selected)}`)
        pushLog(`Exported WebM ${selected}.`)
      } else {
        downloadBlob(blob, `${animation || 'animation'}.webm`)
        setNotice('Exported WebM')
        pushLog(`Exported WebM ${animation || 'animation'}.webm.`)
      }
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`WebM export failed: ${message}`)
    }
  }

  function reset() {
    setReport(null)
    setSliceReport(null)
    setSheetPreview('')
    setComparison(null)
    setHotReloadSignature('')
    setNotice('Ready')
  }

  const displayedAnimations = report?.animations ?? sliceReport?.atlas.animations?.map((animation) => animation.name) ?? []
  const displayedWrote = report?.wrote.join(', ') || sliceReport?.wrote.join(', ') || 'None'
  const displayedAtlas = report?.atlas ?? sliceReport?.atlas
  const displayedPreview = sheetPreview

  return (
    <div className="app-shell theme-neko-tron">
      <header className="topbar">
        <div className="brand-lockup">
          <div className="brand-icon">AP</div>
          <div>
            <h1>Sprite Atlas Packer</h1>
            <p>{notice}</p>
          </div>
        </div>
        <div className="topbar-actions">
          <button className="secondary-action" type="button" onClick={chooseInput} title="Choose input folder">
            <FolderOpen size={17} />
            Input
          </button>
          <button className="secondary-action" type="button" onClick={chooseOutput} title="Choose output folder">
            <FolderOpen size={17} />
            Output
          </button>
          <button className="icon-button" type="button" onClick={reset} title="Reset report">
            <RefreshCw size={17} />
          </button>
          <button className="icon-button" type="button" onClick={loadProject} title="Load project">
            <Upload size={17} />
          </button>
          <button className="icon-button" type="button" onClick={saveProject} title="Save project">
            <Save size={17} />
          </button>
          <button className="icon-button" type="button" onClick={() => void loadComparisonAtlas()} title="Load comparison atlas">
            <FileJson size={17} />
          </button>
          <button className="primary-action" type="button" onClick={() => void runPack(false)} disabled={!canRun}>
            {busy ? <Loader2 className="spin" size={18} /> : <Package size={18} />}
            Pack Atlas
          </button>
        </div>
      </header>

      <main className="workspace">
        <aside className="tool-panel">
          <section className="panel-section">
            <div className="section-title">
              <Sparkles size={16} />
              Source
            </div>
            <div className="folder-row">
              <input value={compactPath(options.input)} readOnly />
              <button className="icon-button compact" type="button" onClick={chooseInput} title="Choose input folder">
                <FolderOpen size={15} />
              </button>
            </div>
            <div className="folder-row">
              <input value={compactPath(options.output)} readOnly />
              <button className="icon-button compact" type="button" onClick={chooseOutput} title="Choose output folder">
                <FolderOpen size={15} />
              </button>
            </div>
            <label className="toggle-row">
              <input
                type="checkbox"
                checked={hotReload}
                onChange={(event) => {
                  setHotReload(event.currentTarget.checked)
                  setHotReloadSignature('')
                }}
              />
              <span>Hot reload preview</span>
            </label>
          </section>

          <section className="panel-section">
            <div className="section-title">
              <ImageIcon size={16} />
              Format
            </div>
            <div className="format-toggle three">
              {(['webp', 'png', 'both'] as OutputFormat[]).map((format) => (
                <button
                  className={classNames('format-button', options.format === format && 'active')}
                  key={format}
                  type="button"
                  onClick={() => setOptions((current) => ({ ...current, format }))}
                >
                  {format.toUpperCase()}
                </button>
              ))}
            </div>
            <div className="format-toggle">
              {(['maxrects', 'guillotine', 'quadtree', 'shelf'] as PackingStrategy[]).map((packingStrategy) => (
                <button
                  className={classNames('format-button', options.packingStrategy === packingStrategy && 'active')}
                  key={packingStrategy}
                  type="button"
                  onClick={() => setOptions((current) => ({ ...current, packingStrategy }))}
                >
                  {packingStrategy === 'maxrects'
                    ? 'MAXRECTS'
                    : packingStrategy === 'guillotine'
                      ? 'GUILLOTINE'
                      : packingStrategy === 'quadtree'
                        ? 'QUADTREE'
                        : 'SHELF'}
                </button>
              ))}
            </div>
            <label className="toggle-row">
              <input
                type="checkbox"
                checked={options.pot}
                onChange={(event) => setOptions((current) => ({ ...current, pot: event.currentTarget.checked }))}
              />
              <span>Power of two</span>
            </label>
            <label className="toggle-row">
              <input
                type="checkbox"
                checked={options.rotate}
                onChange={(event) => setOptions((current) => ({ ...current, rotate: event.currentTarget.checked }))}
              />
              <span>Allow rotation</span>
            </label>
            <label className="toggle-row">
              <input
                type="checkbox"
                checked={options.trim}
                onChange={(event) => setOptions((current) => ({ ...current, trim: event.currentTarget.checked }))}
              />
              <span>Trim transparent borders</span>
            </label>
            <label className="toggle-row">
              <input
                type="checkbox"
                checked={options.asepriteJson}
                onChange={(event) => setOptions((current) => ({ ...current, asepriteJson: event.currentTarget.checked }))}
              />
              <span>Aseprite JSON</span>
            </label>
            <label className="toggle-row">
              <input
                type="checkbox"
                checked={options.godotSpriteframes}
                onChange={(event) => setOptions((current) => ({ ...current, godotSpriteframes: event.currentTarget.checked }))}
              />
              <span>Godot SpriteFrames</span>
            </label>
            <label className="toggle-row">
              <input
                type="checkbox"
                checked={options.unityImporter}
                onChange={(event) => setOptions((current) => ({ ...current, unityImporter: event.currentTarget.checked }))}
              />
              <span>Unity importer</span>
            </label>
            <label className="toggle-row">
              <input
                type="checkbox"
                checked={options.stablePivot}
                onChange={(event) => setOptions((current) => ({ ...current, stablePivot: event.currentTarget.checked }))}
              />
              <span>Stable pivot</span>
            </label>
            <label className="field">
              <span>Animation Regex</span>
              <input
                placeholder="^(?P<name>.+)_(?P<index>\\d+)$"
                value={options.animationRegex ?? ''}
                onChange={(event) => setOptions((current) => ({ ...current, animationRegex: event.currentTarget.value }))}
              />
            </label>
            <label className="field">
              <span>Dedup Threshold</span>
              <input
                min={0}
                max={255}
                placeholder="Off"
                type="number"
                value={options.dedupThreshold ?? ''}
                onChange={(event) =>
                  setOptions((current) => ({
                    ...current,
                    dedupThreshold:
                      event.currentTarget.value === ''
                        ? null
                        : Math.max(0, Math.min(255, Number(event.currentTarget.value) || 0)),
                  }))
                }
              />
            </label>
            <label className="field">
              <span>Linked Channels</span>
              <input
                placeholder="_n,_e,_m"
                value={options.linkedChannels.join(',')}
                onChange={(event) =>
                  setOptions((current) => ({
                    ...current,
                    linkedChannels: normalizeLinkedChannels(event.currentTarget.value),
                  }))
                }
              />
            </label>
          </section>

          <section className="panel-section">
            <div className="section-title">
              <Ruler size={16} />
              Padding
            </div>
            <div className="stepper-row">
              <button
                className="icon-button compact"
                type="button"
                onClick={() => setOptions((current) => ({ ...current, padding: Math.max(0, current.padding - 1) }))}
                title="Decrease padding"
              >
                -
              </button>
              <strong>{options.padding}px</strong>
              <button
                className="icon-button compact"
                type="button"
                onClick={() => setOptions((current) => ({ ...current, padding: Math.min(16, current.padding + 1) }))}
                title="Increase padding"
              >
                +
              </button>
            </div>
            <label className="field">
              <span>Max Size</span>
              <input
                min={1}
                max={16384}
                type="number"
                value={options.maxSize}
                onChange={(event) =>
                  setOptions((current) => ({
                    ...current,
                    maxSize: Math.max(1, Math.min(16384, Number(event.currentTarget.value) || 1)),
                  }))
                }
              />
            </label>
            <button className="secondary-action full" type="button" onClick={() => void runPack(true)} disabled={!options.input || busy}>
              <Play size={16} />
              Dry Run
            </button>
          </section>

          <section className="panel-section">
            <div className="section-title">
              <Radio size={16} />
              Remote
            </div>
            <label className="toggle-row">
              {agentControl?.enabled ? <CheckCircle2 size={17} /> : <TriangleAlert size={17} />}
              <span>{agentControl?.enabled ? 'AI Agent Remote Control On' : 'AI Agent Remote Control Off'}</span>
            </label>
            <label className="field">
              <span>Address</span>
              <input value={agentControl?.baseUrl ?? 'http://127.0.0.1:9877/api/v1'} readOnly />
            </label>
            <label className="field">
              <span>Token</span>
              <input value={agentControl?.token ?? ''} readOnly />
            </label>
            <div className="folder-row">
              <button className="secondary-action full" type="button" onClick={() => void toggleAgentControl()}>
                <Radio size={16} />
                {agentControl?.enabled ? 'Disable Remote' : 'Enable Remote'}
              </button>
              <button
                className="icon-button compact"
                type="button"
                onClick={() => void copyAgentToken()}
                disabled={!agentControl?.token}
                title="Copy bearer token"
              >
                <FileJson size={15} />
              </button>
            </div>
          </section>

          <section className="panel-section">
            <div className="section-title">
              <Layers size={16} />
              Reverse
            </div>
            <div className="folder-row">
              <input value={compactPath(sliceOptions.input)} readOnly />
              <button className="icon-button compact" type="button" onClick={chooseSheet} title="Choose sheet image">
                <ImageIcon size={15} />
              </button>
            </div>
            <div className="format-toggle">
              {(['grid', 'alpha'] as SliceMode[]).map((mode) => (
                <button
                  className={classNames('format-button', sliceOptions.mode === mode && 'active')}
                  key={mode}
                  type="button"
                  onClick={() => setSliceOptions((current) => ({ ...current, mode }))}
                >
                  {mode.toUpperCase()}
                </button>
              ))}
            </div>
            {sliceOptions.mode === 'grid' ? (
              <div className="cell-grid">
                <label>
                  <span>W</span>
                  <input
                    min={1}
                    type="number"
                    value={sliceOptions.cellWidth}
                    onChange={(event) => setSliceOptions((current) => ({ ...current, cellWidth: Math.max(1, Number(event.currentTarget.value) || 1) }))}
                  />
                </label>
                <label>
                  <span>H</span>
                  <input
                    min={1}
                    type="number"
                    value={sliceOptions.cellHeight}
                    onChange={(event) => setSliceOptions((current) => ({ ...current, cellHeight: Math.max(1, Number(event.currentTarget.value) || 1) }))}
                  />
                </label>
              </div>
            ) : null}
            <label className="field">
              <span>Prefix</span>
              <input
                value={sliceOptions.prefix}
                onChange={(event) => setSliceOptions((current) => ({ ...current, prefix: event.currentTarget.value }))}
              />
            </label>
            <label className="toggle-row">
              <input
                type="checkbox"
                checked={sliceOptions.asepriteJson}
                onChange={(event) => setSliceOptions((current) => ({ ...current, asepriteJson: event.currentTarget.checked }))}
              />
              <span>Aseprite JSON</span>
            </label>
            <button className="secondary-action full" type="button" onClick={() => void runSlice(false)} disabled={!sliceOptions.input || busy}>
              <Layers size={16} />
              Slice Sheet
            </button>
          </section>
        </aside>

        <section className="main-surface">
          <div className="stats-row">
            <div>
              <span>{frameCount}</span>
              Frames
            </div>
            <div>
              <span>{animationCount}</span>
              Animations
            </div>
            <div>
              <span>{atlasSize}</span>
              Atlas
            </div>
            <div>
              <span>{waste}</span>
              Waste
            </div>
          </div>

          <button className={classNames('drop-zone', dragging && 'dragging')} type="button" onClick={chooseInput}>
            <div className="drop-glyph">
              <BoxSelect size={28} />
            </div>
            <div>
              <h2>{options.input ? fileName(options.input) : 'Drop Sprite Folder'}</h2>
              <p>{options.input ? compactPath(options.input) : 'PNG, JPG, WebP, GIF, Aseprite'}</p>
            </div>
          </button>

          <div className="preview-layout">
            {displayedAtlas && displayedPreview ? (
              <AtlasPreview
                atlas={displayedAtlas}
                sheetSrc={displayedPreview}
                onExportGif={(animation, fps, frameStart, frameEnd) => void exportGif(animation, fps, frameStart, frameEnd)}
                onExportWebm={(animation, fps, frameStart, frameEnd) => void exportWebm(animation, fps, frameStart, frameEnd)}
              />
            ) : (
              <section className="preview-panel animator-panel">
                <div className="preview-head">
                  <div className="preview-title">
                    <strong>Animation Preview</strong>
                    <small>No atlas yet</small>
                  </div>
                  <Play size={16} />
                </div>
                <div className="preview-empty">
                  <ImageIcon size={30} />
                </div>
              </section>
            )}

            {comparison ? (
              <AtlasPreview atlas={comparison.atlas} sheetSrc={comparison.sheetDataUrl} title="Comparison Preview" />
            ) : (
              <section className="preview-panel sheet-panel">
                <div className="preview-head">
                  <div className="preview-title">
                    <strong>Sheet</strong>
                    <small>{displayedAtlas ? displayedAtlas.meta.image : 'No atlas yet'}</small>
                  </div>
                  {displayedAtlas ? <CheckCircle2 size={16} /> : <TriangleAlert size={16} />}
                </div>
                {sheetPreview ? (
                  <img alt="Packed atlas sheet" src={sheetPreview} />
                ) : (
                  <button className="preview-empty preview-empty-button" type="button" onClick={() => void loadComparisonAtlas()}>
                    <FileJson size={30} />
                  </button>
                )}
              </section>
            )}

            <section className="queue-panel">
              <div className="preview-head">
              <div className="preview-title">
                <strong>Atlas JSON</strong>
                  <small>{displayedWrote}</small>
              </div>
                <FileJson size={16} />
              </div>
              <div className="report-list">
                {(displayedAnimations.length ? displayedAnimations : ['No animations inferred']).slice(0, 18).map((name) => (
                  <div className="report-row" key={name}>
                    <span>{name}</span>
                  </div>
                ))}
              </div>
              <button className="secondary-action full" type="button" onClick={openOutput} disabled={!options.output}>
                <FolderOpen size={16} />
                Open Output
              </button>
              <button
                className="secondary-action full"
                type="button"
                onClick={comparison ? () => setComparison(null) : () => void loadComparisonAtlas()}
              >
                <FileJson size={16} />
                {comparison ? 'Clear Compare' : 'Load Compare'}
              </button>
            </section>
          </div>

          <section className="log-panel">
            <div className="preview-head">
              <strong>Run Log</strong>
              <small>{log.length} entries</small>
            </div>
            <div className="log-list">
              {(log.length ? log : ['Ready.']).map((line, index) => (
                <div key={`${line}-${index}`}>{line}</div>
              ))}
            </div>
          </section>
        </section>
      </main>
    </div>
  )
}

export default App
