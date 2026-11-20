'use client'

import { openUrl } from '@tauri-apps/plugin-opener'
import { FilePlus2, FolderOpen, LoaderCircle, Settings } from 'lucide-react'
import Image from 'next/image'
import { useEffect, useRef, useState, type ComponentProps } from 'react'
import { useTranslation } from 'react-i18next'

import { AboutDialog } from '@/components/app/AboutDialog'
import { useMacOS, WindowControls } from '@/components/app/WindowChrome'
import { call, savePreferences } from '@/lib/backend'
import { selectableLayer } from '@/lib/geometry'
import {
  pageKey,
  pagesKey,
  projectKey,
  refresh,
  useCommand,
  useImportPages,
  usePages,
  useProject,
  useWorkspacePage,
} from '@/lib/queries'
import {
  receiveError,
  receivePreferences,
  useNorrixStore,
} from '@/lib/store'
import {
  commands,
  type Operation,
  type Scope,
  type Stage,
  type ViewPreferences,
} from '@norrix/bridge/protocol'
import {
  Menubar,
  MenubarContent as UiMenubarContent,
  MenubarItem as UiMenubarItem,
  MenubarMenu,
  MenubarSeparator as UiMenubarSeparator,
  MenubarShortcut as UiMenubarShortcut,
  MenubarSub,
  MenubarSubContent,
  MenubarSubTrigger,
  MenubarTrigger as UiMenubarTrigger,
} from '@norrix/ui/components/menubar'
import { Button } from '@norrix/ui/components/button'
import { cn } from '@norrix/ui/lib/utils'
import logo from '../../../../crates/norrix/icons/icon.png'

export function TitleBar() {
  const { t } = useTranslation()
  const [aboutOpen, setAboutOpen] = useState(false)
  const macOS = useMacOS()
  const project = useProject().data
  const pagesQuery = usePages(Boolean(project))
  const pageQuery = useWorkspacePage()
  const pages = project ? (pagesQuery.data ?? []) : []
  const page = project ? (pageQuery.data ?? null) : null
  const selectedPages = useNorrixStore((state) => state.selectedPages)
  const selectedLayers = useNorrixStore((state) => state.selectedLayers)
  const selectLayers = useNorrixStore((state) => state.selectLayers)
  const setSettingsOpen = useNorrixStore((state) => state.setSettingsOpen)
  const requestCanvasFit = useNorrixStore((state) => state.requestCanvasFit)
  const preferences = useNorrixStore((state) => state.preferences)
  const viewPreferences = preferences?.view ?? null
  const viewPreferencesRef = useRef(viewPreferences)
  const viewSaveQueue = useRef<Promise<void>>(Promise.resolve())

  useEffect(() => {
    viewPreferencesRef.current = viewPreferences
  }, [viewPreferences])

  const updateViewPreferences = (update: Partial<ViewPreferences>) => {
    const current = viewPreferencesRef.current
    if (!current) return
    const next = { ...current, ...update }
    viewPreferencesRef.current = next
    const pending = viewSaveQueue.current
      .catch(() => undefined)
      .then(async () => {
        const latest = useNorrixStore.getState().preferences
        if (!latest) throw new Error('Preferences are not loaded.')
        const saved = await savePreferences(
          latest.pipeline,
          latest.providers,
          latest.typesetting,
          next,
        )
        receivePreferences(saved)
      })
    viewSaveQueue.current = pending.then(
      () => undefined,
      () => undefined,
    )
    void pending.catch((error: unknown) =>
      receiveError(error instanceof Error ? error.message : String(error)),
    )
  }
  const { importPages, importing } = useImportPages()
  const { run: exportProject, busy: exporting } = useCommand(
    ['export-project'],
    commands.export,
    'menu.export',
  )

  const run = (scope: Scope, operation: Operation = { operation: 'full' }) =>
    void call(commands.process, scope, operation).catch(() => undefined)

  const closeProject = () => void call(commands.closeProject).catch(() => undefined)

  return (
    <>
      <header
        data-tauri-drag-region='deep'
        className='relative flex h-10 shrink-0 items-center bg-[var(--surface-titlebar)] text-[12px]'
      >
        {macOS ? (
          <div className='w-[84px] shrink-0' />
        ) : (
          <div className='relative z-10 flex h-full w-10 shrink-0 items-center justify-center rounded-br-lg'>
            <Image
              className='pointer-events-none'
              src={logo}
              alt='Norrix'
              width={17}
              height={17}
              draggable={false}
              priority
            />
          </div>
        )}
        <Menubar className='relative z-10 h-full shrink-0 gap-0 border-0 bg-transparent p-0 shadow-none'>
          <MenubarMenu>
            <MenubarTrigger>{t('menu.file')}</MenubarTrigger>
            <MenubarContent>
              <MenubarSub>
                <MenubarSubTrigger
                  disabled={!project || importing}
                  aria-busy={importing}
                  className='min-h-8 gap-1.5 px-2 py-1 text-xs'
                >
                  {importing && <LoaderCircle className='animate-spin' aria-hidden='true' />}
                  {importing ? t('navigator.importing') : t('menu.import')}
                </MenubarSubTrigger>
                <MenubarSubContent className='min-w-40 p-1'>
                  <MenubarItem disabled={importing} onClick={() => importPages('files')}>
                    <FilePlus2 />
                    {t('navigator.importFiles')}
                  </MenubarItem>
                  <MenubarItem disabled={importing} onClick={() => importPages('folder')}>
                    <FolderOpen />
                    {t('navigator.importFolder')}
                  </MenubarItem>
                </MenubarSubContent>
              </MenubarSub>
              <MenubarSub>
                <MenubarSubTrigger
                  disabled={!project || pages.length === 0 || exporting}
                  aria-busy={exporting}
                  className='min-h-8 gap-1.5 px-2 py-1 text-xs'
                >
                  {exporting && <LoaderCircle className='animate-spin' aria-hidden='true' />}
                  {t('menu.export')}
                </MenubarSubTrigger>
                <MenubarSubContent className='min-w-40 p-1'>
                  {(['png', 'psd', 'cbz'] as const).map((format) => (
                    <MenubarItem
                      key={format}
                      disabled={exporting}
                      onClick={() => exportProject(format)}
                    >
                      {format.toUpperCase()}…
                    </MenubarItem>
                  ))}
                </MenubarSubContent>
              </MenubarSub>
              <MenubarSeparator />
              <MenubarItem disabled={!project} onClick={closeProject}>
                {t('menu.closeProject')}
              </MenubarItem>
              <MenubarSeparator />
              <MenubarItem onClick={() => setSettingsOpen(true)}>
                <Settings />
                {t('menu.settings')}
              </MenubarItem>
            </MenubarContent>
          </MenubarMenu>

          <MenubarMenu>
            <MenubarTrigger>{t('menu.edit')}</MenubarTrigger>
            <MenubarContent>
              <MenubarItem
                disabled={!project?.can_undo}
                onClick={() =>
                  void call(commands.undo)
                    .then(() => refresh(projectKey, pagesKey, pageKey))
                    .catch(() => undefined)
                }
              >
                {t('menu.undo')}
                <MenubarShortcut>Ctrl+Z</MenubarShortcut>
              </MenubarItem>
              <MenubarItem
                disabled={!project?.can_redo}
                onClick={() =>
                  void call(commands.redo)
                    .then(() => refresh(projectKey, pagesKey, pageKey))
                    .catch(() => undefined)
                }
              >
                {t('menu.redo')}
                <MenubarShortcut>Ctrl+Shift+Z</MenubarShortcut>
              </MenubarItem>
              <MenubarSeparator />
              <MenubarItem
                disabled={!page}
                onClick={() =>
                  selectLayers(page?.layers.filter(selectableLayer).map((layer) => layer.id) ?? [])
                }
              >
                {t('menu.selectAllLayers')}
                <MenubarShortcut>Ctrl+A</MenubarShortcut>
              </MenubarItem>
              <MenubarItem
                disabled={selectedLayers.length === 0}
                variant='destructive'
                onClick={() =>
                  void call(commands.deleteLayers, selectedLayers)
                    .then(() => refresh(projectKey, pagesKey, pageKey))
                    .catch(() => undefined)
                }
              >
                {t('menu.delete')}
                <MenubarShortcut>Del</MenubarShortcut>
              </MenubarItem>
            </MenubarContent>
          </MenubarMenu>

          <MenubarMenu>
            <MenubarTrigger>{t('menu.process')}</MenubarTrigger>
            <MenubarContent>
              <MenubarItem
                disabled={!project || pages.length === 0}
                onClick={() => run({ scope: 'project' })}
              >
                {t('menu.processProject')}
              </MenubarItem>
              <MenubarItem
                disabled={selectedPages.length === 0}
                onClick={() => run({ scope: 'pages', value: selectedPages })}
              >
                {t('menu.processPages')}
              </MenubarItem>
              <MenubarItem
                disabled={selectedLayers.length === 0}
                onClick={() =>
                  run(
                    { scope: 'entities', value: selectedLayers },
                    { operation: 'stages', stages: ['ocr', 'translation'] },
                  )
                }
              >
                {t('menu.processLayers')}
              </MenubarItem>
              <MenubarSeparator />
              {(['detection', 'ocr', 'translation', 'inpainting'] as Stage[]).map((stage) => (
                <MenubarItem
                  key={stage}
                  disabled={!project || pages.length === 0}
                  onClick={() => run({ scope: 'project' }, { operation: 'through', stage })}
                >
                  {t('menu.runPhase', {
                    phase: t(`phase.${stage}`),
                  })}
                </MenubarItem>
              ))}
            </MenubarContent>
          </MenubarMenu>

          <MenubarMenu>
            <MenubarTrigger>{t('menu.view')}</MenubarTrigger>
            <MenubarContent className='w-[32rem] max-w-[calc(100vw-2rem)]'>
              <MenubarItem disabled={!page} onClick={requestCanvasFit}>
                {t('menu.fit')}
              </MenubarItem>
              <UiMenubarSeparator />
              <div className='grid min-w-56 gap-3 p-2'>
                <fieldset className='grid gap-1.5' disabled={!viewPreferences}>
                  <legend className='mb-1 text-[10px] font-medium text-muted-foreground'>
                    {t('menu.textBoxOutlines')}
                  </legend>
                  <div className='grid grid-cols-2 gap-1' role='group'>
                    <Button
                      type='button'
                      size='sm'
                      variant={viewPreferences?.text_box_outlines ? 'secondary' : 'ghost'}
                      aria-pressed={viewPreferences?.text_box_outlines ?? false}
                      disabled={!viewPreferences}
                      className='h-7 px-2 text-[10px]'
                      onClick={() => updateViewPreferences({ text_box_outlines: true })}
                    >
                      {t('menu.detectionOutlinesAlways')}
                    </Button>
                    <Button
                      type='button'
                      size='sm'
                      variant={viewPreferences && !viewPreferences.text_box_outlines ? 'secondary' : 'ghost'}
                      aria-pressed={viewPreferences ? !viewPreferences.text_box_outlines : false}
                      disabled={!viewPreferences}
                      className='h-7 px-2 text-[10px]'
                      onClick={() => updateViewPreferences({ text_box_outlines: false })}
                    >
                      {t('menu.textBoxOutlinesDisabled')}
                    </Button>
                  </div>
                </fieldset>
                <fieldset className='grid gap-1.5' disabled={!viewPreferences}>
                  <legend className='mb-1 text-[10px] font-medium text-muted-foreground'>
                    {t('menu.detectionOutlines')}
                  </legend>
                  <div className='grid grid-cols-3 gap-1' role='group'>
                    {(['never', 'selected', 'always'] as const).map((mode) => (
                      <Button
                        key={mode}
                        type='button'
                        size='sm'
                        variant={
                          viewPreferences?.detection_outlines === mode ? 'secondary' : 'ghost'
                        }
                        aria-pressed={viewPreferences?.detection_outlines === mode}
                        disabled={!viewPreferences}
                        className='h-7 px-2 text-[10px]'
                        onClick={() => updateViewPreferences({ detection_outlines: mode })}
                      >
                        {mode === 'never'
                          ? t('menu.detectionOutlinesNever')
                          : mode === 'selected'
                            ? t('menu.detectionOutlinesOnSelection')
                            : t('menu.detectionOutlinesAlways')}
                      </Button>
                    ))}
                  </div>
                </fieldset>
              </div>
            </MenubarContent>
          </MenubarMenu>

          <MenubarMenu>
            <MenubarTrigger>{t('menu.help')}</MenubarTrigger>
            <MenubarContent>
              <MenubarItem
                onClick={() => void openUrl('https://discord.gg/mHvHkxGnUY').catch(() => undefined)}
              >
                {t('menu.discord')}
              </MenubarItem>
              <MenubarItem
                onClick={() =>
                  void openUrl('https://github.com/koharu-rs/koharu').catch(() => undefined)
                }
              >
                {t('menu.github')}
              </MenubarItem>
              <MenubarSeparator />
              <MenubarItem onClick={() => setAboutOpen(true)}>{t('menu.about')}</MenubarItem>
            </MenubarContent>
          </MenubarMenu>
        </Menubar>
        <div className='min-w-0 flex-1' />
        <div className='pointer-events-none absolute inset-y-0 left-1/2 flex max-w-[40vw] min-w-16 -translate-x-1/2 items-center justify-center px-3 text-[11px] text-muted-foreground select-none'>
          {project ? (
            <span className='truncate'>
              <span className='font-medium text-foreground'>{project.name}</span>
              {page && (
                <>
                  <span className='mx-2'>/</span>
                  <span>{page.label}</span>
                </>
              )}
            </span>
          ) : (
            <span>Norrix</span>
          )}
        </div>

        {!macOS && <WindowControls />}
      </header>
      <AboutDialog open={aboutOpen} onOpenChange={setAboutOpen} />
    </>
  )
}

function MenubarTrigger({ className, ...props }: ComponentProps<typeof UiMenubarTrigger>) {
  return (
    <UiMenubarTrigger
      className={cn(
        'h-6 px-1.5 text-[11px] text-muted-foreground transition-colors hover:bg-primary/10 hover:text-muted-foreground aria-expanded:bg-primary/10 aria-expanded:text-muted-foreground',
        className,
      )}
      {...props}
    />
  )
}

function MenubarContent({ className, ...props }: ComponentProps<typeof UiMenubarContent>) {
  return <UiMenubarContent className={cn('min-w-44 p-1', className)} {...props} />
}

function MenubarItem({ className, ...props }: ComponentProps<typeof UiMenubarItem>) {
  return (
    <UiMenubarItem
      className={cn(
        "min-h-8 gap-1.5 px-2 py-1 text-xs [&_svg:not([class*='size-'])]:size-3.5",
        className,
      )}
      {...props}
    />
  )
}

function MenubarShortcut({ className, ...props }: ComponentProps<typeof UiMenubarShortcut>) {
  return <UiMenubarShortcut className={cn('text-[11px]', className)} {...props} />
}

function MenubarSeparator({ className, ...props }: ComponentProps<typeof UiMenubarSeparator>) {
  return <UiMenubarSeparator className={cn('my-0.5', className)} {...props} />
}
