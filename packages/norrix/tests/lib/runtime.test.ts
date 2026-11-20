import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { createElement, StrictMode, useEffect } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import Providers from '@/app/providers'
import { call } from '@/lib/backend'
import { POWER_ON_MIN_DURATION_MS } from '@/lib/power-on-config'
import { queryClient, useProject } from '@/lib/queries'
import { useNorrixStore } from '@/lib/store'
import {
  commands,
  type Preferences,
  type CanvasState,
  type ProjectInfo,
  type Job,
  type StartupState,
} from '@norrix/bridge/protocol'
import { toast } from '@norrix/ui/components/toast'

vi.mock('@tauri-apps/api/core', () => ({
  Channel: class<T> {
    onmessage: (payload: T) => void

    constructor(handler: (payload: T) => void) {
      this.onmessage = handler
    }
  },
  invoke: vi.fn(),
}))

const preferences: Preferences = {
  pipeline: {
    detection: { model: 'koharu-layout-rfdetr-seg-2xl' },
    ocr: { model: 'paddleocr-vl-1.6' },
    translation: {
      model: {
        provider: 'local',
        model: 'lfm2.5-1.2b-instruct',
        quantization: null,
        vision: false,
        reasoning: false,
      },
      generation: { vision: true, reasoning: true },
      target_language: 'en-US',
      instructions: null,
    },
    inpainting: { model: 'lama' },
    processor: {},
  },
  providers: {
    entries: [],
  },
  typesetting: {
    font_families: ['CCWildWords', 'Adobe 黑体 Std'],
  },
  view: {
    text_box_outlines: false,
    detection_outlines: 'selected',
  },
  languages: [],
}

const project: ProjectInfo = {
  name: 'Book',
  revision: 3,
  active_page: null,
  can_undo: true,
  can_redo: false,
}

beforeEach(() => {
  queryClient.clear()
  useNorrixStore.setState({ initialized: false, startupError: null, preferences: null })
  vi.spyOn(commands, 'getTranslationModels').mockResolvedValue([])
  vi.spyOn(commands, 'getProject').mockResolvedValue(null)
  vi.spyOn(commands, 'getPages').mockResolvedValue([])
  vi.spyOn(commands, 'getPage').mockResolvedValue(null)
  vi.spyOn(commands, 'getFonts').mockResolvedValue([])
  vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => undefined)
})

afterEach(() => {
  vi.useRealTimers()
})

const startupState = (): StartupState => ({
  preferences,
  jobs: [],
  canvas: {
    trace_id: 'startup-canvas-trace',
    page: null,
    revision: null,
    generation: 0,
    size: [0, 0],
    element_frames: [],
    text_font_sizes: [],
  },
})

async function start() {
  const pending = deferred<StartupState>()
  const binding = vi.spyOn(commands, 'subscribe').mockReturnValue(pending.promise)
  const view = render(createElement(Providers, null, createElement('div')))
  pending.resolve(startupState())
  await waitFor(() => expect(useNorrixStore.getState().preferences).toBe(preferences))
  return { binding, dispose: view.unmount }
}

function ProjectProbe() {
  const project = useProject().data
  return createElement(
    'span',
    null,
    project === undefined ? 'Loading' : (project?.name ?? 'Closed'),
  )
}

describe('Tauri runtime', () => {
  it('forwards the correlated canvas update from the subscribed channel into the store', async () => {
    const { binding, dispose } = await start()
    const canvasChannel = binding.mock.calls[0]?.[0]
    expect(canvasChannel).toBeDefined()

    const update: CanvasState = {
      trace_id: 'canvas-test-trace',
      page: 'page-1',
      revision: 12,
      generation: 4,
      size: [800, 600],
      element_frames: [],
      text_font_sizes: [],
    }
    act(() => canvasChannel?.onmessage(update))

    expect(useNorrixStore.getState()).toMatchObject({
      canvasTraceId: update.trace_id,
      canvasPage: update.page,
      canvasRevision: update.revision,
      canvasGeneration: update.generation,
    })
    dispose()
  })

  it('notifies once for a failed job and not for a completed job', async () => {
    const notify = vi.spyOn(toast, 'add')
    const { binding, dispose } = await start()
    const jobChannel = binding.mock.calls[0]?.[1]
    expect(jobChannel).toBeDefined()

    const job: Job = {
      id: 'translation-job',
      state: 'running',
      completed: 0,
      total: 1,
      skipped: 0,
      failed: 0,
      page: 'page-1',
      stage: 'translation',
      model: 'gemini-3.8-flash',
      error: null,
      rotation_failures: [],
      rotation_successful_key: null,
      rotation_using_key: null,
      rotation_keys_left: [],
      rotation_exhausted: false,
    }
    act(() => jobChannel?.onmessage(job))
    act(() =>
      jobChannel?.onmessage({
        ...job,
        state: 'failed',
        failed: 1,
        error: 'Gemini translation failed: quota exhausted',
      }),
    )
    act(() =>
      jobChannel?.onmessage({
        ...job,
        state: 'failed',
        failed: 1,
        error: 'Gemini translation failed: quota exhausted',
      }),
    )
    act(() =>
      jobChannel?.onmessage({
        ...job,
        state: 'finished',
        completed: 1,
        skipped: 1,
        error: null,
      }),
    )

    expect(notify).toHaveBeenCalledTimes(1)
    expect(notify).toHaveBeenCalledWith(
      expect.objectContaining({
        type: 'error',
        title: 'Gemini translation quota reached',
      }),
    )
    dispose()
  })

  it('updates the rotation blob in place without creating a toast or blocking job updates', async () => {
    const notify = vi.spyOn(toast, 'add')
    const { binding, dispose } = await start()
    const jobChannel = binding.mock.calls[0]?.[1]
    const failures = [10, 6, 4, 1].map((keyIndex) => ({
      keyIndex,
      reason: 'Gemini HTTP 503 server error; retryable failure',
    }))
    const job: Job = {
      id: 'rotation-job',
      state: 'running',
      completed: 1,
      total: 1,
      skipped: 0,
      failed: 0,
      page: 'page-1',
      stage: 'translation',
      model: 'gemini-3.8-flash',
      error: null,
      rotation_failures: failures,
      rotation_successful_key: null,
      rotation_using_key: 4,
      rotation_keys_left: [2, 3, 5],
      rotation_exhausted: false,
    }

    act(() => jobChannel?.onmessage(job))

    expect(await screen.findByTestId('gemini-rotation-blob-rotation-job')).toBeInTheDocument()
    expect(screen.getByText('HTTP 503 · service unavailable')).toBeInTheDocument()
    expect(
      screen.getByText((_, element) => element?.textContent === 'Keys left: 2, 3, 5'),
    ).toBeInTheDocument()
    expect(
      screen.getByText((_, element) => element?.textContent === 'Using: 4'),
    ).toBeInTheDocument()
    expect(notify).not.toHaveBeenCalled()

    const user = userEvent.setup()
    await user.click(screen.getByRole('button', { name: 'Minimize rotation status' }))
    expect(screen.getByTestId('gemini-rotation-blob-rotation-job')).toHaveAttribute(
      'data-minimized',
      'true',
    )
    expect(screen.queryByText('Keys left: 2, 3, 5')).not.toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Maximize rotation status' }))

    act(() =>
      jobChannel?.onmessage({
        ...job,
        rotation_using_key: 5,
        rotation_keys_left: [],
      }),
    )
    expect(
      screen.getByText((_, element) => element?.textContent === 'Using: 5'),
    ).toBeInTheDocument()
    expect(
      screen.getByText((_, element) => element?.textContent === 'Keys left: None'),
    ).toBeInTheDocument()
    expect(notify).not.toHaveBeenCalled()
    dispose()
  })

  it('shows exhaustion in the blob without exposing key material or creating a rotation toast', async () => {
    const notify = vi.spyOn(toast, 'add')
    const { binding, dispose } = await start()
    const jobChannel = binding.mock.calls[0]?.[1]
    const failures: Job['rotation_failures'] = [
      { keyIndex: 5, reason: 'Gemini HTTP 503 server error; retryable failure' },
      { keyIndex: 2, reason: 'Gemini HTTP 401 rejected the API key' },
      { keyIndex: 1, reason: 'Gemini HTTP 503 server error; retryable failure' },
      { keyIndex: 4, reason: 'Gemini HTTP 429 quota exceeded (requests_per_day)' },
      { keyIndex: 3, reason: 'Gemini HTTP 403 project or permission access denied' },
    ]
    const runningJob: Job = {
      id: 'five-key-run',
      state: 'running',
      completed: 0,
      total: 1,
      skipped: 0,
      failed: 0,
      page: 'page-1',
      stage: 'translation',
      model: 'gemini-3.8-flash',
      error: null,
      rotation_failures: failures,
      rotation_successful_key: null,
      rotation_using_key: null,
      rotation_keys_left: [],
      rotation_exhausted: false,
    }

    act(() => jobChannel?.onmessage(runningJob))
    act(() =>
      jobChannel?.onmessage({
        ...runningJob,
        state: 'failed',
        failed: 1,
        error: 'all configured Gemini keys failed',
        rotation_exhausted: true,
      }),
    )
    expect(await screen.findByText('No keys left · rotation exhausted')).toBeInTheDocument()
    expect(
      screen.getByText((_, element) => element?.textContent === 'Keys left: None'),
    ).toBeInTheDocument()
    expect(JSON.stringify(useNorrixStore.getState().jobs['five-key-run'])).not.toContain(
      'api-key-secret',
    )
    expect(notify).not.toHaveBeenCalled()

    const callsBeforeNewRun = notify.mock.calls.length
    act(() =>
      jobChannel?.onmessage({
        ...runningJob,
        id: 'next-run',
        rotation_failures: [],
        rotation_exhausted: false,
      }),
    )
    expect(notify).toHaveBeenCalledTimes(callsBeforeNewRun)
    expect(useNorrixStore.getState().jobs['next-run']?.rotation_failures).toEqual([])
    dispose()
  })

  it('keeps the project unresolved until its backend query returns', async () => {
    const projectPending = deferred<ProjectInfo | null>()
    vi.spyOn(commands, 'getProject').mockReturnValue(projectPending.promise)
    vi.spyOn(commands, 'subscribe').mockResolvedValue(startupState())
    const view = render(createElement(Providers, null, createElement(ProjectProbe)))

    expect(document.querySelector('.power-on-screen')).toBeInTheDocument()
    expect(screen.queryByText('Preparing Norrix')).not.toBeInTheDocument()
    projectPending.resolve(null)
    expect(await screen.findByText('Closed')).toBeInTheDocument()
    view.unmount()
  })

  it('keeps the Power On overlay until native startup and the initial project query finish', async () => {
    const startupPending = deferred<StartupState>()
    const projectPending = deferred<ProjectInfo | null>()
    vi.spyOn(commands, 'subscribe').mockReturnValue(startupPending.promise)
    vi.spyOn(commands, 'getProject').mockReturnValue(projectPending.promise)
    const view = render(createElement(Providers, null, createElement(ProjectProbe)))

    expect(document.querySelector('.power-on-screen')).toBeInTheDocument()
    expect(screen.queryByText('Preparing Norrix')).not.toBeInTheDocument()
    await act(async () => {
      startupPending.resolve(startupState())
      await startupPending.promise
    })
    expect(document.querySelector('.power-on-screen')).toBeInTheDocument()
    expect(screen.queryByText('Preparing Norrix')).not.toBeInTheDocument()
    expect(document.querySelector('.power-on-screen')).toHaveAttribute('data-fading', 'false')

    await act(async () => {
      projectPending.resolve(null)
      await projectPending.promise
    })
    expect(await screen.findByText('Closed')).toBeInTheDocument()
    const powerOn = document.querySelector('.power-on-screen')!
    await waitFor(() => expect(powerOn).toHaveAttribute('data-fading', 'true'), { timeout: 4000 })
    fireEvent.transitionEnd(powerOn, { propertyName: 'opacity' })
    await waitFor(() => expect(document.querySelector('.power-on-screen')).not.toBeInTheDocument())

    view.unmount()
  })

  it('waits for the initial project, pages, workspace page, and fonts before revealing home', async () => {
    const startupPending = deferred<StartupState>()
    const projectPending = deferred<ProjectInfo | null>()
    const pagesPending = deferred<Awaited<ReturnType<typeof commands.getPages>>>()
    const pagePending = deferred<Awaited<ReturnType<typeof commands.getPage>>>()
    const fontsPending = deferred<Awaited<ReturnType<typeof commands.getFonts>>>()
    vi.spyOn(commands, 'subscribe').mockReturnValue(startupPending.promise)
    vi.spyOn(commands, 'getProject').mockReturnValue(projectPending.promise)
    vi.spyOn(commands, 'getPages').mockReturnValue(pagesPending.promise)
    vi.spyOn(commands, 'getPage').mockReturnValue(pagePending.promise)
    vi.spyOn(commands, 'getFonts').mockReturnValue(fontsPending.promise)
    const view = render(createElement(Providers, null, createElement(ProjectProbe)))

    await act(async () => {
      startupPending.resolve(startupState())
      await startupPending.promise
    })
    await act(async () => {
      projectPending.resolve(project)
      await projectPending.promise
    })

    expect(document.querySelector('.power-on-screen')).toBeInTheDocument()
    expect(screen.queryByText('Preparing Norrix')).not.toBeInTheDocument()
    expect(screen.queryByText('Book')).not.toBeInTheDocument()
    expect(document.querySelector('.power-on-screen')).toHaveAttribute('data-fading', 'false')

    await act(async () => {
      pagesPending.resolve([])
      pagePending.resolve(null)
      fontsPending.resolve([])
      await Promise.all([pagesPending.promise, pagePending.promise, fontsPending.promise])
    })

    expect(await screen.findByText('Book')).toBeInTheDocument()
    const powerOn = document.querySelector('.power-on-screen')!
    await waitFor(() => expect(powerOn).toHaveAttribute('data-fading', 'true'), { timeout: 4000 })
    fireEvent.transitionEnd(powerOn, { propertyName: 'opacity' })
    await waitFor(() => expect(document.querySelector('.power-on-screen')).not.toBeInTheDocument())
    view.unmount()
  })

  it('keeps the editor mounted when the selected page query is temporarily pending', async () => {
    const firstPage = {
      id: 'page',
      label: 'Page 1',
      size: { width: 1000, height: 1000 },
      layers: [],
      regions: [],
    }
    const nextPagePending = deferred<Awaited<ReturnType<typeof commands.prepareCanvasPage>>>()
    vi.spyOn(commands, 'subscribe').mockResolvedValue(startupState())
    vi.spyOn(commands, 'getProject').mockResolvedValue({ ...project, active_page: 'page' })
    vi.spyOn(commands, 'getPages').mockResolvedValue([])
    vi.spyOn(commands, 'getPage').mockResolvedValue(firstPage)
    vi.spyOn(commands, 'getFonts').mockResolvedValue([])
    const preparePage = vi
      .spyOn(commands, 'prepareCanvasPage')
      .mockReturnValue(nextPagePending.promise)
    let unmounts = 0
    function EditorProbe() {
      useEffect(() => {
        return () => {
          unmounts += 1
        }
      }, [])
      return createElement('span', null, 'editor-mounted')
    }

    const view = render(createElement(Providers, null, createElement(EditorProbe)))
    await screen.findByText('editor-mounted')
    const powerOn = document.querySelector('.power-on-screen')!
    await waitFor(() => expect(powerOn).toHaveAttribute('data-fading', 'true'), {
      timeout: POWER_ON_MIN_DURATION_MS + 1000,
    })
    fireEvent.transitionEnd(powerOn, { propertyName: 'opacity' })
    await waitFor(() => expect(document.querySelector('.power-on-screen')).not.toBeInTheDocument())

    act(() => useNorrixStore.setState({ workspaceMode: 'single', selectedPage: 'page-2' }))
    await waitFor(() =>
      expect(preparePage).toHaveBeenCalledWith(
        'page-2',
        expect.stringMatching(/^workspace-page-page-2-/),
      ),
    )
    expect(screen.getByText('editor-mounted')).toBeInTheDocument()
    expect(unmounts).toBe(0)

    nextPagePending.resolve(null)
    view.unmount()
  })

  it('keeps the Power On screen while startup remains pending beyond nine seconds', async () => {
    vi.useFakeTimers()
    vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => undefined)
    vi.spyOn(commands, 'subscribe').mockReturnValue(deferred<StartupState>().promise)
    const view = render(createElement(Providers, null, createElement('span', null, 'main-ready')))

    expect(document.querySelector('.power-on-screen')).toBeInTheDocument()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000)
    })

    expect(document.querySelector('.power-on-screen')).toBeInTheDocument()
    expect(document.querySelector('.power-on-screen')).toHaveAttribute('data-fading', 'false')
    expect(screen.queryByText('Preparing Norrix')).not.toBeInTheDocument()
    expect(screen.queryByText('main-ready')).not.toBeInTheDocument()
    view.unmount()
  })

  it('keeps Power On through startup failure without revealing home', async () => {
    vi.useFakeTimers()
    const startupPending = deferred<StartupState>()
    vi.spyOn(commands, 'subscribe').mockReturnValue(startupPending.promise)
    const view = render(createElement(Providers, null, createElement('span', null, 'main-ready')))

    await act(async () => {
      startupPending.reject(new Error('native startup failed'))
      await startupPending.promise.catch(() => undefined)
    })

    expect(document.querySelector('.power-on-screen')).toBeInTheDocument()
    expect(screen.queryByText('native startup failed')).not.toBeInTheDocument()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000)
    })
    expect(document.querySelector('.power-on-screen')).toBeInTheDocument()
    expect(document.querySelector('.power-on-screen')).toHaveAttribute('data-fading', 'false')
    expect(screen.queryByText('main-ready')).not.toBeInTheDocument()
    view.unmount()
  })

  it('keeps one live job channel through Strict Mode effect replay', async () => {
    const binding = vi.spyOn(commands, 'subscribe').mockResolvedValue(startupState())
    const view = render(
      createElement(StrictMode, null, createElement(Providers, null, createElement('div'))),
    )

    await waitFor(() => expect(binding).toHaveBeenCalledTimes(1))
    const [, jobChannel] = binding.mock.calls[0]
    act(() => {
      jobChannel.onmessage({
        id: 'job',
        state: 'running',
        completed: 0,
        total: 4,
        skipped: 0,
        failed: 0,
        page: 'page',
        stage: 'detection',
        model: 'model',
        error: null,
        rotation_failures: [],
        rotation_successful_key: null,
        rotation_using_key: null,
        rotation_keys_left: [],
        rotation_exhausted: false,
      })
    })

    expect(useNorrixStore.getState().jobs.job).toMatchObject({ state: 'running', total: 4 })
    view.unmount()
  })

  it('passes only domain arguments to mutation commands', async () => {
    const rename = vi.spyOn(commands, 'renamePage').mockResolvedValue(null)

    await expect(call(commands.renamePage, 'page', 'Chapter 1')).resolves.toBeNull()
    expect(rename).toHaveBeenCalledWith('page', 'Chapter 1')
  })

  it('passes the managed project name to open', async () => {
    const open = vi.spyOn(commands, 'openProject').mockResolvedValue(null)
    await expect(call(commands.openProject, 'Volume 1')).resolves.toBeNull()
    expect(open).toHaveBeenCalledWith('Volume 1')
  })

  it('applies independent channel updates directly to the store', async () => {
    useNorrixStore.setState({ downloads: {}, resources: null })
    const { binding, dispose } = await start()
    const [, , downloadChannel, resourcesChannel] = binding.mock.calls[0]

    downloadChannel.onmessage({
      id: 7,
      state: 'running',
      name: 'model.bin',
      completed: 25,
      total: 100,
      error: null,
    })
    resourcesChannel.onmessage({
      process_memory: 1024,
      system_memory: 8192,
      process_cpu: 5,
      devices: [
        { name: 'GPU', selected: true, memory_budget: 8192, memory_used: 4096, utilization: 40 },
      ],
    })
    expect(useNorrixStore.getState().downloads[7]).toMatchObject({ completed: 25, total: 100 })
    expect(useNorrixStore.getState().resources).toMatchObject({
      process_cpu: 5,
      devices: [{ memory_used: 4096 }],
    })
    dispose()
  })

  it('refreshes project queries when an autonomous job commits work', async () => {
    vi.spyOn(commands, 'getProject').mockResolvedValueOnce(null).mockResolvedValue(project)
    const binding = vi.spyOn(commands, 'subscribe').mockResolvedValue(startupState())
    const view = render(createElement(Providers, null, createElement(ProjectProbe)))
    expect(await screen.findByText('Closed')).toBeInTheDocument()

    const [, jobChannel] = binding.mock.calls[0]
    jobChannel.onmessage({
      id: 'job',
      state: 'running',
      completed: 1,
      total: 2,
      skipped: 0,
      failed: 0,
      page: 'page',
      stage: 'ocr',
      model: 'model',
      error: null,
      rotation_failures: [],
      rotation_successful_key: null,
      rotation_using_key: null,
      rotation_keys_left: [],
      rotation_exhausted: false,
    })

    expect(await screen.findByText('Book')).toBeInTheDocument()
    view.unmount()
  })
})

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}
