'use client'

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useColorSampling } from '@/components/controls/ColorSampling'
import { CanvasCommandBar } from '@/components/editor/CanvasCommandBar'
import { CanvasOverlay, type TextEditSession } from '@/components/editor/CanvasOverlay'
import { StatusBar } from '@/components/editor/StatusBar'
import { ToolBar } from '@/components/editor/ToolBar'
import { useCanvas } from '@/components/editor/useCanvas'
import { call } from '@/lib/backend'
import { effectiveLayerVisibility, expandLayerSelection, isTextLayer } from '@/lib/document'
import {
  controlFrame,
  draftFrame,
  hitTestLayers,
  layerFrame,
  pagePoint,
  physicalPoint,
  selectableLayer,
  translateFrames,
} from '@/lib/geometry'
import {
  adjacentPagesFor,
  canvasPageLayouts,
  ghostPageOffsets,
  infiniteScrollTranslation,
  pageIndexAtScroll,
  pageSequenceHeight,
  residentPageIds,
} from '@/lib/page-layout'
import {
  pageKey,
  pagesKey,
  preparedPageKey,
  projectKey,
  queryClient,
  refresh,
  usePreparedPages,
  usePage,
  usePages,
  useProject,
  useWorkspacePage,
} from '@/lib/queries'
import {
  isBrushTool,
  MAX_BRUSH_DIAMETER,
  MIN_BRUSH_DIAMETER,
  receiveError,
  useNorrixStore,
  type CanvasTool,
} from '@/lib/store'
import {
  prefetchCanvasPages,
  previewCanvasOpacity,
  workspaceColor,
  type Canvas,
  type CanvasColor,
} from '@norrix/bridge/canvas'
import {
  commands,
  type CanvasPagePreparation,
  type EntityId,
  type Frame,
  type Page,
  type Point,
  type TextLayoutPreview,
  type TransformFrame,
} from '@norrix/bridge/protocol'
import { Button } from '@norrix/ui/components/button'

const BRUSH_DIAMETER_STEP = 4

const canvasCursors = {
  select: undefined,
  text: 'text',
  draw: 'none',
  eraser: 'none',
  color_picker: 'crosshair',
  remove: 'none',
  pan: 'grab',
} as const satisfies Record<CanvasTool, string | undefined>

type Gesture =
  | {
      kind: 'pan'
      pointer: number
      start: Point
      translation: [number, number]
      scrollTop: number
    }
  | {
      kind: 'move'
      pointer: number
      page: string
      revision: number
      offset: Point
      start: Point
      originals: TransformFrame[]
    }
  | {
      kind: 'text'
      pointer: number
      page: string
      revision: number
      offset: Point
      start: Point
      frame: Frame
    }
  | StrokeGesture

interface StrokeGesture {
  kind: 'paint' | 'erase' | 'inpaint'
  pointer: number
  page: string
  revision: number
  offset: Point
  layer: string | null
  points: Point[]
  diameter: number
  color?: CanvasColor
}

interface CanvasView {
  zoom: number
  translation: [number, number]
}

interface StrokeUpdate {
  kind: 'paint' | 'erase' | 'inpaint'
  points: Point[]
}

type ActiveTextEdit = TextEditSession

interface DirectTextEditTarget {
  page: Page
  layerId: EntityId
  frames: Readonly<Record<EntityId, Frame>>
}

interface DirectTextClickCandidate {
  pointer: number
  start: Point
  target: DirectTextEditTarget
  secondClick: boolean
}

interface PendingTextEdit {
  target: DirectTextEditTarget
  projectName: string
  canvas: Canvas
  workspaceMode: 'single' | 'infinite'
}

interface LastTextClick {
  at: number
  point: Point
  page: EntityId
  layerId: EntityId
}

export function CanvasWorkspace() {
  const { t } = useTranslation()
  const surface = useRef<HTMLDivElement>(null)
  const [canvasElement, setCanvasElement] = useState<HTMLCanvasElement | null>(null)
  const gesture = useRef<Gesture | null>(null)
  const spaceHeld = useRef(false)
  const transformActive = useRef(false)
  const transformRevision = useRef<number | null>(null)
  const transformPage = useRef<string | null>(null)
  const transformFinal = useRef<TransformFrame[]>([])
  const commitPending = useRef(false)
  const pendingTextEdit = useRef<PendingTextEdit | null>(null)
  const attemptPendingTextEditRef = useRef<() => void>(() => {})
  const directTextClick = useRef<DirectTextClickCandidate | null>(null)
  const lastTextClick = useRef<LastTextClick | null>(null)
  const beginTextEditingRef = useRef<(target?: DirectTextEditTarget) => boolean>(() => false)
  const commandQueue = useRef<Promise<void>>(Promise.resolve())
  const [previews, setPreviews] = useState<Record<string, Frame>>({})
  const [textEdit, setTextEdit] = useState<ActiveTextEdit | null>(null)
  const textEditDirty = useRef(false)
  const textEditWriteFailed = useRef(false)
  const textEditFinishPending = useRef(false)
  const [draft, setDraft] = useState<Frame | null>(null)
  const [draftPage, setDraftPage] = useState<string | null>(null)
  const [hovered, setHovered] = useState<string | null>(null)
  const [cursor, setCursor] = useState<Point | null>(null)
  const [scrollTop, setScrollTop] = useState(0)
  const [gesturePage, setGesturePage] = useState<string | null>(null)
  const [viewportSize, setViewportSize] = useState<[number, number]>([0, 0])
  const colorSampling = useColorSampling()

  const activePage = usePage().data
  const workspacePage = useWorkspacePage().data
  const pages = usePages().data
  const project = useProject().data
  const camera = useNorrixStore((state) => state.camera)
  const workspaceMode = useNorrixStore((state) => state.workspaceMode)
  const page = workspaceMode === 'infinite' ? workspacePage : activePage
  const previousWorkspaceMode = useRef(workspaceMode)
  const residentIdsRef = useRef<string[]>([])
  const preparedPagesRef = useRef<Record<string, CanvasPagePreparation>>({})
  const pageNavigation = useNorrixStore((state) => state.pageNavigation)
  const handledPageNavigation = useRef(0)
  const selectedPage = useNorrixStore((state) => state.selectedPage)
  const selectedLayersByPage = useNorrixStore((state) => state.selectedLayersByPage)
  const selectPage = useNorrixStore((state) => state.selectPage)
  const canvasPage = useNorrixStore((state) => state.canvasPage)
  const canvasRevision = useNorrixStore((state) => state.canvasRevision)
  const canvasGeneration = useNorrixStore((state) => state.canvasGeneration)
  const canvasSize = useNorrixStore((state) => state.canvasSize)
  const fitCanvasRequest = useNorrixStore((state) => state.fitCanvasRequest)
  const appliedFitRequest = useRef(fitCanvasRequest)
  const layerFrames = useNorrixStore((state) => state.layerFrames)
  const layerFontSizes = useNorrixStore((state) => state.layerFontSizes)
  const tool = useNorrixStore((state) => state.tool)
  const brush = useNorrixStore((state) => state.brush)
  const selected = useNorrixStore((state) => state.selectedLayers)
  const selectLayers = useNorrixStore((state) => state.selectLayers)
  const setTool = useNorrixStore((state) => state.setTool)
  const setBrush = useNorrixStore((state) => state.setBrush)
  const requestCanvasFit = useNorrixStore((state) => state.requestCanvasFit)
  const canvasState = useCanvas(canvasElement, canvasRevision, canvasGeneration)
  const canvas = canvasState.canvas
  const summaries = useMemo(() => pages ?? [], [pages])
  const residentIds = useMemo(() => {
    if (workspaceMode !== 'infinite') return []
    const resident = new Set(
      residentPageIds(summaries, scrollTop, viewportSize[1], camera.zoom, window.devicePixelRatio),
    )
    if (textEdit && summaries.some((summary) => summary.id === textEdit.page)) {
      resident.add(textEdit.page)
    }
    if (gesturePage && summaries.some((summary) => summary.id === gesturePage)) {
      resident.add(gesturePage)
    }
    const next = summaries
      .filter((summary) => resident.has(summary.id))
      .map((summary) => summary.id)
    const previous = residentIdsRef.current
    if (previous.length === next.length && previous.every((id, index) => id === next[index])) {
      return previous
    }
    residentIdsRef.current = next
    return next
  }, [camera.zoom, gesturePage, scrollTop, summaries, textEdit, viewportSize, workspaceMode])
  const pageId = page?.id
  const ghostTargets = useMemo(
    () => (workspaceMode === 'infinite' ? residentIds : pageId ? [pageId] : []),
    [pageId, residentIds, workspaceMode],
  )
  const ghostPages = useMemo(
    () => adjacentPagesFor(summaries, ghostTargets),
    [ghostTargets, summaries],
  )
  const ghostSourceIds = useMemo(() => [...new Set(Object.values(ghostPages).flat())], [ghostPages])
  const preparedPageIds = useMemo(
    () => [...new Set([...residentIds, ...ghostSourceIds])],
    [ghostSourceIds, residentIds],
  )
  const preparedResults = usePreparedPages(preparedPageIds)
  const preparedPages = useMemo(() => {
    const next = Object.fromEntries(
      preparedResults.flatMap((result) =>
        result.data ? [[result.data.page.id, result.data] as const] : [],
      ),
    )
    const previous = preparedPagesRef.current
    const keys = Object.keys(next)
    if (
      keys.length === Object.keys(previous).length &&
      keys.every((key) => previous[key] === next[key])
    ) {
      return previous
    }
    preparedPagesRef.current = next
    return next
  }, [preparedResults])
  const availableGhostPages = useMemo(
    () =>
      Object.fromEntries(
        Object.entries(ghostPages).map(([target, sources]) => [
          target,
          sources.filter((source) => preparedPages[source]),
        ]),
      ),
    [ghostPages, preparedPages],
  )
  const compositionGhostPages = useMemo(
    () =>
      workspaceMode === 'infinite'
        ? Object.fromEntries(
            residentIds.map((target) => [
              target,
              Object.keys(preparedPages).filter((source) => source !== target),
            ]),
          )
        : availableGhostPages,
    [availableGhostPages, preparedPages, residentIds, workspaceMode],
  )
  const pageLayouts = useMemo(
    () =>
      workspaceMode === 'infinite'
        ? canvasPageLayouts(
            summaries,
            preparedPages,
            viewportSize[0],
            camera.zoom,
            window.devicePixelRatio,
            compositionGhostPages,
            new Set(residentIds),
          )
        : pageId && canvasRevision !== null
          ? [
              {
                page: pageId,
                revision: canvasRevision,
                x: 0,
                y: 0,
                ghostPages: ghostPageOffsets(
                  summaries,
                  pageId,
                  compositionGhostPages[pageId] ?? [],
                ),
              },
            ]
          : [],
    [
      camera.zoom,
      canvasRevision,
      compositionGhostPages,
      pageId,
      preparedPages,
      residentIds,
      summaries,
      viewportSize,
      workspaceMode,
    ],
  )
  const pageSequenceSize =
    workspaceMode === 'infinite'
      ? pageSequenceHeight(summaries, camera.zoom, window.devicePixelRatio)
      : 0
  const pageWidth =
    workspaceMode === 'infinite' ? page?.size.width : canvasSize[0] || page?.size.width
  const pageHeight =
    workspaceMode === 'infinite' ? page?.size.height : canvasSize[1] || page?.size.height
  const enqueue = useCallback(<Result,>(operation: () => Promise<Result>): Promise<Result> => {
    const pending = commandQueue.current.then(operation)
    commandQueue.current = pending.then(
      () => undefined,
      () => undefined,
    )
    return pending
  }, [])
  const cancelCanvasTransform = useCallback(() => {
    try {
      canvas?.cancelTransform()
    } catch (error) {
      receiveError(errorMessage(error))
    }
  }, [canvas])
  const cancelCanvasStroke = useCallback(() => {
    try {
      canvas?.cancelStroke()
    } catch (error) {
      receiveError(errorMessage(error))
    }
  }, [canvas])

  const transformUpdates = useFrameCommand((elements: TransformFrame[]) =>
    canvas?.updateTransform(elements),
  )
  const strokeUpdates = useFrameCommand(
    ({ points }: StrokeUpdate) => canvas?.extendStroke(points),
    mergeStrokeUpdates,
  )

  const beginTransform = useCallback(
    (targetPage: string, revision: number, elements: TransformFrame[]) => {
      if (!canvas || !elements.length || transformActive.current || commitPending.current) return
      transformUpdates.clear()
      transformActive.current = true
      transformRevision.current = revision
      transformPage.current = targetPage
      transformFinal.current = elements
      setPreviews(Object.fromEntries(elements.map(({ element, frame }) => [element, frame])))
      try {
        canvas.beginTransform(targetPage, elements)
      } catch (error) {
        cancelCanvasTransform()
        transformActive.current = false
        transformRevision.current = null
        transformPage.current = null
        setPreviews({})
        receiveError(errorMessage(error))
      }
    },
    [cancelCanvasTransform, canvas, transformUpdates],
  )

  const updateTransform = useCallback(
    (elements: TransformFrame[]) => {
      if (!transformActive.current) return
      transformFinal.current = elements
      setPreviews(Object.fromEntries(elements.map(({ element, frame }) => [element, frame])))
      transformUpdates.schedule(elements)
    },
    [transformUpdates],
  )

  const finishTransform = useCallback(() => {
    if (!transformActive.current) {
      return
    }
    transformUpdates.commit()
    transformActive.current = false
    const revision = transformRevision.current
    const targetPage = transformPage.current
    const elements = transformFinal.current
    transformRevision.current = null
    transformPage.current = null
    try {
      canvas?.finishTransform()
    } catch (error) {
      cancelCanvasTransform()
      receiveError(errorMessage(error))
      setPreviews({})
      setGesturePage(null)
      return
    }
    if (revision === null || targetPage === null) {
      cancelCanvasTransform()
      setPreviews({})
      setGesturePage(null)
      return
    }
    commitPending.current = true
    void enqueue(() => call(commands.commitTransform, targetPage, revision, elements))
      .then((nextRevision) => {
        return nextRevision === null ? undefined : refresh(projectKey, pagesKey, pageKey)
      })
      .catch((error: unknown) => {
        cancelCanvasTransform()
        receiveError(errorMessage(error))
      })
      .finally(() => {
        commitPending.current = false
        setPreviews({})
        setGesturePage(null)
        attemptPendingTextEditRef.current()
      })
  }, [cancelCanvasTransform, canvas, enqueue, transformUpdates])

  const cancelGesture = useCallback(
    (preserveRecognizedTextClick = false) => {
      const current = gesture.current
      gesture.current = null
      if (!preserveRecognizedTextClick) pendingTextEdit.current = null
      if (!preserveRecognizedTextClick || !directTextClick.current?.secondClick) {
        directTextClick.current = null
      }
      if (current?.kind === 'paint' || current?.kind === 'erase' || current?.kind === 'inpaint') {
        strokeUpdates.clear()
        cancelCanvasStroke()
      }
      if (transformActive.current) {
        transformUpdates.clear()
        transformActive.current = false
        transformRevision.current = null
        transformPage.current = null
        cancelCanvasTransform()
      }
      setDraft(null)
      setDraftPage(null)
      setPreviews({})
      setGesturePage(null)
    },
    [cancelCanvasStroke, cancelCanvasTransform, strokeUpdates, transformUpdates],
  )

  useEffect(
    () => () => {
      lastTextClick.current = null
      pendingTextEdit.current = null
    },
    [canvasPage, tool, workspaceMode],
  )

  const beginTextEditing = useCallback(
    (target?: DirectTextEditTarget) => {
      const targetPage = target?.page ?? page
      if (!targetPage) {
        return false
      }
      if (!project) {
        return false
      }
      if (textEdit) {
        return false
      }
      if (!target && selected.length !== 1) {
        return false
      }
      if (commitPending.current) {
        return false
      }
      if (transformActive.current) {
        return false
      }
      const expandedSelection = expandLayerSelection(
        targetPage.layers,
        target ? [target.layerId] : selected,
      )
      if (expandedSelection.length !== 1) {
        return false
      }
      const [selectedId] = expandedSelection
      const layer = targetPage.layers.find((candidate) => candidate.id === selectedId)
      if (!layer) {
        return false
      }
      if (!isTextLayer(layer)) {
        return false
      }
      const visibility = effectiveLayerVisibility(targetPage.layers, layer)
      if (!visibility.visible || visibility.opacity <= 0) {
        return false
      }

      const prepared = workspaceMode === 'infinite' ? preparedPages[targetPage.id] : undefined
      const frames = prepared
        ? Object.fromEntries(prepared.element_frames.map(({ element, frame }) => [element, frame]))
        : (target?.frames ?? layerFrames)
      const frame = layerFrame(layer) ?? controlFrame(layer, frames)
      if (!frame) {
        return false
      }

      const field = layer.content.translation ? 'translation' : 'source'
      const value =
        field === 'translation'
          ? (layer.content.translation?.text ?? '')
          : (layer.content.source?.text ?? '')
      const renderedFontSize = prepared
        ? (prepared.text_font_sizes ?? []).find(({ element }) => element === layer.id)?.font_size
        : layerFontSizes[layer.id]
      try {
        previewCanvasOpacity(layer.id, 0)
        textEditDirty.current = false
        textEditWriteFailed.current = false
        textEditFinishPending.current = false
        setTextEdit({
          page: targetPage.id,
          projectName: project.name,
          layer,
          frame,
          field,
          value,
          renderedFontSize: renderedFontSize ?? null,
        })
        return true
      } catch (error) {
        receiveError(errorMessage(error))
        return false
      }
    },
    [layerFontSizes, layerFrames, page, preparedPages, project, selected, textEdit, workspaceMode],
  )
  useEffect(() => {
    beginTextEditingRef.current = beginTextEditing
  }, [beginTextEditing])

  const attemptPendingTextEdit = useCallback(() => {
    const pending = pendingTextEdit.current
    if (!pending) return
    const targetPageId = pending.target.page.id
    if (
      canvas !== pending.canvas ||
      workspaceMode !== pending.workspaceMode ||
      tool !== 'select' ||
      selectedPage !== targetPageId ||
      (workspaceMode === 'single' && canvasPage !== targetPageId)
    ) {
      pendingTextEdit.current = null
      return
    }
    if (project === undefined) {
      return
    }
    if (!project || project.name !== pending.projectName) {
      pendingTextEdit.current = null
      return
    }
    const prepared = workspaceMode === 'infinite' ? preparedPages[targetPageId] : undefined
    const targetPage = prepared?.page ?? (page?.id === targetPageId ? page : undefined)
    if (!targetPage) {
      return
    }
    const layer = targetPage.layers.find((candidate) => candidate.id === pending.target.layerId)
    if (!layer) {
      pendingTextEdit.current = null
      return
    }
    if (!isTextLayer(layer)) {
      pendingTextEdit.current = null
      return
    }
    if (canvasState.status === 'error') {
      pendingTextEdit.current = null
      return
    }
    if (canvasState.status !== 'ready' || commitPending.current || transformActive.current) {
      return
    }
    const frames = prepared
      ? Object.fromEntries(prepared.element_frames.map(({ element, frame }) => [element, frame]))
      : page?.id === targetPageId
        ? layerFrames
        : pending.target.frames
    pendingTextEdit.current = null
    beginTextEditingRef.current({
      page: targetPage,
      layerId: pending.target.layerId,
      frames,
    })
  }, [
    canvas,
    canvasPage,
    canvasState.status,
    layerFrames,
    page,
    preparedPages,
    project,
    selectedPage,
    tool,
    workspaceMode,
  ])
  useEffect(() => {
    attemptPendingTextEditRef.current = attemptPendingTextEdit
  }, [attemptPendingTextEdit])
  useEffect(() => {
    attemptPendingTextEdit()
  }, [attemptPendingTextEdit])

  const updateTextEditing = useCallback(
    (session: TextEditSession, text: string) => {
      if (session.value === text) return
      setTextEdit((current) =>
        current && current.layer.id === session.layer.id && current.page === session.page
          ? { ...current, value: text }
          : current,
      )
      textEditDirty.current = true
      void enqueue(() =>
        session.field === 'translation'
          ? call(commands.setTranslation, session.layer.id, text, null)
          : call(commands.setSourceText, session.layer.id, text, null),
      )
        .then(() => {
          textEditWriteFailed.current = false
        })
        .catch((error: unknown) => {
          textEditWriteFailed.current = true
          receiveError(errorMessage(error))
        })
    },
    [enqueue],
  )

  const finishTextEditing = useCallback(() => {
    const current = textEdit
    if (!current || textEditFinishPending.current) return
    if (!textEditDirty.current) {
      setTextEdit(null)
      return
    }

    textEditFinishPending.current = true
    void commandQueue.current
      .then(async () => {
        if (textEditWriteFailed.current) return
        try {
          await refresh(projectKey, pagesKey, pageKey)
        } catch (error) {
          receiveError(errorMessage(error))
        }
        if (textEditWriteFailed.current) return
        textEditDirty.current = false
        setTextEdit((active) =>
          active?.layer.id === current.layer.id && active.page === current.page ? null : active,
        )
      })
      .finally(() => {
        textEditFinishPending.current = false
      })
  }, [textEdit])

  useEffect(() => {
    if (textEdit && selectedPage && selectedPage !== textEdit.page) finishTextEditing()
  }, [finishTextEditing, selectedPage, textEdit])
  useEffect(() => {
    if (textEdit && project !== undefined && project?.name !== textEdit.projectName) {
      finishTextEditing()
    }
  }, [finishTextEditing, project, textEdit])

  const previewTextLayout = useCallback(
    (
      targetPage: string,
      element: string,
      text: string,
      requestRevision: number,
    ): Promise<TextLayoutPreview> =>
      call(commands.previewTextLayout, targetPage, element, text, requestRevision),
    [],
  )

  const reportTextPreviewError = useCallback((error: unknown) => {
    receiveError(errorMessage(error))
  }, [])

  const textEditLayer = textEdit?.layer.id
  useEffect(() => {
    if (!textEditLayer) return
    return () => {
      try {
        previewCanvasOpacity(textEditLayer, null)
      } catch (error) {
        receiveError(errorMessage(error))
      }
    }
  }, [textEditLayer])

  const fitCanvas = useCallback(() => {
    const element = surface.current
    if (
      !element ||
      !pageId ||
      pageWidth === undefined ||
      (workspaceMode === 'single' && pageHeight === undefined)
    )
      return
    const bounds = element.getBoundingClientRect()
    const dpr = window.devicePixelRatio
    const viewportWidth = element.clientWidth || bounds.width
    const viewportHeight = element.clientHeight || bounds.height
    const fitWidth =
      workspaceMode === 'infinite'
        ? Math.max(0, ...summaries.map((summary) => summary.size.width))
        : pageWidth
    const next =
      workspaceMode === 'infinite'
        ? {
            ...fitWidthCamera(viewportWidth * dpr, fitWidth),
            translation: infiniteScrollTranslation(element.scrollTop, dpr),
          }
        : containCamera(viewportWidth * dpr, viewportHeight * dpr, pageWidth, pageHeight!)
    useNorrixStore.setState({ camera: { ...next, fitted: true } })
  }, [pageHeight, pageId, pageWidth, summaries, workspaceMode])

  const report = useCallback(() => {
    const element = surface.current
    if (!element || !canvas) return
    const bounds = element.getBoundingClientRect()
    const width = element.clientWidth || bounds.width
    const height = element.clientHeight || bounds.height
    canvas.resize(width, height, window.devicePixelRatio, workspaceColor())
    setViewportSize((current) =>
      current[0] === width && current[1] === height ? current : [width, height],
    )
    if (useNorrixStore.getState().camera.fitted) fitCanvas()
  }, [canvas, fitCanvas])

  const setZoom = useCallback((zoom: number) => {
    const element = surface.current
    if (!element) return
    const bounds = element.getBoundingClientRect()
    const dpr = window.devicePixelRatio
    const current = useNorrixStore.getState().camera
    if (useNorrixStore.getState().workspaceMode === 'infinite') {
      const scroll = element.scrollTop
      useNorrixStore.setState({
        camera: {
          zoom,
          translation: infiniteScrollTranslation(scroll, dpr),
          fitted: false,
        },
      })
      return
    }
    const center = { x: bounds.width * dpr * 0.5, y: bounds.height * dpr * 0.5 }
    const pageX = (center.x - current.translation[0]) / current.zoom
    const pageY = (center.y - current.translation[1]) / current.zoom
    useNorrixStore.setState({
      camera: {
        zoom,
        translation: [center.x - pageX * zoom, center.y - pageY * zoom],
        fitted: false,
      },
    })
  }, [])

  useEffect(() => {
    const element = surface.current
    if (!element) return
    report()
    const resize = new ResizeObserver(report)
    const theme = new MutationObserver(report)
    resize.observe(element)
    theme.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] })
    window.addEventListener('resize', report)
    window.visualViewport?.addEventListener('resize', report)
    return () => {
      resize.disconnect()
      theme.disconnect()
      window.removeEventListener('resize', report)
      window.visualViewport?.removeEventListener('resize', report)
    }
  }, [report])

  useEffect(() => {
    const previous = previousWorkspaceMode.current
    previousWorkspaceMode.current = workspaceMode
    if (previous === workspaceMode || !surface.current) return
    if (workspaceMode === 'infinite') {
      const currentCamera = useNorrixStore.getState().camera
      const target = selectedPage ?? pageId
      const index = summaries.findIndex((summary) => summary.id === target)
      const scale = camera.zoom / window.devicePixelRatio
      const top =
        index < 0
          ? surface.current.scrollTop
          : summaries.slice(0, index).reduce((height, summary) => height + summary.size.height, 0) *
            scale
      surface.current.scrollTop = top
      setScrollTop(top)
      useNorrixStore.setState({
        camera: {
          ...currentCamera,
          translation: infiniteScrollTranslation(
            surface.current.scrollTop,
            window.devicePixelRatio,
          ),
        },
      })
      return
    }
    const targetPage = selectedPage ? preparedPages[selectedPage]?.page : page
    if (!targetPage) return
    const bounds = surface.current.getBoundingClientRect()
    const dpr = window.devicePixelRatio
    const currentCamera = useNorrixStore.getState().camera
    if (!currentCamera.fitted) {
      useNorrixStore.setState({
        camera: {
          ...currentCamera,
          translation: [
            (bounds.width * dpr - targetPage.size.width * camera.zoom) * 0.5,
            (bounds.height * dpr - targetPage.size.height * camera.zoom) * 0.5,
          ],
        },
      })
    }
  }, [camera.zoom, page, pageId, preparedPages, selectedPage, summaries, workspaceMode])

  useEffect(() => {
    if (
      workspaceMode !== 'infinite' ||
      !pageNavigation ||
      handledPageNavigation.current === pageNavigation.request ||
      !surface.current
    )
      return
    const index = summaries.findIndex((summary) => summary.id === pageNavigation.page)
    if (index < 0) return
    handledPageNavigation.current = pageNavigation.request
    const top =
      summaries.slice(0, index).reduce((height, summary) => height + summary.size.height, 0) *
      (camera.zoom / window.devicePixelRatio)
    surface.current.scrollTop = top
    setScrollTop(top)
    const currentCamera = useNorrixStore.getState().camera
    useNorrixStore.setState({
      camera: {
        ...currentCamera,
        translation: infiniteScrollTranslation(surface.current.scrollTop, window.devicePixelRatio),
      },
    })
  }, [camera.zoom, pageNavigation, summaries, workspaceMode])

  useEffect(() => {
    canvas?.setView(camera.zoom, camera.translation)
  }, [canvas, camera])

  useEffect(() => {
    if (
      (workspaceMode === 'single' && (!pageId || canvasPage !== pageId)) ||
      canvasState.generation !== canvasGeneration ||
      canvasState.status !== 'ready' ||
      !pages?.length
    )
      return
    if (workspaceMode === 'infinite') {
      if (!residentIds.length) return
      const pagesToPrepare = [
        ...new Set([
          ...residentIds,
          ...Object.values(adjacentPagesFor(summaries, residentIds)).flat(),
        ]),
      ]
      void prefetchCanvasPages(pagesToPrepare)
        .then((prepared) => {
          for (const item of prepared) {
            queryClient.setQueryData(preparedPageKey(item.page.id), item)
          }
        })
        .catch((error: unknown) => receiveError(errorMessage(error)))
      return
    }
    const index = pages.findIndex((candidate) => candidate.id === pageId)
    if (index < 0) return
    const adjacent = [pages[index - 1]?.id, pages[index + 1]?.id].filter(
      (id): id is string => id !== undefined,
    )
    if (adjacent.length) {
      void prefetchCanvasPages(adjacent)
        .then((prepared) => {
          for (const page of prepared) {
            queryClient.setQueryData(preparedPageKey(page.page.id), page)
          }
        })
        .catch((error: unknown) => receiveError(errorMessage(error)))
    }
  }, [
    canvasGeneration,
    canvasPage,
    canvasState.generation,
    canvasState.status,
    pageId,
    pages,
    residentIds,
    summaries,
    workspaceMode,
  ])

  useEffect(() => {
    if (!canvas || canvasState.status !== 'ready') return
    try {
      canvas.setPageLayout(pageLayouts)
    } catch (error) {
      receiveError(errorMessage(error))
    }
  }, [canvas, canvasState.status, pageLayouts])

  useEffect(() => {
    const requested = fitCanvasRequest !== appliedFitRequest.current
    if (camera.fitted || requested) {
      fitCanvas()
      appliedFitRequest.current = fitCanvasRequest
    }
  }, [camera.fitted, fitCanvas, fitCanvasRequest])

  useEffect(
    () => () => {
      cancelGesture()
    },
    [cancelGesture, canvasPage, tool, workspaceMode],
  )

  useEffect(
    () => () => {
      cancelGesture(true)
    },
    [cancelGesture, canvasGeneration, canvasRevision],
  )

  useEffect(() => {
    const editable = (target: EventTarget | null) =>
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement ||
      (target instanceof HTMLElement && target.isContentEditable)

    const down = (event: KeyboardEvent) => {
      if (event.isComposing) return
      if (event.key === 'Alt') {
        event.preventDefault()
        return
      }
      if (editable(event.target)) return
      const state = useNorrixStore.getState()
      if (event.code === 'Space') {
        spaceHeld.current = true
        event.preventDefault()
        return
      }
      const command = event.ctrlKey || event.metaKey
      if (command && event.key.toLowerCase() === 'z') {
        event.preventDefault()
        void call(event.shiftKey ? commands.redo : commands.undo)
          .then(() => refresh(projectKey, pagesKey, pageKey))
          .catch(() => undefined)
        return
      }
      if (command && event.key.toLowerCase() === 'a' && page) {
        event.preventDefault()
        selectLayers(page.layers.filter(selectableLayer).map((layer) => layer.id))
        return
      }
      if (event.key === 'Enter' && !command && !event.altKey) {
        if (beginTextEditing()) event.preventDefault()
        return
      }
      if (
        (event.key === 'Delete' || event.key === 'Backspace') &&
        state.selectedLayers.length > 0
      ) {
        event.preventDefault()
        void call(commands.deleteLayers, state.selectedLayers)
          .then(() => refresh(projectKey, pagesKey, pageKey))
          .catch(() => undefined)
        return
      }
      if (event.key.toLowerCase() === state.shortcuts.fit) {
        requestCanvasFit()
        return
      }
      if (event.key === 'Escape') {
        lastTextClick.current = null
        cancelGesture()
        colorSampling?.cancel()
        selectLayers([])
        return
      }
      const next = (
        ['select', 'text', 'draw', 'eraser', 'color_picker', 'remove', 'pan'] as const
      ).find((action) => state.shortcuts[action] === event.key.toLowerCase())
      if (next) setTool(next)
    }

    const up = (event: KeyboardEvent) => {
      if (event.key === 'Alt') event.preventDefault()
      if (event.code === 'Space') spaceHeld.current = false
    }
    const blur = () => {
      spaceHeld.current = false
      lastTextClick.current = null
      cancelGesture()
    }

    window.addEventListener('keydown', down)
    window.addEventListener('keyup', up)
    window.addEventListener('blur', blur)
    return () => {
      window.removeEventListener('keydown', down)
      window.removeEventListener('keyup', up)
      window.removeEventListener('blur', blur)
    }
  }, [
    beginTextEditing,
    cancelGesture,
    colorSampling,
    page,
    requestCanvasFit,
    selectLayers,
    setTool,
  ])

  const clientPagePoint = (clientX: number, clientY: number, offset: Point = { x: 0, y: 0 }) => {
    const point = pagePoint(
      clientX,
      clientY,
      surface.current!.getBoundingClientRect(),
      useNorrixStore.getState().camera,
    )
    return { x: point.x - offset.x, y: point.y - offset.y }
  }

  const clientPhysicalPoint = (clientX: number, clientY: number) =>
    physicalPoint(clientX, clientY, surface.current!.getBoundingClientRect())

  const pageContextAt = (clientX: number, clientY: number) => {
    const point = clientPagePoint(clientX, clientY)
    if (workspaceMode === 'single') {
      return page && canvasRevision !== null
        ? { page, revision: canvasRevision, offset: { x: 0, y: 0 }, frames: layerFrames }
        : null
    }
    const layout = pageLayouts.find((candidate) => {
      const summary = summaries.find((item) => item.id === candidate.page)
      return (
        summary &&
        point.x >= candidate.x &&
        point.x <= candidate.x + summary.size.width &&
        point.y >= candidate.y &&
        point.y <= candidate.y + summary.size.height
      )
    })
    const preparation = layout ? preparedPages[layout.page] : undefined
    if (!layout || !preparation) return null
    return {
      page: preparation.page,
      revision: preparation.revision,
      offset: { x: layout.x, y: layout.y },
      frames: Object.fromEntries(
        preparation.element_frames.map(({ element, frame }) => [element, frame]),
      ),
    }
  }

  const framesFor = (
    targetPage: NonNullable<typeof page>,
    layers: string[],
    frames: Readonly<Record<string, Frame>>,
  ): TransformFrame[] =>
    expandLayerSelection(targetPage.layers, layers).flatMap((id) => {
      const layer = targetPage.layers.find((candidate) => candidate.id === id)
      const frame = layer && selectableLayer(layer) ? controlFrame(layer, frames) : null
      return frame ? [{ element: id, frame }] : []
    })

  const moveGesture = (
    pointer: number,
    samples: ReadonlyArray<{ clientX: number; clientY: number }>,
  ) => {
    const current = gesture.current
    const sample = samples.at(-1)
    if (!page || !sample) return
    const physical = clientPhysicalPoint(sample.clientX, sample.clientY)
    setCursor(physical)
    if (!current || current.pointer !== pointer) {
      if (tool === 'select') {
        const context = pageContextAt(sample.clientX, sample.clientY)
        setHovered(
          context
            ? (hitTestLayers(
                context.page.layers,
                clientPagePoint(sample.clientX, sample.clientY, context.offset),
                context.frames,
              )?.id ?? null)
            : null,
        )
      }
      return
    }

    if (current.kind === 'pan') {
      if (workspaceMode === 'infinite') {
        const element = surface.current
        if (element) {
          element.scrollTop = Math.max(
            0,
            current.scrollTop - (physical.y - current.start.y) / window.devicePixelRatio,
          )
        }
        return
      }
      const bounds = surface.current!.getBoundingClientRect()
      const dpr = window.devicePixelRatio
      let translation: [number, number] = [
        current.translation[0] + physical.x - current.start.x,
        current.translation[1] + physical.y - current.start.y,
      ]
      translation = clampCameraTranslation(
        translation,
        camera.zoom,
        pageWidth ?? page.size.width,
        pageHeight ?? page.size.height,
        bounds.width * dpr,
        bounds.height * dpr,
        dpr,
      )
      useNorrixStore.setState({ camera: { zoom: camera.zoom, translation, fitted: false } })
      return
    }

    const points = samples.map((value) =>
      clientPagePoint(value.clientX, value.clientY, current.offset),
    )
    const point = points.at(-1)!
    if (current.kind === 'move') {
      updateTransform(
        translateFrames(current.originals, {
          x: point.x - current.start.x,
          y: point.y - current.start.y,
        }),
      )
    } else if (current.kind === 'text') {
      current.frame = draftFrame(current.start, point)
      setDraft(current.frame)
    } else if (current.kind === 'paint' || current.kind === 'erase' || current.kind === 'inpaint') {
      current.points.push(...points)
      strokeUpdates.schedule({ kind: current.kind, points })
    }
  }

  const finishGesture = () => {
    const current = gesture.current
    gesture.current = null
    if (!current) return
    if (current.kind === 'move') {
      finishTransform()
    } else if (current.kind === 'text') {
      const pointText =
        current.frame.width < 4 / camera.zoom && current.frame.height < 4 / camera.zoom
      setDraft(null)
      setDraftPage(null)
      setGesturePage(null)
      void enqueue(() =>
        pointText
          ? call(commands.addPointText, current.page, current.start)
          : call(commands.addTextBox, current.page, current.frame),
      )
        .then(async (result) => {
          await refresh(projectKey, pagesKey, pageKey)
          const refreshedPage = queryClient.getQueryData<Page | null>(pageKey)
          if (
            refreshedPage?.id === current.page &&
            refreshedPage.layers.some((layer) => layer.id === result.layer)
          ) {
            selectLayers([result.layer])
          }
        })
        .catch((error: unknown) => receiveError(errorMessage(error)))
    } else if (current.kind === 'paint' || current.kind === 'erase' || current.kind === 'inpaint') {
      strokeUpdates.commit()
      try {
        canvas?.finishStroke()
      } catch (error) {
        cancelCanvasStroke()
        setGesturePage(null)
        receiveError(errorMessage(error))
        return
      }
      const operation =
        current.kind === 'paint'
          ? enqueue(() =>
              call(
                commands.commitPaint,
                current.page,
                current.revision,
                current.layer,
                current.points,
                {
                  diameter: current.diameter,
                  color: current.color!,
                },
              ),
            ).then((result) => {
              selectLayers([result.layer])
              return refresh(projectKey, pagesKey, pageKey)
            })
          : current.kind === 'erase'
            ? enqueue(() =>
                call(
                  commands.commitErase,
                  current.page,
                  current.revision,
                  current.layer!,
                  current.points,
                  current.diameter,
                ),
              ).then((result) => {
                selectLayers([result.layer])
                return refresh(projectKey, pagesKey, pageKey)
              })
            : enqueue(() =>
                call(
                  commands.commitInpaint,
                  current.page,
                  current.revision,
                  current.points,
                  current.diameter,
                ),
              ).then((job) => {
                cancelCanvasStroke()
                return job
              })
      commitPending.current = true
      void operation
        .catch((error: unknown) => {
          cancelCanvasStroke()
          receiveError(errorMessage(error))
        })
        .finally(() => {
          commitPending.current = false
          setGesturePage(null)
        })
    } else {
      setGesturePage(null)
    }
  }

  const overlayPages = useMemo(
    () =>
      workspaceMode === 'infinite'
        ? pageLayouts.flatMap((layout) => {
            const prepared = preparedPages[layout.page]
            if (!prepared) return []
            return [
              {
                page: prepared.page,
                revision: prepared.revision,
                frames: Object.fromEntries(
                  prepared.element_frames.map(({ element, frame }) => [element, frame]),
                ),
                selected: selectedLayersByPage[layout.page] ?? [],
                camera: {
                  ...camera,
                  translation: [
                    camera.translation[0] + layout.x * camera.zoom,
                    camera.translation[1] + layout.y * camera.zoom,
                  ] as [number, number],
                },
              },
            ]
          })
        : page
          ? [
              {
                page,
                revision: canvasRevision ?? 0,
                frames: layerFrames,
                selected,
                camera,
              },
            ]
          : [],
    [
      camera,
      canvasRevision,
      layerFrames,
      page,
      pageLayouts,
      preparedPages,
      selected,
      selectedLayersByPage,
      workspaceMode,
    ],
  )
  const preserveTextEditor =
    canvasState.status === 'switching' &&
    textEdit !== null &&
    project?.name === textEdit.projectName &&
    selectedPage === textEdit.page &&
    pageId === textEdit.page &&
    canvasPage === textEdit.page
  return (
    <main className='relative flex h-full min-h-0 min-w-0 flex-col overflow-hidden rounded-tl-2xl bg-[var(--surface-canvas)]'>
      <CanvasCommandBar />
      <div className='relative flex min-h-0 min-w-0 flex-1'>
        <ToolBar />
        <div
          ref={surface}
          tabIndex={0}
          aria-label={t('canvas.surface')}
          aria-keyshortcuts='Enter'
          aria-busy={page ? canvasState.status !== 'ready' : undefined}
          className='relative min-h-0 min-w-0 flex-1 overflow-hidden bg-[var(--surface-canvas)] outline-none'
          data-workspace-mode={workspaceMode}
          style={{
            cursor: page && canvasState.status === 'ready' ? canvasCursors[tool] : undefined,
            overflowY: workspaceMode === 'infinite' ? 'auto' : 'hidden',
            touchAction: workspaceMode === 'infinite' ? 'auto' : 'none',
          }}
          onScroll={(event) => {
            if (workspaceMode !== 'infinite') return
            const nextScrollTop = event.currentTarget.scrollTop
            setScrollTop(nextScrollTop)
            const currentCamera = useNorrixStore.getState().camera
            const translation = infiniteScrollTranslation(nextScrollTop, window.devicePixelRatio)
            if (
              currentCamera.translation[0] !== translation[0] ||
              currentCamera.translation[1] !== translation[1]
            ) {
              useNorrixStore.setState({
                camera: { ...currentCamera, translation },
              })
            }
            const index = pageIndexAtScroll(
              summaries,
              nextScrollTop + event.currentTarget.clientHeight * 0.5,
              camera.zoom,
              window.devicePixelRatio,
            )
            const nextPage = summaries[index]
            if (nextPage && nextPage.id !== selectedPage) selectPage(nextPage.id)
          }}
          onContextMenu={(event) => event.preventDefault()}
          onPointerDown={(event) => {
            if (textEdit) {
              const target = event.target instanceof Element ? event.target : null
              if (!target?.closest('[data-text-edit-surface]')) finishTextEditing()
              return
            }
            const context = pageContextAt(event.clientX, event.clientY)
            const documentPoint = context
              ? clientPagePoint(event.clientX, event.clientY, context.offset)
              : null
            const hit =
              context && documentPoint
                ? hitTestLayers(context.page.layers, documentPoint, context.frames)
                : null
            if (
              context &&
              canvas &&
              (canvasState.status === 'ready' || canvasState.status === 'switching') &&
              event.button === 0 &&
              tool === 'select' &&
              !textEdit &&
              !(event.target instanceof Element && event.target.closest('[data-canvas-control]'))
            ) {
              const target = hit
              if (target && isTextLayer(target)) {
                const previousClick = lastTextClick.current
                const point = { x: event.clientX, y: event.clientY }
                const secondClick =
                  event.detail >= 2 ||
                  (previousClick !== null &&
                    previousClick.page === context.page.id &&
                    previousClick.layerId === target.id &&
                    Date.now() - previousClick.at <= 500 &&
                    Math.hypot(point.x - previousClick.point.x, point.y - previousClick.point.y) <=
                      4)
                directTextClick.current = {
                  pointer: event.pointerId,
                  start: point,
                  target: {
                    page: context.page,
                    layerId: target.id,
                    frames: context.frames,
                  },
                  secondClick,
                }
                event.currentTarget.setPointerCapture(event.pointerId)
              } else {
                lastTextClick.current = null
              }
            }
            if (
              !context ||
              !canvas ||
              canvasState.status !== 'ready' ||
              commitPending.current ||
              event.button > 1
            )
              return
            if (event.target instanceof Element && event.target.closest('[data-canvas-control]'))
              return
            event.currentTarget.focus()
            event.currentTarget.setPointerCapture(event.pointerId)
            const physical = clientPhysicalPoint(event.clientX, event.clientY)
            const point = clientPagePoint(event.clientX, event.clientY, context.offset)
            setCursor(physical)
            const state = useNorrixStore.getState()
            const pageSelection = state.selectedLayersByPage[context.page.id] ?? []
            const targetRaster =
              pageSelection.length === 1
                ? context.page.layers.find(
                    (layer) => layer.id === pageSelection[0] && layer.type === 'raster',
                  )
                : undefined

            if (event.button === 1 || tool === 'pan' || spaceHeld.current) {
              gesture.current = {
                kind: 'pan',
                pointer: event.pointerId,
                start: physical,
                translation: camera.translation,
                scrollTop: event.currentTarget.scrollTop,
              }
            } else if (tool === 'select') {
              selectPage(context.page.id)
              const target = hitTestLayers(context.page.layers, point, context.frames)
              const additive = event.shiftKey || event.ctrlKey || event.metaKey
              if (!target) {
                if (!additive) selectLayers([])
                return
              }
              const next = additive
                ? pageSelection.includes(target.id)
                  ? pageSelection.filter((id) => id !== target.id)
                  : [...pageSelection, target.id]
                : pageSelection.includes(target.id)
                  ? pageSelection
                  : [target.id]
              selectLayers(next)
              if (!next.includes(target.id)) return
              const originals = framesFor(context.page, next, context.frames)
              if (!originals.length) return
              gesture.current = {
                kind: 'move',
                pointer: event.pointerId,
                page: context.page.id,
                revision: context.revision,
                offset: context.offset,
                start: point,
                originals,
              }
              setGesturePage(context.page.id)
              beginTransform(context.page.id, context.revision, originals)
            } else if (tool === 'text') {
              selectPage(context.page.id)
              const frame = draftFrame(point, point)
              gesture.current = {
                kind: 'text',
                pointer: event.pointerId,
                page: context.page.id,
                revision: context.revision,
                offset: context.offset,
                start: point,
                frame,
              }
              setGesturePage(context.page.id)
              setDraft(frame)
              setDraftPage(context.page.id)
            } else if (tool === 'draw') {
              selectPage(context.page.id)
              strokeUpdates.clear()
              const color = hexToRgba(brush.color)
              gesture.current = {
                kind: 'paint',
                pointer: event.pointerId,
                page: context.page.id,
                revision: context.revision,
                offset: context.offset,
                layer: targetRaster?.id ?? null,
                points: [point],
                diameter: brush.diameter,
                color,
              }
              setGesturePage(context.page.id)
              try {
                canvas.beginStroke({
                  kind: 'paint',
                  page: context.page.id,
                  revision: context.revision,
                  layer: targetRaster?.id ?? null,
                  point,
                  diameter: brush.diameter,
                  color,
                })
              } catch (error) {
                gesture.current = null
                cancelCanvasStroke()
                setGesturePage(null)
                receiveError(errorMessage(error))
              }
            } else if (tool === 'eraser') {
              if (!targetRaster) {
                receiveError('Select a paint or cleanup layer before using the Eraser.')
                return
              }
              selectPage(context.page.id)
              strokeUpdates.clear()
              gesture.current = {
                kind: 'erase',
                pointer: event.pointerId,
                page: context.page.id,
                revision: context.revision,
                offset: context.offset,
                layer: targetRaster.id,
                points: [point],
                diameter: brush.diameter,
              }
              setGesturePage(context.page.id)
              try {
                canvas.beginStroke({
                  kind: 'erase',
                  page: context.page.id,
                  revision: context.revision,
                  layer: targetRaster.id,
                  point,
                  diameter: brush.diameter,
                })
              } catch (error) {
                gesture.current = null
                cancelCanvasStroke()
                setGesturePage(null)
                receiveError(errorMessage(error))
              }
            } else if (tool === 'remove') {
              selectPage(context.page.id)
              strokeUpdates.clear()
              gesture.current = {
                kind: 'inpaint',
                pointer: event.pointerId,
                page: context.page.id,
                revision: context.revision,
                offset: context.offset,
                layer: null,
                points: [point],
                diameter: brush.diameter,
              }
              setGesturePage(context.page.id)
              try {
                canvas.beginStroke({
                  kind: 'inpaint',
                  page: context.page.id,
                  revision: context.revision,
                  layer: null,
                  point,
                  diameter: brush.diameter,
                })
              } catch (error) {
                gesture.current = null
                cancelCanvasStroke()
                setGesturePage(null)
                receiveError(errorMessage(error))
              }
            } else if (tool === 'color_picker') {
              void canvas
                .sampleColor(physical)
                .then((color) => {
                  const hex = rgbaToHex(color)
                  if (!colorSampling?.complete(hex)) setBrush({ ...brush, color: hex })
                })
                .catch((error: unknown) => receiveError(errorMessage(error)))
            }
            event.preventDefault()
          }}
          onPointerMove={(event) => {
            if (!page) return
            const directClick = directTextClick.current
            if (directClick?.pointer === event.pointerId) {
              const distance = Math.hypot(
                event.clientX - directClick.start.x,
                event.clientY - directClick.start.y,
              )
              if (distance > 4) {
                directTextClick.current = null
                lastTextClick.current = null
                pendingTextEdit.current = null
              }
            }
            const coalesced = event.nativeEvent.getCoalescedEvents?.() ?? [event.nativeEvent]
            moveGesture(event.pointerId, coalesced.length ? coalesced : [event.nativeEvent])
          }}
          onPointerUp={(event) => {
            const directClick = directTextClick.current
            const finalDistance =
              directClick?.pointer === event.pointerId
                ? Math.hypot(
                    event.clientX - directClick.start.x,
                    event.clientY - directClick.start.y,
                  )
                : null
            if (
              directClick?.pointer === event.pointerId &&
              finalDistance !== null &&
              finalDistance > 4
            ) {
              directTextClick.current = null
              lastTextClick.current = null
              pendingTextEdit.current = null
            }
            moveGesture(event.pointerId, [event.nativeEvent])
            finishGesture()
            const completedClick = directTextClick.current
            if (completedClick?.pointer === event.pointerId) {
              directTextClick.current = null
              if (!completedClick.secondClick) {
                lastTextClick.current = {
                  at: Date.now(),
                  point: completedClick.start,
                  page: completedClick.target.page.id,
                  layerId: completedClick.target.layerId,
                }
              } else {
                lastTextClick.current = null
                selectPage(completedClick.target.page.id)
                selectLayers([completedClick.target.layerId])
                if (canvas && project) {
                  pendingTextEdit.current = {
                    target: completedClick.target,
                    projectName: project.name,
                    canvas,
                    workspaceMode,
                  }
                  attemptPendingTextEditRef.current()
                } else {
                  beginTextEditing(completedClick.target)
                }
              }
            }
            if (event.currentTarget.hasPointerCapture(event.pointerId)) {
              event.currentTarget.releasePointerCapture(event.pointerId)
            }
          }}
          onPointerCancel={() => {
            directTextClick.current = null
            lastTextClick.current = null
            pendingTextEdit.current = null
            cancelGesture()
          }}
          onLostPointerCapture={() => {}}
          onPointerLeave={(event) => {
            if (!event.currentTarget.hasPointerCapture(event.pointerId)) {
              setHovered(null)
              setCursor(null)
            }
          }}
          onWheel={(event) => {
            if (!page) return
            if (
              workspaceMode === 'infinite' &&
              !event.ctrlKey &&
              !(event.altKey && isBrushTool(tool))
            )
              return
            event.preventDefault()
            if (event.altKey && isBrushTool(tool)) {
              if (event.deltaY !== 0) {
                const currentBrush = useNorrixStore.getState().brush
                const delta = event.deltaY < 0 ? BRUSH_DIAMETER_STEP : -BRUSH_DIAMETER_STEP
                const nextDiameter = clamp(
                  currentBrush.diameter + delta,
                  MIN_BRUSH_DIAMETER,
                  MAX_BRUSH_DIAMETER,
                )
                if (nextDiameter !== currentBrush.diameter) {
                  setBrush({ ...currentBrush, diameter: nextDiameter })
                }
              }
              return
            }
            const point = clientPhysicalPoint(event.clientX, event.clientY)
            const current = useNorrixStore.getState().camera
            const bounds = event.currentTarget.getBoundingClientRect()
            const dpr = window.devicePixelRatio
            let zoom = current.zoom
            let translation = current.translation
            if (event.ctrlKey) {
              zoom = clamp(current.zoom * Math.exp(-event.deltaY * 0.0015), 0.02, 16)
              if (workspaceMode === 'infinite') {
                translation = infiniteScrollTranslation(event.currentTarget.scrollTop, dpr)
                useNorrixStore.setState({ camera: { zoom, translation, fitted: false } })
                return
              }
              const pageX = (point.x - current.translation[0]) / current.zoom
              const pageY = (point.y - current.translation[1]) / current.zoom
              translation = [point.x - pageX * zoom, point.y - pageY * zoom]
            } else if (workspaceMode === 'single') {
              let deltaX = event.deltaX
              let deltaY = event.deltaY
              if (event.shiftKey && deltaX === 0) {
                deltaX = deltaY
                deltaY = 0
              }
              translation = [
                current.translation[0] - deltaX * dpr,
                current.translation[1] - deltaY * dpr,
              ]
            }
            translation = clampCameraTranslation(
              translation,
              zoom,
              pageWidth ?? page.size.width,
              pageHeight ?? page.size.height,
              bounds.width * dpr,
              bounds.height * dpr,
              dpr,
            )
            useNorrixStore.setState({ camera: { zoom, translation, fitted: false } })
          }}
        >
          <div
            className='relative'
            style={{
              height:
                workspaceMode === 'infinite' ? Math.max(viewportSize[1], pageSequenceSize) : '100%',
              minHeight: '100%',
            }}
          >
            <div
              className={workspaceMode === 'infinite' ? 'sticky top-0 z-10' : 'absolute inset-0'}
              style={
                workspaceMode === 'infinite'
                  ? { height: viewportSize[1], marginBottom: -viewportSize[1] }
                  : undefined
              }
            >
              <canvas
                ref={setCanvasElement}
                data-testid='webgpu-canvas'
                aria-hidden
                className='pointer-events-none absolute inset-0 block size-full'
              />
              {(canvasState.status === 'ready' || preserveTextEditor) &&
                overlayPages.map((overlay) => (
                  <CanvasOverlay
                    key={overlay.page.id}
                    page={overlay.page}
                    camera={overlay.camera}
                    selected={overlay.selected}
                    hovered={hovered}
                    frames={overlay.frames}
                    previews={previews}
                    draft={draftPage === overlay.page.id ? draft : null}
                    cursor={selectedPage === overlay.page.id ? cursor : null}
                    brushSize={brush.diameter}
                    showBrushCursor={isBrushTool(tool)}
                    editing={
                      textEdit?.page === overlay.page.id &&
                      selectedPage === textEdit.page &&
                      project?.name === textEdit.projectName
                        ? textEdit
                        : null
                    }
                    onTextEditChange={updateTextEditing}
                    onTextEditEnd={finishTextEditing}
                    onTextLayoutPreview={previewTextLayout}
                    onTextLayoutPreviewError={reportTextPreviewError}
                    onTransformStart={(elements) =>
                      beginTransform(overlay.page.id, overlay.revision, elements)
                    }
                    onTransformFrame={updateTransform}
                    onTransformEnd={finishTransform}
                    onTransformCancel={cancelGesture}
                  />
                ))}
            </div>
          </div>
          {page && canvasState.status === 'error' && (
            <div
              role='alert'
              className={
                canvasState.hasFrame
                  ? 'absolute inset-x-0 top-3 z-20 flex justify-center px-3'
                  : 'absolute inset-0 z-20 grid place-items-center bg-[var(--surface-canvas)]/90 p-6'
              }
            >
              <div
                className={
                  canvasState.hasFrame
                    ? 'flex max-w-sm flex-col items-center gap-2 rounded-md border border-border bg-popover/95 p-3 text-center shadow-sm'
                    : 'flex max-w-sm flex-col items-center gap-2 text-center'
                }
              >
                <p className='text-sm font-medium text-foreground'>{t('canvas.unavailable')}</p>
                {canvasState.error && (
                  <p className='text-xs leading-relaxed text-muted-foreground'>
                    {canvasState.error.message}
                  </p>
                )}
                <Button
                  data-canvas-control
                  size='sm'
                  variant='outline'
                  className='mt-1'
                  onClick={canvasState.retry}
                >
                  {t('errors.tryAgain')}
                </Button>
              </div>
            </div>
          )}
          {!page && (
            <div className='pointer-events-none absolute inset-0 grid place-items-center'>
              <p className='text-[12px] text-muted-foreground'>{t('canvas.empty')}</p>
            </div>
          )}
        </div>
      </div>
      <StatusBar onZoomChange={setZoom} />
    </main>
  )
}

function hexToRgba(hex: string): [number, number, number, number] {
  const value = hex.replace('#', '')
  return [
    Number.parseInt(value.slice(0, 2), 16),
    Number.parseInt(value.slice(2, 4), 16),
    Number.parseInt(value.slice(4, 6), 16),
    255,
  ]
}

function rgbaToHex(color: [number, number, number, number]): string {
  return `#${color
    .slice(0, 3)
    .map((channel) => channel.toString(16).padStart(2, '0'))
    .join('')}`.toUpperCase()
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value))
}

function containCamera(
  viewportWidth: number,
  viewportHeight: number,
  pageWidth: number,
  pageHeight: number,
): CanvasView {
  if (viewportWidth <= 0 || viewportHeight <= 0 || pageWidth <= 0 || pageHeight <= 0) {
    return { zoom: 1, translation: [0, 0] }
  }
  const zoom = Math.max(
    Number.EPSILON,
    Math.min(viewportWidth / pageWidth, viewportHeight / pageHeight),
  )
  return {
    zoom,
    translation: [
      (viewportWidth - pageWidth * zoom) * 0.5,
      (viewportHeight - pageHeight * zoom) * 0.5,
    ],
  }
}

function fitWidthCamera(viewportWidth: number, pageWidth: number): CanvasView {
  if (viewportWidth <= 0 || pageWidth <= 0) {
    return { zoom: 1, translation: [0, 0] }
  }
  return {
    zoom: Math.max(Number.EPSILON, viewportWidth / pageWidth),
    translation: [0, 0],
  }
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return typeof error === 'string' ? error : 'The WebGPU canvas returned an unknown error.'
}

function clampCameraTranslation(
  translation: [number, number],
  zoom: number,
  pageWidth: number,
  pageHeight: number,
  viewportWidth: number,
  viewportHeight: number,
  dpr: number,
): [number, number] {
  const minOverlap = 100 * dpr
  const minX = minOverlap - pageWidth * zoom
  const minY = minOverlap - pageHeight * zoom
  const maxX = viewportWidth - minOverlap
  const maxY = viewportHeight - minOverlap

  const x =
    minX <= maxX ? clamp(translation[0], minX, maxX) : (viewportWidth - pageWidth * zoom) * 0.5
  const y =
    minY <= maxY ? clamp(translation[1], minY, maxY) : (viewportHeight - pageHeight * zoom) * 0.5
  return [x, y]
}

function mergeStrokeUpdates(current: StrokeUpdate, next: StrokeUpdate): StrokeUpdate {
  if (current.kind !== next.kind) return next
  current.points.push(...next.points)
  return current
}

function useFrameCommand<Value>(
  execute: (value: Value) => void | Promise<unknown>,
  merge?: (current: Value, next: Value) => Value,
): FrameCommand<Value> {
  const executeRef = useRef(execute)
  executeRef.current = execute
  const command = useRef<FrameCommand<Value> | null>(null)
  command.current ??= new FrameCommand((value) => executeRef.current(value), merge)

  useEffect(() => () => command.current?.clear(), [])
  return command.current
}

class FrameCommand<Value> {
  private pending: Value | undefined
  private frame: number | null = null

  constructor(
    private readonly execute: (value: Value) => void | Promise<unknown>,
    private readonly merge: (current: Value, next: Value) => Value = (_current, next) => next,
  ) {}

  schedule(value: Value): void {
    this.pending = this.pending === undefined ? value : this.merge(this.pending, value)
    if (this.frame !== null) return
    this.frame = requestAnimationFrame(() => {
      this.frame = null
      this.executePending()
    })
  }

  commit(): void {
    if (this.frame !== null) {
      cancelAnimationFrame(this.frame)
      this.frame = null
    }
    this.executePending()
  }

  clear(): void {
    if (this.frame !== null) cancelAnimationFrame(this.frame)
    this.frame = null
    this.pending = undefined
  }

  private executePending(): void {
    const value = this.pending
    if (value === undefined) return
    this.pending = undefined
    try {
      void Promise.resolve(this.execute(value)).catch((error: unknown) =>
        receiveError(errorMessage(error)),
      )
    } catch (error) {
      receiveError(errorMessage(error))
    }
  }
}
