import { Slider as SliderPrimitive } from '@base-ui/react/slider'

import { cn } from '@norrix/ui/lib/utils'

type SliderProps = Omit<
  SliderPrimitive.Root.Props<number>,
  'value' | 'defaultValue' | 'onValueChange'
> & {
  value?: number
  defaultValue?: number
  onValueChange?: (value: number) => void
}

function Slider({
  className,
  defaultValue,
  value,
  min = 0,
  max = 100,
  step = 1,
  onValueChange,
  ...props
}: SliderProps) {
  const singleValue = value ?? defaultValue ?? min

  const normalizeValue = (nextValue: number | readonly number[] | undefined, fallback: number) => {
    if (Array.isArray(nextValue)) {
      const first = nextValue[0]
      return Number.isFinite(first) ? first : fallback
    }
    return Number.isFinite(nextValue) ? nextValue : fallback
  }

  const handleRootValueChange = (nextValue: number | readonly number[]) => {
    if (!onValueChange) return
    onValueChange(normalizeValue(nextValue, singleValue))
  }

  const handleNativeChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const next = Number(event.target.value)
    if (!Number.isFinite(next)) return
    onValueChange?.(next)
  }

  const { 'aria-label': ariaLabel, 'aria-labelledby': ariaLabelledBy, ...rest } = props

  return (
    <SliderPrimitive.Root
      className={cn('data-horizontal:w-full data-vertical:h-full', className)}
      data-slot='slider'
      defaultValue={defaultValue}
      value={singleValue}
      min={min}
      max={max}
      step={step}
      thumbAlignment='edge'
      onValueChange={(nextValue) => handleRootValueChange(nextValue as number | readonly number[])}
      aria-label={ariaLabel}
      aria-labelledby={ariaLabelledBy}
      {...rest}
    >
      <input
        type='range'
        aria-label={props['aria-label']}
        aria-labelledby={props['aria-labelledby']}
        min={min}
        max={max}
        step={step}
        value={Number(singleValue)}
        onChange={handleNativeChange}
        className='pointer-events-none absolute inset-0 h-full w-full opacity-0'
      />
      <SliderPrimitive.Control className='relative flex w-full touch-none items-center select-none data-disabled:opacity-50 data-vertical:h-full data-vertical:min-h-40 data-vertical:w-auto data-vertical:flex-col'>
        <SliderPrimitive.Track
          data-slot='slider-track'
          className='relative grow overflow-hidden rounded-full bg-muted select-none data-horizontal:h-1 data-horizontal:w-full data-vertical:h-full data-vertical:w-1'
        >
          <SliderPrimitive.Indicator
            data-slot='slider-range'
            className='bg-primary select-none data-horizontal:h-full data-vertical:w-full'
          />
        </SliderPrimitive.Track>
        <SliderPrimitive.Thumb
          data-slot='slider-thumb'
          aria-label={props['aria-label']}
          aria-labelledby={props['aria-labelledby']}
          className='relative block size-3 shrink-0 rounded-full border border-ring bg-white ring-ring/50 transition-[color,box-shadow] select-none after:absolute after:-inset-2 hover:ring-3 focus-visible:ring-3 focus-visible:outline-hidden active:ring-3 disabled:pointer-events-none disabled:opacity-50'
        />
      </SliderPrimitive.Control>
    </SliderPrimitive.Root>
  )
}

export { Slider }
