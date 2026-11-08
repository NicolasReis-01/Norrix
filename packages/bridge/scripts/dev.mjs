import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const bridgeDirectory = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repositoryRoot = resolve(bridgeDirectory, '../..')
const require = createRequire(import.meta.url)
const nodemon = require.resolve('nodemon/bin/nodemon.js')
const buildScript = resolve(bridgeDirectory, 'scripts/build.mjs')
const watchedDirectories = [
  resolve(repositoryRoot, 'crates/norrix-canvas'),
  resolve(repositoryRoot, 'crates/norrix-rasterizer'),
]
const execCommand = `"${process.execPath}" "${buildScript}"`

const watcher = spawn(
  process.execPath,
  [
    nodemon,
    ...watchedDirectories.flatMap((directory) => ['--watch', directory]),
    '--ext',
    'rs,toml',
    '--delay',
    '250ms',
    '--on-change-only',
    '--exec',
    execCommand,
  ],
  { cwd: repositoryRoot, stdio: 'inherit' },
)

watcher.on('error', (error) => {
  console.error(`Could not start the bridge watcher: ${error.message}`)
  process.exitCode = 1
})
watcher.on('exit', (code, signal) => {
  if (signal) {
    console.error(`Bridge watcher was terminated by ${signal}.`)
    process.exitCode = 1
  } else {
    process.exitCode = code ?? 1
  }
})
