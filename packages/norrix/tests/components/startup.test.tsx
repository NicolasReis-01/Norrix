import { act, fireEvent, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { PowerOnScreen } from '@/components/app/StartupView'
import {
  POWER_ON_FADE_DURATION_MS,
  POWER_ON_MIN_DURATION_MS,
  powerOnMedia,
  type PowerOnMedia,
} from '@/lib/power-on-config'

let animationFrames: FrameRequestCallback[]

const videoMedia: PowerOnMedia = {
  animation: { kind: 'video', src: '/power-on/animation/power-on.webm' },
  logo: null,
}

function flushAnimationFrames() {
  const callbacks = animationFrames
  animationFrames = []
  act(() => callbacks.forEach((callback) => callback(0)))
}

function powerOn(media: PowerOnMedia, onDismiss = vi.fn(), ready = false) {
  const view = render(<PowerOnScreen media={media} onDismiss={onDismiss} ready={ready} />)
  return { ...view, onDismiss }
}

beforeEach(() => {
  animationFrames = []
  vi.useFakeTimers()
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    animationFrames.push(callback)
    return animationFrames.length
  })
  vi.stubGlobal('cancelAnimationFrame', vi.fn())
  vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue()
  vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => undefined)
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe('Power On screen', () => {
  it('renders the configured full-screen video without a separate logo', () => {
    const { container } = powerOn(powerOnMedia)
    flushAnimationFrames()

    const video = container.querySelector('video')!
    expect(video).toHaveAttribute('src', '/power-on/animation/power-on.webm')
    expect(powerOnMedia.logo).toBeNull()
    expect(container.querySelector('img')).not.toBeInTheDocument()
    expect(container.querySelector('audio')).not.toBeInTheDocument()
    expect(container).not.toHaveTextContent(/\S/)

    fireEvent.canPlay(video)
    expect(video.muted).toBe(false)
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledOnce()
    expect(video).toHaveStyle({ visibility: 'visible' })
  })

  it('shows the video muted if autoplay with its embedded audio is rejected', async () => {
    vi.spyOn(HTMLMediaElement.prototype, 'play')
      .mockRejectedValueOnce(new Error('autoplay denied'))
      .mockResolvedValueOnce()
    const { container } = powerOn(videoMedia)
    const video = container.querySelector('video')!

    fireEvent.canPlay(video)
    await act(async () => Promise.resolve())

    expect(video.muted).toBe(true)
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledTimes(2)
    expect(video).toHaveStyle({ visibility: 'visible' })
  })

  it('waits for the configured minimum duration when startup becomes ready', () => {
    const { container, onDismiss, rerender } = powerOn(videoMedia)
    flushAnimationFrames()
    const screen = container.querySelector('.power-on-screen')!

    expect(screen).toHaveAttribute('data-visible', 'true')
    expect(screen).toHaveAttribute('data-fading', 'false')

    rerender(<PowerOnScreen media={videoMedia} onDismiss={onDismiss} ready />)
    expect(screen).toHaveAttribute('data-fading', 'false')
    act(() => vi.advanceTimersByTime(POWER_ON_MIN_DURATION_MS - 1))
    expect(screen).toHaveAttribute('data-fading', 'false')
    act(() => vi.advanceTimersByTime(1))
    expect(screen).toHaveAttribute('data-fading', 'true')
    expect(HTMLMediaElement.prototype.pause).toHaveBeenCalled()
    expect(onDismiss).not.toHaveBeenCalled()
    act(() => {
      fireEvent.transitionEnd(screen, { propertyName: 'opacity' })
    })
    expect(onDismiss).toHaveBeenCalledOnce()
  })

  it('configures a 3.5-second minimum and the expected fade duration', () => {
    expect(POWER_ON_MIN_DURATION_MS).toBe(3500)
    expect(POWER_ON_FADE_DURATION_MS).toBe(450)
  })

  it('keeps Power On visible past the video duration while startup is still pending', () => {
    const { container, onDismiss, rerender } = powerOn(videoMedia)
    flushAnimationFrames()
    const screen = container.querySelector('.power-on-screen')!
    const video = container.querySelector('video')!

    expect(video).toHaveAttribute('loop')
    act(() => vi.advanceTimersByTime(30_000))
    fireEvent.ended(video)
    expect(screen).toHaveAttribute('data-fading', 'false')
    expect(video).toBeInTheDocument()
    expect(HTMLMediaElement.prototype.pause).not.toHaveBeenCalled()
    expect(onDismiss).not.toHaveBeenCalled()

    rerender(<PowerOnScreen media={videoMedia} onDismiss={onDismiss} ready />)
    expect(screen).toHaveAttribute('data-fading', 'true')
    expect(HTMLMediaElement.prototype.pause).toHaveBeenCalledOnce()
    expect(screen).toHaveStyle({
      '--power-on-fade-duration': `${POWER_ON_FADE_DURATION_MS}ms`,
    })
    fireEvent.transitionEnd(screen, { propertyName: 'opacity' })
    expect(onDismiss).toHaveBeenCalledOnce()
  })

  it('does not wait for failed media to transition away from the screen', () => {
    const { container, onDismiss, rerender } = powerOn({
      animation: { kind: 'video', src: '/power-on/animation/missing.webm' },
      logo: null,
    })
    flushAnimationFrames()

    fireEvent.error(container.querySelector('video')!)
    act(() => vi.advanceTimersByTime(POWER_ON_MIN_DURATION_MS))
    expect(container.querySelector('.power-on-screen')).toHaveAttribute('data-fading', 'false')
    rerender(
      <PowerOnScreen
        media={{
          animation: { kind: 'video', src: '/power-on/animation/missing.webm' },
          logo: null,
        }}
        onDismiss={onDismiss}
        ready
      />,
    )
    const readyScreen = container.querySelector('.power-on-screen')!
    expect(readyScreen).toHaveAttribute('data-fading', 'true')
    fireEvent.transitionEnd(readyScreen, { propertyName: 'opacity' })
    expect(onDismiss).toHaveBeenCalledOnce()
  })
})
