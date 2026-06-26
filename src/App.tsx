import { invoke, isTauri } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open } from '@tauri-apps/plugin-dialog'
import { save } from '@tauri-apps/plugin-dialog'
import { decompressFrames, parseGIF, type ParsedFrame } from 'gifuct-js'
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
import { useCallback, useEffect, useMemo, useRef, useState, type ChangeEvent, type DragEvent as ReactDragEvent } from 'react'

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
  normalizeGenerated: boolean
  frameWidth: number
  frameHeight: number
}

type VideoGridPreset = '4x4' | '5x5' | 'custom'

type VideoOptions = {
  input: string
  output: string
  gridPreset: VideoGridPreset
  columns: number
  rows: number
  frameWidth: number
  frameHeight: number
  startTime: number
  endTime: number
  fps: number
  prefix: string
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
  normalization?: SliceNormalizationReport
}

type SliceNormalizationReport = {
  sourceSheetSize: Size
  outputSheetSize: Size
  outputFrameSize: Size
  sourceColumns?: number
  sourceRows?: number
  sourceCells: Rect[]
  frameBboxes: Array<Rect | null>
  rootAnchors: Array<{ x: number; y: number } | null>
  scale: number
  gridPolicy: string
  gridDetection: string
  proportionPolicy: string
  rootAnchorPolicy: string
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

type ControlPackEvent = {
  type: 'pack'
  atlas: Atlas
  sheetPath: string
  outputPath: string
  wrote?: string[]
}

type ControlSliceEvent = {
  type: 'slice' | 'assembleGeneratedLoop' | 'animationJobAssemble'
  atlas: Atlas
  sheetPath: string
  outputPath: string
  wrote?: string[]
}

type ControlEvent = ControlPackEvent | ControlSliceEvent

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
const APP_VERSION_LABEL = 'v26.6.25'
const DEFAULT_GIF_FRAME_DURATION_MS = 120

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

const defaultVideoOptions: VideoOptions = {
  input: '',
  output: '',
  gridPreset: '4x4',
  columns: 4,
  rows: 4,
  frameWidth: 256,
  frameHeight: 256,
  startTime: 0,
  endTime: 0,
  fps: 12,
  prefix: 'video_walk',
}

function isTauriRuntime(): boolean {
  return isTauri()
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

function parentPath(path: string): string {
  const index = Math.max(path.lastIndexOf('\\'), path.lastIndexOf('/'))
  return index > 0 ? path.slice(0, index) : ''
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

function waitForVideoMetadata(video: HTMLVideoElement): Promise<void> {
  if (video.readyState >= 1 && Number.isFinite(video.duration)) return Promise.resolve()
  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      cleanup()
      reject(new Error('Video metadata did not load in time'))
    }, 8000)
    const cleanup = () => {
      window.clearTimeout(timeout)
      video.removeEventListener('loadedmetadata', handleMetadata)
      video.removeEventListener('error', handleError)
    }
    const handleMetadata = () => {
      cleanup()
      resolve()
    }
    const handleError = () => {
      cleanup()
      reject(new Error('Could not load the selected video'))
    }
    video.addEventListener('loadedmetadata', handleMetadata, { once: true })
    video.addEventListener('error', handleError, { once: true })
    video.load()
  })
}

function seekVideo(video: HTMLVideoElement, time: number): Promise<void> {
  return new Promise((resolve, reject) => {
    const target = clamp(time, 0, Number.isFinite(video.duration) ? video.duration : time)
    const timeout = window.setTimeout(() => {
      cleanup()
      reject(new Error(`Video seek timed out at ${target.toFixed(2)}s`))
    }, 8000)
    const cleanup = () => {
      window.clearTimeout(timeout)
      video.removeEventListener('seeked', handleSeeked)
      video.removeEventListener('error', handleError)
    }
    const handleSeeked = () => {
      cleanup()
      window.requestAnimationFrame(() => resolve())
    }
    const handleError = () => {
      cleanup()
      reject(new Error(`Could not seek video to ${target.toFixed(2)}s`))
    }
    video.addEventListener('seeked', handleSeeked, { once: true })
    video.addEventListener('error', handleError, { once: true })
    video.currentTime = target
  })
}

function drawVideoFrame(video: HTMLVideoElement, canvas: HTMLCanvasElement, width: number, height: number) {
  const context = canvas.getContext('2d')
  if (!context) throw new Error('Canvas drawing is unavailable')
  canvas.width = width
  canvas.height = height
  context.clearRect(0, 0, width, height)
  context.imageSmoothingEnabled = true
  context.imageSmoothingQuality = 'high'
  const sourceWidth = video.videoWidth || width
  const sourceHeight = video.videoHeight || height
  const scale = Math.min(width / Math.max(1, sourceWidth), height / Math.max(1, sourceHeight))
  const drawWidth = Math.round(sourceWidth * scale)
  const drawHeight = Math.round(sourceHeight * scale)
  const x = Math.floor((width - drawWidth) / 2)
  const y = Math.floor((height - drawHeight) / 2)
  context.drawImage(video, x, y, drawWidth, drawHeight)
}

function canvasToPngBytes(canvas: HTMLCanvasElement): Promise<number[]> {
  return new Promise((resolve, reject) => {
    canvas.toBlob(async (blob) => {
      if (!blob) {
        reject(new Error('Could not encode sampled frame as PNG'))
        return
      }
      try {
        resolve(Array.from(new Uint8Array(await blob.arrayBuffer())))
      } catch (error) {
        reject(error instanceof Error ? error : new Error(String(error)))
      }
    }, 'image/png')
  })
}

function agentEventsUrl(info: AgentControlInfo): string {
  const url = new URL(info.baseUrl)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  url.pathname = `${url.pathname.replace(/\/$/, '')}/events`
  url.searchParams.set('token', info.token)
  return url.toString()
}

function remotePackReport(event: ControlPackEvent): PackReport {
  const atlasWidth = event.atlas.meta.size.w
  const atlasHeight = event.atlas.meta.size.h
  const usedPixels = event.atlas.frames.reduce((total, frame) => total + frame.frame.w * frame.frame.h, 0)
  const atlasPixels = Math.max(1, atlasWidth * atlasHeight)
  return {
    inputCount: event.atlas.frames.length,
    atlasWidth,
    atlasHeight,
    packingStrategy: 'remote',
    usedPixels,
    wastePercent: Math.max(0, (1 - usedPixels / atlasPixels) * 100),
    frames: event.atlas.frames.map((frame) => frame.filename),
    animations: event.atlas.animations?.map((animation) => animation.name) ?? [],
    wrote: event.wrote ?? [],
    aliases: [],
    atlas: event.atlas,
  }
}

function remoteSliceReport(event: ControlSliceEvent): SliceReport {
  return {
    frameCount: event.atlas.frames.length,
    sheetWidth: event.atlas.meta.size.w,
    sheetHeight: event.atlas.meta.size.h,
    frames: event.atlas.frames.map((frame) => frame.filename),
    wrote: event.wrote ?? [],
    atlas: event.atlas,
  }
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image()
    image.onload = () => resolve(image)
    image.onerror = () => reject(new Error('Could not load sheet image for export'))
    image.src = src
  })
}

function isGifPath(value: string): boolean {
  return /\.gif(?:$|[?#])/i.test(value)
}

function isVideoPath(value: string): boolean {
  return /\.(?:mp4|webm|mov|m4v)(?:$|[?#])/i.test(value)
}

function videoMimeTypeFromPath(value: string): string {
  const extension = value.split(/[?#]/, 1)[0].split('.').pop()?.toLowerCase()
  if (extension === 'webm') return 'video/webm'
  if (extension === 'mov') return 'video/quicktime'
  return 'video/mp4'
}

function localPathFromFileUri(value: string): string | null {
  try {
    const url = new URL(value)
    if (url.protocol !== 'file:') return null
    const decoded = decodeURIComponent(url.pathname)
    const windowsPath = decoded.replace(/^\/([A-Za-z]:)/, '$1').replace(/\//g, '\\')
    return windowsPath || null
  } catch {
    return null
  }
}

function normalizeLocalPath(value: string): string {
  const trimmed = value.trim().replace(/^["']|["']$/g, '')
  return localPathFromFileUri(trimmed) ?? trimmed
}

function localPathsFromText(value: string): string[] {
  return value
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith('#'))
    .map(normalizeLocalPath)
}

function localPathsFromDataTransfer(dataTransfer: DataTransfer): string[] {
  const values = [dataTransfer.getData('text/uri-list'), dataTransfer.getData('text/plain')]
  return values.flatMap(localPathsFromText).filter(Boolean)
}

function isGifFile(file: File): boolean {
  return file.type === 'image/gif' || isGifPath(file.name)
}

function shouldIgnorePasteTarget(target: EventTarget | null): boolean {
  const element = target instanceof HTMLElement ? target : null
  return Boolean(element?.closest('input:not([readonly]), textarea, [contenteditable="true"]'))
}

function averageFrameDuration(frames: AtlasFrame[]): number {
  if (!frames.length) return DEFAULT_GIF_FRAME_DURATION_MS
  return frames.reduce((total, frame) => total + (frame.duration_ms ?? DEFAULT_GIF_FRAME_DURATION_MS), 0) / frames.length
}

async function dataUrlFromFile(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result ?? ''))
    reader.onerror = () => reject(new Error(`Could not read ${file.name}`))
    reader.readAsDataURL(file)
  })
}

async function imageSizeFromSource(src: string): Promise<Size> {
  const image = await loadImage(src)
  return { w: image.naturalWidth || image.width || 1, h: image.naturalHeight || image.height || 1 }
}

function atlasForSingleImage(name: string, size: Size): Atlas {
  const frameName = name.replace(/\.[^.]+$/, '') || 'image'
  return {
    meta: {
      image: name,
      size,
    },
    frames: [
      {
        filename: frameName,
        frame: { x: 0, y: 0, w: size.w, h: size.h },
        rotated: false,
        trimmed: false,
        spriteSourceSize: { x: 0, y: 0, w: size.w, h: size.h },
        sourceSize: size,
        pivot: { x: 0.5, y: 0.5 },
        duration_ms: DEFAULT_GIF_FRAME_DURATION_MS,
      },
    ],
    animations: [{ name: frameName, frames: [frameName], loop: true, fps_hint: 8 }],
  }
}

async function inspectBrowserImageFile(file: File): Promise<AtlasPreviewLoad> {
  const sheetDataUrl = await dataUrlFromFile(file)
  const size = await imageSizeFromSource(sheetDataUrl)
  return {
    atlasPath: file.name,
    sheetPath: file.name,
    sheetDataUrl,
    atlas: atlasForSingleImage(file.name, size),
  }
}

async function inspectBrowserGifFile(file: File): Promise<AtlasPreviewLoad> {
  const parsed = parseGIF(await file.arrayBuffer())
  const frames = decompressFrames(parsed, true)
  if (!frames.length) throw new Error(`GIF has no frames: ${file.name}`)

  const width = Math.max(1, parsed.lsd.width)
  const height = Math.max(1, parsed.lsd.height)
  const columns = Math.max(1, Math.ceil(Math.sqrt(frames.length)))
  const rows = Math.max(1, Math.ceil(frames.length / columns))
  const stage = document.createElement('canvas')
  const stageContext = stage.getContext('2d')
  const sheet = document.createElement('canvas')
  const sheetContext = sheet.getContext('2d')
  if (!stageContext || !sheetContext) throw new Error('Canvas drawing is unavailable')
  stage.width = width
  stage.height = height
  sheet.width = width * columns
  sheet.height = height * rows
  stageContext.clearRect(0, 0, width, height)
  sheetContext.clearRect(0, 0, sheet.width, sheet.height)

  let previousFrame: ParsedFrame | null = null
  let previousRestore: ImageData | null = null
  const stem = file.name.replace(/\.[^.]+$/, '') || 'gif'
  const atlasFrames: AtlasFrame[] = []
  const animationFrames: string[] = []

  frames.forEach((frame, index) => {
    if (previousFrame?.disposalType === 2) {
      stageContext.clearRect(previousFrame.dims.left, previousFrame.dims.top, previousFrame.dims.width, previousFrame.dims.height)
    } else if (previousFrame?.disposalType === 3 && previousRestore) {
      stageContext.putImageData(previousRestore, 0, 0)
    }

    const restoreForFrame = frame.disposalType === 3 ? stageContext.getImageData(0, 0, width, height) : null
    stageContext.putImageData(new ImageData(new Uint8ClampedArray(frame.patch), frame.dims.width, frame.dims.height), frame.dims.left, frame.dims.top)

    const column = index % columns
    const row = Math.floor(index / columns)
    const x = column * width
    const y = row * height
    sheetContext.drawImage(stage, x, y)

    const filename = `${stem}_${String(index).padStart(4, '0')}`
    const duration = Math.max(1, Math.round(frame.delay || DEFAULT_GIF_FRAME_DURATION_MS))
    animationFrames.push(filename)
    atlasFrames.push({
      filename,
      frame: { x, y, w: width, h: height },
      rotated: false,
      trimmed: false,
      spriteSourceSize: { x: 0, y: 0, w: width, h: height },
      sourceSize: { w: width, h: height },
      pivot: { x: 0.5, y: 0.5 },
      duration_ms: duration,
    })
    previousFrame = frame
    previousRestore = restoreForFrame
  })

  const averageDuration = averageFrameDuration(atlasFrames)
  const sheetName = `${stem}_inspect_sheet.png`
  return {
    atlasPath: file.name,
    sheetPath: sheetName,
    sheetDataUrl: sheet.toDataURL('image/png'),
    atlas: {
      meta: {
        image: sheetName,
        size: { w: sheet.width, h: sheet.height },
      },
      frames: atlasFrames,
      animations: [{ name: stem, frames: animationFrames, loop: true, fps_hint: clamp(1000 / averageDuration, 1, 60) }],
    },
  }
}

function reorderItems<T>(items: T[], fromIndex: number, toIndex: number): T[] {
  if (fromIndex === toIndex || fromIndex < 0 || toIndex < 0 || fromIndex >= items.length || toIndex >= items.length) return items
  const next = [...items]
  const [item] = next.splice(fromIndex, 1)
  next.splice(toIndex, 0, item)
  return next
}

function reorderAtlasAnimation(atlas: Atlas, animationName: string, fromIndex: number, toIndex: number): Atlas {
  const animations = atlas.animations?.length
    ? atlas.animations
    : [{ name: 'frames', frames: atlas.frames.map((frame) => frame.filename), loop: true, fps_hint: 8 }]
  const frameByName = new Map(atlas.frames.map((frame) => [frame.filename, frame]))
  const nextAnimations = animations.map((animation) =>
    animation.name === animationName ? { ...animation, frames: reorderItems(animation.frames, fromIndex, toIndex) } : animation,
  )
  const activeAnimation = nextAnimations.find((animation) => animation.name === animationName) ?? nextAnimations[0]
  const activeNames = new Set(activeAnimation.frames)
  const reorderedFrames = [
    ...activeAnimation.frames.map((name) => frameByName.get(name)).filter((frame): frame is AtlasFrame => Boolean(frame)),
    ...atlas.frames.filter((frame) => !activeNames.has(frame.filename)),
  ]
  return { ...atlas, frames: reorderedFrames, animations: nextAnimations }
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
  onReorderFrames,
}: {
  atlas: Atlas
  sheetSrc: string
  title?: string
  onExportGif?: (animation: string, fpsOverride: number, frameStart: number, frameEnd: number) => void
  onExportWebm?: (animation: string, fpsOverride: number, frameStart: number, frameEnd: number) => void
  onReorderFrames?: (animation: string, fromIndex: number, toIndex: number) => void
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const imageRef = useRef<HTMLImageElement | null>(null)
  const draggedFrameIndexRef = useRef<number | null>(null)
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

  function handleFrameDragStart(event: ReactDragEvent<HTMLButtonElement>, index: number) {
    if (!onReorderFrames) return
    draggedFrameIndexRef.current = index
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', String(index))
  }

  function handleFrameDrop(event: ReactDragEvent<HTMLButtonElement>, index: number) {
    if (!onReorderFrames) return
    event.preventDefault()
    const fromText = event.dataTransfer.getData('text/plain')
    const fallbackIndex = draggedFrameIndexRef.current
    const fromIndex = fromText ? Number(fromText) : fallbackIndex
    draggedFrameIndexRef.current = null
    if (typeof fromIndex !== 'number' || !Number.isInteger(fromIndex) || fromIndex === index) return
    setPlaying(false)
    onReorderFrames(activeAnimation.name, fromIndex, index)
    setFrameIndex(index)
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
            reorderable={Boolean(onReorderFrames)}
            selected={index >= rangeStartIndex && index <= rangeEndIndex}
            key={frame.filename}
            onClick={() => {
              setPlaying(false)
              setFrameIndex(index)
            }}
            onDragStart={(event) => handleFrameDragStart(event, index)}
            onDrop={(event) => handleFrameDrop(event, index)}
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
  reorderable,
  selected,
  onClick,
  onDragStart,
  onDrop,
}: {
  active: boolean
  frame: AtlasFrame
  image: HTMLImageElement | null
  imageReady: boolean
  reorderable: boolean
  selected: boolean
  onClick: () => void
  onDragStart: (event: ReactDragEvent<HTMLButtonElement>) => void
  onDrop: (event: ReactDragEvent<HTMLButtonElement>) => void
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
    <button
      className={classNames('timeline-thumb', reorderable && 'reorderable', selected && 'selected', active && 'active')}
      draggable={reorderable}
      type="button"
      onClick={onClick}
      onDragOver={(event) => {
        if (reorderable) event.preventDefault()
      }}
      onDragStart={onDragStart}
      onDrop={onDrop}
      title={frame.filename}
    >
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
    normalizeGenerated: false,
    frameWidth: 256,
    frameHeight: 256,
  })
  const [videoOptions, setVideoOptions] = useState<VideoOptions>(defaultVideoOptions)
  const [videoSourceUrl, setVideoSourceUrl] = useState('')
  const [videoDuration, setVideoDuration] = useState(0)
  const [videoPreviewGif, setVideoPreviewGif] = useState('')
  const [report, setReport] = useState<PackReport | null>(null)
  const [sliceReport, setSliceReport] = useState<SliceReport | null>(null)
  const [notice, setNotice] = useState('Ready')
  const [busy, setBusy] = useState(false)
  const [dragging, setDragging] = useState(false)
  const [sheetPreview, setSheetPreview] = useState<string>('')
  const [inspectedPreview, setInspectedPreview] = useState<AtlasPreviewLoad | null>(null)
  const [comparison, setComparison] = useState<AtlasPreviewLoad | null>(null)
  const [agentControl, setAgentControl] = useState<AgentControlInfo | null>(null)
  const [hotReload, setHotReload] = useState(false)
  const [hotReloadSignature, setHotReloadSignature] = useState('')
  const [log, setLog] = useState<string[]>([])
  const videoRef = useRef<HTMLVideoElement | null>(null)
  const videoObjectUrlRef = useRef<string>('')
  const browserSourceInputRef = useRef<HTMLInputElement | null>(null)
  const browserSheetInputRef = useRef<HTMLInputElement | null>(null)
  const browserVideoInputRef = useRef<HTMLInputElement | null>(null)

  const desktopRuntime = isTauriRuntime()
  const canRun = desktopRuntime && Boolean(options.input.trim()) && Boolean(options.output.trim()) && !busy
  const videoFrameCount = Math.max(1, videoOptions.columns * videoOptions.rows)
  const effectiveVideoEnd = videoDuration > 0 && videoOptions.endTime > videoOptions.startTime ? videoOptions.endTime : videoDuration
  const videoRangeSeconds = Math.max(0, effectiveVideoEnd - videoOptions.startTime)
  const displayedAtlas = inspectedPreview?.atlas ?? report?.atlas ?? sliceReport?.atlas
  const displayedPreview = sheetPreview
  const atlasSize = displayedAtlas ? `${displayedAtlas.meta.size.w} x ${displayedAtlas.meta.size.h}` : '0 x 0'
  const frameCount = displayedAtlas?.frames.length ?? 0
  const animationCount = displayedAtlas?.animations?.length ?? 0
  const waste = report ? `${report.wastePercent.toFixed(1)}%` : '0.0%'
  const packSignature = useMemo(() => JSON.stringify({ ...options, dryRun: false }), [options])

  const pushLog = useCallback((message: string) => {
    setLog((current) => [message, ...current].slice(0, 80))
  }, [])

  const loadGifPathForInspection = useCallback(
    async (path: string) => {
      setBusy(true)
      setReport(null)
      setSliceReport(null)
      setComparison(null)
      setInspectedPreview(null)
      setSheetPreview('')
      setNotice(`Inspecting ${fileName(path)}...`)
      try {
        const loaded = await invoke<AtlasPreviewLoad>('inspect_gif_gui', { path })
        setInspectedPreview(loaded)
        setSheetPreview(loaded.sheetDataUrl)
        setOptions((current) => ({ ...current, input: path, output: current.output || parentPath(path) }))
        setSliceOptions((current) => ({ ...current, input: path, output: current.output || parentPath(path) }))
        const frameTotal = loaded.atlas.frames.length
        setNotice(`GIF inspect: ${fileName(path)} (${frameTotal} frame${frameTotal === 1 ? '' : 's'})`)
        pushLog(`GIF inspect loaded ${frameTotal} frame${frameTotal === 1 ? '' : 's'} from ${path}.`)
      } catch (error) {
        const message = String(error)
        setNotice(message)
        pushLog(`GIF inspect failed: ${message}`)
      } finally {
        setBusy(false)
      }
    },
    [pushLog],
  )

  const loadVideoPath = useCallback(
    async (path: string) => {
      const defaultOutput = videoOptions.output || joinPath(parentPath(path), 'sprite-output')
      setVideoOptions((current) => ({
        ...current,
        input: path,
        output: current.output || defaultOutput,
        prefix: current.prefix || fileName(path).replace(/\.[^.]+$/, '') || 'video_walk',
      }))
      setOptions((current) => ({ ...current, input: '' }))
      setInspectedPreview(null)
      setReport(null)
      setSliceReport(null)
      setSheetPreview('')
      setComparison(null)
      setVideoDuration(0)
      setVideoPreviewGif('')
      setNotice(`Video set to ${fileName(path)} — adjust grid and click Make Sprite Sheet`)
      pushLog(`Video source routed from drop/pick: ${path}`)

      if (!desktopRuntime) return

      try {
        const bytes = await invoke<number[]>('read_binary_file_gui', { path })
        const blobUrl = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: videoMimeTypeFromPath(path) }))
        if (videoObjectUrlRef.current) URL.revokeObjectURL(videoObjectUrlRef.current)
        videoObjectUrlRef.current = blobUrl
        setVideoSourceUrl(blobUrl)
      } catch (error) {
        const message = String(error)
        setVideoSourceUrl('')
        setNotice(`Could not load video file: ${message}`)
        pushLog(`Video file read failed: ${message}`)
      }
    },
    [desktopRuntime, pushLog, videoOptions.output],
  )

  const setSourcePath = useCallback(
    (path: string) => {
      if (isVideoPath(path)) {
        void loadVideoPath(path)
        return
      }
      if (isGifPath(path)) {
        void loadGifPathForInspection(path)
        return
      }
      setInspectedPreview(null)
      setReport(null)
      setSliceReport(null)
      setComparison(null)
      setSheetPreview('')
      setOptions((current) => ({ ...current, input: path }))
      setNotice(`Input set to ${fileName(path)}`)
      pushLog(`Input folder: ${path}`)
    },
    [loadGifPathForInspection, pushLog],
  )

  const loadBrowserFileForInspection = useCallback(
    async (file: File, source: string) => {
      setBusy(true)
      setReport(null)
      setSliceReport(null)
      setComparison(null)
      setInspectedPreview(null)
      setSheetPreview('')
      setNotice(`Inspecting ${file.name}...`)
      try {
        const loaded = isGifFile(file) ? await inspectBrowserGifFile(file) : await inspectBrowserImageFile(file)
        setInspectedPreview(loaded)
        setSheetPreview(loaded.sheetDataUrl)
        setOptions((current) => ({ ...current, input: file.name }))
        setSliceOptions((current) => ({ ...current, input: file.name, output: current.output || options.output }))
        const frameTotal = loaded.atlas.frames.length
        setNotice(`${isGifFile(file) ? 'GIF' : 'Image'} inspect: ${file.name} (${frameTotal} frame${frameTotal === 1 ? '' : 's'})`)
        pushLog(`${source}: loaded ${file.name} with ${frameTotal} frame${frameTotal === 1 ? '' : 's'} for inspection.`)
      } catch (error) {
        const message = String(error)
        setNotice(message)
        pushLog(`Browser inspect failed: ${message}`)
      } finally {
        setBusy(false)
      }
    },
    [options.output, pushLog],
  )

  const loadBrowserFilesForInspection = useCallback(
    async (files: FileList | File[] | null, source: string) => {
      const fileList = Array.from(files ?? [])
      if (!fileList.length) return false
      const videoFile = fileList.find((item) => item.type.startsWith('video/') || isVideoPath(item.name))
      if (videoFile) {
        const sourceUrl = URL.createObjectURL(videoFile)
        if (videoObjectUrlRef.current) URL.revokeObjectURL(videoObjectUrlRef.current)
        videoObjectUrlRef.current = sourceUrl
        setVideoOptions((current) => ({
          ...current,
          input: videoFile.name,
          output: current.output || options.output || 'browser-video-output',
          prefix: current.prefix || videoFile.name.replace(/\.[^.]+$/, '') || 'video_walk',
        }))
        setVideoSourceUrl(sourceUrl)
        setVideoDuration(0)
        setVideoPreviewGif('')
        setNotice(`Video set to ${videoFile.name} — adjust grid and click Make Sprite Sheet`)
        pushLog(`${source}: loaded video ${videoFile.name}.`)
        return true
      }
      const file = fileList.find(isGifFile) ?? fileList.find((item) => item.type.startsWith('image/')) ?? fileList[0]
      await loadBrowserFileForInspection(file, source)
      return true
    },
    [loadBrowserFileForInspection, options.output, pushLog],
  )

  const loadLocalPathsForInspection = useCallback(
    (paths: string[], source: string) => {
      const path = paths.find(isGifPath) ?? paths[0]
      if (!path) return false
      setSourcePath(path)
      pushLog(`${source}: ${path}`)
      return true
    },
    [pushLog, setSourcePath],
  )

  const loadDataTransferForInspection = useCallback(
    async (dataTransfer: DataTransfer, source: string) => {
      if (await loadBrowserFilesForInspection(dataTransfer.files, source)) return true
      if (desktopRuntime) return loadLocalPathsForInspection(localPathsFromDataTransfer(dataTransfer), source)
      return false
    },
    [desktopRuntime, loadBrowserFilesForInspection, loadLocalPathsForInspection],
  )

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      window.localStorage.setItem(SETTINGS_KEY, JSON.stringify({ ...options, dryRun: false }))
    }, 150)
    return () => window.clearTimeout(timeout)
  }, [options])

  useEffect(() => {
    if (!isTauriRuntime()) return
    void setAgentControlEnabled(true)
  }, [])

  useEffect(() => {
    if (!agentControl?.enabled || !agentControl.token) return
    let closed = false
    const socket = new WebSocket(agentEventsUrl(agentControl))

    socket.onopen = () => {
      if (!closed) pushLog('AI Agent Control event stream connected.')
    }
    socket.onmessage = (message) => {
      void (async () => {
        try {
          const event = JSON.parse(String(message.data)) as ControlEvent
          if (
            event.type !== 'pack' &&
            event.type !== 'slice' &&
            event.type !== 'assembleGeneratedLoop' &&
            event.type !== 'animationJobAssemble'
          ) {
            return
          }
          const preview = await invoke<string>('preview_image_data_url', { path: event.sheetPath })
          setSheetPreview(preview)
          setInspectedPreview(null)
          setComparison(null)
          if (event.type === 'pack') {
            const nextReport = remotePackReport(event)
            setReport(nextReport)
            setSliceReport(null)
            setOptions((current) => ({ ...current, output: event.outputPath }))
            const summary = `${nextReport.inputCount} sprites, ${nextReport.atlasWidth} x ${nextReport.atlasHeight}`
            setNotice(`Remote pack: ${summary}`)
            pushLog(`Remote pack preview loaded: ${summary}.`)
          } else {
            const nextReport = remoteSliceReport(event)
            setSliceReport(nextReport)
            setReport(null)
            setOptions((current) => ({ ...current, output: event.outputPath }))
            const summary = `${nextReport.frameCount} frames from ${nextReport.sheetWidth} x ${nextReport.sheetHeight}`
            setNotice(`Remote slice: ${summary}`)
            pushLog(`Remote slice preview loaded: ${summary}.`)
          }
        } catch (error) {
          pushLog(`Remote preview event failed: ${String(error)}`)
        }
      })()
    }
    socket.onerror = () => {
      if (!closed) pushLog('AI Agent Control event stream error.')
    }

    return () => {
      closed = true
      socket.close()
    }
  }, [agentControl, pushLog])

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
        loadLocalPathsForInspection(event.payload.paths.map(normalizeLocalPath), 'Desktop drop')
      }
    })
    return () => {
      void cleanupDrag.then((unlisten) => unlisten())
    }
  }, [loadLocalPathsForInspection])

  useEffect(() => {
    const handlePaste = (event: ClipboardEvent) => {
      if (shouldIgnorePasteTarget(event.target)) return
      const clipboard = event.clipboardData
      if (!clipboard) return
      const hasFiles = clipboard.files.length > 0
      const paths = desktopRuntime ? localPathsFromDataTransfer(clipboard) : []
      if (!hasFiles && !paths.length) return
      event.preventDefault()
      if (hasFiles) {
        void loadBrowserFilesForInspection(clipboard.files, 'Paste')
        return
      }
      loadLocalPathsForInspection(paths, 'Paste')
    }
    window.addEventListener('paste', handlePaste)
    return () => window.removeEventListener('paste', handlePaste)
  }, [desktopRuntime, loadBrowserFilesForInspection, loadLocalPathsForInspection])

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

  async function setAgentControlEnabled(enabled: boolean) {
    if (!isTauriRuntime()) {
      setNotice('Desktop runtime required for remote control')
      return
    }
    if (agentControl?.enabled === enabled) return
    try {
      const next = await invoke<AgentControlInfo>(enabled ? 'start_agent_control_gui' : 'stop_agent_control_gui')
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

  async function openDesktopPath(options: Parameters<typeof open>[0], label: string): Promise<Awaited<ReturnType<typeof open>> | null> {
    try {
      return await open(options)
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`${label} failed: ${message}`)
      return null
    }
  }

  async function saveDesktopPath(options: Parameters<typeof save>[0], label: string): Promise<string | null> {
    try {
      return await save(options)
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`${label} failed: ${message}`)
      return null
    }
  }

  async function chooseInputFile() {
    if (!desktopRuntime) {
      browserSourceInputRef.current?.click()
      return
    }
    const selected = await openDesktopPath({
      directory: false,
      multiple: false,
      defaultPath: options.input || undefined,
      filters: [{ name: 'Images & Video', extensions: ['gif', 'png', 'jpg', 'jpeg', 'webp', 'mp4', 'webm', 'mov', 'm4v'] }],
    }, 'Input picker')
    if (typeof selected === 'string') {
      setSourcePath(selected)
    }
  }

  async function chooseInputFolder() {
    if (!desktopRuntime) {
      browserSourceInputRef.current?.click()
      return
    }
    const selected = await openDesktopPath({ directory: true, multiple: false, defaultPath: options.input || undefined }, 'Input folder picker')
    if (typeof selected === 'string') {
      setSourcePath(selected)
    }
  }

  async function chooseOutput() {
    if (!desktopRuntime) {
      const directoryPicker = (window as Window & { showDirectoryPicker?: () => Promise<{ name: string }> }).showDirectoryPicker
      if (directoryPicker) {
        try {
          const directory = await directoryPicker()
          setOptions((current) => ({ ...current, output: directory.name }))
          setNotice(`Output set to ${directory.name}`)
          pushLog(`Browser output folder selected: ${directory.name}.`)
        } catch {
          setNotice('Output unchanged')
        }
      } else {
        setNotice('Output folders are available in the desktop app')
      }
      return
    }
    const selected = await openDesktopPath({ directory: true, multiple: false, defaultPath: options.output || undefined }, 'Output picker')
    if (typeof selected === 'string') {
      setOptions((current) => ({ ...current, output: selected }))
      setNotice(`Output set to ${fileName(selected)}`)
      pushLog(`Output folder: ${selected}`)
    }
  }

  async function chooseSheet() {
    if (!desktopRuntime) {
      browserSheetInputRef.current?.click()
      return
    }
    const selected = await openDesktopPath({
      directory: false,
      multiple: false,
      filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif'] }],
    }, 'Sheet picker')
    if (typeof selected === 'string') {
      setSliceOptions((current) => ({ ...current, input: selected, output: current.output || options.output }))
      setNotice(`Sheet set to ${fileName(selected)}`)
      pushLog(`Sheet source: ${selected}`)
    }
  }

  async function chooseVideo() {
    if (!desktopRuntime) {
      browserVideoInputRef.current?.click()
      return
    }
    const selected = await openDesktopPath({
      directory: false,
      multiple: false,
      defaultPath: videoOptions.input || options.input || undefined,
      filters: [{ name: 'Videos', extensions: ['mp4', 'webm', 'mov', 'm4v'] }],
    }, 'Video picker')
    if (typeof selected === 'string') {
      await loadVideoPath(selected)
    }
  }

  async function chooseVideoOutput() {
    if (!desktopRuntime) {
      setNotice('Video output folders are available in the desktop app')
      return
    }
    const selected = await openDesktopPath(
      { directory: true, multiple: false, defaultPath: videoOptions.output || options.output || undefined },
      'Video output picker',
    )
    if (typeof selected === 'string') {
      setVideoOptions((current) => ({ ...current, output: selected }))
      setNotice(`Video output set to ${fileName(selected)}`)
      pushLog(`Video output folder: ${selected}`)
    }
  }

  async function handleBrowserSourceChange(event: ChangeEvent<HTMLInputElement>) {
    const input = event.currentTarget
    const files = input.files
    await loadBrowserFilesForInspection(files, 'Source picker')
    input.value = ''
  }

  async function handleBrowserSheetChange(event: ChangeEvent<HTMLInputElement>) {
    const input = event.currentTarget
    const files = input.files
    await loadBrowserFilesForInspection(files, 'Sheet picker')
    input.value = ''
  }

  function handleBrowserVideoChange(event: ChangeEvent<HTMLInputElement>) {
    const input = event.currentTarget
    const file = input.files?.[0]
    input.value = ''
    if (!file) return
    const sourceUrl = URL.createObjectURL(file)
    if (videoObjectUrlRef.current) URL.revokeObjectURL(videoObjectUrlRef.current)
    videoObjectUrlRef.current = sourceUrl
    setVideoOptions((current) => ({
      ...current,
      input: file.name,
      output: current.output || options.output || 'browser-video-output',
    }))
    setVideoSourceUrl(sourceUrl)
    setVideoDuration(0)
    setVideoPreviewGif('')
    setNotice(`Video set to ${file.name}`)
    pushLog(`Browser video source: ${file.name}`)
  }

  function handleDropZoneDragOver(event: ReactDragEvent<HTMLButtonElement>) {
    event.preventDefault()
    setDragging(true)
    event.dataTransfer.dropEffect = 'copy'
  }

  async function handleDropZoneDrop(event: ReactDragEvent<HTMLButtonElement>) {
    event.preventDefault()
    setDragging(false)
    const loaded = await loadDataTransferForInspection(event.dataTransfer, 'Drop')
    if (!loaded) setNotice('Drop a GIF, image, video, or sprite folder')
  }

  function setVideoGridPreset(gridPreset: VideoGridPreset) {
    setVideoOptions((current) => {
      if (gridPreset === '4x4') return { ...current, gridPreset, columns: 4, rows: 4 }
      if (gridPreset === '5x5') return { ...current, gridPreset, columns: 5, rows: 5 }
      return { ...current, gridPreset }
    })
  }

  async function runVideoToSpriteSheet() {
    if (!isTauriRuntime()) {
      setNotice('Desktop runtime required for video extraction')
      return
    }
    if (!videoOptions.input.trim()) {
      setNotice('Choose a video first')
      return
    }
    const video = videoRef.current
    if (!video) {
      setNotice('Video preview is not ready yet')
      return
    }
    const output = videoOptions.output || options.output || joinPath(parentPath(videoOptions.input), 'sprite-output')
    if (!output.trim()) {
      setNotice('Choose an output folder')
      return
    }

    setBusy(true)
    setReport(null)
    setSliceReport(null)
    setInspectedPreview(null)
    setSheetPreview('')
    setComparison(null)
      setVideoPreviewGif('')
      setNotice('Loading video...')
    try {
      if (!videoSourceUrl) {
        setNotice('Video preview is still loading')
        return
      }
      await waitForVideoMetadata(video)
      const duration = Number.isFinite(video.duration) ? video.duration : 0
      const start = clamp(videoOptions.startTime, 0, Math.max(0, duration))
      const requestedEnd = videoOptions.endTime > start ? videoOptions.endTime : duration
      const end = clamp(requestedEnd, start, Math.max(start, duration))
      const usableDuration = Math.max(0.001, end - start)
      const frameWidth = Math.max(16, Math.round(videoOptions.frameWidth) || 256)
      const frameHeight = Math.max(16, Math.round(videoOptions.frameHeight) || 256)
      const columns = Math.max(1, Math.round(videoOptions.columns) || 1)
      const rows = Math.max(1, Math.round(videoOptions.rows) || 1)
      const frameCount = columns * rows
      const prefix = videoOptions.prefix.trim() || 'video_walk'
      const framesDir = joinPath(output, 'video_frames')
      const canvas = document.createElement('canvas')

      for (let index = 0; index < frameCount; index += 1) {
        const sampleTime = start + usableDuration * (index / frameCount)
        const safeTime = Math.min(Math.max(start, sampleTime), Math.max(start, end - 0.001))
        setNotice(`Sampling video frame ${index + 1}/${frameCount}...`)
        await seekVideo(video, safeTime)
        await wait(20)
        drawVideoFrame(video, canvas, frameWidth, frameHeight)
        const bytes = await canvasToPngBytes(canvas)
        const framePath = joinPath(framesDir, `${prefix}_${String(index).padStart(4, '0')}.png`)
        await invoke('write_binary_file_gui', { path: framePath, bytes })
      }

      setNotice('Packing sampled frames...')
      const nextReport = await invoke<SliceReport>('assemble_generated_loop_gui', {
        options: {
          input: framesDir,
          output,
          format: 'png',
          dryRun: false,
          columns,
          prefix,
          asepriteJson: true,
          frameWidth,
          frameHeight,
        },
      })
      setSliceReport(nextReport)
      setOptions((current) => ({ ...current, output }))
      setSliceOptions((current) => ({
        ...current,
        input: joinPath(output, 'sheet.png'),
        output,
        prefix,
        normalizeGenerated: true,
        frameWidth,
        frameHeight,
      }))
      const sheetPath = joinPath(output, 'sheet.png')
      const preview = await invoke<string>('preview_image_data_url', { path: sheetPath })
      setSheetPreview(preview)

      const gifPath = joinPath(output, `${prefix}_preview.gif`)
      const gif = await invoke<GifExportReport>('export_gif_gui', {
        options: {
          atlasPath: joinPath(output, 'atlas.json'),
          sheetPath,
          outputPath: gifPath,
          animation: prefix,
          fpsOverride: videoOptions.fps,
          frameStart: null,
          frameEnd: null,
        },
      })
      const gifPreview = await invoke<string>('preview_image_data_url', { path: gif.outputPath })
      setVideoPreviewGif(gifPreview)
      const summary = `${frameCount} frames, ${columns} x ${rows}, ${usableDuration.toFixed(2)}s sampled`
      setNotice(`Video sprite sheet: ${summary}`)
      pushLog(`Video sprite sheet built from ${fileName(videoOptions.input)}: ${summary}.`)
      pushLog(`Preview GIF: ${gif.outputPath}`)
    } catch (error) {
      const message = String(error)
      setNotice(message)
      pushLog(`Video extraction failed: ${message}`)
    } finally {
      setBusy(false)
    }
  }

  async function runPack(dryRun: boolean) {
    if (!desktopRuntime) {
      setNotice('Packing is available in the desktop app')
      return
    }
    if (!canRun && !dryRun) {
      setNotice('Choose input and output folders')
      return
    }
    if (!options.input || (!options.output && !dryRun)) {
      setNotice('Choose input and output folders')
      return
    }
    if (isVideoPath(options.input)) {
      setNotice('Video files belong in the Video panel — use Make Sprite Sheet')
      return
    }
    setBusy(true)
    setInspectedPreview(null)
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
    setInspectedPreview(null)
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
    if (!desktopRuntime) {
      setNotice('Slicing is available in the desktop app')
      return
    }
    const output = sliceOptions.output || options.output
    if (!sliceOptions.input || (!output && !dryRun)) {
      setNotice('Choose a sheet image and output folder')
      return
    }
    setBusy(true)
    setInspectedPreview(null)
    setSheetPreview('')
    setNotice(dryRun ? 'Planning sheet slice...' : 'Slicing sheet...')
    try {
      const nextReport = await invoke<SliceReport>('slice_sheet_gui', {
        options: { ...sliceOptions, output, dryRun },
      })
      setSliceReport(nextReport)
      setReport(null)
      const normalized = nextReport.normalization
        ? `, normalized ${nextReport.normalization.outputFrameSize.w} x ${nextReport.normalization.outputFrameSize.h}`
        : ''
      const summary = `${nextReport.frameCount} frames from ${nextReport.sheetWidth} x ${nextReport.sheetHeight}${normalized}`
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
    const selected = await openDesktopPath({
      directory: false,
      multiple: false,
      filters: [{ name: 'Atlas Project', extensions: ['atlasproj.json', 'json'] }],
    }, 'Project picker')
    if (typeof selected !== 'string') return
    try {
      const project = await invoke<ProjectOptions>('load_project_gui', { path: selected })
      setOptions({ ...project, dryRun: false })
      setReport(null)
      setInspectedPreview(null)
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
    const selected = await saveDesktopPath({
      defaultPath: 'atlas.atlasproj.json',
      filters: [{ name: 'Atlas Project', extensions: ['json'] }],
    }, 'Project save picker')
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
    const selected = await openDesktopPath({
      directory: false,
      multiple: false,
      filters: [{ name: 'Atlas JSON', extensions: ['json'] }],
    }, 'Comparison picker')
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
    const selected = await saveDesktopPath({
      defaultPath: `${animation || 'animation'}.gif`,
      filters: [{ name: 'GIF', extensions: ['gif'] }],
    }, 'GIF export picker')
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
    const atlas = inspectedPreview?.atlas ?? report?.atlas ?? sliceReport?.atlas
    if (!atlas || !sheetPreview) {
      setNotice('Load an animation preview before exporting')
      return
    }
    let selected = ''
    if (isTauriRuntime()) {
      const saved = await saveDesktopPath({
        defaultPath: `${animation || 'animation'}.webm`,
        filters: [{ name: 'WebM', extensions: ['webm'] }],
      }, 'WebM export picker')
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

  function reorderInspectedPreviewFrames(animation: string, fromIndex: number, toIndex: number) {
    setInspectedPreview((current) => {
      if (!current) return current
      return { ...current, atlas: reorderAtlasAnimation(current.atlas, animation, fromIndex, toIndex) }
    })
    setNotice(`Moved frame ${fromIndex + 1} to ${toIndex + 1}`)
    pushLog(`Reordered ${animation}: frame ${fromIndex + 1} to ${toIndex + 1}.`)
  }

  function reset() {
    setReport(null)
    setSliceReport(null)
    setInspectedPreview(null)
    setSheetPreview('')
    setComparison(null)
    setVideoPreviewGif('')
    setHotReloadSignature('')
    setNotice('Ready')
  }

  const displayedAnimations =
    inspectedPreview?.atlas.animations?.map((animation) => animation.name) ??
    report?.animations ??
    sliceReport?.atlas.animations?.map((animation) => animation.name) ??
    []
  const displayedWrote = inspectedPreview ? 'Inspect only' : report?.wrote.join(', ') || sliceReport?.wrote.join(', ') || 'None'

  return (
    <div className="app-shell theme-neko-tron">
      <input
        accept=".gif,.png,.jpg,.jpeg,.webp,image/gif,image/png,image/jpeg,image/webp"
        className="browser-file-input"
        multiple
        ref={browserSourceInputRef}
        type="file"
        onChange={(event) => void handleBrowserSourceChange(event)}
      />
      <input
        accept=".gif,.png,.jpg,.jpeg,.webp,image/gif,image/png,image/jpeg,image/webp"
        className="browser-file-input"
        ref={browserSheetInputRef}
        type="file"
        onChange={(event) => void handleBrowserSheetChange(event)}
      />
      <input
        accept=".mp4,.webm,.mov,.m4v,video/mp4,video/webm,video/quicktime"
        className="browser-file-input"
        ref={browserVideoInputRef}
        type="file"
        onChange={(event) => handleBrowserVideoChange(event)}
      />
      <header className="topbar">
        <div className="brand-lockup">
          <div>
            <div className="brand-title-row">
              <h1>Sprite Atlas Packer</h1>
              <span className="version-badge">{APP_VERSION_LABEL}</span>
            </div>
            <p>{notice}</p>
          </div>
        </div>
        <div className="topbar-actions">
          <button className="secondary-action" type="button" onClick={chooseInputFile} title="Choose GIF or image">
            <ImageIcon size={17} />
            Input
          </button>
          <button className="icon-button" type="button" onClick={chooseInputFolder} title="Choose input folder">
            <FolderOpen size={17} />
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
              <button className="icon-button compact" type="button" onClick={chooseInputFile} title="Choose GIF or image">
                <ImageIcon size={15} />
              </button>
              <button className="icon-button compact" type="button" onClick={chooseInputFolder} title="Choose input folder">
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
            <button className="secondary-action full" type="button" onClick={() => void runPack(true)} disabled={!desktopRuntime || !options.input || busy}>
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
              <input
                type="checkbox"
                checked={Boolean(agentControl?.enabled)}
                onChange={(event) => void setAgentControlEnabled(event.currentTarget.checked)}
              />
              <span>AI Agent Control</span>
            </label>
            <div className={classNames('remote-status', agentControl?.enabled && 'active')}>
              {agentControl?.enabled ? <CheckCircle2 size={17} /> : <TriangleAlert size={17} />}
              <span>{agentControl?.enabled ? 'API enabled for agents' : 'API disabled'}</span>
            </div>
            <label className="field">
              <span>Address</span>
              <input value={agentControl?.baseUrl ?? 'http://127.0.0.1:9877/api/v1'} readOnly />
            </label>
            <label className="field">
              <span>Token</span>
              <input value={agentControl?.token ?? ''} readOnly />
            </label>
            <button className="secondary-action full" type="button" onClick={() => void copyAgentToken()} disabled={!agentControl?.token}>
              <FileJson size={16} />
              Copy Token
            </button>
          </section>

          <section className="panel-section">
            <div className="section-title">
              <Video size={16} />
              Video
            </div>
            <div className="folder-row">
              <input value={compactPath(videoOptions.input)} readOnly />
              <button className="icon-button compact" type="button" onClick={() => void chooseVideo()} title="Choose video">
                <Video size={15} />
              </button>
            </div>
            <div className="folder-row">
              <input value={compactPath(videoOptions.output)} readOnly />
              <button className="icon-button compact" type="button" onClick={() => void chooseVideoOutput()} title="Choose video output">
                <FolderOpen size={15} />
              </button>
            </div>
            {videoSourceUrl ? (
              <video
                className="video-source-preview"
                controls
                muted
                playsInline
                preload="auto"
                ref={videoRef}
                src={videoSourceUrl}
                onLoadedMetadata={(event) => {
                  const duration = Number.isFinite(event.currentTarget.duration) ? event.currentTarget.duration : 0
                  setVideoDuration(duration)
                  setVideoOptions((current) => ({
                    ...current,
                    endTime: current.endTime > 0 && current.endTime <= duration ? current.endTime : duration,
                  }))
                }}
              />
            ) : (
              <div className="video-source-empty">No video selected</div>
            )}
            <div className="format-toggle three">
              {(['4x4', '5x5', 'custom'] as VideoGridPreset[]).map((gridPreset) => (
                <button
                  className={classNames('format-button', videoOptions.gridPreset === gridPreset && 'active')}
                  key={gridPreset}
                  type="button"
                  onClick={() => setVideoGridPreset(gridPreset)}
                >
                  {gridPreset.toUpperCase()}
                </button>
              ))}
            </div>
            <div className="cell-grid">
              <label>
                <span>Columns</span>
                <input
                  min={1}
                  max={16}
                  type="number"
                  value={videoOptions.columns}
                  onChange={(event) =>
                    setVideoOptions((current) => ({
                      ...current,
                      gridPreset: 'custom',
                      columns: Math.max(1, Math.min(16, Number(event.currentTarget.value) || 1)),
                    }))
                  }
                />
              </label>
              <label>
                <span>Rows</span>
                <input
                  min={1}
                  max={16}
                  type="number"
                  value={videoOptions.rows}
                  onChange={(event) =>
                    setVideoOptions((current) => ({
                      ...current,
                      gridPreset: 'custom',
                      rows: Math.max(1, Math.min(16, Number(event.currentTarget.value) || 1)),
                    }))
                  }
                />
              </label>
              <label>
                <span>Frame W</span>
                <input
                  min={16}
                  type="number"
                  value={videoOptions.frameWidth}
                  onChange={(event) => setVideoOptions((current) => ({ ...current, frameWidth: Math.max(16, Number(event.currentTarget.value) || 16) }))}
                />
              </label>
              <label>
                <span>Frame H</span>
                <input
                  min={16}
                  type="number"
                  value={videoOptions.frameHeight}
                  onChange={(event) => setVideoOptions((current) => ({ ...current, frameHeight: Math.max(16, Number(event.currentTarget.value) || 16) }))}
                />
              </label>
              <label>
                <span>Start</span>
                <input
                  min={0}
                  step={0.05}
                  type="number"
                  value={Number(videoOptions.startTime.toFixed(2))}
                  onChange={(event) =>
                    setVideoOptions((current) => ({
                      ...current,
                      startTime: Math.max(0, Number(event.currentTarget.value) || 0),
                    }))
                  }
                />
              </label>
              <label>
                <span>End</span>
                <input
                  min={0}
                  step={0.05}
                  type="number"
                  value={Number(videoOptions.endTime.toFixed(2))}
                  onChange={(event) =>
                    setVideoOptions((current) => ({
                      ...current,
                      endTime: Math.max(0, Number(event.currentTarget.value) || 0),
                    }))
                  }
                />
              </label>
              <label>
                <span>FPS</span>
                <input
                  min={1}
                  max={60}
                  type="number"
                  value={videoOptions.fps}
                  onChange={(event) => setVideoOptions((current) => ({ ...current, fps: Math.max(1, Math.min(60, Number(event.currentTarget.value) || 1)) }))}
                />
              </label>
              <label>
                <span>Frames</span>
                <input readOnly value={videoFrameCount} />
              </label>
            </div>
            <label className="field">
              <span>Prefix</span>
              <input
                value={videoOptions.prefix}
                onChange={(event) => setVideoOptions((current) => ({ ...current, prefix: event.currentTarget.value }))}
              />
            </label>
            <div className="video-range-note">
              {videoDuration > 0 ? `${videoRangeSeconds.toFixed(2)}s sampled from ${videoDuration.toFixed(2)}s` : 'Duration loads after video selection'}
            </div>
            <button className="secondary-action full" type="button" onClick={() => void runVideoToSpriteSheet()} disabled={!desktopRuntime || !videoOptions.input || busy}>
              {busy ? <Loader2 className="spin" size={16} /> : <Grid3X3 size={16} />}
              Make Sprite Sheet
            </button>
            {videoPreviewGif ? <img className="video-gif-preview" alt="Video animation preview" src={videoPreviewGif} /> : null}
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
                  <span>Cell W</span>
                  <input
                    min={1}
                    type="number"
                    value={sliceOptions.cellWidth}
                    onChange={(event) => setSliceOptions((current) => ({ ...current, cellWidth: Math.max(1, Number(event.currentTarget.value) || 1) }))}
                  />
                </label>
                <label>
                  <span>Cell H</span>
                  <input
                    min={1}
                    type="number"
                    value={sliceOptions.cellHeight}
                    onChange={(event) => setSliceOptions((current) => ({ ...current, cellHeight: Math.max(1, Number(event.currentTarget.value) || 1) }))}
                  />
                </label>
              </div>
            ) : null}
            <label className="toggle-row">
              <input
                type="checkbox"
                checked={sliceOptions.normalizeGenerated}
                onChange={(event) => setSliceOptions((current) => ({ ...current, normalizeGenerated: event.currentTarget.checked }))}
              />
              <span>Normalize generated sheet</span>
            </label>
            {sliceOptions.normalizeGenerated ? (
              <div className="cell-grid">
                <label>
                  <span>Frame W</span>
                  <input
                    min={16}
                    type="number"
                    value={sliceOptions.frameWidth}
                    onChange={(event) => setSliceOptions((current) => ({ ...current, frameWidth: Math.max(16, Number(event.currentTarget.value) || 16) }))}
                  />
                </label>
                <label>
                  <span>Frame H</span>
                  <input
                    min={16}
                    type="number"
                    value={sliceOptions.frameHeight}
                    onChange={(event) => setSliceOptions((current) => ({ ...current, frameHeight: Math.max(16, Number(event.currentTarget.value) || 16) }))}
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
            <button className="secondary-action full" type="button" onClick={() => void runSlice(false)} disabled={!desktopRuntime || !sliceOptions.input || busy}>
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

          <button
            className={classNames('drop-zone', dragging && 'dragging')}
            type="button"
            onClick={chooseInputFile}
            onDragEnter={handleDropZoneDragOver}
            onDragLeave={() => setDragging(false)}
            onDragOver={handleDropZoneDragOver}
            onDrop={(event) => void handleDropZoneDrop(event)}
          >
            <div className="drop-glyph">
              <BoxSelect size={28} />
            </div>
            <div>
              <h2>{options.input ? fileName(options.input) : desktopRuntime ? 'Drop Sprite Folder, GIF, or Video' : 'Drop GIF or Image'}</h2>
              <p>{options.input ? compactPath(options.input) : 'PNG, JPG, WebP, GIF, Aseprite, MP4, WebM'}</p>
            </div>
          </button>

          <div className="preview-layout">
            {displayedAtlas && displayedPreview ? (
              <AtlasPreview
                atlas={displayedAtlas}
                sheetSrc={displayedPreview}
                onExportGif={
                  inspectedPreview ? undefined : (animation, fps, frameStart, frameEnd) => void exportGif(animation, fps, frameStart, frameEnd)
                }
                onExportWebm={(animation, fps, frameStart, frameEnd) => void exportWebm(animation, fps, frameStart, frameEnd)}
                onReorderFrames={inspectedPreview ? reorderInspectedPreviewFrames : undefined}
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
