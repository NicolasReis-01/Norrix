'use client'

import {
  Brush,
  Eraser,
  Hand,
  Minus,
  MousePointer2,
  Pipette,
  Plus,
  Sparkles,
  Type,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { ColorWell } from '@/components/controls/ColorWell'
import { call } from '@/lib/backend'
import { pageKey, pagesKey, projectKey, refresh, usePages, useWorkspacePage } from '@/lib/queries'
import {
  isBrushTool,
  MAX_BRUSH_DIAMETER,
  MIN_BRUSH_DIAMETER,
  useNorrixStore,
  type CanvasTool,
} from '@/lib/store'
import { commands } from '@norrix/bridge/protocol'
import { Button } from '@norrix/ui/components/button'
import {
  NumberField,
  NumberFieldDecrement,
  NumberFieldGroup,
  NumberFieldIncrement,
  NumberFieldInput,
} from '@norrix/ui/components/number-field'
import {
  Popover,
  PopoverContent,
  PopoverTitle,
  PopoverTrigger,
} from '@norrix/ui/components/popover'
import { Slider } from '@norrix/ui/components/slider'
import { Tooltip, TooltipContent, TooltipTrigger } from '@norrix/ui/components/tooltip'

const tools = [
  ['select', MousePointer2],
  ['text', Type],
  ['draw', Brush],
  ['eraser', Eraser],
  ['color_picker', Pipette],
  ['remove', Sparkles],
  ['pan', Hand],
] as const satisfies ReadonlyArray<readonly [CanvasTool, typeof MousePointer2]>

export function ToolBar() {
  const { t } = useTranslation()
  const page = useWorkspacePage().data
  const active = useNorrixStore((state) => state.tool)
  const brush = useNorrixStore((state) => state.brush)
  const setTool = useNorrixStore((state) => state.setTool)
  const setBrush = useNorrixStore((state) => state.setBrush)
  const shortcuts = useNorrixStore((state) => state.shortcuts)
  const selectedPages = useNorrixStore((state) => state.selectedPages)
  const selectedLayers = useNorrixStore((state) => state.selectedLayers)
  const pages = usePages().data ?? []
  const hasBrush = isBrushTool(active)
  const selectedText = page?.layers?.find(
    (layer) => layer.type === 'text' && selectedLayers.includes(layer.id),
  )
  const pageTargets = selectedPages.length > 1 ? selectedPages : page ? [page.id] : []
  const targetPages = pages.filter((candidate) => pageTargets.includes(candidate.id))
  const pageSelectionIsAmbiguous = selectedLayers.length > 0
  const canEditTranslation = selectedLayers.length === 1 && selectedText !== undefined

  return (
    <aside className='absolute top-3 left-3 z-20 flex w-12 flex-col rounded-2xl border border-border bg-[var(--surface-floating)] p-1 shadow-[var(--shadow-toolrail)]'>
      <div className='flex flex-col items-center py-0.5'>
        {tools.map(([tool, Icon], index) => (
          <div key={tool} className='contents'>
            {index === tools.length - 1 && <span className='my-1 h-px w-5 bg-border' />}
            <Tooltip>
              <TooltipTrigger
                render={
                  <Button
                    type='button'
                    variant='ghost'
                    size='icon'
                    disabled={!page}
                    aria-label={t(`tools.${tool}`)}
                    data-active={active === tool}
                    className='relative text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground disabled:opacity-30 data-[active=true]:bg-accent data-[active=true]:text-accent-foreground'
                    onClick={() => setTool(tool)}
                  />
                }
              >
                <Icon className='size-4' />
              </TooltipTrigger>
              <TooltipContent side='right'>
                {t(`tools.${tool}`)}
                <span className='ml-2 opacity-60'>{shortcuts[tool].toUpperCase()}</span>
              </TooltipContent>
            </Tooltip>
          </div>
        ))}
      </div>

      <div className='flex flex-col items-center gap-0.5 border-t border-border/80 py-1.5'>
        {(
          [
            ['detection', 'DTC', 'detection_complete'],
            ['ocr', 'OCR', 'ocr_complete'],
            ['translation', 'TRD', 'translation_complete'],
            ['inpainting', 'PRE', 'inpainting_complete'],
          ] as const
        ).map(([kind, label, field]) => {
          const isTranslation = kind === 'translation'
          const disabled = isTranslation
            ? !canEditTranslation
            : !targetPages.length || pageSelectionIsAmbiguous
          const isComplete = isTranslation
            ? Boolean(selectedText?.type === 'text' && selectedText.content.translation_complete)
            : targetPages.length > 0 && targetPages.every((candidate) => candidate[field])
          const title = isTranslation ? t('progress.textMarker') : t('progress.pageMarker')
          return (
            <Button
              key={kind}
              type='button'
              variant='ghost'
              size='icon'
              aria-label={`${label} — ${title}`}
              aria-pressed={isComplete}
              disabled={disabled}
              data-complete={isComplete}
              className='h-6 w-9 rounded-lg px-1 text-[9px] font-semibold tracking-tight text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground disabled:opacity-30 data-[complete=true]:bg-primary/10 data-[complete=true]:text-primary'
              onClick={() => {
                if (disabled) return
                const textLayer = isTranslation ? (selectedText?.id ?? null) : null
                const pageIds = isTranslation ? [] : targetPages.map((candidate) => candidate.id)
                void call(commands.setProgressMarkers, pageIds, textLayer, kind, !isComplete)
                  .then(() => refresh(projectKey, pagesKey, pageKey))
                  .catch((error: unknown) => {
                    console.error('Failed to update pipeline progress marker', error)
                  })
              }}
            >
              {label}
            </Button>
          )
        })}
      </div>

      {hasBrush && (
        <div className='flex flex-col items-center gap-1 border-t border-border/80 py-1.5'>
          {active === 'draw' && (
            <ColorWell value={brush.color} onChange={(color) => setBrush({ ...brush, color })} />
          )}
          <BrushSize
            value={brush.diameter}
            onChange={(diameter) => setBrush({ ...brush, diameter })}
          />
        </div>
      )}
    </aside>
  )
}

function BrushSize({ value, onChange }: { value: number; onChange: (value: number) => void }) {
  const { t } = useTranslation()
  const roundedValue = Math.round(value)

  return (
    <Popover>
      <Tooltip>
        <PopoverTrigger
          render={
            <TooltipTrigger
              render={
                <Button
                  type='button'
                  variant='ghost'
                  size='icon'
                  aria-label={t('tools.brushSizePixels', { size: roundedValue })}
                  className='size-8 rounded-xl font-sans text-[11px] leading-none font-medium tracking-[-0.02em] text-muted-foreground tabular-nums hover:bg-foreground/[0.06] hover:text-foreground'
                />
              }
            >
              {roundedValue}
            </TooltipTrigger>
          }
        />
        <TooltipContent side='right'>
          {t('tools.brushSizeShort', { size: roundedValue })}
        </TooltipContent>
      </Tooltip>
      <PopoverContent
        side='right'
        align='center'
        sideOffset={8}
        className='w-48 gap-2.5 rounded-xl p-2.5'
      >
        <div className='flex items-center justify-between gap-3'>
          <PopoverTitle className='text-[11px]'>{t('tools.brushSize')}</PopoverTitle>
          <NumberField
            min={MIN_BRUSH_DIAMETER}
            max={MAX_BRUSH_DIAMETER}
            step={1}
            value={roundedValue}
            className='w-20'
            onValueChange={(next) => {
              if (next !== null) onChange(clamp(next, MIN_BRUSH_DIAMETER, MAX_BRUSH_DIAMETER))
            }}
          >
            <NumberFieldGroup className='h-7'>
              <NumberFieldDecrement aria-label={t('tools.decreaseBrushSize')}>
                <Minus />
              </NumberFieldDecrement>
              <NumberFieldInput aria-label={t('tools.brushSizeInput')} />
              <NumberFieldIncrement aria-label={t('tools.increaseBrushSize')}>
                <Plus />
              </NumberFieldIncrement>
            </NumberFieldGroup>
          </NumberField>
        </div>
        <Slider
          aria-label={t('tools.brushSize')}
          min={MIN_BRUSH_DIAMETER}
          max={MAX_BRUSH_DIAMETER}
          step={1}
          value={value}
          className='py-1 [&_[data-slot=slider-thumb]]:size-2.5'
          onValueChange={onChange}
        />
      </PopoverContent>
    </Popover>
  )
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, Number.isFinite(value) ? value : min))
}
