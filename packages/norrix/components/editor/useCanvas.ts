'use client'

import { useCallback, useEffect, useRef, useState } from 'react'

import {
  activateCanvas,
  cancelCanvasPrefetch,
  createCanvas,
  fetchCanvasManifest,
  fetchCanvasResource,
  traceCanvas,
  type Canvas,
  withCanvasPreparation,
} from '@norrix/bridge/canvas'

export type CanvasStatus = 'loading' | 'switching' | 'ready' | 'recovering' | 'error'

interface CanvasState {
  canvas: Canvas | null
  error: Error | null
  generation: number | null
  hasFrame: boolean
  retry: () => void
  status: CanvasStatus
}

interface CanvasLoad {
  key: string
  startedAt: number
  kind: 'initial' | 'update' | 'recovery'
  page: string | null
  revision: number | null
  generation: number
}

export function useCanvas(
  element: HTMLCanvasElement | null,
  revision: number | null,
  generation: number,
  pageId: string | null = null,
  traceId = '',
): CanvasState {
  const [canvasAttempt, setCanvasAttempt] = useState(0)
  const [loadAttempt, setLoadAttempt] = useState(0)
  const [canvas, setCanvas] = useState<Canvas | null>(null)
  const [status, setStatus] = useState<CanvasStatus>('loading')
  const [error, setError] = useState<Error | null>(null)
  const [activeGeneration, setActiveGeneration] = useState<number | null>(null)
  const [hasFrame, setHasFrame] = useState(false)
  const hasFrameRef = useRef(false)
  const generationRef = useRef(generation)
  const revisionRef = useRef(revision)
  const pageIdRef = useRef(pageId)
  const traceIdRef = useRef(traceId)
  const activeFrame = useRef<{
    canvas: Canvas
    generation: number
    page: string | null
    revision: number | null
  } | null>(null)
  const load = useRef<CanvasLoad | null>(null)
  const request = useRef<object | null>(null)
  generationRef.current = generation
  revisionRef.current = revision
  pageIdRef.current = pageId
  traceIdRef.current = traceId

  const updateHasFrame = useCallback((value: boolean) => {
    hasFrameRef.current = value
    setHasFrame(value)
  }, [])

  const retry = useCallback(() => {
    if (canvas) setLoadAttempt((value) => value + 1)
    else setCanvasAttempt((value) => value + 1)
  }, [canvas])

  useEffect(() => {
    if (!element) return
    let active = true
    let owned: Canvas | null = null
    traceCanvas('renderer-create-requested', {
      operation: 'useCanvas.createCanvas',
      page: pageIdRef.current,
      traceId: traceIdRef.current || null,
      revision: revisionRef.current,
      generation: generationRef.current,
      result: 'started',
      canvasWidth: element.width,
      canvasHeight: element.height,
    })
    setCanvas(null)
    setError(null)
    setActiveGeneration(null)
    updateHasFrame(false)
    setStatus(canvasAttempt === 0 ? 'loading' : 'recovering')

    void createCanvas(element, (reason) => {
      if (!active) return
      traceCanvas('renderer-device-lost', {
        operation: 'WebGPU.deviceLost',
        page: pageIdRef.current,
        traceId: traceIdRef.current || null,
        revision: revisionRef.current,
        generation: generationRef.current,
        result: 'lost',
        reason,
      })
      setStatus('recovering')
      setCanvasAttempt((value) => value + 1)
    })
      .then((created) => {
        if (!active) {
          traceCanvas('renderer-dispose-requested', {
            operation: 'useCanvas.createCanvas',
            page: pageIdRef.current,
            traceId: traceIdRef.current || null,
            revision: revisionRef.current,
            generation: generationRef.current,
            result: 'stale-instance',
            reason: 'React effect was cleaned up before initialization completed',
          })
          created.dispose()
          return
        }
        owned = created
        setCanvas(created)
        traceCanvas('renderer-created', {
          operation: 'useCanvas.createCanvas',
          page: pageIdRef.current,
          traceId: traceIdRef.current || null,
          revision: revisionRef.current,
          generation: generationRef.current,
          result: 'ready',
        })
      })
      .catch((reason: unknown) => {
        if (!active) return
        setError(toError(reason))
        setStatus('error')
        traceCanvas('renderer-create-failed', {
          operation: 'useCanvas.createCanvas',
          page: pageIdRef.current,
          traceId: traceIdRef.current || null,
          revision: revisionRef.current,
          generation: generationRef.current,
          result: 'failed',
          reason: toError(reason).message,
        })
      })

    return () => {
      active = false
      request.current = null
      activeFrame.current = null
      traceCanvas('renderer-dispose-requested', {
        operation: 'useCanvas.cleanup',
        page: pageIdRef.current,
        revision: revisionRef.current,
        generation: generationRef.current,
        result: owned ? 'requested' : 'no-owned-instance',
        reason: 'canvas element or initialization attempt changed',
      })
      owned?.dispose()
    }
  }, [canvasAttempt, element, updateHasFrame])

  useEffect(() => {
    if (!canvas) return
    const requestedTraceId = traceIdRef.current
    if (revision === null) {
      cancelCanvasPrefetch()
      request.current = null
      if (pageId !== null && hasFrameRef.current && activeFrame.current?.canvas === canvas) {
        traceCanvas('frame-clear-skipped', {
          operation: 'useCanvas.revision-null',
          page: pageId,
          traceId: requestedTraceId || null,
          revision: null,
          generation,
          result: 'preserved',
          reason: 'a target page exists while its next revision is unavailable',
        })
        setStatus('switching')
        return
      }
      if (
        !hasFrameRef.current &&
        activeFrame.current?.canvas === canvas &&
        activeFrame.current.generation === generation
      )
        return
      try {
        traceCanvas('frame-clear-started', {
          operation: 'useCanvas.revision-null',
          page: pageId,
          revision: null,
          generation,
          result: 'started',
          reason: pageId === null ? 'no target page' : 'no valid frame is currently presented',
        })
        canvas.clear()
        activeFrame.current = { canvas, generation, page: pageId, revision: null }
        setActiveGeneration(generation)
        updateHasFrame(false)
        setError(null)
        setStatus('ready')
        traceCanvas('frame-cleared', {
          operation: 'useCanvas.revision-null',
          page: pageId,
          revision: null,
          generation,
          result: 'cleared',
        })
      } catch (reason) {
        setError(toError(reason))
        setStatus('error')
        traceCanvas('frame-clear-failed', {
          operation: 'useCanvas.revision-null',
          page: pageId,
          revision: null,
          generation,
          result: 'failed',
          reason: toError(reason).message,
        })
      }
      return
    }
    if (
      generation === 0 ||
      (activeFrame.current?.canvas === canvas && activeFrame.current.generation === generation)
    )
      return

    const requested = generation
    cancelCanvasPrefetch()
    const currentRequest = {}
    request.current = currentRequest
    setError(null)
    setStatus((current) =>
      hasFrameRef.current ? 'switching' : current === 'recovering' ? 'recovering' : 'loading',
    )
    traceCanvas('frame-switch-requested', {
      operation: 'useCanvas.prepareFrame',
      page: pageId,
      traceId: requestedTraceId || null,
      revision,
      generation: requested,
      result: 'started',
      previousPage: activeFrame.current?.page ?? null,
      previousRevision: activeFrame.current?.revision ?? null,
      previousGeneration: activeFrame.current?.generation ?? null,
      hasPreviousFrame: hasFrameRef.current,
    })

    const current = () =>
      request.current === currentRequest &&
      generationRef.current === requested &&
      revisionRef.current !== null

    void withCanvasPreparation(canvas, () =>
      prepareFrame(canvas, pageId, revision, requested, current, requestedTraceId),
    )
      .then((activated) => {
        if (!current() || !activated) {
          traceCanvas('frame-switch-cancelled', {
            operation: 'useCanvas.prepareFrame',
            page: pageId,
            traceId: requestedTraceId || null,
            revision,
            generation: requested,
            result: 'cancelled',
            reason: current() ? 'frame was not activated' : 'request was superseded',
          })
          return
        }
        activeFrame.current = { canvas, generation: requested, page: pageId, revision }
        setActiveGeneration(requested)
        request.current = null
        updateHasFrame(true)
        setError(null)
        setStatus('ready')
        traceCanvas('frame-switch-ready', {
          operation: 'useCanvas.prepareFrame',
          page: pageId,
          traceId: requestedTraceId || null,
          revision,
          generation: requested,
          result: 'activated',
        })
      })
      .catch((reason: unknown) => {
        if (!current()) return
        request.current = null
        setError(toError(reason))
        setStatus('error')
        traceCanvas('frame-switch-failed', {
          operation: 'useCanvas.prepareFrame',
          page: pageId,
          traceId: requestedTraceId || null,
          revision,
          generation: requested,
          result: 'failed',
          reason: toError(reason).message,
        })
      })
  }, [canvas, generation, loadAttempt, pageId, revision, updateHasFrame])

  useEffect(() => {
    if (!canvas) return
    return activateCanvas(canvas)
  }, [canvas])

  useEffect(() => {
    const waiting =
      revision !== null &&
      (status === 'loading' || status === 'switching' || status === 'recovering')
    if (waiting) {
      const key = `${pageId ?? ''}:${revision}:${generation}`
      if (load.current?.key !== key) {
        if (load.current) {
          reportCanvasLoad('interrupted', load.current, {
            durationMs: Math.round(performance.now() - load.current.startedAt),
            reason: 'superseded',
          })
        }
        const next: CanvasLoad = {
          key,
          startedAt: performance.now(),
          kind: status === 'recovering' ? 'recovery' : hasFrameRef.current ? 'update' : 'initial',
          page: pageId,
          revision,
          generation,
        }
        load.current = next
        reportCanvasLoad('started', next)
      }
      return
    }

    const current = load.current
    if (!current) return
    if (status === 'ready') {
      reportCanvasLoad('ready', current, {
        durationMs: Math.round(performance.now() - current.startedAt),
      })
      load.current = null
    } else if (status === 'error') {
      reportCanvasLoad('failed', current, {
        durationMs: Math.round(performance.now() - current.startedAt),
        reason: error?.message ?? 'unknown canvas error',
      })
      load.current = null
    } else if (revision === null) {
      reportCanvasLoad('interrupted', current, {
        durationMs: Math.round(performance.now() - current.startedAt),
        reason: 'canvas cleared',
      })
      load.current = null
    }
  }, [error, generation, pageId, revision, status])

  return { canvas, error, generation: activeGeneration, hasFrame, retry, status }
}

function reportCanvasLoad(
  event: string,
  load: CanvasLoad,
  details: Record<string, number | string> = {},
): void {
  if (process.env.NODE_ENV !== 'development') return
  console.debug('[Norrix canvas]', event, {
    kind: load.kind,
    page: load.page,
    revision: load.revision,
    generation: load.generation,
    ...details,
  })
}

async function prepareFrame(
  canvas: Canvas,
  page: string | null,
  revision: number,
  generation: number,
  current: () => boolean,
  traceId: string,
): Promise<boolean> {
  const stage = (event: string, result: string, details: Record<string, unknown> = {}) =>
    traceCanvas(event, {
      operation: 'useCanvas.prepareFrame',
      page,
      traceId: traceId || null,
      revision,
      generation,
      result,
      ...details,
    })
  if (!current()) {
    stage('frame-preparation-cancelled', 'cancelled', {
      reason: 'request superseded before manifest',
    })
    return false
  }
  stage('frame-manifest-requested', 'started')
  const manifest = await fetchCanvasManifest(generation)
  stage('frame-manifest-received', 'received', {
    manifestBytes: manifest.byteLength,
    manifestGeneration: generation,
  })
  if (!current()) {
    stage('frame-preparation-cancelled', 'cancelled', {
      reason: 'request superseded after manifest',
    })
    return false
  }
  if (canvas.hasActiveManifest(manifest)) {
    stage('frame-manifest-already-active', 'reused')
    return true
  }
  const staged = canvas.stageManifest(manifest)
  stage('frame-composition-staged', 'staged', {
    stageToken: staged.token,
    missingResources: staged.missing.length,
  })
  for (let offset = 0; offset < staged.missing.length; offset += 4) {
    const resources = staged.missing.slice(offset, offset + 4)
    const packets = await Promise.all(
      resources.map(
        async (resource) => [resource, await fetchCanvasResource(generation, resource)] as const,
      ),
    )
    if (!current()) {
      stage('frame-preparation-cancelled', 'cancelled', {
        reason: 'request superseded after resource fetch',
      })
      return false
    }
    await Promise.all(packets.map(([resource, packet]) => canvas.installResource(resource, packet)))
    stage('frame-resources-installed', 'installed', {
      resources: resources.length,
      result: 'installed',
    })
  }
  if (!current()) {
    stage('frame-preparation-cancelled', 'cancelled', {
      reason: 'request superseded before activation',
    })
    return false
  }
  const activated = canvas.activateFrame(staged.token)
  stage(
    activated ? 'frame-activation-result' : 'frame-activation-rejected',
    activated ? 'activated' : 'rejected',
    {
      stageToken: staged.token,
      manifestGeneration: generation,
      reason: activated ? null : 'WASM rejected activation token',
    },
  )
  if (!current()) {
    stage('frame-preparation-cancelled', 'cancelled', {
      reason: 'request superseded after activation',
    })
    return false
  }
  if (!activated) {
    stage('frame-activation-rejected', 'failed', { stageToken: staged.token })
    throw new Error('The prepared canvas frame was superseded before activation.')
  }
  stage('frame-activated', 'presented', { stageToken: staged.token })
  return true
}

function toError(value: unknown): Error {
  if (value instanceof Error) return value
  return new Error(typeof value === 'string' ? value : 'Could not initialize the WebGPU canvas.')
}
