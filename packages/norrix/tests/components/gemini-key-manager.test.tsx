import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { GeminiKeyManager } from '@/components/preferences/GeminiKeyManager'
import { commands, type GeminiApiKeyInfo } from '@norrix/bridge/protocol'

function key(
  id: string,
  firstInLegacyConfig = false,
  status: GeminiApiKeyInfo['status'] = 'available',
): GeminiApiKeyInfo {
  return {
    id,
    index: firstInLegacyConfig ? 1 : 2,
    status,
    retry_after_seconds: status === 'coolingDown' ? 45 : status === 'rpmCooldown' ? 60 : null,
    manually_set: false,
  }
}

describe('GeminiKeyManager', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  beforeEach(() => {
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([])
    vi.spyOn(commands, 'getTranslationModels').mockResolvedValue([])
  })

  it('lists only opaque identifiers and backend operational status', async () => {
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([
      key('primary-opaque-id', true),
      key('secondary-cooling-id', false, 'coolingDown'),
      key('secondary-available-id'),
    ])
    render(<GeminiKeyManager />)

    expect(await screen.findByText('Key 1')).toBeInTheDocument()
    expect(screen.getAllByText('Key 2')).toHaveLength(2)
    expect(screen.getAllByText('Available')).toHaveLength(2)
    expect(screen.getByText('Limited · retry in ~45s')).toBeInTheDocument()
    expect(screen.getByText('primary-')).toBeInTheDocument()
    expect(screen.queryByText(/secret|api-key-value/i)).not.toBeInTheDocument()
  })

  it('adds a key and clears the input only after backend success', async () => {
    const user = userEvent.setup()
    const storageWrite = vi.spyOn(Storage.prototype, 'setItem')
    vi.spyOn(commands, 'addGeminiApiKey').mockResolvedValue([key('new-opaque-id', true)])
    render(<GeminiKeyManager />)

    const input = screen.getByLabelText('Add a key')
    await user.type(input, 'fake-gemini-api-key')
    await user.click(screen.getByRole('button', { name: 'Add key' }))

    await waitFor(() => expect(input).toHaveValue(''))
    expect(commands.addGeminiApiKey).toHaveBeenCalledWith('fake-gemini-api-key')
    expect(await screen.findByText('new-opaq')).toBeInTheDocument()
    expect(screen.queryByText('fake-gemini-api-key')).not.toBeInTheDocument()
    expect(storageWrite.mock.calls.flat().join(' ')).not.toContain('fake-gemini-api-key')
  })

  it('does not show a successful addition when keyring persistence fails', async () => {
    const user = userEvent.setup()
    vi.spyOn(commands, 'addGeminiApiKey').mockRejectedValue(new Error('keyring write failed'))
    render(<GeminiKeyManager />)

    const input = screen.getByLabelText('Add a key')
    await user.type(input, 'fake-gemini-api-key')
    await user.click(screen.getByRole('button', { name: 'Add key' }))

    expect(await screen.findByRole('alert')).toHaveTextContent('keyring write failed')
    expect(input).toHaveValue('fake-gemini-api-key')
    expect(screen.getByText('No Gemini API keys are configured.')).toBeInTheDocument()
    expect(screen.queryByText('fake-gemini-api-key')).not.toBeInTheDocument()
  })

  it('keeps a successful key save successful when refreshing models fails', async () => {
    const user = userEvent.setup()
    vi.spyOn(commands, 'addGeminiApiKey').mockResolvedValue([key('secondary-saved-id')])
    vi.spyOn(commands, 'getTranslationModels').mockRejectedValue(
      new Error('model refresh unavailable'),
    )
    render(<GeminiKeyManager />)

    const input = screen.getByLabelText('Add a key')
    await user.type(input, 'fake-gemini-secondary-key')
    await user.click(screen.getByRole('button', { name: 'Add key' }))

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Keys were saved, but translation models could not be refreshed.',
    )
    expect(input).toHaveValue('')
    expect(await screen.findByText('Key 2')).toBeInTheDocument()
    expect(commands.addGeminiApiKey).toHaveBeenCalledOnce()
  })

  it('removes a key by id and renders the backend-confirmed remaining list', async () => {
    const user = userEvent.setup()
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([
      key('primary-opaque-id', true),
      key('removed1-opaque'),
      key('keepkey1-opaque'),
    ])
    vi.spyOn(commands, 'removeGeminiApiKey').mockResolvedValue([
      key('primary-opaque-id', true),
      key('keepkey1-opaque'),
    ])
    render(<GeminiKeyManager />)

    expect(await screen.findByText('Key 1')).toBeInTheDocument()
    const row = (await screen.findAllByRole('listitem'))[1]
    await user.click(within(row).getByRole('button', { name: 'Remove API key 2' }))
    await waitFor(() => expect(commands.removeGeminiApiKey).toHaveBeenCalledWith('removed1-opaque'))
    expect(screen.queryByText('removed1')).not.toBeInTheDocument()
    expect(screen.getByText('Key 1')).toBeInTheDocument()
  })

  it('requires confirmation before clearing configured credentials', async () => {
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
      screen.getByText('This removes all Gemini API keys from the device keyring.'),
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

  it('allows adding keys beyond sixteen', async () => {
    const user = userEvent.setup()
    const initial = Array.from({ length: 16 }, (_, index) =>
      key(`opaque-key-id-${index}`, index === 0),
    )
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue(initial)
    vi.spyOn(commands, 'addGeminiApiKey').mockResolvedValue([...initial, key('opaque-key-id-16')])
    render(<GeminiKeyManager />)

    const input = await screen.findByLabelText('Add a key')
    expect(input).toBeEnabled()
    await user.type(input, 'fake-seventeenth-key')
    await user.click(screen.getByRole('button', { name: 'Add key' }))
    await waitFor(() =>
      expect(commands.addGeminiApiKey).toHaveBeenCalledWith('fake-seventeenth-key'),
    )
    expect(await screen.findByText('17 keys')).toBeInTheDocument()
  })

  it('tests available configured keys sequentially and summarizes skipped keys', async () => {
    const user = userEvent.setup()
    const initial = [
      key('available-key-id', true),
      key('daily-quota-key', false, 'inactiveDailyQuota'),
      key('rpm-limited-key', false, 'rpmCooldown'),
    ]
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue(initial)
    const test = vi.spyOn(commands, 'testGeminiApiKey').mockResolvedValue('verified')
    render(<GeminiKeyManager />)

    await user.click(await screen.findByRole('button', { name: 'Test all keys' }))

    await waitFor(() => expect(test).toHaveBeenCalledOnce())
    expect(test).toHaveBeenCalledWith('available-key-id')
    expect(test).not.toHaveBeenCalledWith('daily-quota-key')
    expect(test).not.toHaveBeenCalledWith('rpm-limited-key')
    expect(await screen.findByText('Valid: 1 · Unavailable: 2 · Errors: 0')).toBeInTheDocument()
    const rows = within(screen.getByRole('list')).getAllByRole('listitem')
    expect(within(rows[1]).getByLabelText('Inactive · 429 Daily Quota')).toBeInTheDocument()
    expect(within(rows[2]).getByLabelText('Countdown · 60s')).toBeInTheDocument()
  })

  it('does not stop testing later keys when an earlier key errors', async () => {
    const user = userEvent.setup()
    const keys = [key('first-key-id', true), key('second-key-id'), key('third-key-id')]
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue(keys)
    const test = vi
      .spyOn(commands, 'testGeminiApiKey')
      .mockRejectedValueOnce(new Error('network unavailable'))
      .mockResolvedValueOnce('verified')
      .mockResolvedValueOnce('rejected')
    render(<GeminiKeyManager />)

    await user.click(await screen.findByRole('button', { name: 'Test all keys' }))

    await waitFor(() => expect(test).toHaveBeenCalledTimes(3))
    expect(await screen.findByText('Valid: 1 · Unavailable: 1 · Errors: 1')).toBeInTheDocument()
    expect(screen.getByText('Test failed')).toBeInTheDocument()
  })

  it('allows temporary manual states and clearing them releases the key', async () => {
    const user = userEvent.setup()
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([key('manual-key-id', true)])
    const setState = vi
      .spyOn(commands, 'setGeminiApiKeyManualStatus')
      .mockResolvedValueOnce([
        { ...key('manual-key-id', true, 'inactiveDailyQuota'), manually_set: true },
      ])
      .mockResolvedValueOnce([key('manual-key-id', true)])
    render(<GeminiKeyManager />)

    const control = await screen.findByRole('combobox', {
      name: 'Manual availability for key 1',
    })
    await user.selectOptions(control, 'inactiveDailyQuota')
    await waitFor(() =>
      expect(setState).toHaveBeenNthCalledWith(1, 'manual-key-id', 'inactiveDailyQuota'),
    )
    expect(await screen.findByText('Manual')).toBeInTheDocument()

    await user.selectOptions(control, 'automatic')
    await waitFor(() => expect(setState).toHaveBeenNthCalledWith(2, 'manual-key-id', null))
    expect(await screen.findByText('Available')).toBeInTheDocument()
  })

  it('refreshes countdown markers while settings remain open', async () => {
    const limited = key('rpm-countdown-key', true, 'rpmCooldown')
    const refreshed = { ...limited, retry_after_seconds: 59 }
    const getKeys = vi
      .spyOn(commands, 'getGeminiApiKeys')
      .mockResolvedValueOnce([limited])
      .mockResolvedValue([refreshed])
    render(<GeminiKeyManager />)

    const row = await screen.findByRole('listitem')
    expect(within(row).getByLabelText('Countdown · 60s')).toBeInTheDocument()
    await new Promise((resolve) => setTimeout(resolve, 1100))
    expect(await within(row).findByLabelText('Countdown · 59s')).toBeInTheDocument()
    expect(getKeys).toHaveBeenCalledTimes(2)
  })

  it('uses a responsive one- or two-column key grid and keeps status chips in each key row', async () => {
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([
      key('wide-key-one', true),
      key('wide-key-two'),
    ])
    render(<GeminiKeyManager />)

    const list = await screen.findByRole('list', { name: 'Gemini API keys' })
    expect(list).toHaveClass('grid-cols-1', 'lg:grid-cols-2')
    const rows = within(list).getAllByRole('listitem')
    expect(rows).toHaveLength(2)
    expect(within(rows[0]).getByText('Available')).toBeInTheDocument()
    expect(within(rows[1]).getByText('Available')).toBeInTheDocument()
  })

  it('uses the Norrix orange theme tokens for automatic and manual unavailable markers', async () => {
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([
      key('daily-quota-marker', true, 'inactiveDailyQuota'),
      { ...key('rpm-marker', false, 'rpmCooldown'), manually_set: true },
      key('cooldown-marker', false, 'coolingDown'),
    ])
    render(<GeminiKeyManager />)

    const list = await screen.findByRole('list')
    for (const label of [
      'Inactive · 429 Daily Quota',
      'Countdown · 60s',
      'Limited · retry in ~45s',
    ]) {
      expect(within(list).getByLabelText(label)).toHaveClass(
        'text-[var(--norrix-primary-dark)]',
        'dark:text-[var(--norrix-primary-light)]',
      )
    }
    expect(within(list).getByText('Manual')).toHaveClass(
      'text-[var(--norrix-primary-dark)]',
      'dark:text-[var(--norrix-primary-light)]',
    )
  })

  it('edits any configured key by its opaque identifier', async () => {
    const user = userEvent.setup()
    vi.spyOn(commands, 'getGeminiApiKeys').mockResolvedValue([
      key('primary-id', true),
      key('secondary-id'),
    ])
    vi.spyOn(commands, 'replaceGeminiApiKey').mockResolvedValue([
      key('primary-id', true),
      key('secondary-id'),
    ])
    render(<GeminiKeyManager />)

    const keyRow = (await screen.findAllByRole('listitem'))[1]
    await user.click(within(keyRow).getByRole('button', { name: 'Edit' }))
    const input = within(keyRow).getByLabelText('Edit', { selector: 'input' })
    await user.type(input, 'fake-replacement-key')
    await user.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() =>
      expect(commands.replaceGeminiApiKey).toHaveBeenCalledWith(
        'secondary-id',
        'fake-replacement-key',
      ),
    )
  })
})
