'use client'

import type { TFunction } from 'i18next'
import { ChevronDown, ChevronUp, GripVertical, LoaderCircle, X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useShallow } from 'zustand/react/shallow'

import { useNorrixStore } from '@/lib/store'
import type { Job } from '@norrix/bridge/protocol'
import { Button } from '@norrix/ui/components/button'

export function GeminiRotationBlobs() {
  const jobs = useNorrixStore(useShallow((state) => Object.values(state.jobs)))
  const activeRotations = jobs.filter((job) => job.rotation_failures.length > 0)

  if (activeRotations.length === 0) return null
  return (
    <div className='pointer-events-none fixed inset-0 z-[10020]'>
      {activeRotations.map((job, index) => (
        <RotationBlob key={job.id} job={job} initialTop={12 + index * 104} />
      ))}
    </div>
  )
}

function RotationBlob({ job, initialTop }: { job: Job; initialTop: number }) {
  const { t } = useTranslation()
  const [minimized, setMinimized] = useState(false)
  const [visible, setVisible] = useState(true)
  const [position, setPosition] = useState<BlobPosition | null>(
    () => rotationBlobPositions.get(job.id) ?? null,
  )
  const [dragging, setDragging] = useState(false)
  const blobRef = useRef<HTMLElement>(null)
  const dragRef = useRef<{ pointerId: number; offsetX: number; offsetY: number } | null>(null)
  const terminal = job.state === 'finished' || job.state === 'failed' || job.state === 'stopped'
  const currentReason = job.rotation_failures.at(-1)?.reason ?? ''
  const usingKey = job.rotation_using_key ?? job.rotation_successful_key

  useEffect(() => {
    if (!terminal) return
    const timer = window.setTimeout(() => setVisible(false), 5000)
    return () => window.clearTimeout(timer)
  }, [terminal])

  useEffect(() => {
    if (!dragging) return
    const handleMove = (event: PointerEvent) => {
      const drag = dragRef.current
      if (!drag || drag.pointerId !== event.pointerId) return
      const next = clampBlobPosition(
        event.clientX - drag.offsetX,
        event.clientY - drag.offsetY,
        blobRef.current,
      )
      rotationBlobPositions.set(job.id, next)
      setPosition(next)
    }
    const handleUp = (event: PointerEvent) => {
      if (dragRef.current?.pointerId !== event.pointerId) return
      dragRef.current = null
      setDragging(false)
    }
    window.addEventListener('pointermove', handleMove)
    window.addEventListener('pointerup', handleUp)
    window.addEventListener('pointercancel', handleUp)
    return () => {
      window.removeEventListener('pointermove', handleMove)
      window.removeEventListener('pointerup', handleUp)
      window.removeEventListener('pointercancel', handleUp)
    }
  }, [dragging, job.id])

  useEffect(() => {
    if (!position || dragging) return
    const handleResize = () => {
      const next = clampBlobPosition(position.left, position.top, blobRef.current)
      rotationBlobPositions.set(job.id, next)
      setPosition(next)
    }
    window.addEventListener('resize', handleResize)
    return () => window.removeEventListener('resize', handleResize)
  }, [dragging, job.id, position])

  useEffect(() => {
    if (!position) return
    const next = clampBlobPosition(position.left, position.top, blobRef.current)
    if (next.left === position.left && next.top === position.top) return
    rotationBlobPositions.set(job.id, next)
    setPosition(next)
  }, [job.id, minimized, position])

  if (!visible) return null

  return (
    <section
      ref={blobRef}
      role='status'
      aria-live='polite'
      data-testid={`gemini-rotation-blob-${job.id}`}
      data-minimized={minimized}
      className={`pointer-events-auto fixed w-[min(24rem,calc(100vw-1.5rem))] overflow-hidden rounded-xl border border-primary/30 bg-popover/95 text-popover-foreground shadow-xl backdrop-blur-md ${dragging ? 'cursor-grabbing' : ''}`}
      style={position ? { left: position.left, top: position.top } : { top: initialTop, right: 12 }}
    >
      <header className='flex items-center gap-2 px-3 py-2'>
        <span
          data-testid={`gemini-rotation-drag-${job.id}`}
          aria-hidden='true'
          className='flex size-5 shrink-0 cursor-grab touch-none items-center justify-center rounded text-muted-foreground select-none hover:bg-muted active:cursor-grabbing'
          onPointerDown={(event) => {
            if (event.button !== 0 && event.pointerType !== 'touch') return
            const rect = blobRef.current?.getBoundingClientRect()
            if (!rect) return
            const next = { left: rect.left, top: rect.top }
            dragRef.current = {
              pointerId: event.pointerId,
              offsetX: event.clientX - rect.left,
              offsetY: event.clientY - rect.top,
            }
            rotationBlobPositions.set(job.id, next)
            setPosition(next)
            setDragging(true)
            event.preventDefault()
          }}
        >
          <GripVertical className='size-3.5' />
        </span>
        {terminal ? (
          <span className='size-2 shrink-0 rounded-full bg-muted-foreground' />
        ) : (
          <LoaderCircle className='size-4 shrink-0 animate-spin text-primary' aria-hidden='true' />
        )}
        <span className='min-w-0 flex-1 truncate text-xs font-semibold'>
          {t('settings.providers.gemini.rotation.title')}
        </span>
        <span className='rounded bg-muted px-1.5 py-0.5 font-mono text-[9px] text-muted-foreground'>
          {job.id.slice(-6)}
        </span>
        <Button
          type='button'
          size='icon-xs'
          variant='ghost'
          aria-label={
            minimized
              ? t('settings.providers.gemini.rotation.maximize')
              : t('settings.providers.gemini.rotation.minimize')
          }
          onClick={() => setMinimized((value) => !value)}
        >
          {minimized ? <ChevronDown /> : <ChevronUp />}
        </Button>
        {terminal && (
          <Button
            type='button'
            size='icon-xs'
            variant='ghost'
            aria-label={t('settings.providers.gemini.rotation.dismiss')}
            onClick={() => setVisible(false)}
          >
            <X />
          </Button>
        )}
      </header>
      {!minimized && (
        <div className='grid gap-1 border-t border-border/70 px-3 py-2 text-[11px]'>
          {job.rotation_exhausted ? (
            <p className='font-medium text-destructive'>
              {t('settings.providers.gemini.rotation.exhausted')}
            </p>
          ) : (
            <p className='truncate text-muted-foreground'>{formatReason(currentReason, t)}</p>
          )}
          <p>
            <span className='text-muted-foreground'>
              {t('settings.providers.gemini.rotation.keysLeft')}:{' '}
            </span>
            {job.rotation_exhausted || job.rotation_keys_left.length === 0
              ? t('settings.providers.gemini.rotation.none')
              : job.rotation_keys_left.join(', ')}
          </p>
          <p>
            <span className='text-muted-foreground'>
              {t('settings.providers.gemini.rotation.using')}:{' '}
            </span>
            {job.rotation_using_key ??
              (terminal
                ? (usingKey ?? t('settings.providers.gemini.rotation.none'))
                : t('settings.providers.gemini.rotation.waiting'))}
          </p>
          {terminal && (
            <p className='text-muted-foreground'>
              {job.rotation_exhausted
                ? t('settings.providers.gemini.rotation.finishedExhausted')
                : t('settings.providers.gemini.rotation.finished')}
            </p>
          )}
        </div>
      )}
    </section>
  )
}

type BlobPosition = { left: number; top: number }

const rotationBlobPositions = new Map<string, BlobPosition>()

function clampBlobPosition(left: number, top: number, element: HTMLElement | null): BlobPosition {
  const { width, height } = element?.getBoundingClientRect() ?? { width: 0, height: 0 }
  return {
    left: Math.max(0, Math.min(left, window.innerWidth - width)),
    top: Math.max(0, Math.min(top, window.innerHeight - height)),
  }
}

function formatReason(reason: string, t: TFunction): string {
  const normalized = reason.toLowerCase()
  if (normalized.includes('requests_per_day')) {
    return t('settings.providers.gemini.rotation.dailyQuota')
  }
  if (normalized.includes('requests_per_minute')) {
    return t('settings.providers.gemini.rotation.rpm')
  }
  if (normalized.includes('tokens_per_minute')) {
    return t('settings.providers.gemini.rotation.tpm')
  }
  if (normalized.includes('authentication') || normalized.includes('rejected the api key')) {
    return t('settings.providers.gemini.rotation.authentication')
  }
  if (normalized.includes('503')) return t('settings.providers.gemini.rotation.serviceUnavailable')
  if (normalized.includes('429')) return t('settings.providers.gemini.rotation.rateLimited')
  return reason || t('settings.providers.gemini.rotation.switching')
}
