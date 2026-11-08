import type { CanvasGhostPage, CanvasPageLayout } from '@norrix/bridge/canvas'
import type { CanvasPagePreparation, PageSummary } from '@norrix/bridge/protocol'

export function infiniteScrollTranslation(scrollTop: number, dpr: number): [number, number] {
  return [0, -scrollTop * dpr]
}

export function adjacentPageIds(pages: PageSummary[], id: string): string[] {
  const index = pages.findIndex((page) => page.id === id)
  if (index < 0) return []
  return [pages[index - 1]?.id, pages[index + 1]?.id].filter(
    (page): page is string => page !== undefined,
  )
}

export function adjacentPagesFor(
  pages: PageSummary[],
  targets: readonly string[],
): Record<string, string[]> {
  return Object.fromEntries(targets.map((id) => [id, adjacentPageIds(pages, id)]))
}

export function pageSequenceHeight(pages: PageSummary[], zoom: number, dpr: number): number {
  if (zoom <= 0 || dpr <= 0) return 0
  const pageHeight = pages.reduce((total, page) => total + page.size.height, 0)
  return (pageHeight * zoom) / dpr
}

export function residentPageIds(
  pages: PageSummary[],
  scrollTop: number,
  viewportHeight: number,
  zoom: number,
  dpr: number,
): string[] {
  if (!pages.length || zoom <= 0 || dpr <= 0) return []
  const scale = zoom / dpr
  const top = Math.max(0, scrollTop)
  const bottom = top + Math.max(0, viewportHeight)
  let pageTop = 0
  let first = -1
  let last = -1
  pages.forEach((page, index) => {
    const pageBottom = pageTop + page.size.height * scale
    if (pageBottom >= top && pageTop <= bottom) {
      if (first < 0) first = index
      last = index
    }
    pageTop = pageBottom
  })
  if (first < 0) {
    first = pages.length - 1
    last = first
  }
  const start = Math.max(0, first - 1)
  const end = Math.min(pages.length, Math.max(first + 3, last + 2))
  return pages.slice(start, end).map((page) => page.id)
}

export function pageIndexAtScroll(
  pages: PageSummary[],
  offset: number,
  zoom: number,
  dpr: number,
): number {
  if (!pages.length || zoom <= 0 || dpr <= 0) return -1
  const scale = zoom / dpr
  let top = 0
  for (let index = 0; index < pages.length; index += 1) {
    const page = pages[index]
    if (!page) continue
    const bottom = top + page.size.height * scale
    if (offset <= bottom) return index
    top = bottom
  }
  return pages.length - 1
}

export function canvasPageLayouts(
  pages: PageSummary[],
  prepared: Readonly<Record<string, CanvasPagePreparation>>,
  viewportWidth: number,
  zoom: number,
  dpr: number,
  ghosts: Readonly<Record<string, string[]>> = {},
  renderedPages?: ReadonlySet<string>,
): CanvasPageLayout[] {
  if (viewportWidth <= 0 || zoom <= 0 || dpr <= 0) return []
  const scale = zoom / dpr
  let y = 0
  const layouts: CanvasPageLayout[] = []
  for (const summary of pages) {
    const current = prepared[summary.id]
    if (current && (!renderedPages || renderedPages.has(summary.id))) {
      layouts.push({
        page: summary.id,
        revision: current.revision,
        x: (viewportWidth / scale - summary.size.width) / 2,
        y,
        ghostPages: ghostPageOffsets(pages, summary.id, ghosts[summary.id] ?? []),
      })
    }
    y += summary.size.height
  }
  return layouts
}

export function ghostPageOffsets(
  pages: PageSummary[],
  targetId: string,
  sourceIds: readonly string[],
): CanvasGhostPage[] {
  const targetIndex = pages.findIndex((page) => page.id === targetId)
  if (targetIndex < 0) return []
  const target = pages[targetIndex]!
  const targetY = pages.slice(0, targetIndex).reduce((height, page) => height + page.size.height, 0)
  return sourceIds.flatMap((sourceId) => {
    const sourceIndex = pages.findIndex((page) => page.id === sourceId)
    const source = pages[sourceIndex]
    if (!source || sourceIndex === targetIndex) return []
    const sourceY = pages
      .slice(0, sourceIndex)
      .reduce((height, page) => height + page.size.height, 0)
    return [
      {
        page: sourceId,
        x: (target.size.width - source.size.width) / 2,
        y: sourceY - targetY,
      },
    ]
  })
}
