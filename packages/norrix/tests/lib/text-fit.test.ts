import { afterEach, describe, expect, it, vi } from 'vitest'

import { previewFontSizeForFrame } from '@/lib/text-fit'
import type { Layer } from '@norrix/bridge/protocol'

const textLayer: Extract<Layer, { type: 'text' }> = {
  type: 'text',
  id: 'text',
  parent: 'page',
  angle_degrees: 0,
  geometry: null,
  visibility: { visible: true, opacity: 1 },
  content: {
    id: 'content',
    source: { text: 'sample content', language: null },
    translation: null,
    role: null,
    source_region: null,
  },
  typography: {
    preferred_font: 'Noto Sans',
    font_weight: 400,
    font_style: 'normal',
    size: null,
    auto_fit: true,
    color: [0, 0, 0, 255],
    stroke_color: [255, 255, 255, 255],
    stroke_width: 0,
    alignment: 'Center',
    writing_mode: null,
    line_height: 1.2,
    letter_spacing: 0,
  },
  layout: 'paragraph',
  automatic_region: null,
}

afterEach(() => {
  vi.restoreAllMocks()
})

describe('previewFontSizeForFrame', () => {
  it('fits against both frame dimensions and grows as horizontal or vertical space grows', () => {
    let fontDeclaration = ''
    const context = {
      get font() {
        return fontDeclaration
      },
      set font(value: string) {
        fontDeclaration = value
      },
      measureText(text: string) {
        const size = Number(fontDeclaration.match(/\d+(?:\.\d+)?px/)?.[0].replace('px', '')) || 12
        return { width: [...text].length * size * 0.5 }
      },
    } as unknown as CanvasRenderingContext2D
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(context)

    const baseline = previewFontSizeForFrame(textLayer, 120, 120)!
    const wider = previewFontSizeForFrame(textLayer, 240, 120)!
    const larger = previewFontSizeForFrame(textLayer, 240, 240)!
    const shortText = {
      ...textLayer,
      content: { ...textLayer.content, source: { text: 'W', language: null } },
    }
    const shortBaseline = previewFontSizeForFrame(shortText, 120, 120)!
    const taller = previewFontSizeForFrame(shortText, 120, 240)!

    expect(wider).toBeGreaterThan(baseline)
    expect(larger).toBeGreaterThan(wider)
    expect(taller).toBeGreaterThan(shortBaseline)
  })

  it('keeps font weight, style, spacing, and line height as independent layout inputs', () => {
    let font = ''
    const measureText = vi.fn((text: string) => ({ width: [...text].length * 8 }))
    const context = {
      get font() {
        return font
      },
      set font(value: string) {
        font = value
      },
      measureText,
    } as unknown as CanvasRenderingContext2D
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(context)
    const styledLayer = {
      ...textLayer,
      typography: {
        ...textLayer.typography!,
        font_weight: 700,
        font_style: 'italic' as const,
        line_height: 1.75,
        letter_spacing: 2,
      },
    }

    expect(previewFontSizeForFrame(styledLayer, 200, 200)).toBeGreaterThan(0)
    expect(font).toContain('italic 700')
    expect(measureText).toHaveBeenCalled()
  })
})
