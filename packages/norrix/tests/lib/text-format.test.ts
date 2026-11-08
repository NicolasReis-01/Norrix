import { describe, expect, it } from 'vitest'

import { transformTextCase } from '@/lib/text-format'

describe('transformTextCase', () => {
  it('converts accented letters and cedilla to uppercase and lowercase', () => {
    expect(transformTextCase('ação, coração! ÇÃ', 'uppercase')).toBe('AÇÃO, CORAÇÃO! ÇÃ')
    expect(transformTextCase('AÇÃO, CORAÇÃO! ÇÃ', 'lowercase')).toBe('ação, coração! çã')
  })

  it('capitalizes each word while preserving punctuation, whitespace, and line breaks', () => {
    const input = '  ação, coração!\nmaçã\tjá'
    expect(transformTextCase(input, 'title')).toBe('  Ação, Coração!\nMaçã\tJá')
  })

  it('applies sentence case across punctuation and line breaks', () => {
    expect(transformTextCase('ação, MAÇÃ!\nJÁ? ÇÉ', 'sentence')).toBe('Ação, maçã!\nJá? Çé')
  })

  it('alternates letter case per word without changing other characters', () => {
    expect(transformTextCase('texto, çedilha!\nAÇÃO', 'alternating')).toBe('tExTo, çEdIlHa!\naÇãO')
  })
})
