import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { GeminiKeyManager } from '@/components/preferences/GeminiKeyManager'
import { commands, type GeminiApiKeyInfo } from '@norrix/bridge/protocol'

function key(
  id: string,
  is_primary = false,
  status: GeminiApiKeyInfo['status'] = 'available',
): GeminiApiKeyInfo {
  return {
    id,
    is_primary,
    status,
    retry_after_seconds: status === 'coolingDown' ? 45 : null,
  }
}

describe('GeminiKeyManager', () => {
  beforeEach(() => {
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([])
    vi.spyOn(commands, 'getTranslationModels').mockResolvedValue([])
  })

  it('lists only opaque identifiers and backend operational status', async () => {
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([
      key('primary-opaque-id', true),
      key('secondary-cooling-id', false, 'coolingDown'),
      key('secondary-rejected-id', false, 'rejected'),
    ])
    render(<GeminiKeyManager />)

    expect(await screen.findByText('Primary')).toBeInTheDocument()
    expect(screen.getAllByText('Secondary')).toHaveLength(2)
    expect(screen.getByText('Available')).toBeInTheDocument()
    expect(screen.getByText('Limited · retry in ~45s')).toBeInTheDocument()
    expect(screen.getByText('Rejected')).toBeInTheDocument()
    expect(screen.getByText('primary-')).toBeInTheDocument()
    expect(screen.queryByText(/secret|api-key-value/i)).not.toBeInTheDocument()
  })

  it('saves a primary key and clears the input only after backend success', async () => {
    const user = userEvent.setup()
    const storageWrite = vi.spyOn(Storage.prototype, 'setItem')
    vi.spyOn(commands, 'setGeminiPrimaryKey').mockResolvedValue([key('primary-new-id', true)])
    render(<GeminiKeyManager />)

    const input = screen.getByLabelText('Replace or set the primary key')
    await user.type(input, 'fake-gemini-primary-key')
    await user.click(screen.getByRole('button', { name: 'Save primary' }))

    await waitFor(() => expect(input).toHaveValue(''))
    expect(commands.setGeminiPrimaryKey).toHaveBeenCalledWith('fake-gemini-primary-key')
    expect(await screen.findByText('primary-')).toBeInTheDocument()
    expect(screen.queryByText('fake-gemini-primary-key')).not.toBeInTheDocument()
    expect(storageWrite.mock.calls.flat().join(' ')).not.toContain('fake-gemini-primary-key')
  })

  it('does not show a successful addition when keyring persistence fails', async () => {
    const user = userEvent.setup()
    vi.spyOn(commands, 'addGeminiSecondaryKey').mockRejectedValue(new Error('keyring write failed'))
    render(<GeminiKeyManager />)

    const input = screen.getByLabelText('Add a secondary key')
    await user.type(input, 'fake-gemini-secondary-key')
    await user.click(screen.getByRole('button', { name: 'Add secondary' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('keyring write failed')
    expect(input).toHaveValue('fake-gemini-secondary-key')
    expect(screen.getByText('No Gemini API keys are configured.')).toBeInTheDocument()
    expect(screen.queryByText('fake-gemini-secondary-key')).not.toBeInTheDocument()
  })

  it('keeps a successful key save successful when refreshing models fails', async () => {
    const user = userEvent.setup()
    vi.spyOn(commands, 'addGeminiSecondaryKey').mockResolvedValue([key('secondary-saved-id')])
    vi.spyOn(commands, 'getTranslationModels').mockRejectedValue(
      new Error('model refresh unavailable'),
    )
    render(<GeminiKeyManager />)

    const input = screen.getByLabelText('Add a secondary key')
    await user.type(input, 'fake-gemini-secondary-key')
    await user.click(screen.getByRole('button', { name: 'Add secondary' }))

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Keys were saved, but translation models could not be refreshed.',
    )
    expect(input).toHaveValue('')
    expect(await screen.findByText('Secondary')).toBeInTheDocument()
    expect(commands.addGeminiSecondaryKey).toHaveBeenCalledOnce()
  })

  it('removes a secondary by id and renders the backend-confirmed remaining list', async () => {
    const user = userEvent.setup()
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([
      key('primary-opaque-id', true),
      key('removed1-opaque'),
      key('keepkey1-opaque'),
    ])
    vi.spyOn(commands, 'removeGeminiSecondaryKey').mockResolvedValue([
      key('primary-opaque-id', true),
      key('keepkey1-opaque'),
    ])
    render(<GeminiKeyManager />)

    expect(await screen.findByText('Primary')).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Remove secondary key removed1' }))
    await waitFor(() =>
      expect(commands.removeGeminiSecondaryKey).toHaveBeenCalledWith('removed1-opaque'),
    )
    expect(screen.queryByText('removed1')).not.toBeInTheDocument()
    expect(screen.getByText('Primary')).toBeInTheDocument()
  })

  it('requires confirmation before clearing both sets of credentials', async () => {
    const user = userEvent.setup()
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([
      key('primary-id', true),
      key('secondary-id'),
    ])
    vi.spyOn(commands, 'clearGeminiApiKeys').mockResolvedValue([])
    render(<GeminiKeyManager />)

    await user.click(screen.getByRole('button', { name: 'Clear all keys' }))
    expect(commands.clearGeminiApiKeys).not.toHaveBeenCalled()
    expect(
      screen.getByText(
        'This removes the primary and all secondary Gemini keys from the device keyring.',
      ),
    ).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Clear credentials' }))

    await waitFor(() => expect(commands.clearGeminiApiKeys).toHaveBeenCalledOnce())
    expect(await screen.findByText('No Gemini API keys are configured.')).toBeInTheDocument()
  })

  it('reports an unavailable test separately from an explicit key rejection', async () => {
    const user = userEvent.setup()
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([key('primary-id', true)])
    vi.spyOn(commands, 'testGeminiApiKey').mockResolvedValue('unavailable')
    render(<GeminiKeyManager />)

    const row = await screen.findByRole('listitem')
    await user.click(within(row).getByRole('button', { name: 'Test' }))
    expect(
      await within(row).findByText('Could not verify; service or network unavailable'),
    ).toBeInTheDocument()
    expect(within(row).queryByText('Key explicitly rejected')).not.toBeInTheDocument()
  })

  it('enforces the backend key limit in the interface', async () => {
    const user = userEvent.setup()
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue(
      Array.from({ length: 16 }, (_, index) => key(`opaque-key-id-${index}`, index === 0)),
    )
    vi.spyOn(commands, 'addGeminiSecondaryKey')
    render(<GeminiKeyManager />)

    const input = await screen.findByLabelText('Add a secondary key')
    expect(input).toBeDisabled()
    expect(screen.getByText('Maximum of 16 keys reached.')).toBeInTheDocument()
    expect(commands.addGeminiSecondaryKey).not.toHaveBeenCalled()
    fireEvent.change(input, { target: { value: 'not-persisted' } })
    expect(input).toBeDisabled()
  })
})
