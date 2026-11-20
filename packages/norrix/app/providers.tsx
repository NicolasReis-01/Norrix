'use client'

import { QueryClientProvider } from '@tanstack/react-query'
import { Channel } from '@tauri-apps/api/core'
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { I18nextProvider } from 'react-i18next'

import { GeminiRotationBlobs } from '@/components/app/GeminiRotationBlobs'
import { StartupLoadingView, StartupView } from '@/components/app/StartupView'
import ClientOnly from '@/components/ClientOnly'
import { refreshTranslationModels } from '@/lib/backend'
import i18n from '@/lib/i18n'
import {
  pageKey,
  pagesKey,
  projectKey,
  queryClient,
  refresh,
  useFonts,
  usePages,
  useProject,
  useWorkspacePage,
} from '@/lib/queries'
import {
  receiveCanvas,
  receiveDownload,
  receiveStartupError,
  receiveStartupState,
  receiveJob,
  receiveResources,
  receiveError,
  useNorrixStore,
} from '@/lib/store'
import { traceCanvas } from '@norrix/bridge/canvas'
import {
  commands,
  type CanvasState,
  type Download,
  type Job,
  type ModelResources,
  type ProjectInfo,
} from '@norrix/bridge/protocol'
import { Toaster } from '@norrix/ui/components/toast'
import { TooltipProvider } from '@norrix/ui/components/tooltip'

export function Providers({ children }: { children: ReactNode }) {
  const runtime = useRef({ active: false, bound: false })

  useEffect(() => {
    const lifecycle = runtime.current
    lifecycle.active = true
    if (!lifecycle.bound) {
      lifecycle.bound = true
      const completed = new Map<string, number>()
      const jobStates = new Map<string, Job['state']>()
      const channel = <T,>(receive: (value: T) => void) =>
        new Channel<T>((value) => {
          if (lifecycle.active) receive(value)
        })

      void refreshTranslationModels().catch(() => undefined)

      void commands
        .subscribe(
          channel<CanvasState>((canvas) => {
            const previous = useNorrixStore.getState()
            traceCanvas('canvas-state-received', {
              operation: 'Providers.CanvasStateChannel',
              traceId: canvas.trace_id,
              page: canvas.page,
              revision: canvas.revision,
              generation: canvas.generation,
              previousPage: previous.canvasPage,
              previousRevision: previous.canvasRevision,
              previousGeneration: previous.canvasGeneration,
              result: 'received',
            })
            receiveCanvas(canvas)
            const next = useNorrixStore.getState()
            traceCanvas('canvas-state-receive-completed', {
              operation: 'Providers.CanvasStateChannel',
              traceId: canvas.trace_id,
              page: next.canvasPage,
              revision: next.canvasRevision,
              generation: next.canvasGeneration,
              previousGeneration: previous.canvasGeneration,
              result:
                canvas.generation < previous.canvasGeneration
                  ? 'older-generation-applied'
                  : 'applied',
              filtering: 'none',
            })
          }),
          channel<Job>((job) => {
            const previous = completed.get(job.id) ?? 0
            const previousState = jobStates.get(job.id)
            completed.set(job.id, job.completed)
            jobStates.set(job.id, job.state)
            receiveJob(job)
            if (job.state === 'failed' && previousState !== 'failed' && job.error) {
              const normalized = job.error.toLowerCase()
              if (job.stage === 'translation' && job.rotation_failures.length > 0) {
                if (
                  normalized.includes('persist') ||
                  normalized.includes('commit') ||
                  normalized.includes('invalid output')
                ) {
                  receiveError(
                    'The provider returned a result, but Norrix could not apply or persist it.',
                    'Translation persistence failure',
                  )
                }
              } else if (job.stage === 'translation') {
                if (
                  normalized.includes('quota exceeded') ||
                  normalized.includes('quota exhausted') ||
                  normalized.includes('rate limit') ||
                  normalized.includes('resource_exhausted') ||
                  normalized.includes('429')
                ) {
                  receiveError(
                    'Gemini quota or rate limit was exhausted. Pending translations remain available to retry.',
                    'Gemini translation quota reached',
                  )
                } else if (
                  normalized.includes('503') ||
                  normalized.includes('server error') ||
                  normalized.includes('service remained unavailable')
                ) {
                  receiveError(
                    'Gemini was temporarily unavailable. Pending translations remain available to retry.',
                    'Gemini translation is unavailable',
                  )
                } else if (
                  normalized.includes('connection') ||
                  normalized.includes('timeout') ||
                  normalized.includes('communication')
                ) {
                  receiveError(
                    'The Gemini translation request could not communicate with the provider. Pending translations remain available to retry.',
                    'Gemini communication failure',
                  )
                } else if (
                  normalized.includes('commit') ||
                  normalized.includes('persist') ||
                  normalized.includes('invalid output')
                ) {
                  receiveError(
                    'The translation provider returned a result, but Norrix could not apply or persist it.',
                    'Translation persistence failure',
                  )
                } else {
                  receiveError(
                    'The Gemini translation stage failed. Pending translations remain available to retry.',
                    'Translation failed',
                  )
                }
              } else {
                receiveError(job.error)
              }
            }
            if (job.completed > previous || job.state !== 'running') {
              const traceId = useNorrixStore.getState().canvasTraceId
              traceCanvas('pipeline-page-query-refresh-started', {
                operation: 'Providers.JobChannel',
                traceId,
                jobId: job.id,
                completed: job.completed,
                state: job.state,
                result: 'started',
              })
              void refresh(projectKey, pagesKey, pageKey)
                .then(() => {
                  const project = queryClient.getQueryData<ProjectInfo | null>(projectKey)
                  const page = queryClient.getQueryData<{ id: string } | null>(pageKey)
                  traceCanvas('pipeline-page-query-refresh-finished', {
                    operation: 'Providers.JobChannel',
                    traceId,
                    jobId: job.id,
                    projectRevision: project?.revision ?? null,
                    page: page?.id ?? null,
                    result: 'refreshed',
                  })
                })
                .catch(() => {
                  traceCanvas('pipeline-page-query-refresh-finished', {
                    operation: 'Providers.JobChannel',
                    traceId,
                    jobId: job.id,
                    result: 'failed',
                  })
                })
            }
          }),
          channel<Download>(receiveDownload),
          channel<ModelResources>(receiveResources),
          channel<ProjectInfo | null>((project) => {
            const previous = queryClient.getQueryData<ProjectInfo | null>(projectKey)
            queryClient.setQueryData(projectKey, project)
            if (previous?.name !== project?.name) {
              const store = useNorrixStore.getState()
              store.selectPages(project?.active_page ? [project.active_page] : [])
              store.selectLayers([])
            }
            if (project) {
              void refresh(pagesKey, pageKey).catch(() => undefined)
            } else {
              queryClient.setQueryData(pagesKey, [])
              queryClient.setQueryData(pageKey, null)
            }
          }),
        )
        .then((state) => {
          if (lifecycle.active) receiveStartupState(state)
        })
        .catch((error: unknown) => {
          if (lifecycle.active) {
            receiveStartupError(error instanceof Error ? error.message : String(error))
          }
        })
    }

    return () => {
      lifecycle.active = false
    }
  }, [])

  useEffect(() => {
    const setLanguage = (language: string) => {
      document.documentElement.lang = language
    }
    setLanguage(i18n.language)
    i18n.on('languageChanged', setLanguage)
    return () => i18n.off('languageChanged', setLanguage)
  }, [])

  useEffect(() => {
    // Prevent the host webview from applying browser zoom; keep Ctrl+wheel for app handlers.
    const preventViewportScaling = (event: WheelEvent) => {
      if (event.ctrlKey) event.preventDefault()
    }

    window.addEventListener('wheel', preventViewportScaling, { capture: true, passive: false })
    return () => window.removeEventListener('wheel', preventViewportScaling, { capture: true })
  }, [])

  return (
    <QueryClientProvider client={queryClient}>
      <I18nextProvider i18n={i18n}>
        <TooltipProvider delay={0}>
          <ClientOnly>
            <StartupBoundary>{children}</StartupBoundary>
          </ClientOnly>
        </TooltipProvider>
      </I18nextProvider>
    </QueryClientProvider>
  )
}

function StartupBoundary({ children }: { children: ReactNode }) {
  const initialized = useNorrixStore((state) => state.initialized)
  const projectQuery = useProject(initialized)
  const projectReady = initialized && projectQuery.isSuccess
  const hasProject = projectQuery.data !== null && projectQuery.data !== undefined
  const pagesQuery = usePages(projectReady && hasProject)
  const workspacePageQuery = useWorkspacePage(projectReady && hasProject)
  const fontsQuery = useFonts(projectReady)
  const canDismiss =
    projectReady &&
    (!hasProject || (pagesQuery.isSuccess && workspacePageQuery.isSuccess)) &&
    fontsQuery.isSuccess
  const [startupDismissed, setStartupDismissed] = useState(false)
  const [startupReady, setStartupReady] = useState(false)
  useEffect(() => {
    if (canDismiss) setStartupReady(true)
  }, [canDismiss])
  const ready = startupReady || canDismiss
  const dismissStartup = useCallback(() => setStartupDismissed(true), [])

  return startupDismissed ? (
    ready ? (
      <>
        {children}
        <GeminiRotationBlobs />
        <Toaster />
      </>
    ) : (
      <StartupLoadingView />
    )
  ) : (
    <>
      {ready ? children : <StartupLoadingView />}
      <GeminiRotationBlobs />
      <StartupView onDismiss={dismissStartup} ready={ready} showFailure={false} />
    </>
  )
}

export default Providers
