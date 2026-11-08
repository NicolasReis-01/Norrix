'use client'

import {
  AlertTriangle,
  Check,
  CircleAlert,
  LoaderCircle,
  Plus,
  ShieldCheck,
  Trash2,
} from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { call, refreshTranslationModels } from '@/lib/backend'
import {
  commands,
  type GeminiApiKeyInfo,
  type GeminiApiKeyTestResult,
} from '@norrix/bridge/protocol'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogMedia,
  AlertDialogTitle,
} from '@norrix/ui/components/alert-dialog'
import { Badge } from '@norrix/ui/components/badge'
import { Button } from '@norrix/ui/components/button'
import { Input } from '@norrix/ui/components/input'

const KEY_LIMIT = 16

export function GeminiKeyManager() {
  const { t } = useTranslation()
  const [keys, setKeys] = useState<GeminiApiKeyInfo[] | null>(null)
  const [primaryDraft, setPrimaryDraft] = useState('')
  const [secondaryDraft, setSecondaryDraft] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [clearOpen, setClearOpen] = useState(false)
  const [testResults, setTestResults] = useState<
    Record<string, GeminiApiKeyTestResult | undefined>
  >({})

  useEffect(() => {
    let active = true
    void call(commands.getGeminiApiKeys)
      .then((result) => {
        if (active) setKeys(result)
      })
      .catch((cause: unknown) => {
        if (active) setError(errorMessage(cause))
      })
    return () => {
      active = false
    }
  }, [])

  const savePrimary = async () => {
    const key = primaryDraft.trim()
    if (!key) {
      setError(t('settings.providers.gemini.inputRequired'))
      return
    }
    setError(null)
    setBusy('primary')
    try {
      const updated = await call(commands.setGeminiPrimaryKey, key)
      setKeys(updated)
      setPrimaryDraft('')
      setTestResults({})
      refreshModels()
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(null)
    }
  }

  const addSecondary = async () => {
    const key = secondaryDraft.trim()
    if (!key) {
      setError(t('settings.providers.gemini.inputRequired'))
      return
    }
    setError(null)
    setBusy('add')
    try {
      const updated = await call(commands.addGeminiSecondaryKey, key)
      setKeys(updated)
      setSecondaryDraft('')
      setTestResults({})
      refreshModels()
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(null)
    }
  }

  const removeSecondary = async (id: string) => {
    setError(null)
    setBusy(id)
    try {
      setKeys(await call(commands.removeGeminiSecondaryKey, id))
      setTestResults((current) => {
        const next = { ...current }
        delete next[id]
        return next
      })
      refreshModels()
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(null)
    }
  }

  const clearKeys = async () => {
    setError(null)
    setBusy('clear')
    try {
      setKeys(await call(commands.clearGeminiApiKeys))
      setPrimaryDraft('')
      setSecondaryDraft('')
      setTestResults({})
      setClearOpen(false)
      refreshModels()
    } catch (cause) {
      setError(errorMessage(cause))
      setClearOpen(false)
    } finally {
      setBusy(null)
    }
  }

  const testKey = async (id: string) => {
    setError(null)
    setBusy(`test:${id}`)
    try {
      const result = await call(commands.testGeminiApiKey, id)
      setTestResults((current) => ({ ...current, [id]: result }))
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(null)
    }
  }

  const configuredCount = keys?.length ?? 0
  const availableCount = keys?.filter((key) => key.status === 'available').length ?? 0

  const refreshModels = () =>
    void refreshTranslationModels(true).catch(() =>
      setError(t('settings.providers.gemini.modelsRefreshFailed')),
    )

  return (
    <div className='grid gap-3'>
      <div className='flex flex-wrap items-center justify-between gap-2'>
        <div className='flex flex-wrap items-center gap-2'>
          <Badge variant='outline'>
            {t('settings.providers.gemini.keyCount', { count: configuredCount, limit: KEY_LIMIT })}
          </Badge>
          <span className='text-[10px] text-muted-foreground'>
            {t('settings.providers.gemini.availableCount', { count: availableCount })}
          </span>
        </div>
        <Button
          type='button'
          variant='outline'
          size='sm'
          disabled={busy !== null || configuredCount === 0}
          onClick={() => setClearOpen(true)}
        >
          <Trash2 /> {t('settings.providers.gemini.clearAll')}
        </Button>
      </div>

      <p className='text-[10px] leading-4 text-muted-foreground'>
        {t('settings.providers.gemini.description')}
      </p>
      <p className='text-[10px] leading-4 text-muted-foreground'>
        {t('settings.providers.gemini.sessionNote')}
      </p>

      {keys === null && !error ? (
        <p className='text-[11px] text-muted-foreground'>{t('common.loading')}</p>
      ) : keys?.length === 0 ? (
        <p className='rounded-lg border border-dashed border-border px-3 py-4 text-center text-[11px] text-muted-foreground'>
          {t('settings.providers.gemini.noKeys')}
        </p>
      ) : (
        keys && (
          <ul className='grid gap-2' aria-label={t('settings.providers.gemini.title')}>
            {keys.map((key) => (
              <li
                key={key.id}
                className='flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border/80 px-3 py-2'
              >
                <div className='flex min-w-0 flex-wrap items-center gap-2'>
                  <span className='text-[11px] font-medium'>
                    {key.is_primary
                      ? t('settings.providers.gemini.primary')
                      : t('settings.providers.gemini.secondary')}
                  </span>
                  <code className='text-[10px] text-muted-foreground'>{key.id.slice(0, 8)}</code>
                  <StatusBadge keyInfo={key} />
                  {testResults[key.id] && (
                    <span
                      role='status'
                      className='text-[10px] text-muted-foreground'
                      data-test-result={testResults[key.id]}
                    >
                      {t(`settings.providers.gemini.test.${testResults[key.id]}`)}
                    </span>
                  )}
                </div>
                <div className='flex shrink-0 items-center gap-1'>
                  <Button
                    type='button'
                    size='sm'
                    variant='outline'
                    disabled={busy !== null}
                    aria-busy={busy === `test:${key.id}`}
                    onClick={() => void testKey(key.id)}
                  >
                    {busy === `test:${key.id}` ? (
                      <LoaderCircle className='animate-spin' />
                    ) : (
                      <ShieldCheck />
                    )}
                    {busy === `test:${key.id}`
                      ? t('settings.providers.gemini.testing')
                      : t('settings.providers.gemini.testAction')}
                  </Button>
                  {!key.is_primary && (
                    <Button
                      type='button'
                      size='icon'
                      variant='ghost'
                      disabled={busy !== null}
                      aria-label={t('settings.providers.gemini.remove', {
                        id: key.id.slice(0, 8),
                      })}
                      onClick={() => void removeSecondary(key.id)}
                    >
                      <Trash2 />
                    </Button>
                  )}
                </div>
              </li>
            ))}
          </ul>
        )
      )}

      <div className='grid gap-2 rounded-lg border border-border/80 p-3'>
        <label htmlFor='gemini-primary-key' className='text-[10px] font-medium'>
          {t('settings.providers.gemini.replacePrimary')}
        </label>
        <div className='flex gap-2'>
          <Input
            id='gemini-primary-key'
            type='password'
            autoComplete='new-password'
            autoCapitalize='none'
            spellCheck={false}
            value={primaryDraft}
            placeholder={t('settings.providers.gemini.primaryPlaceholder')}
            className='h-8 min-w-0 text-[12px]'
            disabled={busy !== null}
            onChange={(event) => setPrimaryDraft(event.currentTarget.value)}
          />
          <Button
            type='button'
            size='sm'
            disabled={busy !== null || keys === null || primaryDraft.trim().length === 0}
            aria-busy={busy === 'primary'}
            onClick={() => void savePrimary()}
          >
            {busy === 'primary' ? <LoaderCircle className='animate-spin' /> : <Check />}
            {t('settings.providers.gemini.savePrimary')}
          </Button>
        </div>
      </div>

      <div className='grid gap-2 rounded-lg border border-border/80 p-3'>
        <label htmlFor='gemini-secondary-key' className='text-[10px] font-medium'>
          {t('settings.providers.gemini.addSecondary')}
        </label>
        <div className='flex gap-2'>
          <Input
            id='gemini-secondary-key'
            type='password'
            autoComplete='new-password'
            autoCapitalize='none'
            spellCheck={false}
            value={secondaryDraft}
            placeholder={t('settings.providers.gemini.secondaryPlaceholder')}
            className='h-8 min-w-0 text-[12px]'
            disabled={busy !== null || configuredCount >= KEY_LIMIT}
            onChange={(event) => setSecondaryDraft(event.currentTarget.value)}
          />
          <Button
            type='button'
            size='sm'
            disabled={
              busy !== null ||
              keys === null ||
              configuredCount >= KEY_LIMIT ||
              secondaryDraft.trim().length === 0
            }
            aria-busy={busy === 'add'}
            onClick={() => void addSecondary()}
          >
            {busy === 'add' ? <LoaderCircle className='animate-spin' /> : <Plus />}
            {t('settings.providers.gemini.addAction')}
          </Button>
        </div>
        {configuredCount >= KEY_LIMIT && (
          <p className='text-[10px] text-muted-foreground'>
            {t('settings.providers.gemini.limitReached', { limit: KEY_LIMIT })}
          </p>
        )}
      </div>

      {error && (
        <p role='alert' className='flex items-start gap-1.5 text-[11px] text-destructive'>
          <CircleAlert className='mt-0.5 size-3.5 shrink-0' />
          <span>{error}</span>
        </p>
      )}

      <AlertDialog open={clearOpen} onOpenChange={setClearOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogMedia className='bg-destructive/10 text-destructive'>
              <AlertTriangle className='size-5' />
            </AlertDialogMedia>
            <AlertDialogTitle>{t('settings.providers.gemini.clearTitle')}</AlertDialogTitle>
            <AlertDialogDescription>
              {t('settings.providers.gemini.clearDescription')}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={busy !== null}>{t('common.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              variant='destructive'
              disabled={busy !== null}
              aria-busy={busy === 'clear'}
              onClick={() => void clearKeys()}
            >
              {busy === 'clear'
                ? t('settings.providers.gemini.clearing')
                : t('settings.providers.gemini.confirmClear')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

function StatusBadge({ keyInfo }: { keyInfo: GeminiApiKeyInfo }) {
  const { t } = useTranslation()
  const status = keyInfo.status
  const label =
    status === 'coolingDown'
      ? t('settings.providers.gemini.status.coolingDown', {
          seconds: keyInfo.retry_after_seconds ?? '',
        })
      : t(`settings.providers.gemini.status.${status}`)
  return (
    <Badge
      variant={status === 'rejected' ? 'destructive' : 'outline'}
      className='text-[9px]'
      aria-label={label}
    >
      {label}
    </Badge>
  )
}

function errorMessage(error: unknown): string {
  return error instanceof Error
    ? error.message
    : 'The native application returned an unknown error.'
}
