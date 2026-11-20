'use client'

import {
  AlertTriangle,
  Check,
  CircleAlert,
  LoaderCircle,
  Pencil,
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
  type GeminiApiKeyManualStatus,
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

export function GeminiKeyManager() {
  const { t } = useTranslation()
  const [keys, setKeys] = useState<GeminiApiKeyInfo[] | null>(null)
  const [keyDraft, setKeyDraft] = useState('')
  const [editingKeyId, setEditingKeyId] = useState<string | null>(null)
  const [editDraft, setEditDraft] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [clearOpen, setClearOpen] = useState(false)
  const [testResults, setTestResults] = useState<
    Record<string, GeminiApiKeyTestResult | 'skipped' | 'error' | undefined>
  >({})
  const [bulkProgress, setBulkProgress] = useState<{ completed: number; total: number } | null>(
    null,
  )
  const [bulkSummary, setBulkSummary] = useState<{
    valid: number
    unavailable: number
    errors: number
  } | null>(null)

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

  useEffect(() => {
    if (busy !== null) return
    let active = true
    const timer = window.setInterval(() => {
      void call(commands.getGeminiApiKeys)
        .then((updated) => {
          if (active) setKeys(updated)
        })
        .catch((cause: unknown) => {
          if (active) setError(errorMessage(cause))
        })
    }, 1000)
    return () => {
      active = false
      window.clearInterval(timer)
    }
  }, [busy])

  const addKey = async () => {
    const key = keyDraft.trim()
    if (!key) {
      setError(t('settings.providers.gemini.inputRequired'))
      return
    }
    setError(null)
    setBusy('add')
    try {
      const updated = await call(commands.addGeminiApiKey, key)
      setKeys(updated)
      setKeyDraft('')
      setTestResults({})
      refreshModels()
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(null)
    }
  }

  const removeKey = async (id: string) => {
    setError(null)
    setBusy(id)
    try {
      setKeys(await call(commands.removeGeminiApiKey, id))
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

  const saveKeyEdit = async (id: string) => {
    const key = editDraft.trim()
    if (!key) {
      setError(t('settings.providers.gemini.inputRequired'))
      return
    }
    setError(null)
    setBusy(`edit:${id}`)
    try {
      setKeys(await call(commands.replaceGeminiApiKey, id, key))
      setEditingKeyId(null)
      setEditDraft('')
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
      setKeyDraft('')
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
      setKeys(await call(commands.getGeminiApiKeys))
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(null)
    }
  }

  const setManualStatus = async (id: string, status: GeminiApiKeyManualStatus | null) => {
    setError(null)
    setBusy(`status:${id}`)
    try {
      setKeys(await call(commands.setGeminiApiKeyManualStatus, id, status))
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(null)
    }
  }

  const testAllKeys = async () => {
    const configured = keys ?? []
    setError(null)
    setBulkSummary(null)
    setBulkProgress({ completed: 0, total: configured.length })
    setBusy('test-all')
    let valid = 0
    let unavailable = 0
    let errors = 0
    for (const [index, key] of configured.entries()) {
      if (key.status !== 'available') {
        unavailable += 1
        setTestResults((current) => ({ ...current, [key.id]: 'skipped' }))
      } else {
        try {
          const result = await call(commands.testGeminiApiKey, key.id)
          setTestResults((current) => ({ ...current, [key.id]: result }))
          if (result === 'verified') valid += 1
          else if (result === 'unavailable') errors += 1
          else unavailable += 1
        } catch {
          errors += 1
          setTestResults((current) => ({ ...current, [key.id]: 'error' }))
        }
      }
      setBulkProgress({ completed: index + 1, total: configured.length })
    }
    try {
      setKeys(await call(commands.getGeminiApiKeys))
    } catch (cause) {
      setError(errorMessage(cause))
    }
    setBulkProgress(null)
    setBulkSummary({ valid, unavailable, errors })
    setBusy(null)
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
            {t('settings.providers.gemini.keyCount', { count: configuredCount })}
          </Badge>
          <span className='text-[10px] text-muted-foreground'>
            {t('settings.providers.gemini.availableCount', { count: availableCount })}
          </span>
        </div>
        <div className='flex flex-wrap items-center gap-1.5'>
          <Button
            type='button'
            variant='outline'
            size='sm'
            disabled={busy !== null || configuredCount === 0}
            aria-busy={busy === 'test-all'}
            onClick={() => void testAllKeys()}
          >
            {busy === 'test-all' ? <LoaderCircle className='animate-spin' /> : <ShieldCheck />}
            {t('settings.providers.gemini.testAll')}
          </Button>
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
      </div>

      <p className='text-[10px] leading-4 text-muted-foreground'>
        {t('settings.providers.gemini.description')}
      </p>
      <p className='text-[10px] leading-4 text-muted-foreground'>
        {t('settings.providers.gemini.sessionNote')}
      </p>
      <p className='text-[10px] leading-4 text-muted-foreground'>
        {t('settings.providers.gemini.testAllNote')}
      </p>
      {bulkProgress && (
        <p role='status' className='text-[10px] text-muted-foreground'>
          {t('settings.providers.gemini.testProgress', bulkProgress)}
        </p>
      )}
      {bulkSummary && (
        <p role='status' className='text-[10px] text-muted-foreground'>
          {t('settings.providers.gemini.testSummary', bulkSummary)}
        </p>
      )}

      {keys === null && !error ? (
        <p className='text-[11px] text-muted-foreground'>{t('common.loading')}</p>
      ) : keys?.length === 0 ? (
        <p className='rounded-lg border border-dashed border-border px-3 py-4 text-center text-[11px] text-muted-foreground'>
          {t('settings.providers.gemini.noKeys')}
        </p>
      ) : (
        keys && (
          <ul
            className='grid w-full min-w-0 grid-cols-1 gap-2 lg:grid-cols-2'
            aria-label={t('settings.providers.gemini.title')}
          >
            {keys.map((key) => (
              <li
                key={key.id}
                className='grid min-w-0 content-start gap-2 rounded-lg border border-border/80 px-3 py-2'
              >
                <div className='flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1'>
                  <span className='text-[11px] font-medium'>
                    {t('settings.providers.gemini.keyLabel', { index: key.index })}
                  </span>
                  <code className='max-w-20 truncate text-[10px] text-muted-foreground'>
                    {key.id.slice(0, 8)}
                  </code>
                  <StatusBadge keyInfo={key} />
                  {key.manually_set && (
                    <span
                      className={`text-[9px] ${key.status === 'available' ? 'text-muted-foreground' : 'text-[var(--norrix-primary-dark)] dark:text-[var(--norrix-primary-light)]'}`}
                    >
                      {t('settings.providers.gemini.status.manual')}
                    </span>
                  )}
                  {testResults[key.id] && (
                    <span
                      role='status'
                      className='text-[10px] text-muted-foreground'
                      data-test-result={testResults[key.id]}
                    >
                      {t(`settings.providers.gemini.test.${testResults[key.id]}`)}
                    </span>
                  )}
                  {editingKeyId === key.id && (
                    <Input
                      type='password'
                      autoComplete='new-password'
                      autoCapitalize='none'
                      spellCheck={false}
                      value={editDraft}
                      placeholder={t('settings.providers.gemini.keyPlaceholder')}
                      className='h-7 min-w-48 text-[11px]'
                      aria-label={t('menu.edit')}
                      disabled={busy !== null}
                      onChange={(event) => setEditDraft(event.currentTarget.value)}
                    />
                  )}
                </div>
                <div className='flex min-w-0 flex-wrap items-center gap-1'>
                  <select
                    aria-label={t('settings.providers.gemini.manualStatus', { index: key.index })}
                    className='h-7 max-w-full min-w-0 rounded-md border border-input bg-transparent px-1.5 text-[9px] text-foreground'
                    value={
                      key.manually_set && key.status === 'inactiveDailyQuota'
                        ? 'inactiveDailyQuota'
                        : key.manually_set && key.status === 'rpmCooldown'
                          ? 'rpmCooldown'
                          : 'automatic'
                    }
                    disabled={busy !== null}
                    onChange={(event) => {
                      const status = event.currentTarget.value
                      void setManualStatus(
                        key.id,
                        status === 'inactiveDailyQuota' || status === 'rpmCooldown' ? status : null,
                      )
                    }}
                  >
                    <option value='automatic'>
                      {t('settings.providers.gemini.status.automatic')}
                    </option>
                    <option value='inactiveDailyQuota'>
                      {t('settings.providers.gemini.status.inactiveDailyQuota')}
                    </option>
                    <option value='rpmCooldown'>
                      {t('settings.providers.gemini.status.rpmCooldown')}
                    </option>
                  </select>
                  {editingKeyId === key.id ? (
                    <>
                      <Button
                        type='button'
                        size='sm'
                        variant='outline'
                        disabled={busy !== null || editDraft.trim().length === 0}
                        aria-busy={busy === `edit:${key.id}`}
                        onClick={() => void saveKeyEdit(key.id)}
                      >
                        {busy === `edit:${key.id}` ? (
                          <LoaderCircle className='animate-spin' />
                        ) : (
                          <Check />
                        )}
                        {t('common.save')}
                      </Button>
                      <Button
                        type='button'
                        size='sm'
                        variant='ghost'
                        disabled={busy !== null}
                        onClick={() => {
                          setEditingKeyId(null)
                          setEditDraft('')
                        }}
                      >
                        {t('common.cancel')}
                      </Button>
                    </>
                  ) : (
                    <Button
                      type='button'
                      size='icon'
                      variant='ghost'
                      disabled={busy !== null}
                      aria-label={t('menu.edit')}
                      onClick={() => {
                        setEditingKeyId(key.id)
                        setEditDraft('')
                      }}
                    >
                      <Pencil />
                    </Button>
                  )}
                  <Button
                    type='button'
                    size='sm'
                    variant='outline'
                    disabled={busy !== null || key.status !== 'available'}
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
                  <Button
                    type='button'
                    size='icon'
                    variant='ghost'
                    disabled={busy !== null}
                    aria-label={t('settings.providers.gemini.remove', {
                      id: key.index,
                    })}
                    onClick={() => void removeKey(key.id)}
                  >
                    <Trash2 />
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        )
      )}

      <div className='grid gap-2 rounded-lg border border-border/80 p-3'>
        <label htmlFor='gemini-api-key' className='text-[10px] font-medium'>
          {t('settings.providers.gemini.addKey')}
        </label>
        <div className='flex gap-2'>
          <Input
            id='gemini-api-key'
            type='password'
            autoComplete='new-password'
            autoCapitalize='none'
            spellCheck={false}
            value={keyDraft}
            placeholder={t('settings.providers.gemini.keyPlaceholder')}
            className='h-8 min-w-0 text-[12px]'
            disabled={busy !== null}
            onChange={(event) => setKeyDraft(event.currentTarget.value)}
          />
          <Button
            type='button'
            size='sm'
            disabled={busy !== null || keys === null || keyDraft.trim().length === 0}
            aria-busy={busy === 'add'}
            onClick={() => void addKey()}
          >
            {busy === 'add' ? <LoaderCircle className='animate-spin' /> : <Plus />}
            {t('settings.providers.gemini.addAction')}
          </Button>
        </div>
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
      : status === 'rpmCooldown'
        ? t('settings.providers.gemini.status.rpmCountdown', {
            seconds: keyInfo.retry_after_seconds ?? 60,
          })
        : t(`settings.providers.gemini.status.${status}`)
  const unavailable = status !== 'available'
  return (
    <Badge
      variant='outline'
      className={`text-[9px] ${unavailable ? 'border-current/40 bg-current/10 text-[var(--norrix-primary-dark)] dark:text-[var(--norrix-primary-light)]' : ''}`}
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
