'use client'

import { Pipette, SquareSlash } from 'lucide-react'
import { useEffect, useState } from 'react'
import { HexColorInput, HexColorPicker } from 'react-colorful'
import { useTranslation } from 'react-i18next'

import { useColorSampling } from '@/components/controls/ColorSampling'
import { Button } from '@norrix/ui/components/button'
import { Popover, PopoverContent, PopoverTrigger } from '@norrix/ui/components/popover'
import { cn } from '@norrix/ui/lib/utils'

type ColorWellProps = {
  label?: string
  size?: 'default' | 'sm'
  disabled?: boolean
} & (
  | {
      value: string
      allowTransparent?: false
      onChange: (color: string) => void
    }
  | {
      value: string | null
      allowTransparent: true
      onChange: (color: string | null) => void
    }
)

export function ColorWell(props: ColorWellProps) {
  const { t } = useTranslation()
  const { value, label, size = 'default', disabled = false, allowTransparent = false } = props
  const [draft, setDraft] = useState(value ?? '#000000')
  const [open, setOpen] = useState(false)
  const sampling = useColorSampling()
  const accessibleLabel = label ?? t('colorPicker.brushColor')
  const transparent = allowTransparent && value === null

  useEffect(() => {
    if (value === null) {
      setDraft('#000000')
      return
    }
    setDraft(normalize(value))
  }, [value])

  const apply = (next: string | null) => {
    if (next === null) {
      if (!props.allowTransparent) return
      setDraft('#000000')
      props.onChange(null)
      return
    }
    const normalized = normalize(next)
    setDraft(normalized)
    props.onChange(normalized)
  }

  const set = (color: string) => apply(color)

  const clear = () => apply(null)

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        aria-label={accessibleLabel}
        disabled={disabled}
        className={cn(
          'grid place-items-center border border-input bg-background disabled:cursor-not-allowed disabled:opacity-40',
          size === 'sm' ? 'size-6 rounded-md' : 'size-8 rounded-lg',
        )}
      >
        {transparent ? (
          <SquareSlash
            aria-hidden='true'
            className={cn('text-muted-foreground', size === 'sm' ? 'size-3.5' : 'size-4')}
          />
        ) : (
          <span
            className={cn(
              'rounded-[3px] ring-1 ring-black/15',
              size === 'sm' ? 'size-3' : 'size-4',
            )}
            style={{ backgroundColor: draft }}
          />
        )}
      </PopoverTrigger>
      <PopoverContent side='right' align='start' className='w-60 rounded-xl p-3'>
        <div>
          <HexColorPicker
            color={draft}
            onChange={(color) => setDraft(normalize(color))}
            onChangeEnd={set}
          />
        </div>
        <div className='mt-3 flex gap-2'>
          <HexColorInput
            prefixed
            color={draft}
            aria-label={t('colorPicker.hexCode')}
            className='h-8 min-w-0 flex-1 rounded-lg border border-input bg-background px-2 font-mono text-[11px] uppercase outline-none focus:border-ring'
            onChange={set}
          />
          {sampling && (
            <Button
              size='icon'
              variant='outline'
              aria-label={t('colorPicker.sampleCanvas')}
              onClick={() => {
                setOpen(false)
                sampling.request(set)
              }}
            >
              <Pipette />
            </Button>
          )}
        </div>
        {allowTransparent && (
          <Button
            type='button'
            size='sm'
            variant={transparent ? 'secondary' : 'ghost'}
            className='mt-2 w-full justify-start gap-2'
            onClick={clear}
          >
            <SquareSlash aria-hidden='true' className='size-3.5 text-muted-foreground' />
            {t('colorPicker.transparent')}
          </Button>
        )}
      </PopoverContent>
    </Popover>
  )
}

function normalize(value: string): string {
  const digits = value.startsWith('#') ? value.slice(1) : value
  const expanded =
    digits.length === 3 ? [...digits].map((digit) => digit.repeat(2)).join('') : digits
  return `#${expanded}`.toUpperCase()
}
