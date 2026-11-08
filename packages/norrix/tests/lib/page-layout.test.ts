import { describe, expect, it } from 'vitest'

import {
  adjacentPageIds,
  adjacentPagesFor,
  canvasPageLayouts,
  ghostPageOffsets,
  infiniteScrollTranslation,
  pageIndexAtScroll,
  pageSequenceHeight,
  residentPageIds,
} from '@/lib/page-layout'
import type { CanvasPagePreparation, PageSummary } from '@norrix/bridge/protocol'

const pages: PageSummary[] = [1, 2, 3].map((number) => ({
  id: `page-${number}`,
  label: `Page ${number}`,
  size: { width: 800, height: 1000 },
  source_asset: null,
  layer_count: 0,
}))

const prepared = Object.fromEntries(
  pages.map((page) => [
    page.id,
    {
      page: { ...page, layers: [], regions: [] },
      revision: 1,
      element_frames: [],
      text_font_sizes: [],
    } satisfies CanvasPagePreparation,
  ]),
)

describe('infinite page layout', () => {
  it('composes adjacent page bounds without gaps or cumulative offsets', () => {
    const layouts = canvasPageLayouts(pages, prepared, 1000, 1, 1)
    expect(pageSequenceHeight(pages, 1, 1)).toBe(3000)
    expect(layouts.map(({ x, y }) => [x, y])).toEqual([
      [100, 0],
      [100, 1000],
      [100, 2000],
    ])
    expect(new Set(layouts.map(({ page }) => page)).size).toBe(3)
  })

  it('keeps pages resident and identifies adjacent pages without scroll thresholds', () => {
    expect(residentPageIds(pages, 0, 500, 1, 1)).toEqual(['page-1', 'page-2', 'page-3'])
    expect(pageIndexAtScroll(pages, 999, 1, 1)).toBe(0)
    expect(pageIndexAtScroll(pages, 1000, 1, 1)).toBe(0)
    expect(pageIndexAtScroll(pages, 1000.25, 1, 1)).toBe(1)
  })

  it('keeps adjacent canvas page edges aligned at fractional scroll and zoom values', () => {
    const zoom = 1.25
    const dpr = 2
    const scrollTop = 327.3
    const [translationX, translationY] = infiniteScrollTranslation(scrollTop, dpr)
    const layouts = canvasPageLayouts(pages, prepared, 1000, zoom, dpr)
    const previousBottom = ((layouts[0]!.y + pages[0]!.size.height) * zoom + translationY) / dpr
    const nextTop = (layouts[1]!.y * zoom + translationY) / dpr
    expect(translationX).toBe(0)
    expect(nextTop).toBe(previousBottom)
    expect(nextTop).toBeCloseTo(297.7)
  })

  it('references only immediate neighboring pages as ghost sources', () => {
    expect(adjacentPageIds(pages, 'page-2')).toEqual(['page-1', 'page-3'])
    expect(adjacentPageIds(pages, 'page-1')).toEqual(['page-2'])
    expect(adjacentPagesFor(pages, ['page-1', 'page-3'])).toEqual({
      'page-1': ['page-2'],
      'page-3': ['page-2'],
    })
  })

  it('attaches ghost sources without laying out source pages unless resident', () => {
    const layouts = canvasPageLayouts(
      pages,
      prepared,
      1000,
      1,
      1,
      { 'page-2': ['page-1', 'page-3'] },
      new Set(['page-2']),
    )
    expect(layouts).toEqual([
      {
        page: 'page-2',
        revision: 1,
        x: 100,
        y: 1000,
        ghostPages: [
          { page: 'page-1', x: 0, y: -1000 },
          { page: 'page-3', x: 0, y: 1000 },
        ],
      },
    ])
  })

  it('maps source-page coordinates into the target page without resetting text position', () => {
    const mixed = [
      { ...pages[0]!, size: { width: 900, height: 700 } },
      { ...pages[1]!, size: { width: 600, height: 1200 } },
      { ...pages[2]!, size: { width: 800, height: 500 } },
    ]
    expect(ghostPageOffsets(mixed, 'page-2', ['page-1', 'page-3'])).toEqual([
      { page: 'page-1', x: -150, y: -700 },
      { page: 'page-3', x: -100, y: 1200 },
    ])
  })
})
