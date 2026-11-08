export type TextCase = 'uppercase' | 'lowercase' | 'sentence' | 'title' | 'alternating'

const letter = /^\p{L}$/u

export function transformTextCase(text: string, mode: TextCase): string {
  if (mode === 'uppercase') return text.toUpperCase()
  if (mode === 'lowercase') return text.toLowerCase()

  if (mode === 'sentence') {
    const sentenceSegmenter = new Intl.Segmenter(undefined, { granularity: 'sentence' })
    return Array.from(sentenceSegmenter.segment(text), ({ segment }) => {
      const characters = Array.from(segment.toLowerCase())
      const firstLetter = characters.findIndex((character) => letter.test(character))
      if (firstLetter >= 0) characters[firstLetter] = characters[firstLetter]!.toUpperCase()
      return characters.join('')
    }).join('')
  }

  const segmenter = new Intl.Segmenter(undefined, { granularity: 'word' })
  return Array.from(segmenter.segment(text), ({ segment, isWordLike }) => {
    if (!isWordLike) return segment
    const characters = Array.from(segment.toLowerCase())
    if (mode === 'title') {
      const firstLetter = characters.findIndex((character) => letter.test(character))
      if (firstLetter >= 0) characters[firstLetter] = characters[firstLetter].toUpperCase()
      return characters.join('')
    }

    let uppercase = false
    return characters
      .map((character) => {
        if (!letter.test(character)) return character
        const result = uppercase ? character.toUpperCase() : character.toLowerCase()
        uppercase = !uppercase
        return result
      })
      .join('')
  }).join('')
}
