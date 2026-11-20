import { describe, expect, it, vi } from 'vitest'

import {
  activateCanvas,
  createCanvas,
  prefetchCanvasPages,
  previewCanvasOpacity,
  showCanvasPage,
  type Canvas,
  withCanvasPreparation,
} from '@norrix/bridge/canvas'

const commands = vi.hoisted(() => ({
  prepareCanvasPage: vi.fn(),
  getCanvasPageManifest: vi.fn(),
  getCanvasPageResource: vi.fn(),
}))
const wasm = vi.hoisted(() => {
  const handle = {
    resize: vi.fn(),
    setView: vi.fn(),
    stageManifest: vi.fn(),
    installResource: vi.fn(),
    activateFrame: vi.fn(),
    activatePage: vi.fn(),
    cacheFrame: vi.fn(),
    setPageLayout: vi.fn(),
    clear: vi.fn(),
    previewOpacity: vi.fn(),
    beginTransform: vi.fn(),
    updateTransform: vi.fn(),
    finishTransform: vi.fn(),
    cancelTransform: vi.fn(),
    beginStroke: vi.fn(),
    extendStroke: vi.fn(),
    finishStroke: vi.fn(),
    cancelStroke: vi.fn(),
    sampleColor: vi.fn(),
    setDeviceLostCallback: vi.fn(),
    dispose: vi.fn(),
    free: vi.fn(),
  }
  return {
    default: vi.fn(async () => {}),
    createCanvas: vi.fn(async () => handle),
    handle,
  }
})

vi.mock('@norrix/bridge/protocol', () => ({ commands }))
vi.mock('../mocks/norrix_canvas.ts', () => wasm)

describe('canvas runtime', () => {
  it('serializes active-page and prefetch staging transactions on one WebGPU canvas', async () => {
    const canvas = {} as Canvas
    const firstStarted = deferred<void>()
    const releaseFirst = deferred<void>()
    const order: string[] = []
    const activePage = withCanvasPreparation(canvas, async () => {
      order.push('active:start')
      firstStarted.resolve()
      await releaseFirst.promise
      order.push('active:finish')
    })
    await firstStarted.promise

    const prefetch = withCanvasPreparation(canvas, async () => {
      order.push('prefetch:start')
    })
    await Promise.resolve()
    expect(order).toEqual(['active:start'])

    releaseFirst.resolve()
    await Promise.all([activePage, prefetch])

    expect(order).toEqual(['active:start', 'active:finish', 'prefetch:start'])
  })

  it('routes inspector previews only to the active canvas', () => {
    const previewOpacity = vi.fn()
    const canvas = { previewOpacity } as unknown as Canvas
    const deactivate = activateCanvas(canvas)

    previewCanvasOpacity('layer', 0.5)
    deactivate()
    previewCanvasOpacity('layer', null)

    expect(previewOpacity).toHaveBeenCalledOnce()
    expect(previewOpacity).toHaveBeenCalledWith('layer', 0.5)
  })

  function deferred<T>() {
    let resolve!: (value: T) => void
    const promise = new Promise<T>((done) => {
      resolve = done
    })
    return { promise, resolve }
  }

  it('shows a cached page immediately on the active canvas', () => {
    const activatePage = vi.fn(() => true)
    const canvas = { activatePage } as unknown as Canvas
    const deactivate = activateCanvas(canvas)

    expect(showCanvasPage('page', 4)).toBe(true)
    deactivate()

    expect(activatePage).toHaveBeenCalledWith('page', 4)
  })

  it('prepares and installs adjacent page resources before caching the frame', async () => {
    const order: string[] = []
    const prepared = {
      revision: 4,
      page: { id: 'page' },
    }
    commands.prepareCanvasPage.mockResolvedValue(prepared)
    commands.getCanvasPageManifest.mockResolvedValue(new ArrayBuffer(1))
    commands.getCanvasPageResource.mockResolvedValue(new ArrayBuffer(1))
    const canvas = {
      stageManifest: vi.fn(() => ({ token: 7, missing: ['resource'] })),
      installResource: vi.fn(async () => void order.push('resource')),
      cacheFrame: vi.fn(() => {
        order.push('frame')
        return true
      }),
    } as unknown as Canvas
    const deactivate = activateCanvas(canvas)

    const result = await prefetchCanvasPages(['page'], { traceId: 'trace-update' })
    deactivate()

    expect(commands.prepareCanvasPage).toHaveBeenCalledWith('page', 'trace-update')
    expect(commands.getCanvasPageManifest).toHaveBeenCalledWith('page', 4)
    expect(commands.getCanvasPageResource).toHaveBeenCalledWith('page', 4, 'resource')
    expect(canvas.cacheFrame).toHaveBeenCalledWith(7, 'page')
    expect(order).toEqual(['resource', 'frame'])
    expect(result).toEqual([prepared])
  })

  it('lets overlapping viewport and intent prefetches cache both pages', async () => {
    commands.prepareCanvasPage.mockImplementation(async (page: string) => ({
      revision: 4,
      page: { id: page },
    }))
    commands.getCanvasPageManifest.mockResolvedValue(new ArrayBuffer(1))
    const cacheFrame = vi.fn(() => true)
    let token = 0
    const canvas = {
      stageManifest: vi.fn(() => ({ token: ++token, missing: [] })),
      cacheFrame,
    } as unknown as Canvas
    const deactivate = activateCanvas(canvas)

    const viewportPrefetch = prefetchCanvasPages(['visible-page'])
    const intentPrefetch = prefetchCanvasPages(['pointed-page'])
    const [viewport, intent] = await Promise.all([viewportPrefetch, intentPrefetch])
    deactivate()

    expect(viewport.map(({ page }) => page.id)).toEqual(['visible-page'])
    expect(intent.map(({ page }) => page.id)).toEqual(['pointed-page'])
    expect(cacheFrame.mock.calls).toEqual([
      [1, 'visible-page'],
      [2, 'pointed-page'],
    ])
  })

  it('keeps each prepared revision pinned through manifest and resource staging', async () => {
    const firstPrepared = deferred<{ revision: number; page: { id: string } }>()
    const order: string[] = []
    commands.prepareCanvasPage.mockImplementation(async (page: string) => {
      order.push(`prepare:${page}`)
      return page === 'first' ? firstPrepared.promise : { revision: 5, page: { id: page } }
    })
    commands.getCanvasPageManifest.mockImplementation(async (page: string) => {
      order.push(`manifest:${page}`)
      return new ArrayBuffer(1)
    })
    const canvas = {
      stageManifest: vi.fn(() => ({ token: 1, missing: [] })),
      cacheFrame: vi.fn((_token, page: string) => {
        order.push(`cache:${page}`)
        return true
      }),
    } as unknown as Canvas
    const deactivate = activateCanvas(canvas)

    const first = prefetchCanvasPages(['first'])
    await Promise.resolve()
    await Promise.resolve()
    const second = prefetchCanvasPages(['second'])
    await Promise.resolve()
    expect(commands.prepareCanvasPage).toHaveBeenCalledTimes(1)

    firstPrepared.resolve({ revision: 4, page: { id: 'first' } })
    await Promise.all([first, second])
    deactivate()

    expect(order).toEqual([
      'prepare:first',
      'manifest:first',
      'cache:first',
      'prepare:second',
      'manifest:second',
      'cache:second',
    ])
  })

  it('does not expose page data when the frame was rejected by the cache', async () => {
    commands.prepareCanvasPage.mockResolvedValue({ revision: 4, page: { id: 'page' } })
    commands.getCanvasPageManifest.mockResolvedValue(new ArrayBuffer(1))
    const canvas = {
      stageManifest: vi.fn(() => ({ token: 7, missing: [] })),
      cacheFrame: vi.fn(() => false),
    } as unknown as Canvas
    const deactivate = activateCanvas(canvas)

    const result = await prefetchCanvasPages(['page'])
    deactivate()

    expect(result).toEqual([])
  })

  it('waits for pending WASM operations before freeing the canvas exactly once', async () => {
    let finishOperation: (() => void) | undefined
    wasm.handle.installResource.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          finishOperation = resolve
        }),
    )
    const canvas = await createCanvas(document.createElement('canvas'), vi.fn())
    const operation = canvas.installResource('resource', new Uint8Array([1]))
    const disposal = canvas.dispose()

    expect(wasm.handle.dispose).not.toHaveBeenCalled()
    expect(wasm.handle.free).not.toHaveBeenCalled()
    expect(wasm.handle.setDeviceLostCallback).toHaveBeenCalledOnce()
    expect(canvas.dispose()).toBe(disposal)
    expect(() => canvas.clear()).toThrow('The WebGPU canvas is disposed.')

    finishOperation?.()
    await operation
    await disposal

    expect(wasm.handle.dispose).toHaveBeenCalledOnce()
    expect(wasm.handle.free).toHaveBeenCalledOnce()
    expect(wasm.handle.setDeviceLostCallback).toHaveBeenLastCalledWith(null)
  })
})
