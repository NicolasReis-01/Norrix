#!/usr/bin/env bun

import { exec as execCallback } from 'node:child_process'
import { readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { promisify } from 'node:util'

const exec = promisify(execCallback)
const root = path.resolve(__dirname, '..')
const execOpts = { cwd: root, maxBuffer: 10 * 1024 * 1024 }

async function main() {
  let bumpedVersion = process.argv[2]?.trim()

  if (bumpedVersion) {
    console.log(`Using provided version: ${bumpedVersion}`)
  } else {
    console.log('Calculating bumped version with git-cliff...')
    bumpedVersion = (
      await exec('bun git-cliff --offline --unreleased --bumped-version', execOpts)
    ).stdout.trim()
  }

  if (!bumpedVersion) {
    throw new Error('git-cliff did not return a bumped version')
  }

  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(bumpedVersion)) {
    throw new Error(`Invalid release version: ${bumpedVersion}`)
  }

  const { stdout: status } = await exec('git status --porcelain', execOpts)
  if (status.trim()) {
    throw new Error('Release requires a clean working tree')
  }

  console.log(`Bumped version: ${bumpedVersion}`)

  const { stdout: existingTags } = await exec(`git tag --list ${bumpedVersion}`, execOpts)
  if (existingTags.trim()) {
    throw new Error(`Git tag ${bumpedVersion} already exists`)
  }

  const cargoTomlPath = path.join(root, 'Cargo.toml')
  const cargoToml = await readFile(cargoTomlPath, 'utf8')
  const versionPattern = /(\[workspace\.package\][\s\S]*?version\s*=\s*")([^"]+)(")/

  if (!versionPattern.test(cargoToml)) {
    throw new Error('Could not find [workspace.package] version in Cargo.toml')
  }

  const updatedCargoToml = cargoToml.replace(versionPattern, `$1${bumpedVersion}$3`)
  await writeFile(cargoTomlPath, updatedCargoToml)
  console.log('Updated Cargo.toml version')

  await exec('cargo metadata --format-version 1', execOpts)
  console.log('Updated Cargo.lock')

  await exec(`bun git-cliff --offline -o CHANGELOG.md`, execOpts)
  console.log('Updated CHANGELOG.md')

  await exec('git add Cargo.toml Cargo.lock CHANGELOG.md', execOpts)
  await exec(`git commit -m "chore(release): ${bumpedVersion}"`, execOpts)
  console.log('Created release commit')

  await exec(`git tag ${bumpedVersion}`, execOpts)
  console.log('Created git tag')

  console.log(`Release commit and tag ${bumpedVersion} created.`)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
