import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { deflateSync } from 'node:zlib'

const root = new URL('..', import.meta.url).pathname
const temp = mkdtempSync(join(tmpdir(), 'sprite-atlas-packer-compat-'))

try {
  const input = join(temp, 'input')
  const output = join(temp, 'output')
  mkdirSync(input, { recursive: true })
  writeFixturePng(join(input, 'hero_idle_0.png'), 10, 10, { x: 2, y: 3, w: 5, h: 4, rgba: [255, 0, 0, 255] })
  writeFixturePng(join(input, 'hero_idle_1.png'), 10, 10, { x: 1, y: 2, w: 6, h: 5, rgba: [0, 255, 0, 255] })
  writeFixturePng(join(input, 'hero_run_0.png'), 8, 8, { x: 2, y: 2, w: 4, h: 4, rgba: [0, 0, 255, 255] })

  execFileSync(
    'cargo',
    [
      'run',
      '--quiet',
      '--manifest-path',
      join(root, 'src-tauri/Cargo.toml'),
      '--',
      '--headless',
      '--input',
      input,
      '--output',
      output,
      '--format',
      'png',
      '--pot',
      '--padding',
      '2',
      '--trim',
    ],
    { stdio: 'pipe' },
  )

  const atlas = JSON.parse(readFileSync(join(output, 'atlas.json'), 'utf8'))
  const schema = JSON.parse(readFileSync(join(root, 'schemas/texturepacker-array.schema.json'), 'utf8'))
  const projectSchema = JSON.parse(readFileSync(join(root, 'schemas/atlas-project.schema.json'), 'utf8'))
  validateJsonSchema(atlas, schema, schema, 'atlas')
  validateJsonSchema(sampleProject(), projectSchema, projectSchema, 'project')
  validateTexturePackerArray(atlas)
  validatePhaserJsonArrayShape(atlas)
  validatePixiProjection(toPixiSpritesheetData(atlas), atlas)

  console.log(
    JSON.stringify({
      status: 'ok',
      frames: atlas.frames.length,
      animations: atlas.animations.length,
      image: atlas.meta.image,
      size: atlas.meta.size,
    }),
  )
} finally {
  rmSync(temp, { recursive: true, force: true })
}

function validateTexturePackerArray(atlas) {
  assert(atlas?.meta?.app === 'sprite-atlas-packer', 'meta.app must identify the packer')
  assert(atlas.meta.image === 'sheet.png', 'meta.image must reference the emitted PNG')
  assert(atlas.meta.format === 'RGBA8888', 'meta.format must be RGBA8888')
  assert(Number.isFinite(atlas.meta.scale), 'meta.scale must be numeric')
  assert(Number.isInteger(atlas.meta.size.w) && Number.isInteger(atlas.meta.size.h), 'meta.size must be integer pixels')
  assert(Array.isArray(atlas.frames) && atlas.frames.length === 3, 'frames must be a JSON Array')
  assert(Array.isArray(atlas.animations) && atlas.animations.length === 2, 'animations extension must group inferred animations')

  const names = new Set()
  for (const frame of atlas.frames) {
    assert(typeof frame.filename === 'string' && frame.filename.length > 0, 'frame filename is required')
    assert(!names.has(frame.filename), `duplicate frame filename ${frame.filename}`)
    names.add(frame.filename)
    assertRect(frame.frame, `frame.${frame.filename}.frame`)
    assertRect(frame.spriteSourceSize, `frame.${frame.filename}.spriteSourceSize`)
    assertSize(frame.sourceSize, `frame.${frame.filename}.sourceSize`)
    assert(frame.rotated === false, 'rotation is present and disabled for v1 compatibility')
    assert(typeof frame.trimmed === 'boolean', 'trimmed flag is required')
    assert(frame.frame.x + frame.frame.w <= atlas.meta.size.w, `${frame.filename} exceeds atlas width`)
    assert(frame.frame.y + frame.frame.h <= atlas.meta.size.h, `${frame.filename} exceeds atlas height`)
  }
}

function validatePhaserJsonArrayShape(atlas) {
  // Phaser documents JSONArray as TexturePacker JSON Array with frames as an array.
  assert(Array.isArray(atlas.frames), 'Phaser JSONArray requires frames[]')
  for (const frame of atlas.frames) {
    assert('filename' in frame, 'Phaser frame requires filename')
    assert('frame' in frame, 'Phaser frame requires frame rect')
    assert('rotated' in frame, 'Phaser frame requires rotated')
    assert('trimmed' in frame, 'Phaser frame requires trimmed')
    assert('spriteSourceSize' in frame, 'Phaser frame requires spriteSourceSize')
    assert('sourceSize' in frame, 'Phaser frame requires sourceSize')
  }
}

function toPixiSpritesheetData(atlas) {
  const frames = Object.fromEntries(
    atlas.frames.map((frame) => [
      frame.filename,
      {
        frame: frame.frame,
        rotated: frame.rotated,
        trimmed: frame.trimmed,
        spriteSourceSize: frame.spriteSourceSize,
        sourceSize: frame.sourceSize,
        anchor: frame.pivot,
      },
    ]),
  )
  const animations = Object.fromEntries(atlas.animations.map((animation) => [animation.name, animation.frames]))
  return {
    frames,
    animations,
    meta: {
      image: atlas.meta.image,
      format: atlas.meta.format,
      size: atlas.meta.size,
      scale: String(atlas.meta.scale),
    },
  }
}

function validatePixiProjection(pixiData, atlas) {
  assert(!Array.isArray(pixiData.frames), 'Pixi documented shape uses frames as an object')
  assert(Object.keys(pixiData.frames).length === atlas.frames.length, 'Pixi projection preserves every frame')
  assert(Object.keys(pixiData.animations).length === atlas.animations.length, 'Pixi projection preserves animations')
  for (const frame of atlas.frames) {
    const projected = pixiData.frames[frame.filename]
    assert(projected, `Pixi projection missing ${frame.filename}`)
    assert(projected.anchor.x === frame.pivot.x && projected.anchor.y === frame.pivot.y, 'pivot maps to Pixi anchor')
  }
}

function sampleProject() {
  return {
    sourceFolder: './raw-sprites',
    outputPath: './build/atlas',
    format: 'both',
    pot: true,
    padding: 2,
    maxSize: 2048,
    rotate: false,
    trim: true,
    asepriteJson: true,
    animationRegex: '^(?P<name>.+)_(?P<index>\\d+)$',
    stablePivot: true,
    dedupThreshold: 1,
    linkedChannels: ['_n', '_e', '_m'],
    godotSpriteframes: true,
    unityImporter: true,
    packingStrategy: 'maxrects',
    animations: [
      {
        name: 'hero_idle',
        frames: ['hero_idle_0', 'hero_idle_1'],
        loop: true,
        fpsHint: 8.3,
        pivot: { x: 0.5, y: 0.5 },
        durationMs: 120,
        frameOverrides: {
          hero_idle_1: {
            pivot: { x: 0.48, y: 0.54 },
            durationMs: 160,
          },
        },
      },
    ],
  }
}

function writeFixturePng(path, width, height, rect) {
  const bytes = Buffer.alloc(width * height * 4)
  for (let y = rect.y; y < rect.y + rect.h; y += 1) {
    for (let x = rect.x; x < rect.x + rect.w; x += 1) {
      const offset = (y * width + x) * 4
      bytes[offset] = rect.rgba[0]
      bytes[offset + 1] = rect.rgba[1]
      bytes[offset + 2] = rect.rgba[2]
      bytes[offset + 3] = rect.rgba[3]
    }
  }
  const scanlines = Buffer.alloc(height * (1 + width * 4))
  for (let y = 0; y < height; y += 1) {
    bytes.copy(scanlines, y * (1 + width * 4) + 1, y * width * 4, (y + 1) * width * 4)
  }
  writeFileSync(path, Buffer.concat([pngSignature(), chunk('IHDR', ihdr(width, height)), chunk('IDAT', deflateSync(scanlines)), chunk('IEND', Buffer.alloc(0))]))
}

function pngSignature() {
  return Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
}

function ihdr(width, height) {
  const data = Buffer.alloc(13)
  data.writeUInt32BE(width, 0)
  data.writeUInt32BE(height, 4)
  data[8] = 8
  data[9] = 6
  return data
}

function chunk(type, data) {
  const typeBytes = Buffer.from(type)
  const length = Buffer.alloc(4)
  length.writeUInt32BE(data.length, 0)
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(Buffer.concat([typeBytes, data])), 0)
  return Buffer.concat([length, typeBytes, data, crc])
}

function crc32(buffer) {
  let crc = 0xffffffff
  for (const byte of buffer) {
    crc ^= byte
    for (let bit = 0; bit < 8; bit += 1) {
      crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1
    }
  }
  return (crc ^ 0xffffffff) >>> 0
}

function assertRect(rect, label) {
  assert(Number.isInteger(rect?.x) && rect.x >= 0, `${label}.x must be a non-negative integer`)
  assert(Number.isInteger(rect?.y) && rect.y >= 0, `${label}.y must be a non-negative integer`)
  assert(Number.isInteger(rect?.w) && rect.w >= 0, `${label}.w must be a non-negative integer`)
  assert(Number.isInteger(rect?.h) && rect.h >= 0, `${label}.h must be a non-negative integer`)
}

function assertSize(size, label) {
  assert(Number.isInteger(size?.w) && size.w >= 0, `${label}.w must be a non-negative integer`)
  assert(Number.isInteger(size?.h) && size.h >= 0, `${label}.h must be a non-negative integer`)
}

function validateJsonSchema(value, schema, rootSchema, label) {
  if (schema.$ref) {
    return validateJsonSchema(value, resolveRef(schema.$ref, rootSchema), rootSchema, label)
  }

  if (schema.const !== undefined) {
    assert(value === schema.const, `${label} must equal ${JSON.stringify(schema.const)}`)
  }

  if (schema.enum) {
    assert(schema.enum.includes(value), `${label} must be one of ${schema.enum.map((item) => JSON.stringify(item)).join(', ')}`)
  }

  if (schema.type) {
    assert(matchesJsonType(value, schema.type), `${label} must be ${schema.type}`)
  }

  if (schema.type === 'object' || schema.properties || schema.required) {
    for (const key of schema.required ?? []) {
      assert(value && Object.prototype.hasOwnProperty.call(value, key), `${label}.${key} is required`)
    }
    for (const [key, childSchema] of Object.entries(schema.properties ?? {})) {
      if (value && Object.prototype.hasOwnProperty.call(value, key)) {
        validateJsonSchema(value[key], childSchema, rootSchema, `${label}.${key}`)
      }
    }
    if (value && schema.additionalProperties === false) {
      for (const key of Object.keys(value)) {
        assert(schema.properties?.[key], `${label}.${key} is not allowed`)
      }
    } else if (value && schema.additionalProperties && typeof schema.additionalProperties === 'object') {
      for (const [key, childValue] of Object.entries(value)) {
        if (!schema.properties?.[key]) {
          validateJsonSchema(childValue, schema.additionalProperties, rootSchema, `${label}.${key}`)
        }
      }
    }
  }

  if (schema.type === 'array' || schema.items) {
    assert(Array.isArray(value), `${label} must be an array`)
    for (const [index, item] of value.entries()) {
      validateJsonSchema(item, schema.items, rootSchema, `${label}[${index}]`)
    }
  }

  if (typeof value === 'string' && schema.minLength !== undefined) {
    assert(value.length >= schema.minLength, `${label} must be at least ${schema.minLength} chars`)
  }

  if (typeof value === 'number') {
    if (schema.minimum !== undefined) assert(value >= schema.minimum, `${label} must be >= ${schema.minimum}`)
    if (schema.maximum !== undefined) assert(value <= schema.maximum, `${label} must be <= ${schema.maximum}`)
    if (schema.exclusiveMinimum !== undefined) assert(value > schema.exclusiveMinimum, `${label} must be > ${schema.exclusiveMinimum}`)
  }
}

function resolveRef(ref, rootSchema) {
  assert(ref.startsWith('#/'), `Only local schema refs are supported: ${ref}`)
  return ref
    .slice(2)
    .split('/')
    .reduce((current, segment) => current?.[segment.replaceAll('~1', '/').replaceAll('~0', '~')], rootSchema)
}

function matchesJsonType(value, type) {
  if (type === 'object') return value !== null && typeof value === 'object' && !Array.isArray(value)
  if (type === 'array') return Array.isArray(value)
  if (type === 'integer') return Number.isInteger(value)
  if (type === 'number') return typeof value === 'number' && Number.isFinite(value)
  if (type === 'string') return typeof value === 'string'
  if (type === 'boolean') return typeof value === 'boolean'
  return true
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message)
  }
}
