import { render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { ProviderPreferences } from '@/components/preferences/ProviderPreferences'
import { commands, type ProviderPreferences as ProviderSettings } from '@norrix/bridge/protocol'

describe('ProviderPreferences', () => {
  beforeEach(() => {
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([
      {
        id: 'opaque-gemini-key',
        index: 1,
        status: 'available',
        retry_after_seconds: null,
        manually_set: false,
      },
    ])
    vi.spyOn(commands, 'getTranslationModels').mockResolvedValue([])
  })

  it('places the Gemini key manager below the provider heading at full row width', async () => {
    const providers: ProviderSettings = {
      entries: [
        {
          name: 'Gemini',
          config: { provider: 'gemini', settings: {} },
          credential: { configured: false, value: null, clear: false },
        },
      ],
    }

    render(<ProviderPreferences value={providers} onChange={vi.fn()} />)

    const keyList = await screen.findByRole('list', { name: 'Gemini API keys' })
    const providerRow = keyList.parentElement?.parentElement?.parentElement?.parentElement

    expect(providerRow).toHaveClass('grid-cols-1')
    expect(keyList).toHaveClass('w-full', 'grid-cols-1', 'lg:grid-cols-2')
    expect(screen.getByText('Gemini')).toBeInTheDocument()
  })
})
