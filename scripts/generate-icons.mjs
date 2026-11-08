import { spawnSync } from 'node:child_process'
import { mkdtemp, copyFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const source = join(root, 'crates', 'norrix', 'icons', 'icon.png')
const icons = join(root, 'crates', 'norrix', 'icons')
const docsIcon = join(root, 'packages', 'docs', 'assets', 'norrix-icon.png')
const generatedNames = [
  '32x32.png',
  '128x128.png',
  '128x128@2x.png',
  'icon.ico',
  'icon.icns',
  'Square30x30Logo.png',
  'Square44x44Logo.png',
  'Square71x71Logo.png',
  'Square89x89Logo.png',
  'Square107x107Logo.png',
  'Square142x142Logo.png',
  'Square150x150Logo.png',
  'Square284x284Logo.png',
  'Square310x310Logo.png',
  'StoreLogo.png',
]
const staging = await mkdtemp(join(tmpdir(), 'norrix-icons-'))

try {
  const result = spawnSync('cargo', ['tauri', 'icon', source, '--output', staging], {
    cwd: root,
    stdio: 'inherit',
  })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error(`Tauri icon generation failed with status ${result.status}`)

  for (const name of generatedNames) {
    await copyFile(join(staging, name), join(icons, name))
  }
  await copyFile(join(staging, '128x128.png'), docsIcon)
} finally {
  await rm(staging, { recursive: true, force: true })
}
