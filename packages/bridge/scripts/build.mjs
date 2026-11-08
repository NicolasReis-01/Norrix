import { spawn } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const bridgeDirectory = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repositoryRoot = resolve(bridgeDirectory, '../..')
const crateDirectory = resolve(repositoryRoot, 'crates/norrix-canvas')
const outputDirectory = resolve(bridgeDirectory, 'src/wasm')

const build = spawn(
  process.execPath,
  [
    'x',
    'wasm-pack@0.15.0',
    'build',
    crateDirectory,
    '--release',
    '--target',
    'web',
    '--out-dir',
    outputDirectory,
    '--out-name',
    'norrix_canvas',
    '--no-pack',
  ],
  { cwd: repositoryRoot, stdio: 'inherit' },
)

build.on('error', (error) => {
  console.error(`Could not start the WASM build: ${error.message}`)
  process.exitCode = 1
})
build.on('exit', (code, signal) => {
  if (signal) {
    console.error(`WASM build was terminated by ${signal}.`)
    process.exitCode = 1
  } else {
    process.exitCode = code ?? 1
  }
})
