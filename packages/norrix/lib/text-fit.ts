import type { Layer } from '@norrix/bridge/protocol'

const MIN_FONT_SIZE = 9
const MAX_FONT_SIZE = 512
const TEXT_INSET = 4

export function previewFontSizeForFrame(
  layer: Extract<Layer, { type: 'text' }>,
  width: number,
  height: number,
): number | null {
  if (typeof document === 'undefined' || width <= 0 || height <= 0) return null
  const text = layer.content.translation?.text ?? layer.content.source?.text ?? ''
  if (!text) return null

  const typography = layer.typography
  const context = document.createElement('canvas').getContext('2d')
  if (!context) return null
  const family = typography?.preferred_font ?? 'sans-serif'
  const weight = typography?.font_weight ?? 400
  const style = typography?.font_style ?? 'normal'
  const lineHeight = typography?.line_height ?? 1.2
  const letterSpacing = typography?.letter_spacing ?? 0
  const vertical = typography?.writing_mode === 'Vertical'
  const availableWidth = Math.max(1, (vertical ? height : width) - TEXT_INSET * 2)
  const availableHeight = Math.max(1, (vertical ? width : height) - TEXT_INSET * 2)
  const paragraphs = text.split(/\r?\n/)

  const fits = (size: number) => {
    context.font = `${style} ${weight} ${size}px "${family}"`
    let lineCount = 0
    for (const paragraph of paragraphs) {
      if (!paragraph) {
        lineCount += 1
        continue
      }
      let line = ''
      for (const word of paragraph.split(/(\s+)/).filter(Boolean)) {
        const candidate = line + word
        const measured =
          context.measureText(candidate).width +
          Math.max(0, Array.from(candidate).length - 1) * letterSpacing
        if (line && measured > availableWidth) {
          lineCount += 1
          line = word.trimStart()
        } else {
          line = candidate
        }
        if (
          context.measureText(line).width +
            Math.max(0, Array.from(line).length - 1) * letterSpacing >
          availableWidth
        ) {
          return false
        }
      }
      lineCount += 1
    }
    return lineCount * size * lineHeight <= availableHeight
  }

  let low = MIN_FONT_SIZE
  let high = Math.min(MAX_FONT_SIZE, Math.max(width, height))
  if (!fits(low)) return low
  for (let iteration = 0; iteration < 12; iteration += 1) {
    const midpoint = (low + high) / 2
    if (fits(midpoint)) low = midpoint
    else high = midpoint
  }
  return Math.round(low * 100) / 100
}
