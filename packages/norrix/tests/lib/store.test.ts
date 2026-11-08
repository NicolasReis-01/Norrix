import { describe, expect, it } from 'vitest'

import { receiveDownload, receiveStartupError, useNorrixStore } from '@/lib/store'

describe('editor state', () => {
  it('retains failed downloads until they are dismissed', () => {
    useNorrixStore.setState({ downloads: {} })
    receiveDownload({
      state: 'failed',
      id: 9,
      name: 'weights.bin',
      completed: 0,
      total: 0,
      error: 'network unavailable',
    })
    expect(useNorrixStore.getState().downloads[9]).toMatchObject({
      state: 'failed',
      name: 'weights.bin',
    })

    useNorrixStore.getState().dismissDownload(9)
    expect(useNorrixStore.getState().downloads[9]).toBeUndefined()
  })

  it('exposes startup runtime failures instead of leaving the startup view indefinite', () => {
    receiveStartupError('Torch archive SHA-256 mismatch')
    expect(useNorrixStore.getState().startupError).toBe('Torch archive SHA-256 mismatch')
  })

  it('defaults paint brush color to pure white (#FFFFFF)', () => {
    expect(useNorrixStore.getState().brush.color).toBe('#FFFFFF')
  })
})
