'use client'

import {
  QueryClient,
  type QueryKey,
  queryOptions,
  useIsMutating,
  useMutation,
  useQuery,
  useQueries,
} from '@tanstack/react-query'

import { commands, type FontFamily, type Page, type ProjectInfo } from '@norrix/bridge/protocol'

import { call } from './backend'
import { useNorrixStore } from './store'

export const projectKey = ['project'] as const
export const pagesKey = ['pages'] as const
export const pageKey = ['page'] as const
export const preparedPagesKey = ['prepared-page'] as const
export const preparedPageKey = (page: string) => [...preparedPagesKey, page] as const
export const fontsKey = ['fonts'] as const

const projectQuery = queryOptions({
  queryKey: projectKey,
  queryFn: () => call(commands.getProject),
})

const pagesQuery = queryOptions({
  queryKey: pagesKey,
  queryFn: () => call(commands.getPages),
})

const pageQuery = queryOptions({
  queryKey: pageKey,
  queryFn: () => call(commands.getPage),
})

const fontsQuery = queryOptions({
  queryKey: fontsKey,
  queryFn: () => call(commands.getFonts),
})

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: Number.POSITIVE_INFINITY,
      retry: false,
      refetchOnReconnect: false,
      refetchOnWindowFocus: false,
    },
  },
})

export function useProject(enabled = true) {
  return useQuery({ ...projectQuery, enabled })
}

export function usePages(enabled = true) {
  return useQuery({ ...pagesQuery, enabled })
}

export function usePage(enabled = true) {
  return useQuery({ ...pageQuery, enabled })
}

export function useWorkspacePage(enabled = true) {
  const selectedPage = useNorrixStore((state) => state.selectedPage)
  const canvasPage = useNorrixStore((state) => state.canvasPage)
  const canvasRevision = useNorrixStore((state) => state.canvasRevision)
  const layerFrames = useNorrixStore((state) => state.layerFrames)
  const layerFontSizes = useNorrixStore((state) => state.layerFontSizes)
  const workspaceMode = useNorrixStore((state) => state.workspaceMode)
  const activePage = usePage(enabled && selectedPage === null)
  const preparedPage = useQuery({
    queryKey: selectedPage ? preparedPageKey(selectedPage) : pageKey,
    queryFn: () => {
      if (!selectedPage) return Promise.resolve(null)
      const currentPage = queryClient.getQueryData<Page | null>(pageKey)
      const project = queryClient.getQueryData<ProjectInfo | null>(projectKey)
      if (
        currentPage?.id === selectedPage &&
        canvasPage === selectedPage &&
        canvasRevision !== null &&
        (project === undefined || (project !== null && canvasRevision === project.revision))
      ) {
        return Promise.resolve({
          page: currentPage,
          revision: canvasRevision,
          element_frames: Object.entries(layerFrames).map(([element, frame]) => ({
            element,
            frame,
          })),
          text_font_sizes: Object.entries(layerFontSizes).map(([element, font_size]) => ({
            element,
            font_size,
          })),
        })
      }
      if (workspaceMode === 'infinite') return Promise.resolve(null)
      return call(commands.prepareCanvasPage, selectedPage)
    },
    enabled: enabled && selectedPage !== null,
    select: (preparation) => preparation?.page ?? null,
  })
  return selectedPage === null ? activePage : preparedPage
}

export function usePreparedPages(pageIds: string[]) {
  return useQueries({
    queries: pageIds.map((page) => ({
      queryKey: preparedPageKey(page),
      queryFn: () => call(commands.prepareCanvasPage, page),
      enabled: false,
    })),
  })
}

export function useFonts(enabled = true) {
  return useQuery({ ...fontsQuery, enabled })
}

export function useFontPreview(font: FontFamily | undefined, enabled = true) {
  return useQuery({
    queryKey: ['font-preview', font?.name],
    queryFn: async () => {
      if (!font) return null
      try {
        return new Uint8Array(await commands.getFontPreview(font.name))
      } catch {
        return null
      }
    },
    enabled: enabled && font !== undefined,
    gcTime: 5 * 60 * 1000,
  })
}

export function useCommand<Args extends unknown[], Result>(
  key: QueryKey,
  command: (...args: Args) => Promise<Result>,
  label: string,
  onSuccess?: () => Promise<void>,
) {
  const busy = useIsMutating({ mutationKey: key }) > 0
  const mutation = useMutation({
    mutationKey: key,
    mutationFn: (args: Args) => call(command, ...args),
    meta: { activity: label },
    onSuccess,
  })
  return { run: (...args: Args) => mutation.mutate(args), busy }
}

export function useImportPages() {
  const { run, busy } = useCommand(['import-pages'], commands.import, 'navigator.importing', () =>
    refresh(projectKey, pagesKey, pageKey),
  )
  return { importPages: run, importing: busy }
}

export async function refresh(...keys: QueryKey[]): Promise<void> {
  await Promise.all(keys.map((queryKey) => queryClient.invalidateQueries({ queryKey })))
  if (
    keys.some((key) => key[0] === projectKey[0] || key[0] === pagesKey[0] || key[0] === pageKey[0])
  ) {
    queryClient.removeQueries({ queryKey: preparedPagesKey })
  }
}
