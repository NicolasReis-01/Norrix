'use client'

import { ArrowRight, Replace } from 'lucide-react'
import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
} from 'react'
import { useTranslation } from 'react-i18next'

import { call } from '@/lib/backend'
import { expandLayerSelection, isTextLayer } from '@/lib/document'
import {
  pageKey,
  pagesKey,
  preparedPageKey,
  projectKey,
  queryClient,
  refresh,
  useWorkspacePage,
} from '@/lib/queries'
import { receiveError, useNorrixStore } from '@/lib/store'
import { transformTextCase, type TextCase } from '@/lib/text-format'
import {
  commands,
  type Layer,
  type Page,
  type TextContentUpdate,
  type Typography,
  type TypographyUpdate,
} from '@norrix/bridge/protocol'
import { Button } from '@norrix/ui/components/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@norrix/ui/components/dialog'
import { Slider } from '@norrix/ui/components/slider'

const defaultTypography: Typography = {
  preferred_font: null,
  font_weight: 400,
  font_style: 'normal',
  size: null,
  auto_fit: true,
  color: [0, 0, 0, 255],
  stroke_color: [255, 255, 255, 255],
  stroke_width: 0,
  alignment: null,
  writing_mode: null,
  line_height: 1.2,
  letter_spacing: 0,
}

const textCases: { mode: TextCase; symbol: string; key: string; fallback: string }[] = [
  { mode: 'uppercase', symbol: 'AA', key: 'uppercase', fallback: 'Uppercase' },
  { mode: 'lowercase', symbol: 'aa', key: 'lowercase', fallback: 'Lowercase' },
  { mode: 'sentence', symbol: 'Aa', key: 'sentence', fallback: 'Sentence case' },
  { mode: 'title', symbol: 'AaAa', key: 'word', fallback: 'Word case' },
]

type TextTarget = { page: Page; layer: Extract<Layer, { type: 'text' }> }
export function useTypographyMenu(disabled: boolean): {
  tools: ReactNode
  dialog: ReactNode
} {
  const { t } = useTranslation()
  const selectedPage = useNorrixStore((state) => state.selectedPage)
  const workspacePage = useWorkspacePage().data
  const page =
    workspacePage ??
    (selectedPage
      ? queryClient.getQueryData<{ page: Page } | null>(preparedPageKey(selectedPage))?.page
      : queryClient.getQueryData<Page | null>(pageKey)) ??
    null
  const selectedIds = useNorrixStore((state) => state.selectedLayers)
  const setTypographyPreview = useNorrixStore((state) => state.setTypographyPreview)
  const selectionKey = `${selectedPage ?? page?.id ?? ''}:${selectedIds.join(',')}`
  const allPages = useRef<Promise<Page[]> | null>(null)
  const targetsCache = useRef<TextTarget[] | null>(null)
  const previewLayerIds = useRef<Set<string> | null>(null)
  const [spacing, setSpacing] = useState({ line_height: 1.2, letter_spacing: 0 })
  const pendingSpacing = useRef(spacing)
  const [replaceOpen, setReplaceOpen] = useState(false)
  const [findText, setFindText] = useState('')
  const [replacementText, setReplacementText] = useState('')
  const [matchCount, setMatchCount] = useState(0)
  const [replacedCount, setReplacedCount] = useState<number | null>(null)
  const countRequest = useRef(0)
  const countMatchesRef = useRef<(term: string) => void>(() => undefined)
  const selectedText = page
    ? (() => {
        const expandedSelection = new Set(expandLayerSelection(page.layers, selectedIds))
        return page.layers.filter(
          (layer): layer is Extract<Layer, { type: 'text' }> =>
            isTextLayer(layer) && expandedSelection.has(layer.id),
        )
      })()
    : []
  const hasSelectedText = selectedText.length > 0
  const selectedLineHeight = selectedText[0]?.typography?.line_height
  const selectedLetterSpacing = selectedText[0]?.typography?.letter_spacing

  countMatchesRef.current = (term) => {
    const request = ++countRequest.current
    if (!term) {
      setMatchCount(0)
      return
    }
    void loadTargets()
      .then((targets) => {
        if (request !== countRequest.current) return
        setMatchCount(
          targets.reduce((count, { layer }) => {
            const text = layer.content.translation?.text ?? layer.content.source?.text ?? ''
            return count + occurrenceCount(text, term)
          }, 0),
        )
      })
      .catch(reportTypographyError)
  }

  useEffect(() => {
    allPages.current = null
    targetsCache.current = null
    previewLayerIds.current = null
    setTypographyPreview(null)
  }, [selectionKey, page?.id, setTypographyPreview])

  useEffect(
    () => () => {
      setTypographyPreview(null)
    },
    [setTypographyPreview],
  )

  useEffect(() => {
    if (hasSelectedText) {
      const next = {
        line_height: selectedLineHeight ?? 1.2,
        letter_spacing: selectedLetterSpacing ?? 0,
      }
      pendingSpacing.current = next
      setSpacing(next)
    }
  }, [hasSelectedText, selectedLineHeight, selectedLetterSpacing])

  useEffect(() => {
    if (!replaceOpen) return
    setReplacedCount(null)
    setMatchCount(0)
    countMatchesRef.current(findText)
    return () => {
      countRequest.current += 1
    }
  }, [findText, page?.id, replaceOpen, selectionKey])

  const loadPages = async (): Promise<Page[]> => {
    if (selectedText.length > 0 && page) return [page]
    if (!allPages.current) {
      allPages.current = call(commands.getPages).then((summaries) =>
        Promise.all(summaries.map(({ id }) => call(commands.getPageById, id))),
      )
    }
    return allPages.current
  }

  const loadTargets = async (): Promise<TextTarget[]> => {
    if (targetsCache.current) return targetsCache.current
    const pages = await loadPages()
    const targets = pages.flatMap((candidate) =>
      candidate.layers
        .filter(isTextLayer)
        .filter((layer) =>
          selectedText.length > 0 && page
            ? selectedText.some((selected) => selected.id === layer.id) && candidate.id === page.id
            : true,
        )
        .map((layer) => ({ page: candidate, layer })),
    )
    targetsCache.current = targets
    return targets
  }

  const commitText = async (updates: TextContentUpdate[]) => {
    if (updates.length === 0) return
    await call(commands.setTextContent, updates)
    targetsCache.current = null
    allPages.current = null
    await refresh(projectKey, pagesKey, pageKey)
  }

  const transformVisibleText = async (mode: TextCase) => {
    const targets = await loadTargets()
    const updates: TextContentUpdate[] = []
    for (const { layer } of targets) {
      if (layer.content.translation) {
        const text = transformTextCase(layer.content.translation.text, mode)
        if (text === layer.content.translation.text) continue
        updates.push({
          layer: layer.id,
          value: {
            field: 'translation',
            text,
          },
        })
      } else if (layer.content.source) {
        const text = transformTextCase(layer.content.source.text, mode)
        if (text === layer.content.source.text) continue
        updates.push({
          layer: layer.id,
          value: {
            field: 'source',
            text,
          },
        })
      }
    }
    await commitText(updates)
  }

  const replaceVisibleText = async () => {
    if (!findText) return
    const targets = await loadTargets()
    const updates: TextContentUpdate[] = []
    let replacedCount = 0
    for (const { layer } of targets) {
      if (layer.content.translation) {
        const text = layer.content.translation.text
        if (text.includes(findText)) {
          const replaced = text.split(findText).join(replacementText)
          if (replaced === text) continue
          replacedCount += occurrenceCount(text, findText)
          updates.push({
            layer: layer.id,
            value: {
              field: 'translation',
              text: replaced,
            },
          })
        }
      } else {
        const source = layer.content.source?.text
        if (source?.includes(findText)) {
          const replaced = source.split(findText).join(replacementText)
          if (replaced !== source) {
            replacedCount += occurrenceCount(source, findText)
            updates.push({
              layer: layer.id,
              value: { field: 'source', text: replaced },
            })
          }
        }
      }
    }
    await commitText(updates)
    setReplacedCount(replacedCount)
  }

  const previewTypography = async (field: 'line_height' | 'letter_spacing', next: number) => {
    const spacingValue = { ...pendingSpacing.current, [field]: next }
    pendingSpacing.current = spacingValue
    setSpacing(spacingValue)
    const targets = await loadTargets()
    previewLayerIds.current ??= new Set(targets.map(({ layer }) => layer.id))
    setTypographyPreview({ layerIds: previewLayerIds.current, field, value: next })
  }

  const commitTypography = async (field: 'line_height' | 'letter_spacing', finalValue: number) => {
    const next = Number(finalValue.toFixed(2))
    await previewTypography(field, next)
    const targets = await loadTargets()
    const updates: TypographyUpdate[] = targets.map((target) => {
      target.layer = {
        ...target.layer,
        typography: {
          ...(target.layer.typography ?? defaultTypography),
          [field]: next,
        },
      }
      queryClient.setQueryData<Page | null>(pageKey, (cached) =>
        cached?.id === target.page.id
          ? {
              ...cached,
              layers: cached.layers.map((layer) =>
                layer.id === target.layer.id && isTextLayer(layer) ? target.layer : layer,
              ),
            }
          : cached,
      )
      queryClient.setQueryData<{ page: Page } | null>(preparedPageKey(target.page.id), (cached) =>
        cached
          ? {
              ...cached,
              page: {
                ...cached.page,
                layers: cached.page.layers.map((layer) =>
                  layer.id === target.layer.id && isTextLayer(layer) ? target.layer : layer,
                ),
              },
            }
          : cached,
      )
      return { layer: target.layer.id, typography: target.layer.typography! }
    })
    if (!updates.length) {
      previewLayerIds.current = null
      setTypographyPreview(null)
      return
    }
    try {
      await call(commands.setTypography, updates)
      void refresh(projectKey)
    } catch (error) {
      targetsCache.current = null
      allPages.current = null
      await refresh(projectKey, pagesKey, pageKey)
      throw error
    } finally {
      previewLayerIds.current = null
      setTypographyPreview(null)
    }
  }

  const lineHeight = spacing.line_height
  const letterSpacing = spacing.letter_spacing

  return {
    tools: (
      <div className='mt-2 border-t border-border/70 pt-2' data-testid='typography-tools'>
        <div className='px-2 py-1 text-[10px] font-medium text-muted-foreground'>
          {t('inspector.caseTransform')}
        </div>
        <div className='flex justify-around px-1 pb-1'>
          {textCases.map(({ mode, symbol, key, fallback }) => (
            <Button
              key={mode}
              type='button'
              size='xs'
              variant='ghost'
              aria-label={t(`inspector.textCase.${key}`, { defaultValue: fallback })}
              className='h-7 min-w-8 px-1 text-[10px] font-semibold'
              disabled={disabled}
              onClick={() => void transformVisibleText(mode).catch(() => undefined)}
            >
              {symbol}
            </Button>
          ))}
        </div>
        <div className='my-1 h-px bg-border' />
        <SpacingControl
          label={t('inspector.lineSpacing')}
          value={lineHeight}
          min={0.5}
          max={4}
          step={0.05}
          suffix='×'
          disabled={disabled}
          onChange={(value) =>
            void previewTypography('line_height', value).catch(reportTypographyError)
          }
          onCommit={(value) =>
            void commitTypography('line_height', value).catch(reportTypographyError)
          }
        />
        <SpacingControl
          label={t('inspector.letterSpacing')}
          value={letterSpacing}
          min={-60}
          max={60}
          step={0.05}
          suffix=' px'
          disabled={disabled}
          onChange={(value) =>
            void previewTypography('letter_spacing', value).catch(reportTypographyError)
          }
          onCommit={(value) =>
            void commitTypography('letter_spacing', value).catch(reportTypographyError)
          }
        />
        <div className='my-1 h-px bg-border' />
        <Button
          type='button'
          variant='outline'
          size='sm'
          className='h-8 w-full justify-between px-2 text-xs font-medium'
          onClick={() => setReplaceOpen(true)}
        >
          <span className='flex items-center gap-2'>
            <Replace className='size-3.5 text-muted-foreground' />
            {t('inspector.replaceText', { defaultValue: 'Find and replace…' })}
          </span>
          <ArrowRight className='size-3.5 text-muted-foreground' />
        </Button>
      </div>
    ),
    dialog: (
      <Dialog open={replaceOpen} onOpenChange={setReplaceOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {t('inspector.replaceText', { defaultValue: 'Find and replace' })}
            </DialogTitle>
          </DialogHeader>
          <label className='grid gap-1 text-xs'>
            {t('inspector.findText', { defaultValue: 'Find' })}
            <input
              autoFocus
              value={findText}
              onChange={(event) => setFindText(event.currentTarget.value)}
              className='h-8 rounded-md border border-input bg-background px-2'
            />
          </label>
          <label className='grid gap-1 text-xs'>
            {t('inspector.replacementText', { defaultValue: 'Replace with' })}
            <input
              value={replacementText}
              onChange={(event) => setReplacementText(event.currentTarget.value)}
              className='h-8 rounded-md border border-input bg-background px-2'
            />
          </label>
          <div className='flex min-h-7 items-center justify-between rounded-md bg-muted/50 px-2 text-xs'>
            <span className='text-muted-foreground'>
              {replacedCount === null
                ? t('inspector.matchCount', {
                    count: matchCount,
                    defaultValue: '{{count}} matches',
                  })
                : t('inspector.replacedCount', {
                    count: replacedCount,
                    defaultValue: '{{count}} replaced',
                  })}
            </span>
            <span className='text-[10px] text-muted-foreground'>
              {selectedText.length > 0
                ? t('inspector.selectedTextScope', {
                    defaultValue: 'Selected text',
                  })
                : t('inspector.allPagesScope', {
                    defaultValue: 'All pages',
                  })}
            </span>
          </div>
          <DialogFooter>
            <Button
              disabled={!findText}
              onClick={() => void replaceVisibleText().catch(() => undefined)}
            >
              {t('inspector.replace', { defaultValue: 'Replace all' })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    ),
  }
}

function occurrenceCount(text: string, search: string): number {
  return search ? text.split(search).length - 1 : 0
}

function reportTypographyError(error: unknown) {
  receiveError(error instanceof Error ? error.message : String(error))
}

function SpacingControl({
  label,
  value,
  min,
  max,
  step,
  suffix,
  disabled,
  onChange,
  onCommit,
}: {
  label: string
  value: number
  min: number
  max: number
  step: number
  suffix: string
  disabled: boolean
  onChange: (value: number) => void
  onCommit: (value: number) => void
}) {
  const latestValue = useRef(value)
  const startValue = useRef(value)
  const interacting = useRef(false)
  latestValue.current = value
  const finishInteraction = () => {
    if (!interacting.current) return
    interacting.current = false
    if (latestValue.current !== startValue.current) onCommit(latestValue.current)
  }
  const onKeyDownCapture = (event: KeyboardEvent<HTMLDivElement>) => {
    if (
      [
        'ArrowLeft',
        'ArrowRight',
        'ArrowUp',
        'ArrowDown',
        'Home',
        'End',
        'PageUp',
        'PageDown',
      ].includes(event.key)
    ) {
      interacting.current = true
      startValue.current = latestValue.current
    }
  }
  const onPointerDownCapture = (_event: PointerEvent<HTMLDivElement>) => {
    interacting.current = true
    startValue.current = latestValue.current
  }
  return (
    <div className='px-2 py-1.5'>
      <div className='mb-1 flex items-center justify-between gap-2'>
        <span className='text-[10px] text-muted-foreground'>{label}</span>
        <span className='text-[10px] tabular-nums'>
          {value.toFixed(step < 0.1 ? 1 : 2)}
          {suffix}
        </span>
      </div>
      <Slider
        aria-label={label}
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onValueChange={(next) => {
          latestValue.current = next
          onChange(next)
        }}
        onKeyDownCapture={onKeyDownCapture}
        onKeyUpCapture={finishInteraction}
        onPointerDownCapture={onPointerDownCapture}
        onPointerUpCapture={finishInteraction}
        onPointerCancelCapture={finishInteraction}
        onValueCommitted={finishInteraction}
      />
    </div>
  )
}
