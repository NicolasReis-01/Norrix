'use client'

import { Fragment, useEffect, useMemo, useRef, useState } from 'react'
import type { KeyboardEvent, PointerEvent as ReactPointerEvent } from 'react'
import { useTranslation } from 'react-i18next'

import { SelectionControls } from '@/components/editor/SelectionControls'
import { effectiveLayerVisibility, expandLayerSelection, isTextLayer } from '@/lib/document'
import { controlFrame, cssFrame, selectableLayer, type Camera } from '@/lib/geometry'
import { useNorrixStore } from '@/lib/store'
import { previewFontSizeForFrame } from '@/lib/text-fit'
import type {
  EntityId,
  Frame,
  Geometry,
  Layer,
  Page,
  Point,
  TextLayoutPreview,
  TransformFrame,
} from '@norrix/bridge/protocol'

interface CanvasOverlayProps {
  page: Page
  camera: Camera
  selected: EntityId[]
  hovered: EntityId | null
  frames: Readonly<Record<EntityId, Frame>>
  previews: Readonly<Record<EntityId, Frame>>
  draft: Frame | null
  cursor: Point | null
  brushSize: number
  showBrushCursor: boolean
  editing: TextEditSession | null
  onTextEditChange: (session: TextEditSession, text: string) => void
  onTextEditEnd: () => void
  onTextLayoutPreview: (
    page: EntityId,
    element: EntityId,
    text: string,
    revision: number,
  ) => Promise<TextLayoutPreview>
  onTextLayoutPreviewError: (error: unknown) => void
  onTransformStart: (elements: TransformFrame[]) => void
  onTransformFrame: (elements: TransformFrame[]) => void
  onTransformEnd: () => void
  onTransformCancel: () => void
}

export interface TextEditSession {
  page: EntityId
  projectName: string
  layer: Extract<Layer, { type: 'text' }>
  frame: Frame
  field: 'source' | 'translation'
  value: string
  renderedFontSize: number | null
}

export function CanvasOverlay({
  page,
  camera,
  selected,
  hovered,
  frames,
  previews,
  draft,
  cursor,
  brushSize,
  showBrushCursor,
  editing,
  onTextEditChange,
  onTextEditEnd,
  onTextLayoutPreview,
  onTextLayoutPreviewError,
  onTransformStart,
  onTransformFrame,
  onTransformEnd,
  onTransformCancel,
}: CanvasOverlayProps) {
  const root = useRef<HTMLDivElement>(null)
  const expandedSelection = useMemo(
    () => expandLayerSelection(page.layers, selected),
    [page.layers, selected],
  )
  const typographyPreview = useNorrixStore((state) => state.typographyPreview)
  const layerFontSizes = useNorrixStore((state) => state.layerFontSizes)
  const selectedIds = useMemo(() => new Set(expandedSelection), [expandedSelection])
  const multipleSelected = expandedSelection.length > 1
  const layers = useMemo(
    () =>
      page.layers.flatMap((layer) => {
        const visibility = effectiveLayerVisibility(page.layers, layer)
        if (!visibility.visible || visibility.opacity <= 0) return []
        const frame = previews[layer.id] ?? controlFrame(layer, frames)
        return frame ? [{ layer, frame, opacity: visibility.opacity }] : []
      }),
    [page.layers, previews, frames],
  )
  const selectedLayer =
    expandedSelection.length === 1
      ? page.layers.find((layer) => layer.id === expandedSelection[0])
      : undefined
  const selectedTextLayer = selectedLayer && isTextLayer(selectedLayer) ? selectedLayer : undefined
  const automaticRegion = selectedTextLayer?.automatic_region
    ? page.regions.find((region) => region.id === selectedTextLayer.automatic_region)
    : undefined
  const selectionControl = multipleSelected
    ? undefined
    : editing
      ? undefined
      : layers.find(({ layer }) => selectedIds.has(layer.id) && selectableLayer(layer))
  const scale = camera.zoom / window.devicePixelRatio

  return (
    <div
      ref={root}
      data-testid='canvas-overlay'
      className='pointer-events-none absolute inset-0 overflow-hidden'
      aria-hidden={!editing}
    >
      {automaticRegion && (
        <AutomaticRegionOverlay geometry={automaticRegion.geometry} camera={camera} />
      )}
      {layers.map(({ layer, frame, opacity }) => {
        const position = cssFrame(frame, camera)
        const selected = selectedIds.has(layer.id) && selectableLayer(layer)
        const highlighted = !selected && hovered === layer.id
        const previewFontSize = isTextLayer(layer)
          ? (layer.typography?.size ?? layerFontSizes[layer.id] ?? 24)
          : 24
        const previewTypography =
          isTextLayer(layer) && typographyPreview?.layerIds.has(layer.id)
            ? {
                ...layer.typography,
                [typographyPreview.field]: typographyPreview.value,
              }
            : null
        const originalFrame = controlFrame(layer, frames)
        const resizing =
          isTextLayer(layer) &&
          originalFrame !== null &&
          (Math.abs(frame.width - originalFrame.width) > 0.01 ||
            Math.abs(frame.height - originalFrame.height) > 0.01)
        const resizedFontSize =
          resizing && isTextLayer(layer)
            ? previewFontSizeForFrame(layer, frame.width, frame.height)
            : null
        const displayTypography =
          previewTypography ??
          (resizedFontSize !== null && isTextLayer(layer) ? layer.typography : null)
        return (
          <Fragment key={layer.id}>
            <div
              data-element={layer.id}
              className='absolute box-border bg-transparent'
              style={{
                left: position.left,
                top: position.top,
                width: position.width,
                height: position.height,
                transform: `rotate(${position.angle}deg)`,
                transformOrigin: '50% 50%',
                border:
                  highlighted || (selected && multipleSelected)
                    ? '1px solid var(--canvas-selection)'
                    : undefined,
                opacity,
                willChange: selected ? 'left, top, width, height, transform' : undefined,
              }}
            />
            {isTextLayer(layer) &&
              (previewTypography || resizedFontSize !== null) &&
              !(editing?.layer.id === layer.id) && (
                <div
                  aria-hidden='true'
                  data-typography-preview={layer.id}
                  className='pointer-events-none absolute z-10 box-border overflow-hidden break-words whitespace-pre-wrap'
                  style={{
                    left: position.left,
                    top: position.top,
                    width: position.width,
                    height: position.height,
                    transform: `rotate(${position.angle}deg)`,
                    transformOrigin: '50% 50%',
                    fontFamily: displayTypography?.preferred_font ?? 'sans-serif',
                    fontSize: `${(resizedFontSize ?? displayTypography?.size ?? previewFontSize) * scale}px`,
                    fontWeight: displayTypography?.font_weight ?? 400,
                    fontStyle: displayTypography?.font_style ?? 'normal',
                    lineHeight: `${(resizedFontSize ?? displayTypography?.size ?? previewFontSize) * (displayTypography?.line_height ?? 1.2) * scale}px`,
                    letterSpacing: `${(displayTypography?.letter_spacing ?? 0) * scale}px`,
                    textAlign:
                      displayTypography?.alignment === 'Start'
                        ? 'start'
                        : displayTypography?.alignment === 'End'
                          ? 'end'
                          : 'center',
                    writingMode:
                      displayTypography?.writing_mode === 'Vertical'
                        ? 'vertical-rl'
                        : 'horizontal-tb',
                    color: rgba(displayTypography?.color ?? [0, 0, 0, 255]),
                    WebkitTextStroke:
                      (displayTypography?.stroke_width ?? 0) > 0
                        ? `${(displayTypography?.stroke_width ?? 0) * scale}px ${rgba(displayTypography?.stroke_color ?? [255, 255, 255, 255])}`
                        : undefined,
                    opacity,
                  }}
                >
                  {layer.content.translation?.text ?? layer.content.source?.text ?? ''}
                </div>
              )}
          </Fragment>
        )
      })}

      {draft && <DraftOverlay frame={draft} camera={camera} />}
      {editing && (
        <InlineTextEditor
          session={editing}
          camera={camera}
          onChange={onTextEditChange}
          onEnd={onTextEditEnd}
          onPreview={onTextLayoutPreview}
          onPreviewError={onTextLayoutPreviewError}
        />
      )}
      {showBrushCursor && cursor && (
        <div
          className='absolute rounded-full border border-white/95 shadow-[0_0_0_1px_rgb(0_0_0/0.9),0_1px_3px_rgb(0_0_0/0.45)]'
          style={{
            left: cursor.x / window.devicePixelRatio - (brushSize * scale) / 2,
            top: cursor.y / window.devicePixelRatio - (brushSize * scale) / 2,
            width: brushSize * scale,
            height: brushSize * scale,
          }}
        />
      )}

      {selectionControl && (
        <SelectionControls
          container={root}
          element={selectionControl.layer.id}
          frame={selectionControl.frame}
          camera={camera}
          edgesOnly={Boolean(selectedTextLayer)}
          onTransformStart={onTransformStart}
          onTransformFrame={onTransformFrame}
          onTransformEnd={onTransformEnd}
          onTransformCancel={onTransformCancel}
        />
      )}
    </div>
  )
}

function InlineTextEditor({
  session,
  camera,
  onChange,
  onEnd,
  onPreview,
  onPreviewError,
}: {
  session: TextEditSession
  camera: Camera
  onChange: (session: TextEditSession, text: string) => void
  onEnd: () => void
  onPreview: (
    page: EntityId,
    element: EntityId,
    text: string,
    revision: number,
  ) => Promise<TextLayoutPreview>
  onPreviewError: (error: unknown) => void
}) {
  const { t } = useTranslation()
  const input = useRef<HTMLTextAreaElement>(null)
  const previewSvg = useRef<SVGSVGElement>(null)
  const previewRevision = useRef(0)
  const previewFailed = useRef(false)
  const [preview, setPreview] = useState<{
    text: string
    layout: Omit<TextLayoutPreview, 'request_revision'>
  } | null>(null)
  const [previewFailedText, setPreviewFailedText] = useState<string | null>(null)
  const [selection, setSelection] = useState<[number, number]>([
    session.value.length,
    session.value.length,
  ])
  const dragging = useRef(false)
  const dragAnchor = useRef(0)
  const composing = useRef(false)
  const restoreFocusOnWindowFocus = useRef(false)
  const initialSelectionApplied = useRef(false)
  const onEndRef = useRef(onEnd)
  onEndRef.current = onEnd
  const layout = preview?.text === session.value ? preview.layout : null
  const fallbackVisible = previewFailedText === session.value
  const position = cssFrame(session.frame, camera)
  const typography = session.layer.typography
  const scale = camera.zoom / window.devicePixelRatio
  const fallbackFontSize = session.renderedFontSize ?? typography?.size ?? 24
  const resolvedFontSize = layout?.font_size ?? fallbackFontSize
  const resolvedLineHeight = layout?.line_height ?? fallbackFontSize * 1.2
  const editorFontSize = resolvedFontSize * scale
  const editorLineHeight = resolvedLineHeight * scale
  const color = layout?.color ?? typography?.color ?? [0, 0, 0, 255]
  const stroke = layout?.stroke_color ?? typography?.stroke_color ?? [255, 255, 255, 255]
  const strokeWidth = layout?.stroke_width ?? typography?.stroke_width ?? 0

  const restoreEditorFocus = () => {
    if (!restoreFocusOnWindowFocus.current || composing.current) return
    restoreFocusOnWindowFocus.current = false
    if (input.current && document.activeElement !== input.current) input.current.focus()
  }

  useEffect(() => {
    let focusRestoreTimer: number | null = null
    const handleWindowBlur = () => {
      restoreFocusOnWindowFocus.current =
        input.current !== null && document.activeElement === input.current
    }
    const handleWindowFocus = () => {
      if (focusRestoreTimer !== null) window.clearTimeout(focusRestoreTimer)
      focusRestoreTimer = window.setTimeout(() => {
        focusRestoreTimer = null
        if (!restoreFocusOnWindowFocus.current || composing.current) return
        const activeElement = document.activeElement
        if (
          activeElement !== input.current &&
          activeElement !== document.body &&
          activeElement !== document.documentElement
        ) {
          restoreFocusOnWindowFocus.current = false
          onEndRef.current()
          return
        }
        restoreEditorFocus()
      })
    }

    window.addEventListener('blur', handleWindowBlur)
    window.addEventListener('focus', handleWindowFocus)
    return () => {
      if (focusRestoreTimer !== null) window.clearTimeout(focusRestoreTimer)
      window.removeEventListener('blur', handleWindowBlur)
      window.removeEventListener('focus', handleWindowFocus)
    }
  }, [])

  useEffect(() => {
    const revision = ++previewRevision.current
    const timer = window.setTimeout(() => {
      void onPreview(session.page, session.layer.id, session.value, revision)
        .then((result) => {
          if (previewRevision.current !== revision || result.request_revision !== revision) return
          previewFailed.current = false
          setPreviewFailedText(null)
          setPreview({
            text: session.value,
            layout: {
              font_size: result.font_size,
              width: result.width,
              height: result.height,
              line_height: result.line_height,
              line_breaks: result.line_breaks,
              path_data: result.path_data,
              caret_stops: result.caret_stops,
              synthetic_bold_width: result.synthetic_bold_width,
              color: result.color,
              stroke_color: result.stroke_color,
              stroke_width: result.stroke_width,
            },
          })
        })
        .catch((error: unknown) => {
          if (previewRevision.current !== revision) return
          if (!previewFailed.current) onPreviewError(error)
          previewFailed.current = true
          setPreviewFailedText(session.value)
        })
    }, 120)
    return () => {
      window.clearTimeout(timer)
      previewRevision.current += 1
    }
  }, [onPreview, onPreviewError, session.layer.id, session.page, session.value])

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    event.stopPropagation()
  }

  const selectionDirection = useRef<'forward' | 'backward' | 'none'>('none')

  const syncSelection = (inputElement: HTMLTextAreaElement) => {
    const nextSelectionStart = inputElement.selectionStart ?? 0
    const nextSelectionEnd = inputElement.selectionEnd ?? 0
    selectionDirection.current = inputElement.selectionDirection ?? 'none'
    setSelection([nextSelectionStart, nextSelectionEnd])
  }

  const readSelection = (inputElement: HTMLTextAreaElement) => ({
    start: inputElement.selectionStart ?? 0,
    end: inputElement.selectionEnd ?? 0,
    direction: (inputElement.selectionDirection ?? 'none') as 'forward' | 'backward' | 'none',
  })

  const restoreSelection = (
    start: number,
    end: number,
    direction: 'forward' | 'backward' | 'none' = 'none',
  ) => {
    const inputElement = input.current
    if (!inputElement) return
    const selectionStart = Math.min(start, end)
    const selectionEnd = Math.max(start, end)
    selectionDirection.current = direction
    inputElement.setSelectionRange(selectionStart, selectionEnd, direction)
    setSelection([selectionStart, selectionEnd])
  }

  const pointerCaret = (event: ReactPointerEvent<SVGSVGElement>) => {
    const svg = previewSvg.current
    if (!svg || !layout?.caret_stops.length) return 0
    const matrix = typeof svg.getScreenCTM === 'function' ? svg.getScreenCTM() : null
    let localX: number
    let localY: number
    if (matrix && typeof svg.createSVGPoint === 'function') {
      const point = svg.createSVGPoint()
      point.x = event.clientX
      point.y = event.clientY
      const local = point.matrixTransform(matrix.inverse())
      localX = local.x
      localY = local.y
    } else {
      const rect = svg.getBoundingClientRect()
      localX = ((event.clientX - rect.left) / (rect.width || position.width)) * session.frame.width
      localY =
        ((event.clientY - rect.top) / (rect.height || position.height)) * session.frame.height
    }
    return layout.caret_stops.reduce((nearest, stop) => {
      const currentDistance =
        (stop.x + stop.width / 2 - localX) ** 2 + (stop.y + stop.height / 2 - localY) ** 2
      const nearestDistance =
        (nearest.x + nearest.width / 2 - localX) ** 2 +
        (nearest.y + nearest.height / 2 - localY) ** 2
      return currentDistance < nearestDistance ? stop : nearest
    }).offset
  }

  const handlePointerDown = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (!layout) return
    event.preventDefault()
    event.stopPropagation()
    input.current?.focus()
    const offset = pointerCaret(event)
    dragAnchor.current = offset
    dragging.current = true
    selectionDirection.current = 'none'
    input.current?.setSelectionRange(offset, offset, 'none')
    setSelection([offset, offset])
    if (typeof event.currentTarget.setPointerCapture === 'function') {
      event.currentTarget.setPointerCapture(event.pointerId)
    }
  }

  const handlePointerMove = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (!dragging.current || !input.current) return
    event.preventDefault()
    const offset = pointerCaret(event)
    const direction = dragAnchor.current <= offset ? 'forward' : 'backward'
    restoreSelection(dragAnchor.current, offset, direction)
  }

  const selectionRects = layout
    ? textSelectionRects(layout.caret_stops, selection[0], selection[1])
    : []
  const caret =
    layout && selection[0] === selection[1]
      ? [...layout.caret_stops].reverse().find((stop) => stop.offset === selection[0])
      : undefined

  return (
    <>
      <textarea
        ref={input}
        autoFocus
        data-canvas-control
        data-text-edit-surface
        data-testid='inline-text-editor'
        aria-label={t('canvas.editText')}
        dir='auto'
        data-preview-font-size={layout?.font_size}
        data-preview-width={layout?.width}
        data-preview-height={layout?.height}
        data-preview-line-breaks={layout?.line_breaks.join(',')}
        className={`absolute z-20 box-border resize-none border-0 p-0 outline-none ${
          fallbackVisible
            ? 'pointer-events-auto overflow-hidden bg-transparent whitespace-pre-wrap'
            : 'pointer-events-none opacity-0'
        }`}
        style={{
          left: position.left,
          top: position.top,
          width: position.width,
          height: position.height,
          transform: `rotate(${position.angle}deg)`,
          transformOrigin: '50% 50%',
          opacity: fallbackVisible ? 1 : 0,
          pointerEvents: fallbackVisible ? 'auto' : 'none',
          display: 'block',
          margin: 0,
          boxSizing: 'border-box',
          fontFamily: typography?.preferred_font ?? 'sans-serif',
          fontSize: editorFontSize,
          fontWeight: typography?.font_weight ?? 400,
          fontStyle: typography?.font_style ?? 'normal',
          textAlign:
            typography?.alignment === 'Start'
              ? 'start'
              : typography?.alignment === 'End'
                ? 'end'
                : 'center',
          lineHeight: `${editorLineHeight}px`,
          letterSpacing: `${(typography?.letter_spacing ?? 0) * scale}px`,
          color: rgba(typography?.color ?? [0, 0, 0, 255]),
          whiteSpace: 'pre-wrap',
          verticalAlign: 'top',
          WebkitTextStroke:
            (typography?.stroke_width ?? 0) > 0
              ? `${(typography?.stroke_width ?? 0) * scale}px ${rgba(typography?.stroke_color ?? [255, 255, 255, 255])}`
              : undefined,
        }}
        value={session.value}
        onChange={(event) => {
          onChange(session, event.currentTarget.value)
          syncSelection(event.currentTarget)
        }}
        onBlur={() => {
          if (!restoreFocusOnWindowFocus.current && !composing.current) onEnd()
        }}
        onKeyDown={handleKeyDown}
        onKeyUp={(event) => syncSelection(event.currentTarget)}
        onSelect={(event) => syncSelection(event.currentTarget)}
        onFocus={(event) => {
          if (initialSelectionApplied.current) return
          initialSelectionApplied.current = true
          const { start, end, direction } = readSelection(event.currentTarget)
          restoreSelection(start, end, direction)
        }}
        onCompositionStart={() => {
          composing.current = true
        }}
        onCompositionEnd={(event) => {
          composing.current = false
          syncSelection(event.currentTarget)
          if (restoreFocusOnWindowFocus.current) {
            if (document.hasFocus()) restoreEditorFocus()
          } else if (document.activeElement !== input.current) {
            onEnd()
          }
        }}
      />
      <svg
        ref={previewSvg}
        data-text-edit-surface
        data-testid='inline-text-preview'
        className={`${layout ? 'pointer-events-auto' : 'pointer-events-none'} absolute z-20 overflow-visible`}
        viewBox={`0 0 ${session.frame.width} ${session.frame.height}`}
        preserveAspectRatio='none'
        style={{
          left: position.left,
          top: position.top,
          width: position.width,
          height: position.height,
          transform: `rotate(${position.angle}deg)`,
          transformOrigin: '50% 50%',
          cursor: 'text',
        }}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={() => {
          dragging.current = false
        }}
        onPointerCancel={() => {
          dragging.current = false
        }}
      >
        <rect
          width={session.frame.width}
          height={session.frame.height}
          fill='transparent'
          stroke='var(--canvas-selection)'
          strokeWidth={1}
          vectorEffect='non-scaling-stroke'
        />
        {selectionRects.map((rect) => (
          <rect
            key={`${rect.line}-${rect.start}-${rect.end}`}
            data-testid='inline-text-selection'
            x={rect.x}
            y={rect.y}
            width={rect.width}
            height={rect.height}
            fill='var(--canvas-selection)'
            fillOpacity={0.32}
          />
        ))}
        {layout?.path_data && (
          <>
            {layout.synthetic_bold_width > 0 && (
              <path
                d={layout.path_data}
                fill={rgba(color)}
                stroke={rgba(color)}
                strokeWidth={layout.synthetic_bold_width}
                strokeLinejoin='round'
              />
            )}
            {strokeWidth > 0 && stroke[3] > 0 && (
              <path
                d={layout.path_data}
                fill={rgba(color)}
                stroke={rgba(stroke)}
                strokeWidth={strokeWidth * 2}
                strokeLinejoin='round'
              />
            )}
            <path d={layout.path_data} fill={rgba(color)} data-testid='inline-text-glyphs' />
          </>
        )}
        {caret && (
          <rect
            data-testid='inline-text-caret'
            x={caret.x}
            y={caret.y}
            width={caret.width}
            height={caret.height}
            fill={rgba(color)}
          />
        )}
      </svg>
    </>
  )
}

function textSelectionRects(
  stops: TextLayoutPreview['caret_stops'],
  selectionStart: number,
  selectionEnd: number,
): Array<{
  line: number
  start: number
  end: number
  x: number
  y: number
  width: number
  height: number
}> {
  const start = Math.min(selectionStart, selectionEnd)
  const end = Math.max(selectionStart, selectionEnd)
  if (start === end) return []

  const lines = new Map<number, typeof stops>()
  for (const stop of stops) {
    const lineStops = lines.get(stop.line) ?? []
    lines.set(stop.line, [...lineStops, stop])
  }
  return [...lines].flatMap(([line, lineStops]) => {
    const ordered = [...lineStops].sort((left, right) => left.offset - right.offset)
    return ordered.slice(0, -1).flatMap((left, index) => {
      const right = ordered[index + 1]
      if (left.offset < start || right.offset > end || left.offset === right.offset) return []
      return [
        {
          line,
          start: left.offset,
          end: right.offset,
          x: Math.min(left.x, right.x),
          y: Math.min(left.y, right.y),
          width: Math.max(Math.abs(right.x - left.x), left.width, right.width),
          height: Math.max(Math.abs(right.y - left.y), left.height, right.height),
        },
      ]
    })
  })
}

function rgba(color: [number, number, number, number]): string {
  return `rgba(${color[0]}, ${color[1]}, ${color[2]}, ${color[3] / 255})`
}

function AutomaticRegionOverlay({ geometry, camera }: { geometry: Geometry; camera: Camera }) {
  const dpr = window.devicePixelRatio
  const points = geometry.points
    .map(
      (point) =>
        `${(point.x * camera.zoom + camera.translation[0]) / dpr},${(point.y * camera.zoom + camera.translation[1]) / dpr}`,
    )
    .join(' ')
  if (!points) return null

  return (
    <svg className='absolute inset-0 size-full overflow-hidden' data-testid='text-fit-region'>
      <polygon
        points={points}
        fill='none'
        stroke='var(--canvas-region-contrast)'
        strokeWidth='7'
        strokeDasharray='10 6'
        strokeLinecap='round'
        strokeLinejoin='round'
        vectorEffect='non-scaling-stroke'
      />
      <polygon
        points={points}
        fill='none'
        stroke='var(--canvas-region-stroke)'
        strokeWidth='3'
        strokeDasharray='10 6'
        strokeLinecap='round'
        strokeLinejoin='round'
        vectorEffect='non-scaling-stroke'
      />
    </svg>
  )
}

function DraftOverlay({ frame, camera }: { frame: Frame; camera: Camera }) {
  const position = cssFrame(frame, camera)
  return (
    <div
      className='absolute border border-dashed border-primary bg-primary/5'
      style={{
        left: position.left,
        top: position.top,
        width: position.width,
        height: position.height,
        transform: `rotate(${position.angle}deg)`,
      }}
    />
  )
}
