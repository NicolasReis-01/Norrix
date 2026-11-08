import assert from 'node:assert/strict'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { chromium } from 'playwright'
import { createServer } from 'vite'

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const vite = await createServer({
  configFile: resolve(appRoot, 'vitest.config.ts'),
  root: dirname(fileURLToPath(import.meta.url)),
  server: { host: '127.0.0.1', port: 0, strictPort: true },
})
await vite.listen()
const address = vite.httpServer?.address()
if (!address || typeof address === 'string') throw new Error('Vite did not open an HTTP port')
const baseUrl = `http://127.0.0.1:${address.port}`
let browser

try {
  browser = await chromium.launch({ channel: 'msedge', headless: true })
  const page = await browser.newPage({
    viewport: { width: 1100, height: 850 },
    deviceScaleFactor: 1.5,
  })
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.goto(`${baseUrl}/inline-editor-geometry.html?frameSource=renderer`)
  await page.getByTestId('begin-edit').waitFor()

  const measure = async (label) =>
    page.evaluate((name) => {
      const rect = (element) => {
        if (!element) return null
        const bounds = element.getBoundingClientRect()
        return {
          left: bounds.left,
          top: bounds.top,
          right: bounds.right,
          bottom: bounds.bottom,
          width: bounds.width,
          height: bounds.height,
          transform: getComputedStyle(element).transform,
          transformOrigin: getComputedStyle(element).transformOrigin,
          opacity: getComputedStyle(element).opacity,
        }
      }
      const quad = (element) => {
        if (!element) return null
        const points = element.getBoxQuads?.()[0]
        if (points)
          return [points.p1, points.p2, points.p3, points.p4].map(({ x, y }) => ({ x, y }))

        const style = getComputedStyle(element)
        const matrix = new DOMMatrix(style.transform)
        const origin = style.transformOrigin.split(' ').map(Number.parseFloat)
        const parent = element.offsetParent.getBoundingClientRect()
        const left = Number.parseFloat(style.left)
        const top = Number.parseFloat(style.top)
        const width = Number.parseFloat(style.width)
        const height = Number.parseFloat(style.height)
        return [
          [0, 0],
          [width, 0],
          [width, height],
          [0, height],
        ].map(([x, y]) => ({
          x:
            parent.left +
            left +
            origin[0] +
            matrix.a * (x - origin[0]) +
            matrix.c * (y - origin[1]) +
            matrix.e,
          y:
            parent.top +
            top +
            origin[1] +
            matrix.b * (x - origin[0]) +
            matrix.d * (y - origin[1]) +
            matrix.f,
        }))
      }
      const bounds = (value) => ({
        x: value.x,
        y: value.y,
        width: value.width,
        height: value.height,
      })
      const viewport = document.querySelector('[data-testid="canvas-viewport"]')
      const overlay = document.querySelector('[data-testid="canvas-overlay"]')
      const sourceFrame = document.querySelector('[data-element="geometry-text"]')
      const editor = document.querySelector('[data-testid="inline-text-editor"]')
      const preview = document.querySelector('[data-testid="inline-text-preview"]')
      const path = preview?.querySelector('[data-testid="inline-text-glyphs"]')
      const viewportRect = viewport.getBoundingClientRect()
      const frame = window.geometryHarness.frame()
      const camera = window.geometryHarness.camera()
      const dpr = window.devicePixelRatio
      const left = (frame.x * camera.zoom + camera.translation[0]) / dpr
      const top = (frame.y * camera.zoom + camera.translation[1]) / dpr
      const width = (frame.width * camera.zoom) / dpr
      const height = (frame.height * camera.zoom) / dpr
      const centerX = left + width / 2
      const centerY = top + height / 2
      const angle = (frame.angle_degrees * Math.PI) / 180
      const projectPagePoint = (x, y) => ({
        x:
          viewportRect.left +
          ((frame.x +
            frame.width / 2 +
            Math.cos(angle) * (x - frame.width / 2) -
            Math.sin(angle) * (y - frame.height / 2)) *
            camera.zoom +
            camera.translation[0]) /
            dpr,
        y:
          viewportRect.top +
          ((frame.y +
            frame.height / 2 +
            Math.sin(angle) * (x - frame.width / 2) +
            Math.cos(angle) * (y - frame.height / 2)) *
            camera.zoom +
            camera.translation[1]) /
            dpr,
      })
      const project = ([x, y]) => ({
        x: viewportRect.left + centerX + Math.cos(angle) * x - Math.sin(angle) * y,
        y: viewportRect.top + centerY + Math.sin(angle) * x + Math.cos(angle) * y,
      })
      const pathBox = path?.getBBox()
      const expectedPathPoints = pathBox
        ? [
            projectPagePoint(pathBox.x, pathBox.y),
            projectPagePoint(pathBox.x + pathBox.width, pathBox.y),
            projectPagePoint(pathBox.x + pathBox.width, pathBox.y + pathBox.height),
            projectPagePoint(pathBox.x, pathBox.y + pathBox.height),
          ]
        : []
      return {
        stage: name,
        dpr,
        rendererProjectedCorners: [
          project([-width / 2, -height / 2]),
          project([width / 2, -height / 2]),
          project([width / 2, height / 2]),
          project([-width / 2, height / 2]),
        ],
        rendererProjectedBounds: {
          left: viewportRect.left + left,
          top: viewportRect.top + top,
          width,
          height,
          angle: frame.angle_degrees,
        },
        sourceFrame: rect(sourceFrame),
        editor: rect(editor),
        preview: rect(preview),
        sourceCorners: quad(sourceFrame),
        editorCorners: quad(editor),
        previewPathBounds: path ? bounds(path.getBBox()) : null,
        previewPathScreenBounds: path ? bounds(path.getBoundingClientRect()) : null,
        rendererProjectedPathBounds: expectedPathPoints.length
          ? {
              left: Math.min(...expectedPathPoints.map(({ x }) => x)),
              top: Math.min(...expectedPathPoints.map(({ y }) => y)),
              right: Math.max(...expectedPathPoints.map(({ x }) => x)),
              bottom: Math.max(...expectedPathPoints.map(({ y }) => y)),
            }
          : null,
        viewport: {
          left: viewportRect.left,
          top: viewportRect.top,
          width: viewportRect.width,
          height: viewportRect.height,
        },
        overlay: rect(overlay),
      }
    }, label)

  const before = await measure('before edit (renderer text bounds)')
  await page.getByTestId('begin-edit').click()
  await page.getByTestId('inline-text-preview').waitFor()
  await page.waitForTimeout(180)
  const oldDuring = await measure('old behavior: editor uses renderer text bounds')

  await page.goto(`${baseUrl}/inline-editor-geometry.html?frameSource=layout`)
  await page.getByTestId('begin-edit').waitFor()
  await page.evaluate(() => window.geometryHarness.resize())
  await page.waitForTimeout(30)
  const layoutBefore = await measure('before edit (layout frame)')
  await page.getByTestId('begin-edit').click()
  await page.getByTestId('inline-text-preview').waitFor()
  await page.waitForTimeout(180)
  const during = await measure('corrected behavior: editor uses layout frame')
  await page.evaluate(() => {
    window.geometryHarness.zoom()
  })
  await page.waitForTimeout(50)
  const updated = await measure('after zoom')
  await page.getByTestId('end-edit').click()
  await page.waitForTimeout(50)
  const after = await measure('after edit')

  const oldGlyphOffset = Math.hypot(
    oldDuring.previewPathScreenBounds.x - oldDuring.rendererProjectedPathBounds.left,
    oldDuring.previewPathScreenBounds.y - oldDuring.rendererProjectedPathBounds.top,
  )
  assert.ok(oldGlyphOffset > 5, `old glyph mismatch should be measurable; got ${oldGlyphOffset}px`)

  for (const snapshot of [layoutBefore, during, updated, after]) {
    assert.ok(snapshot.sourceFrame, 'source frame overlay should remain mounted')
    const projectedCorners = snapshot.rendererProjectedCorners
    const projectedBounds = {
      left: Math.min(...projectedCorners.map(({ x }) => x)),
      top: Math.min(...projectedCorners.map(({ y }) => y)),
      right: Math.max(...projectedCorners.map(({ x }) => x)),
      bottom: Math.max(...projectedCorners.map(({ y }) => y)),
    }
    const duringEdit = snapshot.editor !== null
    if (duringEdit) {
      for (const edge of ['left', 'top', 'right', 'bottom']) {
        assert.ok(
          Math.abs(snapshot.editor[edge] - projectedBounds[edge]) < 0.51,
          `${snapshot.stage}: textarea ${edge} differs from projected layer box`,
        )
      }
    }

    for (const [label, corners] of [['textarea', snapshot.editorCorners]]) {
      if (!snapshot.editor) continue
      assert.ok(corners, `${snapshot.stage}: ${label} corners must be measurable`)
      for (let index = 0; index < 4; index += 1) {
        assert.ok(
          Math.abs(corners[index].x - projectedCorners[index].x) < 0.51 &&
            Math.abs(corners[index].y - projectedCorners[index].y) < 0.51,
          `${snapshot.stage}: ${label} transformed corner ${index + 1} differs from canvas projection`,
        )
      }
    }
    if (snapshot.previewPathScreenBounds) {
      for (const edge of ['left', 'top', 'right', 'bottom']) {
        const actualPathEdge =
          snapshot.previewPathScreenBounds[edge] ??
          (edge === 'right'
            ? snapshot.previewPathScreenBounds.x + snapshot.previewPathScreenBounds.width
            : edge === 'bottom'
              ? snapshot.previewPathScreenBounds.y + snapshot.previewPathScreenBounds.height
              : edge === 'left'
                ? snapshot.previewPathScreenBounds.x
                : snapshot.previewPathScreenBounds.y)
        assert.ok(
          Math.abs(actualPathEdge - snapshot.rendererProjectedPathBounds[edge]) < 0.51,
          `${snapshot.stage}: preview glyph ${edge} differs from canvas projection: ${actualPathEdge} vs ${snapshot.rendererProjectedPathBounds[edge]}`,
        )
      }
    }
  }
  assert.equal(after.editor, null, 'editor should be removed after ending the edit')
  assert.ok(after.sourceFrame, 'source frame should remain after editing')
  assert.deepEqual(errors, [], 'browser should not report runtime errors')
  const brief = (snapshot) => ({
    stage: snapshot.stage,
    dpr: snapshot.dpr,
    expectedCanvasFrame: snapshot.rendererProjectedCorners,
    rendererControlFrame: snapshot.sourceFrame,
    editor: snapshot.editor,
    visiblePreviewGlyph: snapshot.previewPathScreenBounds,
    expectedCanvasGlyph: snapshot.rendererProjectedPathBounds,
  })
  console.log(
    JSON.stringify({
      before: brief(before),
      previousFrameMismatch: {
        ...brief(oldDuring),
        glyphOffsetPixels: oldGlyphOffset,
      },
      afterCorrection: brief(during),
      afterZoom: brief(updated),
      afterEdit: brief(after),
    }),
  )
} finally {
  if (browser) await browser.close()
  await vite.close()
}
