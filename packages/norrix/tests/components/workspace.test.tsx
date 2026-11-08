import { QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { CanvasWorkspace } from '@/components/editor/CanvasWorkspace'
import { cssFrame } from '@/lib/geometry'
import { pageKey, pagesKey, preparedPageKey, projectKey, queryClient } from '@/lib/queries'
import { useNorrixStore } from '@/lib/store'
import { commands, type Frame, type Layer, type TextLayoutPreview } from '@norrix/bridge/protocol'
import { TooltipProvider } from '@norrix/ui/components/tooltip'

const canvas = vi.hoisted(() => ({
  resize: vi.fn(),
  setView: vi.fn(),
  stageManifest: vi.fn(),
  installResource: vi.fn(),
  activateFrame: vi.fn(),
  activatePage: vi.fn(),
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
  dispose: vi.fn(),
}))

const canvasState = vi.hoisted(() => ({
  canvas,
  error: null as Error | null,
  generation: 1 as number | null,
  hasFrame: true,
  retry: vi.fn(),
  status: 'ready' as 'loading' | 'switching' | 'ready' | 'recovering' | 'error',
}))
const prefetchCanvasPages = vi.hoisted(() => vi.fn(async () => []))

vi.mock('@/components/editor/useCanvas', () => ({
  useCanvas: () => canvasState,
}))
vi.mock('@norrix/bridge/canvas', () => ({
  prefetchCanvasPages,
  previewCanvasOpacity: (element: string, opacity: number | null) =>
    canvas.previewOpacity(element, opacity),
  workspaceColor: () => [245, 245, 245],
}))

const layer: Layer = {
  type: 'image',
  id: 'element',
  parent: 'page',
  geometry: {
    points: [
      { x: 10, y: 20 },
      { x: 110, y: 20 },
      { x: 110, y: 70 },
      { x: 10, y: 70 },
    ],
  },
  visibility: { visible: true, opacity: 1 },
  image: 'image',
}

const paintLayer: Layer = {
  type: 'raster',
  id: 'paint',
  parent: 'page',
  visibility: { visible: true, opacity: 1 },
  image: 'paint-image',
  name: 'Paint 1',
  kind: 'paint',
}

function textPreview(text: string, requestRevision: number): TextLayoutPreview {
  const caretStops: TextLayoutPreview['caret_stops'] = []
  let offset = 0
  let x = 0
  let y = 0
  let line = 0
  caretStops.push({ offset, x, y, width: 1, height: 16, line })
  for (const character of text) {
    offset += character.length
    if (character === '\n') {
      line++
      x = 0
      y += 20
    } else {
      x += 10
    }
    caretStops.push({ offset, x, y, width: 1, height: 16, line })
  }
  return {
    request_revision: requestRevision,
    font_size: 18,
    width: 70,
    height: 36,
    line_height: 16,
    line_breaks: [],
    path_data: 'M1 1L8 1L8 12Z',
    caret_stops: caretStops,
    synthetic_bold_width: 0,
    color: [20, 40, 60, 255],
    stroke_color: [255, 255, 255, 255],
    stroke_width: 1,
  }
}

function textLayer(id: string): Layer {
  return {
    type: 'text',
    id,
    parent: 'page',
    geometry: {
      points: [
        { x: 10, y: 20 },
        { x: 110, y: 20 },
        { x: 110, y: 70 },
        { x: 10, y: 70 },
      ],
    },
    angle_degrees: 0,
    visibility: { visible: true, opacity: 1 },
    content: {
      id,
      source: { text: 'Matching line', language: 'en' },
      translation: null,
      role: null,
      source_region: null,
    },
    typography: null,
    layout: 'paragraph',
    automatic_region: null,
  }
}

function geometryPoints(frame: Frame): Array<{ x: number; y: number }> {
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

let nextAnimationFrame = 1
let animationFrames = new Map<number, FrameRequestCallback>()

beforeEach(() => {
  nextAnimationFrame = 1
  animationFrames = new Map()
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    const frame = nextAnimationFrame++
    animationFrames.set(frame, callback)
    return frame
  })
  vi.stubGlobal('cancelAnimationFrame', (frame: number) => animationFrames.delete(frame))
  canvasState.error = null
  canvasState.generation = 1
  canvasState.hasFrame = true
  canvasState.status = 'ready'
  canvas.setPageLayout.mockReturnValue(true)
  prefetchCanvasPages.mockClear()
})

afterEach(() => vi.unstubAllGlobals())

function installProject() {
  const page = {
    id: 'page',
    label: 'Page',
    size: { width: 1000, height: 1000 },
    layers: [layer],
    regions: [],
  }
  queryClient.setQueryData(projectKey, {
    name: 'Book',
    revision: 1,
    active_page: 'page',
    can_undo: false,
    can_redo: false,
  })
  queryClient.setQueryData(pagesKey, [])
  queryClient.setQueryData(pageKey, page)
  queryClient.setQueryData(preparedPageKey('page'), {
    revision: 1,
    page,
    element_frames: [],
    text_font_sizes: [],
  })
  useNorrixStore.setState({
    selectedLayers: [],
    selectedPage: 'page',
    selectedLayersByPage: {},
    layerFontSizes: {},
    workspaceMode: 'single',
    tool: 'select',
  })
  useNorrixStore.setState({
    canvasPage: 'page',
    canvasRevision: 1,
    canvasGeneration: 1,
    canvasSize: [1000, 1000],
  })
}

function installEditableText(mode: 'single' | 'infinite' = 'single', translated = true) {
  installProject()
  const text = textLayer('editable')
  if (text.type !== 'text') throw new Error('test fixture must be a text layer')
  text.content = {
    ...text.content,
    source: { text: 'Source text', language: 'en' },
    translation: translated ? { text: 'Original translation', language: 'fr' } : null,
  }
  text.typography = {
    preferred_font: 'Noto Sans',
    font_weight: 700,
    font_style: 'italic',
    size: 18,
    auto_fit: false,
    color: [20, 40, 60, 255],
    stroke_color: [255, 255, 255, 255],
    stroke_width: 1,
    alignment: 'End',
    writing_mode: null,
    line_height: null,
    letter_spacing: null,
  }
  const page = {
    id: 'page',
    label: 'Page',
    size: { width: 1000, height: 1000 },
    layers: [layer, text],
    regions: [],
  }
  queryClient.setQueryData(pageKey, page)
  queryClient.setQueryData(preparedPageKey('page'), {
    revision: 1,
    page,
    element_frames: [
      {
        element: 'editable',
        frame: { x: 10, y: 20, width: 100, height: 50, angle_degrees: 0 },
      },
    ],
    text_font_sizes: [{ element: 'editable', font_size: 18 }],
  })
  if (mode === 'infinite') {
    queryClient.setQueryData(pagesKey, [
      {
        id: 'page',
        label: 'Page',
        size: { width: 1000, height: 1000 },
        source_asset: null,
        layer_count: 2,
      },
    ])
  }
  useNorrixStore.setState({
    selectedLayers: ['editable'],
    selectedPage: 'page',
    selectedLayersByPage: { page: ['editable'] },
    workspaceMode: mode,
    camera: { zoom: 1, translation: [0, 0], fitted: false },
    layerFrames: {
      editable: { x: 10, y: 20, width: 100, height: 50, angle_degrees: 0 },
    },
    layerFontSizes: { editable: 18 },
  })
  return page
}

function installEmptyEditableText() {
  installProject()
  const text = textLayer('editable')
  if (text.type !== 'text') throw new Error('test fixture must be a text layer')
  text.content = {
    ...text.content,
    source: { text: '', language: null },
    translation: { text: '', language: null },
  }
  const page = {
    id: 'page',
    label: 'Page',
    size: { width: 1000, height: 1000 },
    layers: [layer, text],
    regions: [],
  }
  const frame = { x: 10, y: 20, width: 100, height: 50, angle_degrees: 0 }
  queryClient.setQueryData(pageKey, page)
  queryClient.setQueryData(preparedPageKey('page'), {
    revision: 1,
    page,
    element_frames: [{ element: 'editable', frame }],
    text_font_sizes: [],
  })
  useNorrixStore.setState({
    selectedLayers: ['editable'],
    selectedPage: 'page',
    selectedLayersByPage: { page: ['editable'] },
    layerFrames: { editable: frame },
    layerFontSizes: {},
  })
  return page
}

function renderWorkspace() {
  render(
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>
        <CanvasWorkspace />
      </TooltipProvider>
    </QueryClientProvider>,
  )

  const surface = screen.getByLabelText('Norrix canvas')
  Object.defineProperty(surface, 'getBoundingClientRect', {
    value: () => ({ x: 10, y: 20, width: 800, height: 600 }),
  })
  Object.defineProperty(surface, 'clientHeight', { value: 600 })
  return surface
}

function transitionCanvasGeneration(generation: number) {
  act(() => {
    canvasState.status = 'switching'
    useNorrixStore.setState({ canvasGeneration: generation })
  })
  act(() => {
    canvasState.status = 'ready'
    canvasState.generation = generation
    useNorrixStore.setState({ canvasRevision: generation })
  })
}

async function clickTextAndStartCanvasSwitch(surface: HTMLElement) {
  const pointer = { button: 0, clientX: 30, clientY: 50 }
  fireEvent.pointerDown(surface, { ...pointer, pointerId: 201, detail: 1 })
  fireEvent.pointerUp(surface, { ...pointer, pointerId: 201, detail: 1 })
  await waitFor(() => expect(canvas.finishTransform).toHaveBeenCalled())
  act(() => {
    canvasState.status = 'switching'
    useNorrixStore.setState({ canvasGeneration: 2 })
  })
  return pointer
}

function clickTextDuringCanvasSwitch(
  surface: HTMLElement,
  pointer: { button: number; clientX: number; clientY: number },
  pointerId = 202,
) {
  fireEvent.pointerDown(surface, { ...pointer, pointerId, detail: 1 })
  fireEvent.pointerUp(surface, { ...pointer, pointerId, detail: 1 })
}

describe('canvas interaction adapter', () => {
  it('prefetches only after the authoritative canvas page and generation are active', async () => {
    installProject()
    queryClient.setQueryData(pagesKey, [
      {
        id: 'page',
        label: 'Page',
        size: { width: 1000, height: 1000 },
        source_asset: null,
        layer_count: 1,
      },
      {
        id: 'next',
        label: 'Next',
        size: { width: 1000, height: 1000 },
        source_asset: null,
        layer_count: 1,
      },
    ])
    useNorrixStore.setState({ canvasPage: 'previous' })
    renderWorkspace()

    await Promise.resolve()
    expect(prefetchCanvasPages).not.toHaveBeenCalled()

    act(() => useNorrixStore.setState({ canvasPage: 'page' }))
    await waitFor(() => expect(prefetchCanvasPages).toHaveBeenCalledWith(['next']))
  })

  it('renders an accessible browser canvas and keeps camera updates local', async () => {
    installProject()
    const surface = renderWorkspace()
    expect(screen.getByTestId('webgpu-canvas')).toBeInstanceOf(HTMLCanvasElement)

    fireEvent.wheel(surface, { clientX: 100, clientY: 100, deltaY: 4 })
    fireEvent.wheel(surface, { clientX: 100, clientY: 100, deltaY: 6 })

    await waitFor(() => expect(canvas.setView).toHaveBeenLastCalledWith(1, [0, -10]))
  })

  it('preserves a manual camera when a newer canvas generation becomes ready', () => {
    installProject()
    renderWorkspace()
    const camera = { zoom: 2, translation: [-120, -80] as [number, number], fitted: false }

    act(() => useNorrixStore.setState({ camera }))
    act(() => {
      canvasState.status = 'switching'
      useNorrixStore.setState({ canvasGeneration: 2 })
    })
    act(() => {
      canvasState.status = 'ready'
      canvasState.generation = 2
      useNorrixStore.setState({ canvasRevision: 2 })
    })

    expect(useNorrixStore.getState().camera).toEqual(camera)
  })

  it('switches presentation modes without clearing page selections', async () => {
    installProject()
    useNorrixStore.setState({
      selectedLayers: ['element'],
      selectedPage: 'page',
      selectedLayersByPage: { page: ['element'] },
    })
    const selected = vi.spyOn(commands, 'selectPage').mockResolvedValue({
      project: {
        name: 'Book',
        revision: 1,
        active_page: 'page',
        can_undo: false,
        can_redo: false,
      },
      page: {
        id: 'page',
        label: 'Page',
        size: { width: 1000, height: 1000 },
        layers: [layer],
        regions: [],
      },
    })
    renderWorkspace()

    const infiniteButton = screen.getByRole('button', { name: 'Infinite Scroll' })
    const singleButton = screen.getByRole('button', { name: 'Single Page' })
    expect(singleButton).toHaveAttribute('aria-pressed', 'true')
    expect(infiniteButton).toHaveAttribute('aria-pressed', 'false')
    fireEvent.click(infiniteButton)
    expect(useNorrixStore.getState().workspaceMode).toBe('infinite')
    expect(infiniteButton).toHaveAttribute('aria-pressed', 'true')
    fireEvent.click(singleButton)

    await waitFor(() => expect(selected).toHaveBeenCalledWith('page'))
    expect(useNorrixStore.getState().workspaceMode).toBe('single')
    expect(singleButton).toHaveAttribute('aria-pressed', 'true')
    expect(useNorrixStore.getState().selectedLayersByPage).toEqual({ page: ['element'] })
  })

  it('updates the viewport camera continuously without moving or recomposing page coordinates', async () => {
    installProject()
    const summaries = [0, 1, 2].map((index) => ({
      id: index === 0 ? 'page' : `page-${index + 1}`,
      label: `Page ${index + 1}`,
      size: { width: 1000, height: 1000 },
      source_asset: null,
      layer_count: 1,
    }))
    queryClient.setQueryData(pagesKey, summaries)
    for (const summary of summaries) {
      queryClient.setQueryData(preparedPageKey(summary.id), {
        page: {
          ...queryClient.getQueryData(pageKey)!,
          id: summary.id,
          label: summary.label,
        },
        revision: 1,
        element_frames: [],
        text_font_sizes: [],
      })
    }

    const surface = renderWorkspace()
    const infiniteButton = screen.getByRole('button', { name: 'Infinite Scroll' })
    fireEvent.click(infiniteButton)
    await waitFor(() =>
      expect(canvas.setPageLayout).toHaveBeenLastCalledWith(
        expect.arrayContaining([
          expect.objectContaining({ page: 'page' }),
          expect.objectContaining({ page: 'page-2' }),
          expect.objectContaining({ page: 'page-3' }),
        ]),
      ),
    )
    await waitFor(() => expect(useNorrixStore.getState().camera.zoom).toBeCloseTo(0.8))

    const layoutCallCount = canvas.setPageLayout.mock.calls.length
    Object.defineProperty(surface, 'scrollTop', {
      configurable: true,
      writable: true,
      value: 325.5,
    })
    fireEvent.scroll(surface)

    await waitFor(() =>
      expect(useNorrixStore.getState().camera.translation[1]).toBe(
        -325.5 * window.devicePixelRatio,
      ),
    )
    expect(canvas.setView).toHaveBeenLastCalledWith(
      useNorrixStore.getState().camera.zoom,
      useNorrixStore.getState().camera.translation,
    )
    expect(canvas.setPageLayout).toHaveBeenCalledTimes(layoutCallCount)
  })

  it('composes adjacent-page ghost text automatically in both workspace modes', async () => {
    installProject()
    const pageData = (id: string, label: string) => ({
      id,
      label,
      size: { width: 1000, height: 1000 },
      layers: [textLayer(`${id}-text`)],
      regions: [],
    })
    const summaries = [
      {
        id: 'previous',
        label: 'Previous',
        size: { width: 1000, height: 1000 },
        source_asset: null,
        layer_count: 1,
      },
      {
        id: 'page',
        label: 'Page',
        size: { width: 1000, height: 1000 },
        source_asset: null,
        layer_count: 1,
      },
      {
        id: 'next',
        label: 'Next',
        size: { width: 1000, height: 1000 },
        source_asset: null,
        layer_count: 1,
      },
    ]
    queryClient.setQueryData(pageKey, pageData('page', 'Page'))
    queryClient.setQueryData(pagesKey, summaries)
    for (const summary of summaries) {
      queryClient.setQueryData(preparedPageKey(summary.id), {
        page: pageData(summary.id, summary.label),
        revision: 1,
        element_frames: [],
        text_font_sizes: [],
      })
    }
    renderWorkspace()

    expect(screen.queryByRole('button', { name: /ghost/i })).not.toBeInTheDocument()
    expect(screen.queryByTestId('cross-page-ghost')).not.toBeInTheDocument()
    await waitFor(() =>
      expect(canvas.setPageLayout).toHaveBeenLastCalledWith([
        expect.objectContaining({
          page: 'page',
          ghostPages: [
            { page: 'previous', x: 0, y: -1000 },
            { page: 'next', x: 0, y: 1000 },
          ],
        }),
      ]),
    )

    fireEvent.click(screen.getByRole('button', { name: 'Infinite Scroll' }))
    await waitFor(() =>
      expect(canvas.setPageLayout).toHaveBeenLastCalledWith(
        expect.arrayContaining([
          expect.objectContaining({
            page: 'previous',
            ghostPages: [
              { page: 'page', x: 0, y: 1000 },
              { page: 'next', x: 0, y: 2000 },
            ],
          }),
          expect.objectContaining({
            page: 'page',
            ghostPages: [
              { page: 'previous', x: 0, y: -1000 },
              { page: 'next', x: 0, y: 1000 },
            ],
          }),
          expect.objectContaining({
            page: 'next',
            ghostPages: [
              { page: 'previous', x: 0, y: -2000 },
              { page: 'page', x: 0, y: -1000 },
            ],
          }),
        ]),
      ),
    )
    expect(screen.queryByTestId('cross-page-ghost')).not.toBeInTheDocument()
  })

  it('suppresses Alt browser handling while an editor input retains focus', () => {
    installProject()
    renderWorkspace()
    const input = document.createElement('input')
    document.body.append(input)
    input.focus()

    const event = new KeyboardEvent('keydown', { key: 'Alt', bubbles: true, cancelable: true })
    input.dispatchEvent(event)

    expect(event.defaultPrevented).toBe(true)
    input.remove()
  })

  it('announces WebGPU startup failures and offers recovery', () => {
    installProject()
    canvasState.status = 'error'
    canvasState.error = new Error('No compatible WebGPU adapter was found.')
    renderWorkspace()
    expect(screen.getByRole('alert')).toHaveTextContent('WebGPU canvas unavailable')
    expect(screen.getByRole('alert')).toHaveTextContent('No compatible WebGPU adapter was found.')
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(canvasState.retry).toHaveBeenCalledOnce()
  })

  it('previews brush input locally and sends only the durable paint commit to Rust', async () => {
    installProject()
    useNorrixStore.setState({ tool: 'draw', brush: { diameter: 48, color: '#FFFFFF' } })
    const commit = vi
      .spyOn(commands, 'commitPaint')
      .mockResolvedValue({ revision: 2, layer: 'paint' })
    const surface = renderWorkspace()
    expect(surface).toHaveStyle({ cursor: 'none' })

    fireEvent.pointerDown(surface, { button: 0, pointerId: 7, clientX: 30, clientY: 40 })
    fireEvent.pointerMove(surface, { pointerId: 7, clientX: 55, clientY: 65 })
    fireEvent.pointerUp(surface, { pointerId: 7, clientX: 58, clientY: 70 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(canvas.beginStroke).toHaveBeenCalledWith({
      kind: 'paint',
      page: 'page',
      revision: 1,
      layer: null,
      point: { x: 20, y: 20 },
      diameter: 48,
      color: [255, 255, 255, 255],
    })
    expect(canvas.extendStroke).toHaveBeenCalledWith(expect.arrayContaining([{ x: 45, y: 45 }]))
    expect(canvas.finishStroke).toHaveBeenCalledOnce()
    expect(commit).toHaveBeenCalledWith(
      'page',
      1,
      null,
      expect.arrayContaining([{ x: 45, y: 45 }]),
      {
        diameter: 48,
        color: [255, 255, 255, 255],
      },
    )
  })

  it('completes paint, move, rotation and text edits consecutively', async () => {
    installProject()
    useNorrixStore.setState({
      camera: { zoom: 1, translation: [0, 0], fitted: false },
      tool: 'draw',
    })
    const paint = vi
      .spyOn(commands, 'commitPaint')
      .mockResolvedValue({ revision: 2, layer: 'paint' })
    const transform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(3)
    const text = vi.spyOn(commands, 'addTextBox').mockResolvedValue({ revision: 4, layer: 'text' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 31, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 31, clientX: 30, clientY: 40 })
    await waitFor(() => expect(paint).toHaveBeenCalledOnce())

    act(() => useNorrixStore.setState({ tool: 'select' }))
    fireEvent.pointerDown(surface, { button: 0, pointerId: 32, clientX: 50, clientY: 60 })
    fireEvent.pointerMove(surface, { pointerId: 32, clientX: 60, clientY: 70 })
    fireEvent.pointerUp(surface, { pointerId: 32, clientX: 60, clientY: 70 })
    await waitFor(() => expect(transform).toHaveBeenCalledOnce())

    Object.defineProperty(screen.getByTestId('canvas-overlay'), 'getBoundingClientRect', {
      value: () => ({ x: 10, y: 20, width: 800, height: 600 }),
    })
    const rotate = document.querySelector<HTMLElement>('[data-rotate-handle]')!
    fireEvent.pointerDown(rotate, { button: 0, pointerId: 33, clientX: 60, clientY: 24 })
    fireEvent.pointerMove(rotate, { pointerId: 33, clientX: 80, clientY: 45 })
    fireEvent.pointerUp(rotate, { pointerId: 33, clientX: 80, clientY: 45 })
    await waitFor(() => expect(transform).toHaveBeenCalledTimes(2))

    act(() => useNorrixStore.setState({ tool: 'text' }))
    fireEvent.pointerDown(surface, { button: 0, pointerId: 34, clientX: 40, clientY: 50 })
    fireEvent.pointerMove(surface, { pointerId: 34, clientX: 100, clientY: 110 })
    fireEvent.pointerUp(surface, { pointerId: 34, clientX: 100, clientY: 110 })
    await waitFor(() => expect(text).toHaveBeenCalledOnce())

    expect(canvas.finishStroke).toHaveBeenCalledOnce()
    expect(canvas.finishTransform).toHaveBeenCalledTimes(2)
    expect(canvas.cancelStroke).not.toHaveBeenCalled()
    expect(canvas.cancelTransform).not.toHaveBeenCalled()
  })

  it('releases a failed stroke transaction and accepts the next paint gesture', async () => {
    installProject()
    useNorrixStore.setState({
      camera: { zoom: 1, translation: [0, 0], fitted: false },
      tool: 'draw',
    })
    canvas.beginStroke.mockImplementationOnce(() => {
      throw new Error('preview initialization failed')
    })
    canvas.finishStroke.mockImplementationOnce(() => {
      throw new Error('preview finalization failed')
    })
    const paint = vi
      .spyOn(commands, 'commitPaint')
      .mockResolvedValue({ revision: 2, layer: 'paint' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 35, clientX: 30, clientY: 40 })
    expect(canvas.cancelStroke).toHaveBeenCalledOnce()
    fireEvent.pointerDown(surface, { button: 0, pointerId: 36, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 36, clientX: 30, clientY: 40 })
    expect(canvas.cancelStroke).toHaveBeenCalledTimes(2)
    fireEvent.pointerDown(surface, { button: 0, pointerId: 37, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 37, clientX: 30, clientY: 40 })

    await waitFor(() => expect(paint).toHaveBeenCalledOnce())
    expect(canvas.beginStroke).toHaveBeenCalledTimes(3)
    expect(canvas.finishStroke).toHaveBeenCalledTimes(2)
  })

  it.each([
    ['revision', { canvasRevision: 2 }],
    ['generation', { canvasGeneration: 2 }],
  ])('cancels an active gesture when the canvas %s changes', async (_name, update) => {
    installProject()
    useNorrixStore.setState({ tool: 'draw', brush: { diameter: 48, color: '#FFFFFF' } })
    const commit = vi
      .spyOn(commands, 'commitPaint')
      .mockResolvedValue({ revision: 2, layer: 'paint' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 8, clientX: 30, clientY: 40 })
    expect(canvas.beginStroke).toHaveBeenCalledOnce()

    act(() => {
      useNorrixStore.setState(update)
    })

    await waitFor(() => expect(canvas.cancelStroke).toHaveBeenCalledOnce())
    fireEvent.pointerUp(surface, { pointerId: 8, clientX: 30, clientY: 40 })
    expect(canvas.finishStroke).not.toHaveBeenCalled()
    expect(commit).not.toHaveBeenCalled()
  })

  it('uses rendered text bounds for hit testing and semantic transforms', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [
        {
          type: 'text',
          id: 'element',
          parent: 'page',
          geometry: layer.geometry,
          angle_degrees: 0,
          visibility: { visible: true, opacity: 1 },
          content: {
            id: 'content',
            source: { text: 'Source', language: 'en' },
            translation: { text: 'Rendered', language: null },
            role: null,
            source_region: null,
          },
          typography: null,
          layout: 'paragraph',
          automatic_region: null,
        },
      ],
    }))
    useNorrixStore.setState({
      layerFrames: {
        element: { x: 30, y: 40, width: 50, height: 20, angle_degrees: 0 },
      },
    })
    const commit = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 9, clientX: 50, clientY: 60 })
    fireEvent.pointerMove(surface, { pointerId: 9, clientX: 70, clientY: 80 })
    fireEvent.pointerUp(surface, { pointerId: 9, clientX: 70, clientY: 80 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(useNorrixStore.getState().selectedLayers).toEqual(['element'])
    expect(canvas.beginTransform).toHaveBeenCalledWith('page', [
      {
        element: 'element',
        frame: { x: 30, y: 40, width: 50, height: 20, angle_degrees: 0 },
      },
    ])
    expect(canvas.updateTransform).toHaveBeenCalledWith(
      expect.arrayContaining([
        expect.objectContaining({
          element: 'element',
          frame: expect.objectContaining({ x: 50, y: 60 }),
        }),
      ]),
    )
    expect(canvas.finishTransform).toHaveBeenCalledOnce()
    expect(commit).toHaveBeenCalledWith(
      'page',
      1,
      expect.arrayContaining([
        expect.objectContaining({
          element: 'element',
          frame: expect.objectContaining({ x: 50, y: 60 }),
        }),
      ]),
    )
  })

  it.each(['single', 'infinite'] as const)(
    'updates selected rendered text continuously in %s mode',
    async (mode) => {
      const page = installEditableText(mode)
      const project = {
        name: 'Book',
        revision: 1,
        active_page: 'page',
        can_undo: false,
        can_redo: false,
      }
      const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
      vi.spyOn(commands, 'previewTextLayout').mockImplementation(
        async (_page, _element, text, revision) => textPreview(text, revision),
      )
      const undo = vi.spyOn(commands, 'undo').mockResolvedValue(null)
      const redo = vi.spyOn(commands, 'redo').mockResolvedValue(null)
      vi.spyOn(commands, 'getPage').mockResolvedValue(page)
      vi.spyOn(commands, 'getPages').mockResolvedValue([])
      vi.spyOn(commands, 'getProject').mockResolvedValue(project)
      canvas.previewOpacity.mockClear()
      const surface = renderWorkspace()
      if (mode === 'infinite') fireEvent(window, new Event('resize'))

      fireEvent.keyDown(surface, { key: 'Enter' })
      const editor = await screen.findByRole('textbox', { name: 'Edit text' })
      expect(editor).toHaveValue('Original translation')
      expect(editor).toHaveStyle({
        width: '100px',
        height: '50px',
        opacity: '0',
      })
      const previewSvg = screen.getByTestId('inline-text-preview')
      await waitFor(() =>
        expect(screen.getAllByTestId('inline-text-glyphs')[0]).toHaveAttribute(
          'd',
          'M1 1L8 1L8 12Z',
        ),
      )
      expect(previewSvg).toHaveAttribute('viewBox', '0 0 100 50')
      const textInput = editor as HTMLTextAreaElement
      textInput.setSelectionRange(1, 4)
      fireEvent.select(textInput)
      expect(screen.getAllByTestId('inline-text-selection').length).toBeGreaterThan(0)
      expect(fireEvent.copy(textInput)).toBe(true)
      expect(canvas.previewOpacity).toHaveBeenCalledWith('editable', 0)
      expect(canvas.beginTransform).not.toHaveBeenCalled()

      act(() =>
        useNorrixStore.setState({
          camera: { zoom: 2, translation: [12, 8], fitted: false },
        }),
      )
      expect(editor).toHaveStyle({ width: '200px', height: '100px' })

      fireEvent.change(editor, { target: { value: 'Edited line one\nEdited line two' } })
      fireEvent.keyDown(editor, { key: 'z', ctrlKey: true })
      expect(undo).not.toHaveBeenCalled()
      await waitFor(() =>
        expect(setTranslation).toHaveBeenCalledWith(
          'editable',
          'Edited line one\nEdited line two',
          null,
        ),
      )

      fireEvent.keyDown(editor, { key: 'Enter', ctrlKey: true })
      expect(screen.getByTestId('inline-text-editor')).toBeInTheDocument()
      expect(setTranslation).toHaveBeenCalledOnce()
      fireEvent.blur(editor)
      await waitFor(() =>
        expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument(),
      )
      expect(canvas.previewOpacity).toHaveBeenLastCalledWith('editable', null)
      fireEvent.keyDown(surface, { key: 'z', ctrlKey: true })
      fireEvent.keyDown(surface, { key: 'z', ctrlKey: true, shiftKey: true })
      expect(undo).toHaveBeenCalledOnce()
      expect(redo).toHaveBeenCalledOnce()
    },
  )

  it('maps mouse selection and caret geometry through the Rust preview clusters', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')
    const preview = screen.getByTestId('inline-text-preview')
    await waitFor(() => expect(screen.getAllByTestId('inline-text-glyphs')).not.toHaveLength(0))

    fireEvent.pointerDown(preview, { pointerId: 1, clientX: 10, clientY: 10 })
    fireEvent.pointerMove(preview, { pointerId: 1, clientX: 40, clientY: 10 })
    fireEvent.pointerUp(preview, { pointerId: 1, clientX: 40, clientY: 10 })

    expect(editor).toHaveProperty('selectionStart', 1)
    expect(editor).toHaveProperty('selectionEnd', 4)
    expect(screen.getAllByTestId('inline-text-selection').length).toBeGreaterThan(0)

    await userEvent.setup().keyboard('new')

    await waitFor(() => expect(editor).toHaveValue('Onewinal translation'))
    await waitFor(() =>
      expect(setTranslation).toHaveBeenLastCalledWith('editable', 'Onewinal translation', null),
    )
  })

  it('replaces selected text when typing or pasting into the native editor', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = (await screen.findByTestId('inline-text-editor')) as HTMLTextAreaElement
    const user = userEvent.setup()

    editor.focus()
    editor.setSelectionRange(1, 4)
    await user.keyboard('new')

    await waitFor(() => expect(editor).toHaveValue('Onewinal translation'))
    await waitFor(() =>
      expect(setTranslation).toHaveBeenCalledWith('editable', 'Onewinal translation', null),
    )

    editor.setSelectionRange(1, 4)
    await user.paste('paste')
    await waitFor(() => expect(editor).toHaveValue('Opasteinal translation'))
    await waitFor(() =>
      expect(setTranslation).toHaveBeenLastCalledWith('editable', 'Opasteinal translation', null),
    )
  })

  it('preserves reverse selections and direction while typing', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = (await screen.findByTestId('inline-text-editor')) as HTMLTextAreaElement

    fireEvent.change(editor, { target: { value: 'abcde' } })
    editor.focus()
    editor.setSelectionRange(2, 5, 'backward')
    expect(editor.selectionDirection).toBe('backward')
    expect(editor.selectionStart).toBe(2)
    expect(editor.selectionEnd).toBe(5)

    fireEvent.change(editor, { target: { value: 'abXe' } })
    await waitFor(() => expect(editor).toHaveValue('abXe'))
    await waitFor(() => expect(setTranslation).toHaveBeenLastCalledWith('editable', 'abXe', null))
  })

  it('keeps the active editor, focus, and caret through canvas generation transitions', async () => {
    installEditableText()
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = (await screen.findByTestId('inline-text-editor')) as HTMLTextAreaElement

    editor.focus()
    editor.setSelectionRange(8, 8)
    fireEvent.select(editor)
    transitionCanvasGeneration(2)

    expect(screen.getByTestId('inline-text-editor')).toBe(editor)
    expect(editor).toHaveFocus()
    expect(editor.selectionStart).toBe(8)
    expect(editor.selectionEnd).toBe(8)
  })

  it('retains backward selection direction across canvas updates', async () => {
    installEditableText()
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = (await screen.findByTestId('inline-text-editor')) as HTMLTextAreaElement

    editor.focus()
    editor.setSelectionRange(3, 8, 'backward')
    expect(editor.selectionDirection).toBe('backward')
    expect(editor.selectionStart).toBe(3)
    expect(editor.selectionEnd).toBe(8)

    transitionCanvasGeneration(2)

    expect(screen.getByTestId('inline-text-editor')).toBe(editor)
    expect(editor.selectionDirection).toBe('backward')
    expect(editor.selectionStart).toBe(3)
    expect(editor.selectionEnd).toBe(8)
  })

  it('replaces all and partial selections without restoring stale text after frame updates', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = (await screen.findByTestId('inline-text-editor')) as HTMLTextAreaElement
    const user = userEvent.setup()

    editor.focus()
    await user.keyboard('{Control>}a{/Control}new')
    await waitFor(() => expect(editor).toHaveValue('new'))
    transitionCanvasGeneration(2)
    expect(screen.getByTestId('inline-text-editor')).toBe(editor)
    expect(editor).toHaveFocus()

    editor.setSelectionRange(1, 2)
    await user.keyboard('X')
    await waitFor(() => expect(editor).toHaveValue('nXw'))
    transitionCanvasGeneration(3)

    editor.setSelectionRange(1, 2)
    await user.paste('P')
    await waitFor(() => expect(editor).toHaveValue('nPw'))
    transitionCanvasGeneration(4)

    expect(editor).toHaveValue('nPw')
    expect(setTranslation).toHaveBeenLastCalledWith('editable', 'nPw', null)
  })

  it('preserves insertion and deletion caret positions during canvas updates', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = (await screen.findByTestId('inline-text-editor')) as HTMLTextAreaElement
    const user = userEvent.setup()

    editor.focus()
    editor.setSelectionRange(0, 0)
    await user.keyboard('abc')
    await waitFor(() => expect(editor).toHaveValue('abcOriginal translation'))
    expect(editor.selectionStart).toBe(3)
    transitionCanvasGeneration(2)
    expect(editor.selectionStart).toBe(3)

    editor.setSelectionRange(5, 5)
    await user.keyboard('XYZ')
    await waitFor(() => expect(editor).toHaveValue('abcOrXYZiginal translation'))
    expect(editor.selectionStart).toBe(8)
    transitionCanvasGeneration(3)
    expect(editor.selectionStart).toBe(8)

    editor.setSelectionRange(editor.value.length, editor.value.length)
    await user.keyboard('!')
    await waitFor(() => expect(editor).toHaveValue('abcOrXYZiginal translation!'))
    expect(editor.selectionStart).toBe(editor.value.length)
    await user.keyboard('{Backspace}')
    await waitFor(() => expect(editor).toHaveValue('abcOrXYZiginal translation'))

    editor.setSelectionRange(3, 3)
    await user.keyboard('{Delete}')
    await waitFor(() => expect(editor).toHaveValue('abcrXYZiginal translation'))
    expect(editor.selectionStart).toBe(3)
    transitionCanvasGeneration(4)

    expect(editor).toHaveFocus()
    expect(editor.selectionStart).toBe(3)
    expect(setTranslation).toHaveBeenLastCalledWith('editable', 'abcrXYZiginal translation', null)
  })

  it('ends inline editing when the selected page or project changes', async () => {
    installEditableText()
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')

    act(() => useNorrixStore.setState({ selectedPage: 'another-page' }))
    await waitFor(() => expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument())

    act(() => useNorrixStore.setState({ selectedPage: 'page' }))
    fireEvent.keyDown(surface, { key: 'Enter' })
    await screen.findByTestId('inline-text-editor')
    act(() =>
      queryClient.setQueryData(projectKey, {
        name: 'Another book',
        revision: 1,
        active_page: 'page',
        can_undo: false,
        can_redo: false,
      }),
    )

    await waitFor(() => expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument())
    expect(editor).not.toBeInTheDocument()
  })

  it('still ends inline editing on deliberate blur during a canvas update', async () => {
    installEditableText()
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')

    act(() => {
      canvasState.status = 'switching'
      useNorrixStore.setState({ canvasGeneration: 2 })
    })
    expect(screen.getByTestId('inline-text-editor')).toBe(editor)

    const otherControl = document.createElement('input')
    document.body.append(otherControl)
    otherControl.focus()
    fireEvent.blur(editor)

    await waitFor(() => expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument())
    otherControl.remove()
  })

  it('persists IME composition changes without a confirmation step', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')

    fireEvent.compositionStart(editor)
    fireEvent.keyDown(editor, { key: 'Enter', ctrlKey: true, isComposing: true })
    fireEvent.change(editor, { target: { value: 'composed draft' } })
    await waitFor(() =>
      expect(setTranslation).toHaveBeenCalledWith('editable', 'composed draft', null),
    )
    fireEvent.compositionEnd(editor, { data: ' composed' })
    fireEvent.keyDown(editor, { key: 'Enter', ctrlKey: true })

    expect(screen.getByTestId('inline-text-editor')).toBeInTheDocument()
  })

  it('serializes rapid text changes in order without losing the latest value', async () => {
    installEditableText()
    const writes: string[] = []
    const setTranslation = vi
      .spyOn(commands, 'setTranslation')
      .mockImplementation(async (_layer, text) => {
        writes.push(text ?? '')
        return null
      })
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')

    fireEvent.change(editor, { target: { value: 'A' } })
    fireEvent.change(editor, { target: { value: 'Ab' } })
    fireEvent.change(editor, { target: { value: 'Abc' } })

    await waitFor(() => expect(writes).toEqual(['A', 'Ab', 'Abc']))
    expect(setTranslation).toHaveBeenCalledTimes(3)
    expect(editor).toHaveValue('Abc')
  })

  it('does not require Cmd+Enter to apply text edits', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')
    fireEvent.change(editor, { target: { value: 'Confirmed with Command' } })
    fireEvent.keyDown(editor, { key: 'Enter', metaKey: true })

    await waitFor(() =>
      expect(setTranslation).toHaveBeenCalledWith('editable', 'Confirmed with Command', null),
    )
    expect(screen.getByTestId('inline-text-editor')).toBeInTheDocument()
    fireEvent.blur(editor)
    await waitFor(() => expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument())
    expect(canvas.previewOpacity).toHaveBeenLastCalledWith('editable', null)
  })

  it('persists text immediately and preserves the editor across window focus changes', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')
    fireEvent.change(editor, { target: { value: 'Keep this draft' } })

    fireEvent(window, new Event('blur'))
    fireEvent.blur(editor)

    expect(screen.getByTestId('inline-text-editor')).toHaveValue('Keep this draft')
    await waitFor(() =>
      expect(setTranslation).toHaveBeenCalledWith('editable', 'Keep this draft', null),
    )
    expect(canvas.previewOpacity).toHaveBeenLastCalledWith('editable', 0)
  })

  it('restores editor focus after the window regains focus when the editor owned focus', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')
    fireEvent.change(editor, { target: { value: 'Keep this draft' } })

    fireEvent(window, new Event('blur'))
    editor.blur()
    fireEvent(window, new Event('focus'))

    await waitFor(() => expect(editor).toHaveFocus())
    expect(editor).toHaveValue('Keep this draft')
    await waitFor(() =>
      expect(setTranslation).toHaveBeenCalledWith('editable', 'Keep this draft', null),
    )

    fireEvent.keyDown(editor, { key: 'Escape' })
    expect(screen.getByTestId('inline-text-editor')).toBeInTheDocument()
  })

  it('ends editing when focus is deliberately moved to another control', async () => {
    installEditableText()
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')
    const otherControl = document.createElement('input')
    document.body.append(otherControl)
    otherControl.focus()
    fireEvent.blur(editor)

    fireEvent(window, new Event('blur'))
    fireEvent(window, new Event('focus'))

    expect(otherControl).toHaveFocus()
    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
    otherControl.remove()
  })

  it('does not steal focus from a control selected while the window regains focus', async () => {
    installEditableText()
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')
    const otherControl = document.createElement('input')
    document.body.append(otherControl)

    fireEvent(window, new Event('blur'))
    editor.blur()
    fireEvent(window, new Event('focus'))
    otherControl.focus()

    await waitFor(() => expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument())
    expect(otherControl).toHaveFocus()
    otherControl.remove()
  })

  it('does not require Ctrl+Enter after restoring editor focus', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')

    fireEvent(window, new Event('blur'))
    editor.blur()
    fireEvent(window, new Event('focus'))

    await waitFor(() => expect(editor).toHaveFocus())
    fireEvent.keyDown(editor, { key: 'Enter', ctrlKey: true })

    expect(screen.getByTestId('inline-text-editor')).toBeInTheDocument()
    expect(setTranslation).not.toHaveBeenCalled()
  })

  it('persists IME input while the window focus changes', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')
    fireEvent.compositionStart(editor)
    fireEvent(window, new Event('blur'))
    fireEvent.change(editor, { target: { value: 'Composed draft' } })
    fireEvent.blur(editor)

    expect(screen.getByTestId('inline-text-editor')).toHaveValue('Composed draft')
    await waitFor(() =>
      expect(setTranslation).toHaveBeenCalledWith('editable', 'Composed draft', null),
    )

    fireEvent.compositionEnd(editor, { data: 'Composed draft' })
    fireEvent.keyDown(editor, { key: 'Enter', metaKey: true })
    expect(screen.getByTestId('inline-text-editor')).toBeInTheDocument()
  })

  it('keeps text visible when persistence fails', async () => {
    installEditableText()
    const setTranslation = vi
      .spyOn(commands, 'setTranslation')
      .mockRejectedValue(new Error('save failed'))
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')
    fireEvent.change(editor, { target: { value: 'Unsaved draft' } })

    await waitFor(() => expect(setTranslation).toHaveBeenCalledOnce())
    expect(screen.getByTestId('inline-text-editor')).toHaveValue('Unsaved draft')
    expect(canvas.previewOpacity).toHaveBeenLastCalledWith('editable', 0)
  })

  it('enters text editing on a second click while preserving single-click selection', async () => {
    installEditableText()
    useNorrixStore.setState({
      selectedLayers: [],
      selectedLayersByPage: { page: [] },
    })
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    const pointer = { button: 0, pointerId: 81, clientX: 30, clientY: 50 }

    fireEvent.pointerDown(surface, { ...pointer, detail: 1 })
    fireEvent.pointerUp(surface, pointer)
    await waitFor(() => expect(useNorrixStore.getState().selectedLayers).toEqual(['editable']))
    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
    expect(commitTransform).toHaveBeenCalledOnce()

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 84, detail: 1 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 84, detail: 1 })
    expect(await screen.findByTestId('inline-text-editor')).toBeInTheDocument()
  })

  it('selects and transforms an empty text box', async () => {
    installEmptyEditableText()
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, {
      button: 0,
      pointerId: 99,
      clientX: 30,
      clientY: 50,
      detail: 1,
    })
    fireEvent.pointerUp(surface, { pointerId: 99, clientX: 30, clientY: 50, detail: 1 })

    await waitFor(() => expect(commitTransform).toHaveBeenCalledOnce())
    expect(canvas.beginTransform).toHaveBeenCalledWith('page', [
      {
        element: 'editable',
        frame: { x: 10, y: 20, width: 100, height: 50, angle_degrees: 0 },
      },
    ])
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('opens and accepts text in an initially empty text box by double-clicking', async () => {
    installEmptyEditableText()
    useNorrixStore.setState({
      selectedLayers: [],
      selectedLayersByPage: { page: [] },
    })
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    const pointer = { button: 0, clientX: 30, clientY: 50 }

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 100, detail: 1 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 100, detail: 1 })
    await waitFor(() => expect(commitTransform).toHaveBeenCalledOnce())
    fireEvent.pointerDown(surface, { ...pointer, pointerId: 101, detail: 1 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 101, detail: 1 })

    const editor = await screen.findByTestId('inline-text-editor')
    expect(editor).toHaveValue('')
    fireEvent.change(editor, { target: { value: 'New text' } })
    await waitFor(() => expect(setTranslation).toHaveBeenCalledWith('editable', 'New text', null))
    expect(editor).toHaveValue('New text')
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('opens an empty text box after its prepared canvas state updates', async () => {
    installEmptyEditableText()
    act(() => {
      useNorrixStore.setState({
        canvasRevision: 2,
        canvasGeneration: 2,
        layerFrames: {
          editable: { x: 10, y: 20, width: 100, height: 50, angle_degrees: 0 },
        },
      })
    })
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()

    fireEvent.keyDown(surface, { key: 'Enter' })

    expect(await screen.findByTestId('inline-text-editor')).toHaveValue('')
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('keeps a recognized second click when the canvas revision changes before pointerup', async () => {
    installEditableText()
    useNorrixStore.setState({
      selectedLayers: [],
      selectedLayersByPage: { page: [] },
    })
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    const pointer = { button: 0, clientX: 30, clientY: 50 }

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 89, detail: 1 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 89, detail: 1 })
    await waitFor(() => expect(commitTransform).toHaveBeenCalledOnce())
    await waitFor(() => expect(useNorrixStore.getState().selectedLayers).toEqual(['editable']))

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 90, detail: 1 })
    act(() => {
      useNorrixStore.setState({ canvasRevision: 2, canvasGeneration: 2 })
    })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 90, detail: 1 })

    expect(await screen.findByTestId('inline-text-editor')).toBeInTheDocument()
    expect(canvas.previewOpacity).toHaveBeenCalledWith('editable', 0)
  })

  it('opens a second-click edit after the switching canvas becomes ready', async () => {
    installEditableText()
    useNorrixStore.setState({
      selectedLayers: [],
      selectedLayersByPage: { page: [] },
    })
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = await clickTextAndStartCanvasSwitch(surface)
    clickTextDuringCanvasSwitch(surface, pointer)
    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
    expect(commitTransform).toHaveBeenCalledOnce()

    transitionCanvasGeneration(2)

    expect(await screen.findByTestId('inline-text-editor')).toBeInTheDocument()
    expect(canvas.previewOpacity).toHaveBeenCalledWith('editable', 0)
    expect(commitTransform).toHaveBeenCalledOnce()
  })

  it('invalidates a pending text edit when the page changes', async () => {
    installEditableText()
    useNorrixStore.setState({ selectedLayers: [], selectedLayersByPage: { page: [] } })
    vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = await clickTextAndStartCanvasSwitch(surface)
    clickTextDuringCanvasSwitch(surface, pointer)

    act(() => useNorrixStore.setState({ selectedPage: 'another-page' }))
    transitionCanvasGeneration(2)

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('invalidates a pending text edit when the project changes', async () => {
    installEditableText()
    useNorrixStore.setState({ selectedLayers: [], selectedLayersByPage: { page: [] } })
    vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = await clickTextAndStartCanvasSwitch(surface)
    clickTextDuringCanvasSwitch(surface, pointer)

    act(() =>
      queryClient.setQueryData(projectKey, {
        name: 'Another project',
        revision: 1,
        active_page: 'page',
        can_undo: false,
        can_redo: false,
      }),
    )
    transitionCanvasGeneration(2)

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('invalidates a pending text edit when the tool changes', async () => {
    installEditableText()
    useNorrixStore.setState({ selectedLayers: [], selectedLayersByPage: { page: [] } })
    vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = await clickTextAndStartCanvasSwitch(surface)
    clickTextDuringCanvasSwitch(surface, pointer)

    act(() => useNorrixStore.setState({ tool: 'text' }))
    transitionCanvasGeneration(2)

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('invalidates a pending text edit when the workspace mode changes', async () => {
    installEditableText()
    useNorrixStore.setState({ selectedLayers: [], selectedLayersByPage: { page: [] } })
    vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = await clickTextAndStartCanvasSwitch(surface)
    clickTextDuringCanvasSwitch(surface, pointer)

    act(() => useNorrixStore.setState({ workspaceMode: 'infinite' }))
    transitionCanvasGeneration(2)

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('invalidates a pending text edit when its layer is removed', async () => {
    const page = installEditableText()
    useNorrixStore.setState({ selectedLayers: [], selectedLayersByPage: { page: [] } })
    vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = await clickTextAndStartCanvasSwitch(surface)
    clickTextDuringCanvasSwitch(surface, pointer)

    act(() => queryClient.setQueryData(pageKey, { ...page, layers: [layer] }))
    transitionCanvasGeneration(2)

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('does not open a pending text edit when the second click is cancelled', async () => {
    installEditableText()
    useNorrixStore.setState({ selectedLayers: [], selectedLayersByPage: { page: [] } })
    vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = await clickTextAndStartCanvasSwitch(surface)

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 203, detail: 1 })
    fireEvent.pointerCancel(surface, { pointerId: 203 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 203, detail: 1 })
    transitionCanvasGeneration(2)

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('does not open a pending text edit after the second click moves more than four pixels', async () => {
    installEditableText()
    useNorrixStore.setState({ selectedLayers: [], selectedLayersByPage: { page: [] } })
    vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = await clickTextAndStartCanvasSwitch(surface)

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 204, detail: 1 })
    fireEvent.pointerMove(surface, { pointerId: 204, clientX: 35, clientY: 50 })
    fireEvent.pointerUp(surface, {
      ...pointer,
      pointerId: 204,
      clientX: 35,
      clientY: 50,
      detail: 1,
    })
    transitionCanvasGeneration(2)

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('rejects a second click that only exceeds the movement threshold on pointerup', async () => {
    installEditableText()
    useNorrixStore.setState({ selectedLayers: [], selectedLayersByPage: { page: [] } })
    vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = await clickTextAndStartCanvasSwitch(surface)

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 206, detail: 1 })
    fireEvent.pointerUp(surface, {
      ...pointer,
      pointerId: 206,
      clientX: pointer.clientX + 5,
      detail: 1,
    })
    transitionCanvasGeneration(2)

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('does not open a pending text edit after window blur', async () => {
    installEditableText()
    useNorrixStore.setState({ selectedLayers: [], selectedLayersByPage: { page: [] } })
    vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = await clickTextAndStartCanvasSwitch(surface)

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 205, detail: 1 })
    fireEvent(window, new Event('blur'))
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 205, detail: 1 })
    transitionCanvasGeneration(2)

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('invalidates a pending second click on pointercancel', async () => {
    installEditableText()
    useNorrixStore.setState({
      selectedLayers: [],
      selectedLayersByPage: { page: [] },
    })
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = { button: 0, clientX: 30, clientY: 50 }

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 93, detail: 1 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 93, detail: 1 })
    await waitFor(() => expect(commitTransform).toHaveBeenCalledOnce())

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 94, detail: 1 })
    fireEvent.pointerCancel(surface, { pointerId: 94 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 94, detail: 1 })

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
    expect(canvas.cancelTransform).toHaveBeenCalled()
  })

  it('does not recognize a second click after movement exceeds four CSS pixels', async () => {
    installEditableText()
    useNorrixStore.setState({
      selectedLayers: [],
      selectedLayersByPage: { page: [] },
    })
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = { button: 0, clientX: 30, clientY: 50 }

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 95, detail: 1 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 95, detail: 1 })
    await waitFor(() => expect(commitTransform).toHaveBeenCalledOnce())
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 96, detail: 1 })
    fireEvent.pointerMove(surface, { pointerId: 96, clientX: 35, clientY: 50 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 96, clientX: 35, clientY: 50, detail: 1 })

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('invalidates the pending second click on window blur', async () => {
    installEditableText()
    useNorrixStore.setState({
      selectedLayers: [],
      selectedLayersByPage: { page: [] },
    })
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = { button: 0, clientX: 30, clientY: 50 }

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 97, detail: 1 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 97, detail: 1 })
    await waitFor(() => expect(commitTransform).toHaveBeenCalledOnce())
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })

    fireEvent(window, new Event('blur'))
    fireEvent.pointerDown(surface, { ...pointer, pointerId: 98, detail: 1 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 98, detail: 1 })

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('preserves the second-click candidate when the first click advances the canvas revision', async () => {
    installEditableText()
    useNorrixStore.setState({
      selectedLayers: [],
      selectedLayersByPage: { page: [] },
    })
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    const pointer = { button: 0, clientX: 30, clientY: 50 }

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 87, detail: 1 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 87 })
    await waitFor(() => expect(commitTransform).toHaveBeenCalledOnce())

    act(() => {
      useNorrixStore.setState({ canvasRevision: 2, canvasGeneration: 2 })
    })

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 88, detail: 1 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 88 })
    expect(await screen.findByTestId('inline-text-editor')).toBeInTheDocument()
  })

  it('keeps a second-click drag as a move instead of entering text editing', async () => {
    installEditableText()
    const commitTransform = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    const surface = renderWorkspace()
    const pointer = { button: 0, pointerId: 85, clientX: 30, clientY: 50 }

    fireEvent.pointerDown(surface, { ...pointer, detail: 1 })
    fireEvent.pointerUp(surface, pointer)
    await waitFor(() => expect(commitTransform).toHaveBeenCalledOnce())

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 86, detail: 2 })
    fireEvent.pointerMove(surface, { pointerId: 86, clientX: 40, clientY: 60 })
    fireEvent.pointerUp(surface, { pointerId: 86, clientX: 40, clientY: 60 })

    await waitFor(() => expect(commitTransform).toHaveBeenCalledTimes(2))
    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
  })

  it('waits for the click transform to finish before entering text editing on double-click', async () => {
    installEditableText()
    useNorrixStore.setState({
      selectedLayers: [],
      selectedLayersByPage: { page: [] },
    })
    let finishTransform!: (revision: number) => void
    vi.spyOn(commands, 'commitTransform').mockImplementation(
      () =>
        new Promise((resolve) => {
          finishTransform = resolve
        }),
    )
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, text, revision) => textPreview(text, revision),
    )
    const surface = renderWorkspace()
    const pointer = { button: 0, pointerId: 82, clientX: 30, clientY: 50 }
    fireEvent.pointerDown(surface, { ...pointer, detail: 1 })
    fireEvent.pointerUp(surface, pointer)
    await waitFor(() => expect(finishTransform).toBeTypeOf('function'))

    fireEvent.pointerDown(surface, { ...pointer, pointerId: 83, detail: 2 })
    fireEvent.pointerUp(surface, { ...pointer, pointerId: 83, detail: 2 })
    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()

    await act(async () => finishTransform(2))
    expect(await screen.findByTestId('inline-text-editor')).toBeInTheDocument()
  })

  it.each(['single', 'infinite'] as const)(
    'updates editor layout while persisting text in %s mode',
    async (mode) => {
      const page = installEditableText(mode)
      if (mode === 'infinite') {
        queryClient.setQueryData(pagesKey, [
          {
            id: 'page',
            label: 'Page',
            size: { width: 1000, height: 1000 },
            source_asset: null,
            layer_count: 2,
          },
        ])
      }
      const preview = vi
        .spyOn(commands, 'previewTextLayout')
        .mockImplementation(async (_page, _element, text, requestRevision) => ({
          ...textPreview(text, requestRevision),
          font_size: text.length > 10 ? 13 : 20,
          line_breaks: [text.length],
        }))
      const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
      const surface = renderWorkspace()
      if (mode === 'infinite') fireEvent(window, new Event('resize'))
      fireEvent.keyDown(surface, { key: 'Enter' })
      const editor = await screen.findByTestId('inline-text-editor')
      fireEvent.change(editor, { target: { value: 'A much longer\npreview line' } })

      await waitFor(() =>
        expect(preview).toHaveBeenCalledWith(
          'page',
          'editable',
          'A much longer\npreview line',
          expect.any(Number),
        ),
      )
      await waitFor(() =>
        expect(setTranslation).toHaveBeenCalledWith(
          'editable',
          'A much longer\npreview line',
          null,
        ),
      )
      await waitFor(() => {
        expect(editor).toHaveAttribute('data-preview-font-size', '13')
        expect(editor).toHaveAttribute('data-preview-width', '70')
        expect(editor).toHaveAttribute(
          'data-preview-line-breaks',
          String('A much longer\npreview line'.length),
        )
        expect(screen.getAllByTestId('inline-text-glyphs')[0]).toHaveAttribute(
          'd',
          'M1 1L8 1L8 12Z',
        )
      })
      expect(page.layers.find((candidate) => candidate.id === 'editable')).toMatchObject({
        content: { translation: { text: 'Original translation' } },
        typography: { size: 18, auto_fit: false },
      })
    },
  )

  it('ignores stale layout preview responses while continuing to persist input', async () => {
    installEditableText()
    const pending: Array<{
      text: string
      revision: number
      resolve: (result: TextLayoutPreview) => void
    }> = []
    const preview = vi
      .spyOn(commands, 'previewTextLayout')
      .mockImplementation(
        (_page, _element, text, revision) =>
          new Promise((resolve) => pending.push({ text, revision, resolve })),
      )
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')

    await waitFor(() => expect(pending).toHaveLength(1))
    await act(async () =>
      pending[0].resolve({
        ...textPreview(pending[0].text, pending[0].revision),
        width: 100,
        height: 50,
        line_height: 22,
      }),
    )

    fireEvent.change(editor, { target: { value: 'older draft' } })
    await waitFor(() => expect(pending).toHaveLength(2))
    const older = pending[1]
    fireEvent.change(editor, { target: { value: 'latest draft' } })
    await waitFor(() => expect(pending).toHaveLength(3))
    const latest = pending[2]
    await act(async () =>
      latest.resolve({
        ...textPreview(latest.text, latest.revision),
        font_size: 12,
        width: 72,
        height: 30,
        line_height: 15,
      }),
    )
    expect(editor).toHaveAttribute('data-preview-font-size', '12')
    await act(async () =>
      older.resolve({
        ...textPreview(older.text, older.revision),
        font_size: 8,
        width: 30,
        height: 20,
        line_height: 10,
      }),
    )
    expect(editor).toHaveAttribute('data-preview-font-size', '12')

    fireEvent.change(editor, { target: { value: 'cancel this preview' } })
    fireEvent.keyDown(editor, { key: 'Escape' })
    await new Promise((resolve) => window.setTimeout(resolve, 150))
    expect(preview).toHaveBeenCalledTimes(4)
    expect(setTranslation).toHaveBeenLastCalledWith('editable', 'cancel this preview', null)
    expect(screen.getByTestId('inline-text-editor')).toHaveValue('cancel this preview')
  })

  it('falls back to saved typography when preview fails but still applies text edits', async () => {
    installEditableText()
    const preview = vi
      .spyOn(commands, 'previewTextLayout')
      .mockRejectedValue(new Error('preview unavailable'))
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')
    fireEvent.change(editor, { target: { value: 'Can still save this' } })

    await waitFor(() => expect(preview).toHaveBeenCalledOnce(), { timeout: 1000 })
    expect(editor).toHaveStyle({ opacity: '1' })
    expect(screen.queryByTestId('inline-text-glyphs')).not.toBeInTheDocument()
    await waitFor(() =>
      expect(setTranslation).toHaveBeenCalledWith('editable', 'Can still save this', null),
    )
  })

  it('edits source text when no translation exists', async () => {
    const page = installEditableText('single', false)
    const setSourceText = vi.spyOn(commands, 'setSourceText').mockResolvedValue(null)
    const setTranslation = vi.spyOn(commands, 'setTranslation')
    vi.spyOn(commands, 'getPage').mockResolvedValue(page)
    vi.spyOn(commands, 'getPages').mockResolvedValue([])
    vi.spyOn(commands, 'getProject').mockResolvedValue({
      name: 'Book',
      revision: 1,
      active_page: 'page',
      can_undo: false,
      can_redo: false,
    })
    const surface = renderWorkspace()

    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByRole('textbox', { name: 'Edit text' })
    expect(editor).toHaveValue('Source text')
    fireEvent.change(editor, { target: { value: 'Edited source' } })

    await waitFor(() =>
      expect(setSourceText).toHaveBeenCalledWith('editable', 'Edited source', null),
    )
    expect(setTranslation).not.toHaveBeenCalled()
  })

  it('uses the renderer geometry when previewed text metrics are available', async () => {
    const page = installEditableText('single')
    const text = page.layers.find((candidate) => candidate.id === 'editable')
    if (!text || text.type !== 'text') throw new Error('editable text fixture is missing')
    text.typography = {
      ...text.typography!,
      auto_fit: false,
      size: 18,
      line_height: 2,
      letter_spacing: 2.5,
    }
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, value, revision) => ({
        ...textPreview(value, revision),
        font_size: 11.25,
        line_height: 15,
      }),
    )
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByTestId('inline-text-editor')
    await waitFor(() => expect(editor).toHaveAttribute('data-preview-font-size', '11.25'))
    expect((editor as HTMLTextAreaElement).style.fontSize).toBe('11.25px')
    expect((editor as HTMLTextAreaElement).style.lineHeight).toBe('15px')
    expect((editor as HTMLTextAreaElement).style.letterSpacing).toBe('2.5px')
  })

  it('keeps the textarea box model aligned with the source frame geometry', async () => {
    const page = installEditableText('single')
    const editable = page.layers.find((candidate) => candidate.id === 'editable')
    if (!editable || editable.type !== 'text') throw new Error('editable text fixture is missing')
    const frame = { x: 14, y: 18, width: 120, height: 60, angle_degrees: 8 }
    editable.angle_degrees = frame.angle_degrees
    editable.geometry = { points: geometryPoints(frame) }
    useNorrixStore.setState({
      selectedLayers: ['editable'],
      selectedPage: 'page',
      selectedLayersByPage: { page: ['editable'] },
      layerFrames: { editable: frame },
      camera: { zoom: 1.5, translation: [12, 8], fitted: false },
    })
    vi.spyOn(commands, 'previewTextLayout').mockImplementation(
      async (_page, _element, value, revision) => textPreview(value, revision),
    )
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = (await screen.findByTestId('inline-text-editor')) as HTMLTextAreaElement
    const expected = cssFrame(frame, useNorrixStore.getState().camera)
    expect(Number.parseFloat(editor.style.left)).toBeCloseTo(expected.left, 5)
    expect(Number.parseFloat(editor.style.top)).toBeCloseTo(expected.top, 5)
    expect(Number.parseFloat(editor.style.width)).toBeCloseTo(expected.width, 5)
    expect(Number.parseFloat(editor.style.height)).toBeCloseTo(expected.height, 5)
    expect(editor.style.transform).toBe(`rotate(${expected.angle}deg)`)
    expect(editor.style.margin).toBe('0px')
  })

  it('uses the text layout frame instead of the renderer glyph bounds for editing', async () => {
    const page = installEditableText('single')
    const editable = page.layers.find((candidate) => candidate.id === 'editable')
    if (!editable || editable.type !== 'text') throw new Error('editable text fixture is missing')
    const layoutFrame = { x: 14, y: 18, width: 120, height: 60, angle_degrees: 8 }
    const renderedGlyphFrame = {
      x: 36,
      y: 39,
      width: 72,
      height: 20,
      angle_degrees: 8,
    }
    editable.angle_degrees = layoutFrame.angle_degrees
    editable.geometry = { points: geometryPoints(layoutFrame) }
    useNorrixStore.setState({
      selectedLayers: ['editable'],
      selectedPage: 'page',
      selectedLayersByPage: { page: ['editable'] },
      layerFrames: { editable: renderedGlyphFrame },
      camera: { zoom: 1.5, translation: [12, 8], fitted: false },
    })
    const surface = renderWorkspace()
    fireEvent.keyDown(surface, { key: 'Enter' })

    const editor = (await screen.findByTestId('inline-text-editor')) as HTMLTextAreaElement
    const expectedEditor = cssFrame(layoutFrame, useNorrixStore.getState().camera)
    const glyphFrame = screen
      .getByTestId('canvas-overlay')
      .querySelector<HTMLElement>('[data-element="editable"]')
    const expectedGlyph = cssFrame(renderedGlyphFrame, useNorrixStore.getState().camera)

    expect(Number.parseFloat(editor.style.left)).toBeCloseTo(expectedEditor.left, 5)
    expect(Number.parseFloat(editor.style.top)).toBeCloseTo(expectedEditor.top, 5)
    expect(Number.parseFloat(editor.style.width)).toBeCloseTo(expectedEditor.width, 5)
    expect(Number.parseFloat(editor.style.height)).toBeCloseTo(expectedEditor.height, 5)
    expect(editor.style.transform).toBe(`rotate(${expectedEditor.angle}deg)`)
    expect(glyphFrame?.style.left).toBe(`${expectedGlyph.left}px`)
    expect(glyphFrame?.style.top).toBe(`${expectedGlyph.top}px`)
    expect(editor.style.left).not.toBe(glyphFrame?.style.left)
  })

  it.each([
    ['single', true],
    ['single', false],
    ['infinite', true],
    ['infinite', false],
  ] as const)(
    'uses the renderer-computed font size in %s mode with auto_fit=%s',
    async (mode, autoFit) => {
      const page = installEditableText(mode)
      const text = page.layers.find((candidate) => candidate.id === 'editable')
      if (!text || text.type !== 'text') throw new Error('editable text fixture is missing')
      text.typography = { ...text.typography!, auto_fit: autoFit, size: 18 }
      vi.spyOn(commands, 'previewTextLayout').mockImplementation(
        async (_page, _element, value, revision) => ({
          ...textPreview(value, revision),
          font_size: 11.25,
        }),
      )
      queryClient.setQueryData(pageKey, page)
      const prepared = queryClient.getQueryData<{
        revision: number
        page: typeof page
        element_frames: Array<{
          element: string
          frame: { x: number; y: number; width: number; height: number; angle_degrees: number }
        }>
        text_font_sizes: Array<{ element: string; font_size: number }>
      }>(preparedPageKey('page'))
      if (!prepared) throw new Error('prepared page fixture is missing')
      queryClient.setQueryData(preparedPageKey('page'), {
        ...prepared,
        text_font_sizes: [{ element: 'editable', font_size: 11.25 }],
      })
      useNorrixStore.setState({
        layerFontSizes: { editable: 11.25 },
        camera: { zoom: 2, translation: [12, 8], fitted: false },
      })
      if (mode === 'infinite') {
        queryClient.setQueryData(pagesKey, [
          {
            id: 'page',
            label: 'Page',
            size: { width: 1000, height: 1000 },
            source_asset: null,
            layer_count: 2,
          },
        ])
      }
      const surface = renderWorkspace()
      if (mode === 'infinite') fireEvent(window, new Event('resize'))

      fireEvent.keyDown(surface, { key: 'Enter' })
      const editor = await screen.findByTestId('inline-text-editor')
      await waitFor(() => expect(editor).toHaveAttribute('data-preview-font-size', '11.25'))
      expect(editor).toHaveStyle({ width: '200px', height: '100px', opacity: '0' })
      expect(screen.getAllByTestId('inline-text-glyphs')[0]).toHaveAttribute('d', 'M1 1L8 1L8 12Z')
      expect(text.typography?.size).toBe(18)
      expect(text.typography?.auto_fit).toBe(autoFit)
    },
  )

  it('keeps Escape as a text input and persists edits before leaving the editor', async () => {
    installEditableText()
    const setTranslation = vi.spyOn(commands, 'setTranslation').mockResolvedValue(null)
    const surface = renderWorkspace()

    fireEvent.keyDown(surface, { key: 'Enter' })
    const editor = await screen.findByRole('textbox', { name: 'Edit text' })
    fireEvent.change(editor, { target: { value: 'Persisted while typing' } })
    fireEvent.keyDown(editor, { key: 'Escape' })

    await waitFor(() =>
      expect(setTranslation).toHaveBeenCalledWith('editable', 'Persisted while typing', null),
    )
    expect(screen.getByTestId('inline-text-editor')).toHaveValue('Persisted while typing')
    fireEvent.blur(editor)
    await waitFor(() => expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument())
    expect(canvas.previewOpacity).toHaveBeenLastCalledWith('editable', null)
  })

  it('does not open a text editor for a cross-page ghost that is not a page entity', () => {
    installProject()
    useNorrixStore.setState({
      selectedLayers: ['ghost-only-text'],
      selectedPage: 'page',
      selectedLayersByPage: { page: ['ghost-only-text'] },
    })
    const surface = renderWorkspace()

    fireEvent.keyDown(surface, { key: 'Enter' })

    expect(screen.queryByTestId('inline-text-editor')).not.toBeInTheDocument()
    expect(canvas.previewOpacity).not.toHaveBeenCalled()
  })

  it('resizes a selected layer through Norrix selection controls', async () => {
    installProject()
    useNorrixStore.setState({ selectedLayers: ['element'] })
    const commit = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    renderWorkspace()
    Object.defineProperty(screen.getByTestId('canvas-overlay'), 'getBoundingClientRect', {
      value: () => ({ x: 10, y: 20, width: 800, height: 600 }),
    })

    const handle = document.querySelector<HTMLElement>('[data-resize-handle="e"]')!

    fireEvent.pointerDown(handle, { button: 0, pointerId: 10, clientX: 120, clientY: 65 })
    fireEvent.pointerMove(handle, { pointerId: 10, clientX: 140, clientY: 65 })
    fireEvent.pointerUp(handle, { pointerId: 10, clientX: 140, clientY: 65 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(canvas.beginTransform).toHaveBeenCalledWith('page', [
      { element: 'element', frame: { x: 10, y: 20, width: 100, height: 50, angle_degrees: 0 } },
    ])
    expect(canvas.updateTransform).toHaveBeenCalledWith(
      expect.arrayContaining([
        {
          element: 'element',
          frame: { x: 10, y: 20, width: 120, height: 50, angle_degrees: 0 },
        },
      ]),
    )
    expect(commit).toHaveBeenCalledWith(
      'page',
      1,
      expect.arrayContaining([
        {
          element: 'element',
          frame: { x: 10, y: 20, width: 120, height: 50, angle_degrees: 0 },
        },
      ]),
    )
  })

  it.each(['single', 'infinite'] as const)(
    'moves the selected text through its drag handle in %s mode at the current zoom',
    async (mode) => {
      const page = installEditableText(mode)
      const frame = { x: 10, y: 20, width: 100, height: 50, angle_degrees: 27 }
      const prepared = queryClient.getQueryData<{
        revision: number
        page: typeof page
        element_frames: Array<{ element: string; frame: typeof frame }>
      }>(preparedPageKey('page'))
      if (!prepared) throw new Error('test fixture must include prepared page data')
      queryClient.setQueryData(preparedPageKey('page'), {
        ...prepared,
        element_frames: [{ element: 'editable', frame }],
      })
      useNorrixStore.setState({
        layerFrames: { editable: frame },
        camera: { zoom: 2, translation: [12, 8], fitted: false },
      })
      const commit = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
      renderWorkspace()
      if (mode === 'infinite') fireEvent(window, new Event('resize'))

      const handle = await screen.findByTestId('text-move-handle')
      Object.defineProperty(screen.getByTestId('canvas-overlay'), 'getBoundingClientRect', {
        value: () => ({ x: 10, y: 20, width: 800, height: 600 }),
      })
      fireEvent.pointerDown(handle, { button: 0, pointerId: 71, clientX: 100, clientY: 100 })
      fireEvent.pointerMove(handle, { pointerId: 71, clientX: 140, clientY: 120 })
      fireEvent.pointerUp(handle, { pointerId: 71, clientX: 140, clientY: 120 })

      await waitFor(() => expect(commit).toHaveBeenCalledOnce())
      expect(canvas.beginTransform).toHaveBeenCalledWith('page', [{ element: 'editable', frame }])
      expect(canvas.updateTransform).toHaveBeenCalledWith([
        {
          element: 'editable',
          frame: { ...frame, x: 30, y: 30 },
        },
      ])
      expect(commit).toHaveBeenCalledWith('page', 1, [
        {
          element: 'editable',
          frame: { ...frame, x: 30, y: 30 },
        },
      ])
    },
  )

  it('hides the text move handle while editing and when its selection is cleared', async () => {
    installEditableText()
    const surface = renderWorkspace()

    expect(await screen.findByTestId('text-move-handle')).toBeInTheDocument()
    fireEvent.keyDown(surface, { key: 'Enter' })
    expect(await screen.findByTestId('inline-text-editor')).toBeInTheDocument()
    expect(screen.queryByTestId('text-move-handle')).not.toBeInTheDocument()

    fireEvent.blur(screen.getByTestId('inline-text-editor'))
    expect(await screen.findByTestId('text-move-handle')).toBeInTheDocument()
    act(() =>
      useNorrixStore.setState({
        selectedLayers: [],
        selectedLayersByPage: { page: [] },
      }),
    )
    expect(screen.queryByTestId('text-move-handle')).not.toBeInTheDocument()
  })

  it('cancels moving a text box without committing its preview', async () => {
    installEditableText()
    const commit = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    renderWorkspace()
    const handle = await screen.findByTestId('text-move-handle')

    fireEvent.pointerDown(handle, { button: 0, pointerId: 72, clientX: 100, clientY: 100 })
    fireEvent.pointerMove(handle, { pointerId: 72, clientX: 120, clientY: 120 })
    fireEvent.pointerCancel(handle, { pointerId: 72 })

    expect(commit).not.toHaveBeenCalled()
    expect(canvas.cancelTransform).toHaveBeenCalledOnce()
  })

  it('does not expose a move handle for a cross-page ghost selection', () => {
    installProject()
    useNorrixStore.setState({
      selectedLayers: ['ghost-only-text'],
      selectedPage: 'page',
      selectedLayersByPage: { page: ['ghost-only-text'] },
    })
    renderWorkspace()

    expect(screen.queryByTestId('text-move-handle')).not.toBeInTheDocument()
  })

  it('shows the live rotation angle only while rotating and hides it on completion or cancel', async () => {
    installProject()
    useNorrixStore.setState({
      selectedLayers: ['element'],
      camera: { zoom: 1, translation: [0, 0], fitted: false },
    })
    const commit = vi.spyOn(commands, 'commitTransform').mockResolvedValue(2)
    renderWorkspace()
    Object.defineProperty(screen.getByTestId('canvas-overlay'), 'getBoundingClientRect', {
      value: () => ({ x: 10, y: 20, width: 800, height: 600 }),
    })

    const rotate = document.querySelector<HTMLElement>('[data-rotate-handle]')!
    expect(screen.queryByTestId('rotation-angle-indicator')).not.toBeInTheDocument()
    fireEvent.pointerDown(rotate, { button: 0, pointerId: 11, clientX: 70, clientY: 40 })
    expect(screen.getByTestId('rotation-angle-indicator')).toHaveTextContent('0°')
    fireEvent.pointerMove(rotate, { pointerId: 11, clientX: 95, clientY: 65 })
    expect(screen.getByTestId('rotation-angle-indicator')).toHaveTextContent('90°')
    fireEvent.pointerUp(rotate, { pointerId: 11, clientX: 95, clientY: 65 })
    expect(screen.queryByTestId('rotation-angle-indicator')).not.toBeInTheDocument()
    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(commit).toHaveBeenCalledWith(
      'page',
      1,
      expect.arrayContaining([
        expect.objectContaining({
          frame: expect.objectContaining({ angle_degrees: 90 }),
        }),
      ]),
    )

    fireEvent.pointerDown(rotate, { button: 0, pointerId: 12, clientX: 70, clientY: 40 })
    expect(screen.getByTestId('rotation-angle-indicator')).toBeInTheDocument()
    fireEvent.pointerCancel(rotate, { pointerId: 12 })
    expect(screen.queryByTestId('rotation-angle-indicator')).not.toBeInTheDocument()
    expect(canvas.cancelTransform).toHaveBeenCalled()
  })

  it('shows the automatic region behind the selected text controls', () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [
        {
          type: 'text',
          id: 'element',
          parent: 'page',
          geometry: null,
          angle_degrees: null,
          visibility: { visible: true, opacity: 1 },
          content: {
            id: 'content',
            source: { text: 'Source', language: 'en' },
            translation: { text: 'Rendered', language: null },
            role: null,
            source_region: null,
          },
          typography: null,
          layout: 'paragraph',
          automatic_region: 'bubble',
        },
      ],
      regions: [
        {
          id: 'bubble',
          parent: 'page',
          geometry: {
            points: [
              { x: 20, y: 30 },
              { x: 100, y: 30 },
              { x: 100, y: 90 },
              { x: 20, y: 90 },
            ],
          },
          kind: 'bubble',
          label: null,
        },
      ],
    }))
    useNorrixStore.setState({
      selectedLayers: ['element'],
      layerFrames: {
        element: { x: 30, y: 40, width: 50, height: 20, angle_degrees: 0 },
      },
    })

    renderWorkspace()

    expect(screen.getByTestId('text-fit-region').querySelector('polygon')).toHaveAttribute(
      'points',
      '20,30 100,30 100,90 20,90',
    )
  })

  it('targets the selected raster layer with the eraser', async () => {
    installProject()
    queryClient.setQueryData(pageKey, (page: { layers: Layer[] }) => ({
      ...page,
      layers: [...page.layers, paintLayer],
    }))
    useNorrixStore.setState({
      tool: 'eraser',
      selectedLayers: ['paint'],
      selectedLayersByPage: { page: ['paint'] },
    })
    const commit = vi
      .spyOn(commands, 'commitErase')
      .mockResolvedValue({ revision: 2, layer: 'paint' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 11, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 11, clientX: 30, clientY: 40 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(canvas.beginStroke).toHaveBeenCalledWith({
      kind: 'erase',
      page: 'page',
      revision: 1,
      layer: 'paint',
      point: { x: 20, y: 20 },
      diameter: 48,
    })
    expect(commit).toHaveBeenCalledWith(
      'page',
      1,
      'paint',
      expect.arrayContaining([{ x: 20, y: 20 }]),
      48,
    )
  })

  it('maps the Remove tool to an inpainting mask gesture', async () => {
    installProject()
    useNorrixStore.setState({ tool: 'remove' })
    const commit = vi.spyOn(commands, 'commitInpaint').mockResolvedValue('job')
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 12, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 12, clientX: 30, clientY: 40 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    await waitFor(() => expect(canvas.cancelStroke).toHaveBeenCalledOnce())
    expect(canvas.beginStroke).toHaveBeenCalledWith({
      kind: 'inpaint',
      page: 'page',
      revision: 1,
      layer: null,
      point: { x: 20, y: 20 },
      diameter: 48,
    })
    expect(commit).toHaveBeenCalledWith('page', 1, expect.arrayContaining([{ x: 20, y: 20 }]), 48)
  })

  it('creates point text on click and paragraph text on drag', async () => {
    installProject()
    useNorrixStore.setState({ tool: 'text' })
    const point = vi
      .spyOn(commands, 'addPointText')
      .mockResolvedValue({ revision: 2, layer: 'point-text' })
    const box = vi
      .spyOn(commands, 'addTextBox')
      .mockResolvedValue({ revision: 3, layer: 'box-text' })
    const surface = renderWorkspace()

    fireEvent.pointerDown(surface, { button: 0, pointerId: 13, clientX: 30, clientY: 40 })
    fireEvent.pointerUp(surface, { pointerId: 13, clientX: 30, clientY: 40 })
    await waitFor(() => expect(point).toHaveBeenCalledWith('page', { x: 20, y: 20 }))

    fireEvent.pointerDown(surface, { button: 0, pointerId: 14, clientX: 40, clientY: 50 })
    fireEvent.pointerMove(surface, { pointerId: 14, clientX: 140, clientY: 110 })
    fireEvent.pointerUp(surface, { pointerId: 14, clientX: 140, clientY: 110 })
    await waitFor(() => expect(box).toHaveBeenCalledOnce())
    expect(box).toHaveBeenCalledWith('page', {
      x: 30,
      y: 30,
      width: 100,
      height: 60,
      angle_degrees: 0,
    })
  })

  it('routes transforms from a scrolled page to that page without changing another selection', async () => {
    installProject()
    const summaries = [1, 2, 3].map((number) => ({
      id: `page-${number}`,
      label: `Page ${number}`,
      size: { width: 600, height: 1000 },
      source_asset: null,
      layer_count: 1,
    }))
    const prepared = summaries.map((summary) => ({
      revision: 20 + Number(summary.id.slice(-1)),
      page: {
        ...summary,
        layers: [
          {
            ...layer,
            id: `element-${summary.id.slice(-1)}`,
            parent: summary.id,
          },
        ],
        regions: [],
      },
      element_frames: [],
      text_font_sizes: [],
    }))
    queryClient.setQueryData(pagesKey, summaries)
    for (const preparation of prepared) {
      queryClient.setQueryData(preparedPageKey(preparation.page.id), preparation)
    }
    useNorrixStore.setState({
      selectedPage: 'page-1',
      selectedLayers: ['element-1'],
      selectedLayersByPage: { 'page-1': ['element-1'] },
      workspaceMode: 'infinite',
      camera: { zoom: 1, translation: [0, 0], fitted: false },
    })
    const commit = vi.spyOn(commands, 'commitTransform').mockResolvedValue(24)
    const surface = renderWorkspace()
    fireEvent(window, new Event('resize'))
    await waitFor(() =>
      expect(canvas.setPageLayout).toHaveBeenLastCalledWith(
        expect.arrayContaining([
          expect.objectContaining({ page: 'page-1' }),
          expect.objectContaining({ page: 'page-2' }),
          expect.objectContaining({ page: 'page-3' }),
        ]),
      ),
    )
    surface.scrollTop = 950
    fireEvent.scroll(surface)
    await waitFor(() => expect(useNorrixStore.getState().selectedPage).toBe('page-2'))

    fireEvent.pointerDown(surface, { button: 0, pointerId: 15, clientX: 140, clientY: 110 })
    fireEvent.pointerMove(surface, { pointerId: 15, clientX: 150, clientY: 120 })
    fireEvent.pointerUp(surface, { pointerId: 15, clientX: 150, clientY: 120 })

    await waitFor(() => expect(commit).toHaveBeenCalledOnce())
    expect(commit).toHaveBeenCalledWith(
      'page-2',
      22,
      expect.arrayContaining([
        expect.objectContaining({
          element: 'element-2',
          frame: expect.objectContaining({ x: 20, y: 30 }),
        }),
      ]),
    )
    expect(useNorrixStore.getState().selectedLayersByPage).toMatchObject({
      'page-1': ['element-1'],
      'page-2': ['element-2'],
    })
  })
})
