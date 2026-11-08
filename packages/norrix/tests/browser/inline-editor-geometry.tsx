import { createInstance } from 'i18next'
import { useCallback, useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { I18nextProvider, initReactI18next } from 'react-i18next'

import { CanvasOverlay, type TextEditSession } from '@/components/editor/CanvasOverlay'
import type { Camera } from '@/lib/geometry'
import type { Frame, Layer, Page, TextLayoutPreview } from '@norrix/bridge/protocol'

declare global {
  interface Window {
    geometryHarness: {
      resize: () => void
      zoom: () => void
      frame: () => Frame
      camera: () => Camera
    }
  }
}

const i18n = createInstance()
await i18n.use(initReactI18next).init({
  lng: 'en',
  resources: { en: { translation: { 'canvas.editText': 'Edit text' } } },
})

const initialFrame: Frame = {
  x: 10,
  y: 10,
  width: 240,
  height: 120,
  angle_degrees: 28,
}

function geometryPoints(frame: Frame) {
  const angle = (frame.angle_degrees * Math.PI) / 180
  const centerX = frame.x + frame.width / 2
  const centerY = frame.y + frame.height / 2
  return [
    [frame.x, frame.y],
    [frame.x + frame.width, frame.y],
    [frame.x + frame.width, frame.y + frame.height],
    [frame.x, frame.y + frame.height],
  ].map(([x, y]) => ({
    x: centerX + Math.cos(angle) * (x - centerX) - Math.sin(angle) * (y - centerY),
    y: centerY + Math.sin(angle) * (x - centerX) + Math.cos(angle) * (y - centerY),
  }))
}

function rendererFrame(frame: Frame): Frame {
  const x = (frame.width - preview.width) / 2
  const y = (frame.height - preview.height) / 2
  const angle = (frame.angle_degrees * Math.PI) / 180
  const centerX = frame.x + frame.width / 2
  const centerY = frame.y + frame.height / 2
  const corners = [
    [x, y],
    [x + preview.width, y],
    [x + preview.width, y + preview.height],
    [x, y + preview.height],
  ].map(([localX, localY]) => ({
    x:
      centerX +
      Math.cos(angle) * (localX - frame.width / 2) -
      Math.sin(angle) * (localY - frame.height / 2),
    y:
      centerY +
      Math.sin(angle) * (localX - frame.width / 2) +
      Math.cos(angle) * (localY - frame.height / 2),
  }))
  const xs = corners.map((point) => point.x)
  const ys = corners.map((point) => point.y)
  return {
    x: Math.min(...xs),
    y: Math.min(...ys),
    width: Math.max(...xs) - Math.min(...xs),
    height: Math.max(...ys) - Math.min(...ys),
    angle_degrees: frame.angle_degrees,
  }
}

const textLayer: Extract<Layer, { type: 'text' }> = {
  type: 'text',
  id: 'geometry-text',
  parent: 'geometry-page',
  geometry: {
    points: [...geometryPoints(initialFrame)],
  },
  angle_degrees: initialFrame.angle_degrees,
  visibility: { visible: true, opacity: 1 },
  content: {
    id: 'geometry-text',
    source: { text: 'Hi', language: 'en' },
    translation: { text: 'Hi', language: 'en' },
    role: null,
    source_region: null,
  },
  typography: {
    preferred_font: 'Arial',
    font_weight: 400,
    font_style: 'normal',
    size: 20,
    auto_fit: false,
    color: [20, 40, 60, 255],
    stroke_color: [255, 255, 255, 255],
    stroke_width: 0,
    alignment: 'Start',
    writing_mode: null,
    line_height: null,
    letter_spacing: null,
  },
  layout: 'paragraph',
  automatic_region: null,
}

const page: Page = {
  id: 'geometry-page',
  label: 'Geometry page',
  size: { width: 900, height: 700 },
  layers: [textLayer],
  regions: [],
}

const preview: TextLayoutPreview = {
  request_revision: 1,
  font_size: 20,
  width: 19.951874,
  height: 19.921875,
  line_height: 24,
  line_breaks: [],
  path_data:
    'M121.727 68.961C122.758 68.961 123.399 68.430 123.399 67.367C123.399 67.227 123.399 67.102 123.383 66.961C123.133 63.930 123.102 60.883 123.102 57.852C123.102 56.367 123.180 54.914 123.180 53.430C123.180 52.305 122.962 51.039 121.587 51.039C120.774 51.039 120.055 51.539 120.055 52.414C120.055 53.727 120.227 55.039 120.227 56.367L120.227 57.570C118.133 57.992 116.087 58.492 114.087 59.180L114.055 58.039C114.055 56.867 114.165 55.680 114.165 54.508C114.165 53.711 113.915 52.805 112.962 52.805C112.055 52.805 111.024 53.273 111.024 54.305C111.024 55.414 111.274 56.461 111.274 57.570C111.274 59.523 111.337 61.492 111.337 63.445C111.337 64.430 111.055 65.414 111.055 66.383C111.055 67.305 111.227 68.414 112.383 68.414C114.165 68.414 114.399 67.523 114.399 65.961C114.399 65.305 114.337 64.633 114.337 63.961C114.337 63.711 114.368 63.445 114.383 63.180C116.227 62.164 118.337 61.773 120.087 61.523L120.415 61.523L120.415 61.961C120.415 63.680 120.165 65.383 120.165 67.102C120.165 68.211 120.477 68.961 121.727 68.961ZM127.554 68.414C128.304 68.414 128.757 67.367 128.757 66.727C128.757 65.305 128.617 63.867 128.617 62.445C128.617 59.789 128.976 57.148 128.976 54.492C128.976 53.586 128.913 51.773 127.663 51.773C126.820 51.773 125.867 52.164 125.867 53.148C125.867 53.633 125.976 54.102 125.976 54.586C125.976 55.383 125.913 56.164 125.898 56.961C125.835 60.180 125.804 63.414 125.804 66.633C125.804 67.664 126.523 68.414 127.554 68.414Z',
  caret_stops: [
    { offset: 0, x: 109.82094, y: 47.726563, width: 1, height: 24, line: 0 },
    { offset: 1, x: 124.60094, y: 47.726563, width: 1, height: 24, line: 0 },
    { offset: 2, x: 130.18094, y: 47.726563, width: 1, height: 24, line: 0 },
  ],
  synthetic_bold_width: 0,
  color: [20, 40, 60, 255],
  stroke_color: [255, 255, 255, 255],
  stroke_width: 0,
}

function translatePathData(pathData: string, deltaX: number, deltaY: number): string {
  const tokens = pathData.match(/[A-Za-z]|[-+]?\d*\.?\d+(?:[eE][-+]?\d+)?/g) ?? []
  const arity: Record<string, number> = { M: 2, L: 2, C: 6, Z: 0 }
  let command = ''
  let argument = 0
  return tokens
    .map((token) => {
      if (/^[A-Za-z]$/.test(token)) {
        command = token
        argument = 0
        return token
      }
      const coordinate = Number(token)
      const axis = argument++ % arity[command]
      return String(coordinate + (axis % 2 === 0 ? deltaX : deltaY))
    })
    .join(' ')
}

function previewForFrame(frame: Frame): TextLayoutPreview {
  const deltaX = (frame.width - initialFrame.width) / 2
  const deltaY = (frame.height - initialFrame.height) / 2
  return {
    ...preview,
    path_data: translatePathData(preview.path_data, deltaX, deltaY),
    caret_stops: preview.caret_stops.map((stop) => ({
      ...stop,
      x: stop.x + deltaX,
      y: stop.y + deltaY,
    })),
  }
}

function Harness() {
  const [frame, setFrame] = useState(initialFrame)
  const [camera, setCamera] = useState<Camera>({
    zoom: 1.35,
    translation: [23, 17],
  })
  const [editing, setEditing] = useState<TextEditSession | null>(null)
  const layer: Extract<Layer, { type: 'text' }> = {
    ...textLayer,
    geometry: { points: geometryPoints(frame) },
  }
  const currentPage = { ...page, layers: [layer] }

  const begin = () => {
    const source =
      new URLSearchParams(window.location.search).get('frameSource') === 'renderer'
        ? rendererFrame(frame)
        : frame
    setEditing({
      page: currentPage.id,
      projectName: 'Browser geometry fixture',
      layer,
      frame: source,
      field: 'translation',
      value: layer.content.translation?.text ?? '',
      renderedFontSize: 20,
    })
  }

  const resize = useCallback(() => {
    const next = { ...frame, x: 25, y: 25, width: 220, height: 96 }
    const nextLayer: Extract<Layer, { type: 'text' }> = {
      ...textLayer,
      geometry: { points: geometryPoints(next) },
    }
    setFrame(next)
    if (editing) {
      const source =
        new URLSearchParams(window.location.search).get('frameSource') === 'renderer'
          ? rendererFrame(next)
          : next
      setEditing({ ...editing, layer: nextLayer, frame: source })
    }
  }, [editing, frame])

  const zoom = useCallback(() => setCamera({ zoom: 1.8, translation: [31, 26] }), [])

  useEffect(() => {
    window.geometryHarness = {
      resize,
      zoom,
      frame: () => frame,
      camera: () => camera,
    }
  }, [camera, frame, resize, zoom])

  return (
    <>
      <div className='controls'>
        <button data-testid='begin-edit' onClick={begin}>
          Begin edit
        </button>
        <button data-testid='resize-frame' onClick={resize}>
          Resize frame
        </button>
        <button data-testid='change-zoom' onClick={zoom}>
          Change zoom
        </button>
        <button data-testid='end-edit' onClick={() => setEditing(null)}>
          End edit
        </button>
      </div>
      <div
        data-testid='canvas-viewport'
        style={{
          position: 'absolute',
          left: 120,
          top: 90,
          width: 800,
          height: 620,
          overflow: 'hidden',
          background: '#e8e8e8',
        }}
      >
        <canvas
          data-testid='webgpu-canvas'
          style={{ position: 'absolute', inset: 0, width: '100%', height: '100%' }}
        />
        <CanvasOverlay
          page={currentPage}
          camera={camera}
          selected={['geometry-text']}
          hovered={null}
          frames={{ 'geometry-text': rendererFrame(frame) }}
          previews={{}}
          draft={null}
          cursor={null}
          brushSize={48}
          showBrushCursor={false}
          editing={editing}
          onTextEditChange={(session, value) =>
            setEditing((current) =>
              current?.layer.id === session.layer.id ? { ...current, value } : current,
            )
          }
          onTextEditEnd={() => setEditing(null)}
          onTextLayoutPreview={async (_page, _element, _text, revision) => ({
            ...previewForFrame(frame),
            request_revision: revision,
          })}
          onTextLayoutPreviewError={(error) => {
            throw error
          }}
          onTransformStart={() => {}}
          onTransformFrame={() => {}}
          onTransformEnd={() => {}}
          onTransformCancel={() => {}}
        />
      </div>
    </>
  )
}

createRoot(document.getElementById('root')!).render(
  <I18nextProvider i18n={i18n}>
    <Harness />
  </I18nextProvider>,
)
