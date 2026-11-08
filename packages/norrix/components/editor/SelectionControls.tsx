'use client'

import { useRef, useState, type PointerEvent as ReactPointerEvent, type RefObject } from 'react'
import { useTranslation } from 'react-i18next'

import {
  cssFrame,
  pagePoint,
  resizeFrame,
  rotateFrame,
  translateFrames,
  type Camera,
  type ResizeHandle,
} from '@/lib/geometry'
import type { EntityId, Frame, Point, TransformFrame } from '@norrix/bridge/protocol'

interface SelectionControlsProps {
  container: RefObject<HTMLDivElement | null>
  element: EntityId
  frame: Frame
  camera: Camera
  edgesOnly: boolean
  onTransformStart: (elements: TransformFrame[]) => void
  onTransformFrame: (elements: TransformFrame[]) => void
  onTransformEnd: () => void
  onTransformCancel: () => void
}

type ControlGesture =
  | { kind: 'move'; pointer: number; original: Frame; start: Point }
  | { kind: 'resize'; pointer: number; original: Frame; handle: ResizeHandle }
  | { kind: 'rotate'; pointer: number; original: Frame; start: Point }

const handles: Array<{
  handle: ResizeHandle
  left: string
  top: string
  cursor: string
}> = [
  { handle: 'nw', left: '0%', top: '0%', cursor: 'nwse-resize' },
  { handle: 'n', left: '50%', top: '0%', cursor: 'ns-resize' },
  { handle: 'ne', left: '100%', top: '0%', cursor: 'nesw-resize' },
  { handle: 'e', left: '100%', top: '50%', cursor: 'ew-resize' },
  { handle: 'se', left: '100%', top: '100%', cursor: 'nwse-resize' },
  { handle: 's', left: '50%', top: '100%', cursor: 'ns-resize' },
  { handle: 'sw', left: '0%', top: '100%', cursor: 'nesw-resize' },
  { handle: 'w', left: '0%', top: '50%', cursor: 'ew-resize' },
]

export function SelectionControls({
  container,
  element,
  frame,
  camera,
  edgesOnly,
  onTransformStart,
  onTransformFrame,
  onTransformEnd,
  onTransformCancel,
}: SelectionControlsProps) {
  const { t } = useTranslation()
  const gesture = useRef<ControlGesture | null>(null)
  const [rotationAngle, setRotationAngle] = useState<number | null>(null)
  const position = cssFrame(frame, camera)

  const eventPoint = (event: { clientX: number; clientY: number }): Point | null => {
    const bounds = container.current?.getBoundingClientRect()
    return bounds ? pagePoint(event.clientX, event.clientY, bounds, camera) : null
  }

  const capture = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault()
    event.stopPropagation()
    event.currentTarget.setPointerCapture(event.pointerId)
  }

  const startResize = (handle: ResizeHandle) => (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || gesture.current) return
    capture(event)
    gesture.current = { kind: 'resize', pointer: event.pointerId, original: frame, handle }
    onTransformStart([{ element, frame }])
  }

  const startMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0 || gesture.current) return
    const start = eventPoint(event)
    if (!start) return
    event.preventDefault()
    event.stopPropagation()
    event.currentTarget.setPointerCapture(event.pointerId)
    gesture.current = { kind: 'move', pointer: event.pointerId, original: frame, start }
    onTransformStart([{ element, frame }])
  }

  const startRotate = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || gesture.current) return
    const start = eventPoint(event)
    if (!start) return
    capture(event)
    gesture.current = { kind: 'rotate', pointer: event.pointerId, original: frame, start }
    setRotationAngle(frame.angle_degrees)
    onTransformStart([{ element, frame }])
  }

  const update = (event: ReactPointerEvent<HTMLElement>) => {
    const current = gesture.current
    if (!current || current.pointer !== event.pointerId) return
    event.preventDefault()
    event.stopPropagation()
    const point = eventPoint(event)
    if (!point) return
    const next =
      current.kind === 'move'
        ? translateFrames([{ element, frame: current.original }], {
            x: point.x - current.start.x,
            y: point.y - current.start.y,
          })[0].frame
        : current.kind === 'resize'
          ? resizeFrame(
              current.original,
              current.handle,
              point,
              window.devicePixelRatio / camera.zoom,
            )
          : rotateFrame(current.original, current.start, point)
    if (current.kind === 'rotate') setRotationAngle(next.angle_degrees)
    onTransformFrame([{ element, frame: next }])
  }

  const finish = (event: ReactPointerEvent<HTMLElement>) => {
    const current = gesture.current
    if (!current || current.pointer !== event.pointerId) return
    event.preventDefault()
    event.stopPropagation()
    gesture.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
    setRotationAngle(null)
    onTransformEnd()
  }

  const cancel = (event: ReactPointerEvent<HTMLElement>) => {
    const current = gesture.current
    if (!current || current.pointer !== event.pointerId) return
    event.preventDefault()
    event.stopPropagation()
    gesture.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
    setRotationAngle(null)
    onTransformCancel()
  }

  const lostCapture = (event: ReactPointerEvent<HTMLElement>) => {
    const current = gesture.current
    if (!current || current.pointer !== event.pointerId) return
    gesture.current = null
    setRotationAngle(null)
    onTransformCancel()
  }

  const pointerEvents = {
    onPointerMove: update,
    onPointerUp: finish,
    onPointerCancel: cancel,
    onLostPointerCapture: lostCapture,
  }

  return (
    <div
      data-testid='selection-controls'
      className='pointer-events-none absolute box-border border border-[var(--canvas-selection)]'
      style={{
        left: position.left,
        top: position.top,
        width: position.width,
        height: position.height,
        transform: `rotate(${position.angle}deg)`,
        transformOrigin: '50% 50%',
      }}
    >
      {handles.map(({ handle, left, top, cursor }) => (
        <div
          key={handle}
          data-canvas-control
          data-resize-handle={handle}
          className='pointer-events-auto absolute grid size-3.5 -translate-x-1/2 -translate-y-1/2 touch-none place-items-center'
          style={{
            left,
            top,
            cursor,
            width:
              edgesOnly && (handle === 'n' || handle === 's')
                ? 'max(14px, calc(100% - 14px))'
                : undefined,
            height:
              edgesOnly && (handle === 'e' || handle === 'w')
                ? 'max(14px, calc(100% - 14px))'
                : undefined,
          }}
          onPointerDown={startResize(handle)}
          {...pointerEvents}
        >
          {!edgesOnly && (
            <span className='size-1.5 rounded-[1px] border border-primary-foreground bg-[var(--canvas-selection)] shadow-[0_0_0_1px_rgb(0_0_0/0.12)]' />
          )}
        </div>
      ))}

      {edgesOnly && (
        <button
          type='button'
          data-canvas-control
          data-move-handle
          data-testid='text-move-handle'
          aria-label={t('canvas.moveText')}
          title={t('canvas.moveText')}
          className='pointer-events-auto absolute top-[-12px] right-[-12px] grid size-6 cursor-move touch-none place-items-center rounded-full border border-[var(--canvas-selection)] bg-popover text-foreground shadow-sm'
          onPointerDown={startMove}
          {...pointerEvents}
        >
          <svg aria-hidden='true' viewBox='0 0 16 16' className='size-3.5'>
            <path
              d='M8 1.5v13M1.5 8h13M8 1.5 5.8 3.7M8 1.5l2.2 2.2M8 14.5l-2.2-2.2M8 14.5l2.2-2.2M1.5 8l2.2-2.2M1.5 8l2.2 2.2M14.5 8l-2.2-2.2M14.5 8l-2.2 2.2'
              fill='none'
              stroke='currentColor'
              strokeWidth='1.3'
              strokeLinecap='round'
              strokeLinejoin='round'
            />
          </svg>
        </button>
      )}

      <span className='pointer-events-none absolute top-0 left-1/2 h-2 w-px -translate-x-1/2 -translate-y-full bg-[var(--canvas-selection)]' />
      <div
        data-canvas-control
        data-rotate-handle
        className='pointer-events-auto absolute top-[-16px] left-1/2 grid size-3.5 -translate-x-1/2 -translate-y-1/2 cursor-alias touch-none place-items-center'
        onPointerDown={startRotate}
        {...pointerEvents}
      >
        <span className='size-1.5 rounded-[1px] border border-primary-foreground bg-[var(--canvas-selection)] shadow-[0_0_0_1px_rgb(0_0_0/0.12)]' />
      </div>
      {rotationAngle !== null && (
        <span
          data-testid='rotation-angle-indicator'
          className='pointer-events-none absolute top-[-40px] left-1/2 -translate-x-1/2 rounded bg-popover px-1.5 py-0.5 text-[10px] leading-none font-medium whitespace-nowrap text-popover-foreground shadow-sm'
        >
          {`${Math.round(rotationAngle)}°`}
        </span>
      )}
    </div>
  )
}
