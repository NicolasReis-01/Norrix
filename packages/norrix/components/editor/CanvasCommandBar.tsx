'use client'

import { InferenceControl } from '@/components/editor/InferenceControl'
import { call } from '@/lib/backend'
import { pageKey, projectKey, queryClient, useWorkspacePage } from '@/lib/queries'
import { pipelineStages, receiveError, useNorrixStore, type PipelineScope } from '@/lib/store'
import { commands, type Scope, type Stage } from '@norrix/bridge/protocol'
import { Button } from '@norrix/ui/components/button'

export function CanvasCommandBar() {
  const page = useWorkspacePage().data
  const workspaceMode = useNorrixStore((state) => state.workspaceMode)
  const selectedPage = useNorrixStore((state) => state.selectedPage)
  const setWorkspaceMode = useNorrixStore((state) => state.setWorkspaceMode)
  const setSelectedPage = useNorrixStore((state) => state.selectPage)
  const selectedPages = useNorrixStore((state) => state.selectedPages)
  const jobs = useNorrixStore((state) => state.jobs)
  const running = Object.values(jobs).find((job) => job.state === 'running')

  const run = (selection: PipelineScope, stages: Stage[]) => {
    if (!page) return
    const scope: Scope =
      selection === 'project'
        ? { scope: 'project' }
        : selection === 'selected-pages'
          ? { scope: 'pages', value: selectedPages }
          : { scope: 'pages', value: [page.id] }
    const operation =
      stages.length === pipelineStages.length
        ? ({ operation: 'full' } as const)
        : stages.length === 1
          ? ({ operation: 'only', stage: stages[0]! } as const)
          : ({ operation: 'stages', stages } as const)
    void call(commands.process, scope, operation).catch(() => undefined)
  }

  const selectWorkspaceMode = (mode: 'single' | 'infinite') => {
    if (mode === workspaceMode) return
    if (mode === 'infinite') {
      setWorkspaceMode(mode)
      return
    }
    const target = selectedPage ?? page?.id
    if (!target) {
      setWorkspaceMode(mode)
      return
    }
    void call(commands.selectPage, target)
      .then((selection) => {
        queryClient.setQueryData(projectKey, selection.project)
        queryClient.setQueryData(pageKey, selection.page)
        setSelectedPage(target)
        setWorkspaceMode(mode)
      })
      .catch((error: unknown) =>
        receiveError(error instanceof Error ? error.message : String(error)),
      )
  }

  return (
    <header className='flex h-10 shrink-0 items-center gap-2 border-b border-border/80 bg-[var(--surface-toolbar)] px-2.5'>
      <Button
        size='sm'
        variant={workspaceMode === 'single' ? 'secondary' : 'ghost'}
        aria-pressed={workspaceMode === 'single'}
        onClick={() => selectWorkspaceMode('single')}
      >
        Single Page
      </Button>
      <Button
        size='sm'
        variant={workspaceMode === 'infinite' ? 'secondary' : 'ghost'}
        aria-pressed={workspaceMode === 'infinite'}
        onClick={() => selectWorkspaceMode('infinite')}
      >
        Infinite Scroll
      </Button>
      <div className='min-w-0 flex-1' />
      <InferenceControl disabled={!page || Boolean(running)} onRun={run} />
    </header>
  )
}
