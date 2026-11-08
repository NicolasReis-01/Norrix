import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { createElement, StrictMode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import Providers from '@/app/providers'
import { call } from '@/lib/backend'
import { POWER_ON_MAX_DURATION_MS } from '@/lib/power-on-config'
import { queryClient, useProject } from '@/lib/queries'
import { useNorrixStore } from '@/lib/store'
import {
  commands,
  type Preferences,
  type ProjectInfo,
  type StartupState,
} from '@norrix/bridge/protocol'

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

  it('switches to a black screen at nine seconds if startup is still pending', async () => {
    vi.useFakeTimers()
    vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => undefined)
    vi.spyOn(commands, 'subscribe').mockReturnValue(deferred<StartupState>().promise)
    const view = render(createElement(Providers, null, createElement('span', null, 'main-ready')))

    expect(document.querySelector('.power-on-screen')).toBeInTheDocument()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(POWER_ON_MAX_DURATION_MS)
    })

    expect(document.querySelector('.power-on-screen')).not.toBeInTheDocument()
    expect(document.body.querySelector('.bg-black')).toBeInTheDocument()
    expect(screen.queryByText('Preparing Norrix')).not.toBeInTheDocument()
    expect(screen.queryByText('main-ready')).not.toBeInTheDocument()
    view.unmount()
  })

  it('keeps Power On through startup failure then switches to black without revealing home', async () => {
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
      await vi.advanceTimersByTimeAsync(POWER_ON_MAX_DURATION_MS)
    })
    expect(document.querySelector('.power-on-screen')).not.toBeInTheDocument()
    expect(document.body.querySelector('.bg-black')).toBeInTheDocument()
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
        page: 'page',
        stage: 'detection',
        model: 'model',
        error: null,
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
      page: 'page',
      stage: 'ocr',
      model: 'model',
      error: null,
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
