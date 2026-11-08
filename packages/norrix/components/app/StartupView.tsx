'use client'

import { AlertCircle } from 'lucide-react'
import Image from 'next/image'
import { useEffect, useRef, useState, type SyntheticEvent, type TransitionEvent } from 'react'
import { useTranslation } from 'react-i18next'

import { useMacOS, WindowControls } from '@/components/app/WindowChrome'
import {
  POWER_ON_FADE_DURATION_MS,
  POWER_ON_FADE_OUT_AT_MS,
  POWER_ON_HARD_STOP_AT_MS,
  POWER_ON_MAX_DURATION_MS,
  powerOnMedia,
  type PowerOnMedia,
} from '@/lib/power-on-config'
import { useNorrixStore } from '@/lib/store'

import logo from '../../../../crates/norrix/icons/icon.png'

interface StartupViewProps {
  onDismiss: () => void
  ready: boolean
  showFailure?: boolean
}

export function StartupView({ onDismiss, ready, showFailure = true }: StartupViewProps) {
  const { t } = useTranslation()
  const macOS = useMacOS()
  const startupError = useNorrixStore((state) => state.startupError)
  const downloads = useNorrixStore((state) => state.downloads)
  if (!startupError || !showFailure) {
    return <PowerOnScreen media={powerOnMedia} onDismiss={onDismiss} ready={ready} />
  }

  const visibleDownloads = Object.values(downloads).filter(
    (download) => download.state === 'running' || download.state === 'failed',
  )

  return (
    <div className='relative flex h-screen w-screen flex-col overflow-hidden bg-[var(--surface-titlebar)] text-foreground'>
      <header
        data-tauri-drag-region='deep'
        className='grid h-10 shrink-0 grid-cols-[132px_1fr_132px] items-center border-b border-border/80 bg-[var(--surface-titlebar)] shadow-[var(--shadow-titlebar)]'
      >
        <div className='flex h-full items-center'>
          {!macOS && (
            <div className='grid h-full w-10 place-items-center rounded-br-lg'>
              <Image
                className='pointer-events-none'
                src={logo}
                alt=''
                width={17}
                height={17}
                draggable={false}
                priority
              />
            </div>
          )}
        </div>
        <div className='grid h-full place-items-center text-[11px] text-muted-foreground select-none'>
          Norrix
        </div>
        {!macOS && <WindowControls />}
      </header>

      <main className='relative grid min-h-0 flex-1 place-items-center overflow-hidden bg-[var(--surface-canvas)]'>
        <section
          className='w-full max-w-[620px] px-10 text-center'
          role='status'
          aria-live='polite'
          aria-labelledby='startup-title'
        >
          <AlertCircle className='mx-auto size-11 text-destructive' aria-hidden='true' />
          <h1
            id='startup-title'
            className='mt-5 text-[24px] font-semibold tracking-[-0.025em] text-balance'
          >
            {t('startup.failed')}
          </h1>
          <p className='mx-auto mt-2 max-w-[48ch] text-[13px] leading-5 text-muted-foreground'>
            {startupError}
          </p>

          <p className='mx-auto mt-7 max-w-[48ch] border-t border-border/80 pt-4 text-[12px] text-muted-foreground'>
            {t('startup.restart')}
          </p>

          {visibleDownloads.length > 0 && (
            <div className='mx-auto mt-5 flex max-w-[480px] flex-col gap-3 text-left'>
              {visibleDownloads.map((download) => {
                const percent =
                  download.total > 0
                    ? Math.min(100, Math.round((download.completed / download.total) * 100))
                    : null
                return (
                  <div key={download.id} className='rounded-lg border border-border/80 p-3'>
                    <div className='flex items-center justify-between gap-3 text-[11px]'>
                      <span className='truncate'>
                        {download.state === 'failed'
                          ? download.error || t('startup.downloadFailed')
                          : t('startup.downloading', {
                              name: download.name ?? t('activity.modelDownload'),
                            })}
                      </span>
                      {percent !== null && download.state === 'running' && (
                        <span className='shrink-0 tabular-nums'>{percent}%</span>
                      )}
                    </div>
                    {download.state === 'running' && (
                      <div
                        role='progressbar'
                        aria-valuemin={0}
                        aria-valuemax={100}
                        aria-valuenow={percent ?? undefined}
                        className='mt-2 h-1 overflow-hidden rounded-full bg-muted'
                      >
                        <div
                          className={`h-full rounded-full bg-primary ${percent === null ? 'w-1/2' : ''}`}
                          style={percent === null ? undefined : { width: `${percent}%` }}
                        />
                      </div>
                    )}
                  </div>
                )
              })}
            </div>
          )}
        </section>
      </main>
    </div>
  )
}

interface PowerOnScreenProps extends StartupViewProps {
  media: PowerOnMedia
}

export function PowerOnScreen({ media, onDismiss, ready }: PowerOnScreenProps) {
  const [visible, setVisible] = useState(false)
  const [fadingOut, setFadingOut] = useState(false)
  const [fadeDuration, setFadeDuration] = useState(POWER_ON_FADE_DURATION_MS)
  const [animationSettled, setAnimationSettled] = useState(media.animation === null)
  const [animationAvailable, setAnimationAvailable] = useState(media.animation !== null)
  const [logoSettled, setLogoSettled] = useState(media.logo === null)
  const [logoAvailable, setLogoAvailable] = useState(media.logo !== null)
  const videoRef = useRef<HTMLVideoElement>(null)
  const startedAt = useRef<number | null>(null)
  const fadeOutTimer = useRef<number | null>(null)
  const dismissTimer = useRef<number | null>(null)
  const playbackStarted = useRef(false)
  const fadingOutRef = useRef(false)
  const dismissedRef = useRef(false)
  const mediaSettled = animationSettled && logoSettled

  useEffect(() => {
    startedAt.current = performance.now()
    const frame = window.requestAnimationFrame(() => setVisible(true))
    const beginFadeOut = () => {
      if (fadingOutRef.current) return
      fadingOutRef.current = true
      const elapsed = startedAt.current === null ? 0 : performance.now() - startedAt.current
      setFadeDuration(
        Math.max(0, Math.min(POWER_ON_FADE_DURATION_MS, POWER_ON_MAX_DURATION_MS - elapsed)),
      )
      videoRef.current?.pause()
      setFadingOut(true)
    }
    const dismiss = () => {
      if (dismissedRef.current) return
      dismissedRef.current = true
      onDismiss()
    }
    fadeOutTimer.current = window.setTimeout(beginFadeOut, POWER_ON_FADE_OUT_AT_MS)
    dismissTimer.current = window.setTimeout(dismiss, POWER_ON_HARD_STOP_AT_MS)
    return () => {
      window.cancelAnimationFrame(frame)
      if (fadeOutTimer.current !== null) window.clearTimeout(fadeOutTimer.current)
      if (dismissTimer.current !== null) window.clearTimeout(dismissTimer.current)
    }
  }, [onDismiss])

  useEffect(() => {
    if (!ready) return
    if (fadeOutTimer.current !== null) window.clearTimeout(fadeOutTimer.current)
    if (fadingOutRef.current) return
    fadingOutRef.current = true
    const elapsed = startedAt.current === null ? 0 : performance.now() - startedAt.current
    setFadeDuration(
      Math.max(0, Math.min(POWER_ON_FADE_DURATION_MS, POWER_ON_MAX_DURATION_MS - elapsed)),
    )
    videoRef.current?.pause()
    setFadingOut(true)
  }, [ready])

  function failAnimation(
    kind: 'image' | 'video',
    event: SyntheticEvent<HTMLImageElement | HTMLVideoElement>,
  ) {
    console.warn(
      `[Norrix Power On] Could not load startup ${kind}`,
      event.currentTarget.currentSrc || event.currentTarget.src,
    )
    setAnimationSettled(true)
    setAnimationAvailable(false)
  }

  function handleImageLoad() {
    setAnimationSettled(true)
    setAnimationAvailable(true)
  }

  function handleVideoCanPlay() {
    setAnimationSettled(true)
    setAnimationAvailable(true)
    if (playbackStarted.current || fadingOutRef.current) return
    playbackStarted.current = true
    const video = videoRef.current
    if (!video) return
    video.currentTime = 0
    video.muted = false
    void video.play().catch((error: unknown) => {
      console.warn('[Norrix Power On] Could not autoplay startup video with audio', error)
      video.muted = true
      void video.play().catch((retryError: unknown) => {
        console.warn('[Norrix Power On] Could not autoplay startup video muted', retryError)
      })
    })
  }

  function handleLogoLoad() {
    setLogoSettled(true)
    setLogoAvailable(true)
  }

  function handleLogoError(event: SyntheticEvent<HTMLImageElement>) {
    console.warn(
      '[Norrix Power On] Could not load startup logo',
      event.currentTarget.currentSrc || event.currentTarget.src,
    )
    setLogoSettled(true)
    setLogoAvailable(false)
  }

  function handleTransitionEnd(event: TransitionEvent<HTMLDivElement>) {
    if (event.target === event.currentTarget && event.propertyName === 'opacity' && fadingOut) {
      if (dismissedRef.current) return
      dismissedRef.current = true
      if (dismissTimer.current !== null) window.clearTimeout(dismissTimer.current)
      onDismiss()
    }
  }

  return (
    <div
      aria-hidden='true'
      className='power-on-screen'
      data-visible={visible}
      data-fading={fadingOut}
      style={{ ['--power-on-fade-duration' as string]: `${fadeDuration}ms` }}
      onTransitionEnd={handleTransitionEnd}
    >
      {media.animation?.kind === 'image' && (
        <img
          className='power-on-animation'
          src={media.animation.src}
          alt=''
          draggable={false}
          style={{ visibility: mediaSettled && animationAvailable ? 'visible' : 'hidden' }}
          onLoad={handleImageLoad}
          onError={(event) => failAnimation('image', event)}
        />
      )}
      {media.animation?.kind === 'video' && (
        <video
          ref={videoRef}
          className='power-on-animation'
          src={media.animation.src}
          playsInline
          preload='auto'
          style={{ visibility: mediaSettled && animationAvailable ? 'visible' : 'hidden' }}
          onCanPlay={handleVideoCanPlay}
          onError={(event) => failAnimation('video', event)}
        />
      )}
      {media.logo && (
        <img
          className='power-on-logo'
          src={media.logo}
          alt=''
          draggable={false}
          style={{ visibility: mediaSettled && logoAvailable ? 'visible' : 'hidden' }}
          onLoad={handleLogoLoad}
          onError={handleLogoError}
        />
      )}
    </div>
  )
}

export function StartupLoadingView() {
  return <div aria-busy='true' className='h-screen w-screen bg-black' />
}
