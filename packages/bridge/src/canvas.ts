'use client'

import { commands, type CanvasPagePreparation, type Point, type TransformFrame } from './protocol'

declare const process: { env: { NODE_ENV?: string } }

export type CanvasColor = [number, number, number, number]
export type CanvasStrokeKind = 'paint' | 'erase' | 'inpaint'
export type WorkspaceColor = [number, number, number]
const MAX_CACHED_CANVAS_MANIFESTS = 8

export interface CanvasStroke {
  kind: CanvasStrokeKind
  page: string
  revision: number
  layer: string | null
  point: Point
  diameter: number
  color?: CanvasColor
}

export interface Canvas {
  resize(width: number, height: number, dpr: number, background: WorkspaceColor): void
  setView(zoom: number, translation: [number, number]): void
  stageManifest(manifest: Uint8Array): { token: number; missing: string[] }
  installResource(resource: string, packet: Uint8Array): Promise<void>
  hasActiveManifest(manifest: Uint8Array): boolean
  activateFrame(token: number): boolean
  activatePage(page: string, expectedRevision: number): boolean
  cacheFrame(token: number, page: string): boolean
  pinPageManifests(pages: string[]): void
  unpinPageManifests(pages: string[]): void
  getLastLayoutRejection(): string | null
  setPageLayout(pages: CanvasPageLayout[]): boolean
  clear(): void
  previewOpacity(element: string, opacity: number | null): void
  beginTransform(page: string, elements: TransformFrame[]): void
  updateTransform(elements: TransformFrame[]): void
  finishTransform(): void
  cancelTransform(): void
  beginStroke(stroke: CanvasStroke): void
  extendStroke(points: Point[]): void
  finishStroke(): void
  cancelStroke(): void
  sampleColor(point: Point): Promise<CanvasColor>
  dispose(): Promise<void>
}

export interface CanvasPageLayout {
  page: string
  revision: number
  x: number
  y: number
  ghostPages: CanvasGhostPage[]
}

export interface CanvasGhostPage {
  page: string
  x: number
  y: number
}

interface CanvasTransformFrame {
  element: string
  frame: {
    x: number
    y: number
    width: number
    height: number
    angleDegrees: number
  }
}

interface CanvasHandle {
  resize(width: number, height: number, dpr: number, background: Uint8Array): void
  setView(zoom: number, translationX: number, translationY: number): void
  stageManifest(manifest: Uint8Array): { token: number; missing: string[] }
  installResource(resource: string, packet: Uint8Array): Promise<void>
  activateFrame(token: number): boolean
  activatePage(page: string, expectedRevision: bigint): boolean
  cacheFrame(token: number): boolean
  pinPageManifests(pages: string[]): void
  unpinPageManifests(pages: string[]): void
  getLastLayoutRejection(): string | null
  setPageLayout(
    pages: {
      page: string
      revision: bigint
      x: number
      y: number
      ghostPages: CanvasGhostPage[]
    }[],
  ): boolean
  clear(): void
  previewOpacity(element: string, opacity: number | null): void
  beginTransform(page: string, elements: CanvasTransformFrame[]): void
  updateTransform(sequence: number, elements: CanvasTransformFrame[]): void
  finishTransform(): unknown
  cancelTransform(): void
  beginStroke(
    page: string,
    expectedRevision: bigint,
    kind: CanvasStrokeKind,
    layer: string | null,
    point: Point,
    diameter: number,
    color: Uint8Array,
  ): void
  extendStroke(points: Point[]): void
  finishStroke(): unknown
  cancelStroke(): void
  sampleColor(x: number, y: number): Promise<Uint8Array>
  setDeviceLostCallback(callback: ((reason: string) => void) | null): void
  dispose(): void
  free(): void
}

interface CanvasModule {
  default(): Promise<unknown>
  createCanvas(element: HTMLCanvasElement): Promise<CanvasHandle>
}

let modulePromise: Promise<CanvasModule> | null = null
let activeCanvas: Canvas | null = null
let prefetchGeneration = 0
let nextPrefetchRequest = 0
const preparationQueues = new WeakMap<Canvas, Promise<void>>()

export function traceCanvas(event: string, details: Record<string, unknown>): void {
  if (typeof process === 'undefined' || process.env.NODE_ENV !== 'development') return
  console.debug('[Norrix canvas]', event, {
    timestamp: new Date().toISOString(),
    monotonicMs: typeof performance === 'undefined' ? null : performance.now(),
    ...details,
  })
}

export async function withCanvasPreparation<Result>(
  canvas: Canvas,
  prepare: () => Promise<Result>,
): Promise<Result> {
  const previous = preparationQueues.get(canvas) ?? Promise.resolve()
  let release!: () => void
  const gate = new Promise<void>((resolve) => {
    release = resolve
  })
  const queued = previous.then(() => gate)
  preparationQueues.set(canvas, queued)
  await previous
  try {
    return await prepare()
  } finally {
    release()
    if (preparationQueues.get(canvas) === queued) preparationQueues.delete(canvas)
  }
}

export async function createCanvas(
  element: HTMLCanvasElement,
  onDeviceLost: (reason: string) => void,
): Promise<Canvas> {
  const module = await loadCanvasModule()
  const canvas = await module.createCanvas(element)
  let transformSequence = 0
  let disposed = false
  let stagedManifest: { token: number; bytes: Uint8Array } | null = null
  let activeManifest: Uint8Array | null = null
  const cachedManifests = new Map<string, Uint8Array>()
  canvas.setDeviceLostCallback(onDeviceLost)
  traceCanvas('wasm-renderer-created', {
    operation: 'createCanvas',
    result: 'created',
    initialWidth: element.width,
    initialHeight: element.height,
  })

  const methods: Canvas = {
    resize: (width, height, dpr, background) => {
      traceCanvas('canvas-size-requested', {
        operation: 'Canvas.resize',
        cssWidth: width,
        cssHeight: height,
        devicePixelRatio: dpr,
        result: 'requested',
      })
      canvas.resize(width, height, dpr, new Uint8Array([...background, 255]))
    },
    setView: (zoom, translation) => canvas.setView(zoom, translation[0], translation[1]),
    stageManifest: (manifest) => {
      const staged = canvas.stageManifest(manifest)
      stagedManifest = { token: staged.token, bytes: manifest.slice() }
      return staged
    },
    installResource: (resource, packet) => canvas.installResource(resource, packet),
    hasActiveManifest: (manifest) =>
      activeManifest !== null && equalBytes(activeManifest, manifest),
    activateFrame: (token) => {
      const activated = canvas.activateFrame(token)
      if (activated && stagedManifest?.token === token) {
        activeManifest = stagedManifest.bytes
        stagedManifest = null
      }
      return activated
    },
    activatePage: (page, expectedRevision) => {
      const activated = canvas.activatePage(page, BigInt(expectedRevision))
      if (activated) activeManifest = cachedManifests.get(page) ?? null
      return activated
    },
    cacheFrame: (token, page) => {
      const cached = canvas.cacheFrame(token)
      if (stagedManifest?.token === token) {
        if (cached) {
          cachedManifests.delete(page)
          cachedManifests.set(page, stagedManifest.bytes)
          while (cachedManifests.size > MAX_CACHED_CANVAS_MANIFESTS) {
            const oldest = cachedManifests.keys().next().value
            if (oldest === undefined) break
            cachedManifests.delete(oldest)
          }
        }
        stagedManifest = null
      }
      return cached
    },
    pinPageManifests: (pages) => canvas.pinPageManifests(pages),
    unpinPageManifests: (pages) => {
      if (disposed) {
        traceCanvas('page-manifest-unpin-skipped', {
          operation: 'Canvas.unpinPageManifests',
          pages,
          result: 'skipped',
          reason: 'renderer already disposed; pinned manifests were released with the renderer',
        })
        return
      }
      canvas.unpinPageManifests(pages)
    },
    getLastLayoutRejection: () => canvas.getLastLayoutRejection(),
    setPageLayout: (pages) =>
      canvas.setPageLayout(pages.map((page) => ({ ...page, revision: BigInt(page.revision) }))),
    clear: () => {
      canvas.clear()
      stagedManifest = null
      activeManifest = null
      cachedManifests.clear()
    },
    previewOpacity: (element, opacity) => canvas.previewOpacity(element, opacity),
    beginTransform: (page, elements) => {
      transformSequence = 0
      canvas.beginTransform(page, canvasTransformFrames(elements))
    },
    updateTransform: (elements) =>
      canvas.updateTransform(++transformSequence, canvasTransformFrames(elements)),
    finishTransform: () => void canvas.finishTransform(),
    cancelTransform: () => canvas.cancelTransform(),
    beginStroke: (stroke) =>
      canvas.beginStroke(
        stroke.page,
        BigInt(stroke.revision),
        stroke.kind,
        stroke.layer,
        stroke.point,
        stroke.diameter,
        new Uint8Array(stroke.color ?? [0, 0, 0, 0]),
      ),
    extendStroke: (points) => canvas.extendStroke(points),
    finishStroke: () => void canvas.finishStroke(),
    cancelStroke: () => canvas.cancelStroke(),
    sampleColor: async (point) => {
      const color = await canvas.sampleColor(point.x, point.y)
      if (color.length !== 4) throw new Error('The WebGPU canvas returned an invalid color.')
      return [color[0], color[1], color[2], color[3]]
    },
    dispose: () => dispose(),
  }

  let activeOperations = 0
  let disposal: Promise<void> | null = null
  let finishDisposal: (() => void) | null = null
  let rejectDisposal: ((reason: unknown) => void) | null = null
  let disposalError: unknown

  const finalizeDisposal = () => {
    if (!disposed || activeOperations !== 0 || !finishDisposal) return
    traceCanvas('wasm-renderer-dispose-started', {
      operation: 'Canvas.dispose',
      activeOperations,
      result: 'started',
    })
    const finish = finishDisposal
    finishDisposal = null
    const fail = rejectDisposal
    rejectDisposal = null
    try {
      canvas.setDeviceLostCallback(null)
    } catch (error) {
      disposalError ??= error
    }
    try {
      canvas.dispose()
    } catch (error) {
      disposalError ??= error
    }
    try {
      canvas.free()
    } catch (error) {
      disposalError ??= error
    }
    if (disposalError === undefined) {
      traceCanvas('wasm-renderer-disposed', {
        operation: 'Canvas.dispose',
        activeOperations,
        result: 'disposed',
      })
      finish()
    } else {
      traceCanvas('wasm-renderer-dispose-failed', {
        operation: 'Canvas.dispose',
        activeOperations,
        result: 'failed',
        reason:
          disposalError instanceof Error
            ? disposalError.message
            : typeof disposalError === 'string'
              ? disposalError
              : 'non-Error disposal failure',
      })
      fail?.(disposalError)
    }
  }

  const releaseOperation = () => {
    activeOperations -= 1
    finalizeDisposal()
  }

  const runAsync = <Result>(operation: () => Promise<Result>): Promise<Result> => {
    if (disposed) return Promise.reject(new Error('The WebGPU canvas is disposed.'))
    activeOperations += 1
    try {
      return operation().finally(releaseOperation)
    } catch (error) {
      releaseOperation()
      return Promise.reject(error)
    }
  }

  function dispose(): Promise<void> {
    if (disposal) return disposal
    disposed = true
    traceCanvas('wasm-renderer-dispose-requested', {
      operation: 'Canvas.dispose',
      activeOperations,
      result: 'pending',
      reason: activeOperations ? 'waiting for active WASM operations' : 'no active operations',
    })
    disposal = new Promise<void>((resolve, reject) => {
      finishDisposal = resolve
      rejectDisposal = reject
      finalizeDisposal()
    })
    return disposal
  }

  const wrappedMethods = new Map<PropertyKey, (...args: never[]) => unknown>()
  return new Proxy(methods, {
    get(target, property, receiver) {
      if (property === 'dispose') return dispose
      const method = Reflect.get(target, property, receiver)
      if (typeof method !== 'function') return method
      const existing = wrappedMethods.get(property)
      if (existing) return existing
      const wrapped =
        property === 'installResource'
          ? (((resource: string, packet: Uint8Array) =>
              runAsync(() => canvas.installResource(resource, packet))) as (
              ...args: never[]
            ) => unknown)
          : property === 'sampleColor'
            ? (((point: Point) => runAsync(() => methods.sampleColor(point))) as (
                ...args: never[]
              ) => unknown)
            : (((...args: never[]) => {
                if (disposed) throw new Error('The WebGPU canvas is disposed.')
                return Reflect.apply(method, target, args)
              }) as (...args: never[]) => unknown)
      wrappedMethods.set(property, wrapped)
      return wrapped
    },
  })
}

function loadCanvasModule(): Promise<CanvasModule> {
  // @ts-ignore -- wasm-pack output is created by bridge build tasks and absent in clean checkouts.
  modulePromise ??= import('./wasm/norrix_canvas.js')
    .then(async (module) => {
      const canvas = module as CanvasModule
      await canvas.default()
      return canvas
    })
    .catch((error: unknown) => {
      modulePromise = null
      throw error
    })
  return modulePromise
}

export async function fetchCanvasManifest(generation: number): Promise<Uint8Array> {
  return canvasBytesView(await commands.getCanvasManifest(generation))
}

export async function fetchCanvasResource(
  generation: number,
  resource: string,
): Promise<Uint8Array> {
  return canvasBytesView(await commands.getCanvasResource(generation, resource))
}

// Specta exposes CanvasBytes as number[], while its IpcResponse sends an ArrayBuffer at runtime.
function canvasBytesView(bytes: number[] | ArrayBuffer): Uint8Array {
  return new Uint8Array(bytes)
}

function equalBytes(left: Uint8Array, right: Uint8Array): boolean {
  return (
    left.byteLength === right.byteLength && left.every((value, index) => value === right[index])
  )
}

export function activateCanvas(canvas: Canvas): () => void {
  prefetchGeneration++
  activeCanvas = canvas
  traceCanvas('renderer-activated', {
    operation: 'activateCanvas',
    generation: prefetchGeneration,
    result: 'active',
  })
  return () => {
    if (activeCanvas === canvas) {
      prefetchGeneration++
      activeCanvas = null
      traceCanvas('renderer-deactivated', {
        operation: 'activateCanvas.cleanup',
        generation: prefetchGeneration,
        result: 'disposed',
      })
    }
  }
}

export function previewCanvasOpacity(element: string, opacity: number | null): void {
  activeCanvas?.previewOpacity(element, opacity)
}

export function showCanvasPage(page: string, expectedRevision: number | null): boolean {
  prefetchGeneration++
  const activated =
    expectedRevision === null
      ? false
      : (activeCanvas?.activatePage(page, expectedRevision) ?? false)
  traceCanvas('page-switch-requested', {
    operation: 'showCanvasPage',
    page,
    revision: expectedRevision,
    generation: prefetchGeneration,
    result: activated ? 'activated-from-cache' : 'requires-preparation',
  })
  return activated
}

export function cancelCanvasPrefetch(): void {
  prefetchGeneration++
  traceCanvas('prefetch-invalidated', {
    operation: 'cancelCanvasPrefetch',
    generation: prefetchGeneration,
    result: 'cancelled',
  })
}

export async function prefetchCanvasPages(
  pages: string[],
  context: {
    operation?: string
    generation?: number | null
    revision?: number | null
    traceId?: string
  } = {},
) {
  const canvas = activeCanvas
  if (pages.length === 0) return []
  if (!canvas) {
    traceCanvas('frame-preparation-skipped', {
      operation: context.operation ?? 'prefetchCanvasPages',
      traceId: context.traceId ?? null,
      pages,
      generation: context.generation ?? null,
      requestedRevision: context.revision ?? null,
      result: 'skipped',
      reason: 'no active WebGPU canvas',
    })
    return []
  }
  const preparedPages: CanvasPagePreparation[] = []
  const generation = prefetchGeneration
  const request = ++nextPrefetchRequest
  const operation = context.operation ?? 'prefetchCanvasPages'
  const current = () => activeCanvas === canvas && prefetchGeneration === generation
  const isCurrent = (page: string, revision: number | null, stage: string) => {
    if (current()) return true
    traceCanvas('frame-preparation-cancelled', {
      request,
      traceId: context.traceId ?? null,
      operation,
      stage,
      page,
      revision,
      generation: context.generation ?? null,
      requestedRevision: context.revision ?? revision,
      prefetchGeneration: generation,
      currentPrefetchGeneration: prefetchGeneration,
      result: 'cancelled',
      reason: activeCanvas !== canvas ? 'renderer-replaced' : 'superseded',
    })
    return false
  }
  traceCanvas('frame-preparation-requested', {
    request,
    traceId: context.traceId ?? null,
    operation,
    pages,
    generation: context.generation ?? null,
    requestedRevision: context.revision ?? null,
    prefetchGeneration: generation,
    result: 'started',
  })
  for (const page of pages) {
    let stage = 'queued'
    let revision: number | null = null
    let prepared: CanvasPagePreparation | false
    try {
      prepared = await withCanvasPreparation(canvas, async () => {
        stage = 'before-prepare'
        if (!isCurrent(page, null, 'before-prepare')) return false
        stage = 'prepareCanvasPage'
        traceCanvas('frame-preparation-started', {
          request,
          traceId: context.traceId ?? null,
          operation,
          page,
          generation: context.generation ?? null,
          requestedRevision: context.revision ?? null,
          result: 'started',
        })
        const prepared = await commands.prepareCanvasPage(page, context.traceId ?? null)
        if (!isCurrent(page, prepared?.revision ?? null, 'after-prepare')) return false
        if (prepared === null) {
          traceCanvas('frame-preparation-failed', {
            request,
            traceId: context.traceId ?? null,
            operation,
            page,
            revision: null,
            generation: context.generation ?? null,
            result: 'failed',
            reason: 'prepareCanvasPage returned no frame',
          })
          return false
        }
        const preparedRevision = prepared.revision
        revision = preparedRevision
        traceCanvas('frame-prepared', {
          request,
          traceId: context.traceId ?? null,
          operation,
          page,
          revision: preparedRevision,
          generation: context.generation ?? null,
          result: 'prepared',
        })
        stage = 'getCanvasPageManifest'
        const manifest = canvasBytesView(
          await commands.getCanvasPageManifest(page, preparedRevision),
        )
        if (!isCurrent(page, preparedRevision, 'after-manifest')) return false
        const staged = canvas.stageManifest(manifest)
        traceCanvas('frame-composition-staged', {
          request,
          traceId: context.traceId ?? null,
          operation,
          page,
          revision: preparedRevision,
          generation: context.generation ?? null,
          stageToken: staged.token,
          missingResources: staged.missing.length,
          result: 'staged',
        })
        for (let offset = 0; offset < staged.missing.length; offset += 4) {
          const resources = staged.missing.slice(offset, offset + 4)
          stage = 'getCanvasPageResource'
          const packets = await Promise.all(
            resources.map(
              async (resource) =>
                [
                  resource,
                  canvasBytesView(
                    await commands.getCanvasPageResource(page, preparedRevision, resource),
                  ),
                ] as const,
            ),
          )
          if (!isCurrent(page, preparedRevision, 'after-resource-fetch')) return false
          stage = 'installResource'
          await Promise.all(
            packets.map(([resource, packet]) => canvas.installResource(resource, packet)),
          )
          traceCanvas('frame-resources-installed', {
            request,
            traceId: context.traceId ?? null,
            operation,
            page,
            revision: preparedRevision,
            generation: context.generation ?? null,
            resources: resources.length,
            result: 'installed',
          })
        }
        stage = 'cacheFrame'
        if (!isCurrent(page, preparedRevision, 'before-cache')) return false
        const cached = canvas.cacheFrame(staged.token, page)
        traceCanvas(cached ? 'frame-cached' : 'frame-cache-rejected', {
          request,
          traceId: context.traceId ?? null,
          operation,
          page,
          revision: preparedRevision,
          generation: context.generation ?? null,
          stageToken: staged.token,
          result: cached ? 'cached' : 'rejected',
          reason: cached ? null : 'WASM cache rejected the staged revision',
        })
        return cached ? prepared : false
      })
    } catch (error) {
      traceCanvas('frame-preparation-failed', {
        request,
        operation,
        stage,
        page,
        revision,
        generation: context.generation ?? null,
        requestedRevision: context.revision ?? revision,
        result: 'failed',
        reason: describeCanvasFailure(error),
      })
      throw error
    }
    if (prepared) preparedPages.push(prepared)
  }
  traceCanvas('frame-preparation-finished', {
    request,
    traceId: context.traceId ?? null,
    operation,
    pages,
    prepared: preparedPages.map(({ page, revision }) => ({ page: page.id, revision })),
    generation: context.generation ?? null,
    result: preparedPages.length === pages.length ? 'complete' : 'partial',
    reason: preparedPages.length === pages.length ? null : 'one or more frames were not cached',
  })
  return preparedPages
}

function describeCanvasFailure(value: unknown): {
  name: string
  message: string
  stack: string | null
} {
  if (value instanceof Error) {
    return {
      name: value.name || 'Error',
      message: value.message,
      stack: value.stack ?? null,
    }
  }
  if (typeof value === 'string') return { name: 'NonErrorString', message: value, stack: null }
  if (typeof value === 'object' && value !== null) {
    const candidate = value as { name?: unknown; message?: unknown; stack?: unknown }
    return {
      name: typeof candidate.name === 'string' ? candidate.name : 'NonErrorObject',
      message: typeof candidate.message === 'string' ? candidate.message : 'Unknown canvas failure',
      stack: typeof candidate.stack === 'string' ? candidate.stack : null,
    }
  }
  return { name: typeof value, message: String(value), stack: null }
}

export function workspaceColor(): WorkspaceColor {
  const raw = window
    .getComputedStyle(document.documentElement)
    .getPropertyValue('--workspace-background-rgb')
  const values = raw.trim().split(/\s+/).map(Number)
  if (values.length !== 3 || values.some((value) => !Number.isFinite(value))) {
    return [183, 180, 174]
  }
  return values.map((value) => Math.min(255, Math.max(0, Math.round(value)))) as WorkspaceColor
}

function canvasTransformFrames(elements: TransformFrame[]): CanvasTransformFrame[] {
  return elements.map(({ element, frame }) => ({
    element,
    frame: {
      x: frame.x,
      y: frame.y,
      width: frame.width,
      height: frame.height,
      angleDegrees: frame.angle_degrees,
    },
  }))
}
