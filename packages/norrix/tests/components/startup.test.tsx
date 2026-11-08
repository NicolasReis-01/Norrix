import { act, fireEvent, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { PowerOnScreen } from '@/components/app/StartupView'
import {
  POWER_ON_FADE_OUT_AT_MS,
  POWER_ON_FADE_DURATION_MS,
  POWER_ON_HARD_STOP_AT_MS,
  POWER_ON_MAX_DURATION_MS,
  type PowerOnMedia,
} from '@/lib/power-on-config'

let animationFrames: FrameRequestCallback[]

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
  it('renders only the configured video and logo, and starts the video with its embedded audio', () => {
    const { container } = powerOn({
      animation: { kind: 'video', src: '/power-on/animation/power-on.webm' },
      logo: '/power-on/logo/power-on-logo.png',
    })
    flushAnimationFrames()

    const video = container.querySelector('video')!
    const logo = container.querySelector('img')!
    expect(video).toHaveAttribute('src', '/power-on/animation/power-on.webm')
    expect(logo).toHaveAttribute('src', '/power-on/logo/power-on-logo.png')
    expect(container.querySelector('audio')).not.toBeInTheDocument()
    expect(container).not.toHaveTextContent(/\S/)

    fireEvent.canPlay(video)
    expect(video.muted).toBe(false)
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledOnce()

    fireEvent.load(logo)
    expect(video).toHaveStyle({ visibility: 'visible' })
    expect(logo).toHaveStyle({ visibility: 'visible' })
  })

  it('shows the video muted if autoplay with its embedded audio is rejected', async () => {
    vi.spyOn(HTMLMediaElement.prototype, 'play')
      .mockRejectedValueOnce(new Error('autoplay denied'))
      .mockResolvedValueOnce()
    const { container } = powerOn({
      animation: { kind: 'video', src: '/power-on/animation/power-on.webm' },
      logo: '/power-on/logo/power-on-logo.png',
    })
    const video = container.querySelector('video')!

    fireEvent.canPlay(video)
    await act(async () => Promise.resolve())

    expect(video.muted).toBe(true)
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledTimes(2)
    fireEvent.load(container.querySelector('img')!)
    expect(video).toHaveStyle({ visibility: 'visible' })
  })

  it('fades out immediately when startup is ready without an artificial minimum duration', () => {
    const { container, onDismiss, rerender } = powerOn({
      animation: { kind: 'video', src: '/power-on/animation/power-on.webm' },
      logo: '/power-on/logo/power-on-logo.png',
    })
    flushAnimationFrames()
    const screen = container.querySelector('.power-on-screen')!

    expect(screen).toHaveAttribute('data-visible', 'true')
    expect(screen).toHaveAttribute('data-fading', 'false')

    rerender(
      <PowerOnScreen
        media={{
          animation: { kind: 'video', src: '/power-on/animation/power-on.webm' },
          logo: '/power-on/logo/power-on-logo.png',
        }}
        onDismiss={onDismiss}
        ready
      />,
    )
    expect(screen).toHaveAttribute('data-fading', 'true')
    expect(HTMLMediaElement.prototype.pause).toHaveBeenCalled()
    expect(onDismiss).not.toHaveBeenCalled()
    act(() => {
      fireEvent.transitionEnd(screen, { propertyName: 'opacity' })
    })
    expect(onDismiss).toHaveBeenCalledOnce()
  })

  it('keeps Power On until the nine-second deadline when startup is slow', () => {
    const { container, onDismiss } = powerOn({
      animation: { kind: 'video', src: '/power-on/animation/power-on.webm' },
      logo: '/power-on/logo/power-on-logo.png',
    })
    flushAnimationFrames()
    const screen = container.querySelector('.power-on-screen')!

    act(() => vi.advanceTimersByTime(POWER_ON_FADE_OUT_AT_MS))
    expect(screen).toHaveAttribute('data-fading', 'true')
    expect(HTMLMediaElement.prototype.pause).toHaveBeenCalledOnce()
    expect(onDismiss).not.toHaveBeenCalled()
    expect(screen).toHaveStyle({
      '--power-on-fade-duration': `${POWER_ON_FADE_DURATION_MS}ms`,
    })
    act(() => vi.advanceTimersByTime(POWER_ON_HARD_STOP_AT_MS - POWER_ON_FADE_OUT_AT_MS))
    expect(onDismiss).toHaveBeenCalledOnce()
    expect(POWER_ON_HARD_STOP_AT_MS).toBe(POWER_ON_MAX_DURATION_MS)
    expect(POWER_ON_MAX_DURATION_MS).toBe(9000)
  })

  it('does not wait for failed media to transition away from the screen', () => {
    const { container, onDismiss } = powerOn({
      animation: { kind: 'video', src: '/power-on/animation/missing.mp4' },
      logo: '/power-on/logo/missing.png',
    })
    flushAnimationFrames()

    fireEvent.error(container.querySelector('video')!)
    fireEvent.error(container.querySelector('img')!)
    act(() => vi.advanceTimersByTime(POWER_ON_FADE_OUT_AT_MS))
    expect(container.querySelector('.power-on-screen')).toHaveAttribute('data-fading', 'true')
    act(() => vi.advanceTimersByTime(POWER_ON_HARD_STOP_AT_MS - POWER_ON_FADE_OUT_AT_MS))

    expect(onDismiss).toHaveBeenCalledOnce()
  })
})
