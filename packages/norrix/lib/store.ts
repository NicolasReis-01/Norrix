'use client'

import { create } from 'zustand'

import type {
  CanvasState,
  Download,
  EntityId,
  Job,
  Model,
  Frame,
  Preferences,
  Stage,
  StartupState,
  ModelResources,
} from '@norrix/bridge/protocol'
import { toast } from '@norrix/ui/components/toast'

export type CanvasTool = 'select' | 'text' | 'draw' | 'eraser' | 'color_picker' | 'remove' | 'pan'
export const MIN_BRUSH_DIAMETER = 1
export const MAX_BRUSH_DIAMETER = 128

export function isBrushTool(tool: CanvasTool): boolean {
  return tool === 'draw' || tool === 'eraser' || tool === 'remove'
}

export interface CanvasBrush {
  diameter: number
  color: string
}
export interface TypographyPreview {
  layerIds: ReadonlySet<EntityId>
  field: 'line_height' | 'letter_spacing'
  value: number
}
export type InspectorSection = 'copy' | 'type' | 'layers'
export type ShortcutAction = CanvasTool | 'fit'
export type Shortcuts = Record<ShortcutAction, string>
export type PipelineScope = 'page' | 'selected-pages' | 'project'
export type WorkspaceMode = 'single' | 'infinite'
export const pipelineStages: readonly Stage[] = ['detection', 'ocr', 'translation', 'inpainting']

interface NorrixStore {
  initialized: boolean
  startupError: string | null
  preferences: Preferences | null
  translationModels: Model[]
  resources: ModelResources | null
  jobs: Record<string, Job>
  downloads: Record<string, Download>
  camera: { zoom: number; translation: [number, number]; fitted: boolean }
  canvasPage: EntityId | null
  canvasRevision: number | null
  canvasGeneration: number
  canvasSize: [number, number]
  workspaceMode: WorkspaceMode
  selectedPage: EntityId | null
  selectedLayersByPage: Record<EntityId, EntityId[]>
  pageNavigation: { page: EntityId; request: number } | null
  fitCanvasRequest: number
  layerFrames: Record<EntityId, Frame>
  layerFontSizes: Record<EntityId, number>
  typographyPreview: TypographyPreview | null
  selectedLayers: EntityId[]
  selectedPages: EntityId[]
  tool: CanvasTool
  brush: CanvasBrush
  inspector: InspectorSection
  processingScope: PipelineScope
  processingStages: Stage[]
  settingsOpen: boolean
  shortcuts: Shortcuts
  selectPages: (pages: EntityId[]) => void
  showInspector: (section: InspectorSection) => void
  setProcessingScope: (scope: PipelineScope) => void
  setProcessingStages: (stages: Stage[]) => void
  setSettingsOpen: (open: boolean) => void
  selectLayers: (layers: EntityId[]) => void
  selectPage: (page: EntityId) => void
  navigateToPage: (page: EntityId) => void
  setWorkspaceMode: (mode: WorkspaceMode) => void
  setTool: (tool: CanvasTool) => void
  setBrush: (brush: CanvasBrush) => void
  setTypographyPreview: (preview: TypographyPreview | null) => void
  setShortcut: (action: ShortcutAction, key: string) => void
  requestCanvasFit: () => void
  dismissJob: (id: string) => void
  dismissDownload: (id: number) => void
}

export const defaultShortcuts: Shortcuts = {
  select: 'v',
  text: 't',
  draw: 'b',
  eraser: 'e',
  color_picker: 'i',
  remove: 'j',
  pan: 'h',
  fit: '0',
}

export const useNorrixStore = create<NorrixStore>()((set) => ({
  initialized: false,
  startupError: null,
  preferences: null,
  translationModels: [],
  resources: null,
  jobs: {},
  downloads: {},
  camera: { zoom: 1, translation: [0, 0], fitted: true },
  canvasPage: null,
  canvasRevision: null,
  canvasGeneration: 0,
  canvasSize: [0, 0],
  workspaceMode: 'single',
  selectedPage: null,
  selectedLayersByPage: {},
  pageNavigation: null,
  fitCanvasRequest: 0,
  layerFrames: {},
  layerFontSizes: {},
  typographyPreview: null,
  selectedLayers: [],
  selectedPages: [],
  tool: 'select',
  brush: { diameter: 48, color: '#FFFFFF' },
  inspector: 'copy',
  processingScope: 'selected-pages',
  processingStages: [...pipelineStages],
  settingsOpen: false,
  shortcuts: defaultShortcuts,
  selectPages: (selectedPages) => set({ selectedPages: [...new Set(selectedPages)] }),
  showInspector: (inspector) => set({ inspector }),
  setProcessingScope: (processingScope) => set({ processingScope }),
  setProcessingStages: (processingStages) => set({ processingStages }),
  setSettingsOpen: (settingsOpen) => set({ settingsOpen }),
  selectLayers: (layers) =>
    set((state) => {
      const selectedLayers = [...new Set(layers)]
      const page = state.selectedPage ?? state.canvasPage
      return {
        selectedLayers,
        selectedLayersByPage: page
          ? { ...state.selectedLayersByPage, [page]: selectedLayers }
          : state.selectedLayersByPage,
      }
    }),
  selectPage: (selectedPage) =>
    set((state) => ({
      selectedPage,
      selectedLayers: state.selectedLayersByPage[selectedPage] ?? [],
    })),
  navigateToPage: (page) =>
    set((state) => ({
      pageNavigation: { page, request: (state.pageNavigation?.request ?? 0) + 1 },
    })),
  setWorkspaceMode: (workspaceMode) => set({ workspaceMode }),
  setTool: (tool) => set({ tool }),
  setBrush: (brush) => set({ brush }),
  setTypographyPreview: (typographyPreview) => set({ typographyPreview }),
  setShortcut: (action, key) =>
    set((state) => ({
      shortcuts: { ...state.shortcuts, [action]: key.toLowerCase().slice(0, 1) },
    })),
  requestCanvasFit: () => set((state) => ({ fitCanvasRequest: state.fitCanvasRequest + 1 })),
  dismissJob: (id) =>
    set((state) => {
      const jobs = { ...state.jobs }
      delete jobs[id]
      return { jobs }
    }),
  dismissDownload: (id) =>
    set((state) => {
      const downloads = { ...state.downloads }
      delete downloads[String(id)]
      return { downloads }
    }),
}))

export function receiveStartupState(state: StartupState): void {
  useNorrixStore.setState({
    initialized: true,
    startupError: null,
    preferences: state.preferences,
    jobs: byId(state.jobs),
    selectedLayersByPage: {},
    selectedPage: state.canvas.page,
    pageNavigation: null,
    selectedLayers: [],
    ...canvasSnapshot(state.canvas),
  })
}

export function receiveStartupError(startupError: string): void {
  useNorrixStore.setState({ startupError })
}

export function receiveCanvas(canvas: CanvasState): void {
  useNorrixStore.setState((state) => {
    const selectedPage =
      state.workspaceMode === 'single' || state.selectedPage === null
        ? canvas.page
        : state.selectedPage
    return {
      ...canvasSnapshot(canvas),
      selectedPage,
      selectedLayers: selectedPage ? (state.selectedLayersByPage[selectedPage] ?? []) : [],
    }
  })
}

export function receiveJob(job: Job): void {
  useNorrixStore.setState((state) => ({ jobs: { ...state.jobs, [job.id]: job } }))
}

export function receiveDownload(download: Download): void {
  useNorrixStore.setState((state) => {
    const downloads = { ...state.downloads }
    if (download.state === 'finished') delete downloads[String(download.id)]
    else downloads[String(download.id)] = download
    return { downloads }
  })
}

export function receivePreferences(preferences: Preferences): void {
  useNorrixStore.setState({ preferences })
}

export function receiveTranslationModels(translationModels: Model[]): void {
  useNorrixStore.setState({ translationModels })
}

export function receiveResources(resources: ModelResources): void {
  useNorrixStore.setState({ resources })
}

export function receiveError(message: string): void {
  toast.add({ type: 'error', title: 'Could not complete that action', description: message })
}

function canvasSnapshot(canvas: CanvasState) {
  return {
    canvasPage: canvas.page,
    canvasRevision: canvas.revision,
    canvasGeneration: canvas.generation,
    canvasSize: canvas.size,
    layerFrames: Object.fromEntries(
      canvas.element_frames.map(({ element, frame }) => [element, frame]),
    ),
    layerFontSizes: Object.fromEntries(
      (canvas.text_font_sizes ?? []).map(({ element, font_size }) => [element, font_size]),
    ),
  }
}

function byId<T extends { id: string | number }>(items: T[]): Record<string, T> {
  return Object.fromEntries(items.map((item) => [String(item.id), item]))
}
