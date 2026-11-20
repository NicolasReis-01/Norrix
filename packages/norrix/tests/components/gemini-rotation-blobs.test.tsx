import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { GeminiRotationBlobs } from '@/components/app/GeminiRotationBlobs'
import { useNorrixStore } from '@/lib/store'
import type { Job } from '@norrix/bridge/protocol'

function rotationJob(overrides: Partial<Job> = {}): Job {
  return {
    id: 'rotation-job-123456',
    state: 'running',
    completed: 0,
    total: 1,
    skipped: 0,
    failed: 0,
    page: 'page-1',
    stage: 'translation',
    model: 'gemini-3.8-flash',
    error: null,
    rotation_failures: [{ keyIndex: 1, reason: 'Gemini HTTP 503 server error; retryable failure' }],
    rotation_successful_key: null,
    rotation_using_key: 2,
    rotation_keys_left: [3, 5],
    rotation_exhausted: false,
    ...overrides,
  }
}

afterEach(() => {
  vi.useRealTimers()
  useNorrixStore.setState({ jobs: {} })
})

describe('Gemini rotation blobs', () => {
  it('updates current key and keys left in one minimizable blob', () => {
    useNorrixStore.setState({ jobs: { 'rotation-job-123456': rotationJob() } })
    render(<GeminiRotationBlobs />)

    expect(screen.getByTestId('gemini-rotation-blob-rotation-job-123456')).toBeInTheDocument()
    expect(
      screen.getByText((_, element) => element?.textContent === 'Keys left: 3, 5'),
    ).toBeInTheDocument()
    expect(
      screen.getByText((_, element) => element?.textContent === 'Using: 2'),
    ).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: 'Minimize rotation status' }))
    expect(screen.getByTestId('gemini-rotation-blob-rotation-job-123456')).toHaveAttribute(
      'data-minimized',
      'true',
    )

    fireEvent.click(screen.getByRole('button', { name: 'Maximize rotation status' }))
    act(() => {
      useNorrixStore.setState({
        jobs: {
          'rotation-job-123456': rotationJob({
            rotation_using_key: 5,
            rotation_keys_left: [],
          }),
        },
      })
    })
    expect(
      screen.getByText((_, element) => element?.textContent === 'Using: 5'),
    ).toBeInTheDocument()
    expect(
      screen.getByText((_, element) => element?.textContent === 'Keys left: None'),
    ).toBeInTheDocument()
  })

  it('automatically closes after the operation finishes without a pipeline callback', () => {
    vi.useFakeTimers()
    useNorrixStore.setState({
      jobs: {
        'rotation-job-123456': rotationJob({
          state: 'finished',
          rotation_using_key: null,
          rotation_successful_key: 5,
        }),
      },
    })
    render(<GeminiRotationBlobs />)

    expect(screen.getByTestId('gemini-rotation-blob-rotation-job-123456')).toBeInTheDocument()
    act(() => vi.advanceTimersByTime(5000))
    expect(screen.queryByTestId('gemini-rotation-blob-rotation-job-123456')).not.toBeInTheDocument()
  })

  it('moves from the dedicated handle, preserves position when toggled, and clamps to the window', () => {
    useNorrixStore.setState({
      jobs: {
        'drag-rotation-job': rotationJob({ id: 'drag-rotation-job' }),
      },
    })
    render(<GeminiRotationBlobs />)

    const blob = screen.getByTestId('gemini-rotation-blob-drag-rotation-job')
    vi.spyOn(blob, 'getBoundingClientRect').mockReturnValue({
      x: 0,
      y: 12,
      left: 0,
      top: 12,
      right: 300,
      bottom: 112,
      width: 300,
      height: 100,
      toJSON: () => ({}),
    })

    fireEvent.pointerDown(screen.getByTestId('gemini-rotation-drag-drag-rotation-job'), {
      pointerId: 7,
      pointerType: 'mouse',
      button: 0,
      clientX: 20,
      clientY: 22,
    })
    fireEvent.pointerMove(window, {
      pointerId: 7,
      pointerType: 'mouse',
      clientX: 100,
      clientY: 102,
    })
    expect(blob).toHaveStyle({ left: '80px', top: '92px' })
    fireEvent.pointerUp(window, { pointerId: 7, pointerType: 'mouse' })

    fireEvent.click(screen.getByRole('button', { name: 'Minimize rotation status' }))
    expect(blob).toHaveStyle({ left: '80px', top: '92px' })
    fireEvent.click(screen.getByRole('button', { name: 'Maximize rotation status' }))
    expect(blob).toHaveStyle({ left: '80px', top: '92px' })

    fireEvent.pointerDown(screen.getByRole('button', { name: 'Minimize rotation status' }), {
      pointerId: 8,
      pointerType: 'mouse',
      button: 0,
      clientX: 100,
      clientY: 100,
    })
    expect(blob).toHaveStyle({ left: '80px', top: '92px' })
  })

  it('keeps dragged blobs inside the visible window bounds', () => {
    useNorrixStore.setState({
      jobs: {
        'bounded-rotation-job': rotationJob({ id: 'bounded-rotation-job' }),
      },
    })
    render(<GeminiRotationBlobs />)

    const blob = screen.getByTestId('gemini-rotation-blob-bounded-rotation-job')
    vi.spyOn(blob, 'getBoundingClientRect').mockReturnValue({
      x: 0,
      y: 12,
      left: 0,
      top: 12,
      right: 300,
      bottom: 112,
      width: 300,
      height: 100,
      toJSON: () => ({}),
    })

    fireEvent.pointerDown(screen.getByTestId('gemini-rotation-drag-bounded-rotation-job'), {
      pointerId: 9,
      pointerType: 'touch',
      button: -1,
      clientX: 1,
      clientY: 13,
    })
    fireEvent.pointerMove(window, {
      pointerId: 9,
      pointerType: 'touch',
      clientX: 5000,
      clientY: 5000,
    })

    expect(blob).toHaveStyle({
      left: `${window.innerWidth - 300}px`,
      top: `${window.innerHeight - 100}px`,
    })
  })

  it('labels exhaustion clearly and distinguishes concurrent jobs', () => {
    useNorrixStore.setState({
      jobs: {
        'rotation-job-123456': rotationJob({
          rotation_keys_left: [],
          rotation_using_key: null,
          rotation_exhausted: true,
        }),
        'rotation-job-654321': rotationJob({
          id: 'rotation-job-654321',
          rotation_failures: [
            { keyIndex: 3, reason: 'Gemini HTTP 429 quota exceeded (requests_per_minute)' },
          ],
        }),
      },
    })
    render(<GeminiRotationBlobs />)

    expect(screen.getByText('No keys left · rotation exhausted')).toBeInTheDocument()
    expect(screen.getByText('429 RPM')).toBeInTheDocument()
    expect(screen.getByTestId('gemini-rotation-blob-rotation-job-123456')).toBeInTheDocument()
    expect(screen.getByTestId('gemini-rotation-blob-rotation-job-654321')).toBeInTheDocument()
  })
})
