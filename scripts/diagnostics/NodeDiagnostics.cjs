'use strict'

const fs = require('node:fs')

if (!process.versions.bun && process.release && process.release.name === 'node') {
  process.on('uncaughtExceptionMonitor', (error, origin) => {
    const path = process.env.NORRIX_DIAG_NODE_LOG
    if (!path) return

    const record = {
      timestamp: new Date().toISOString(),
      pid: process.pid,
      ppid: process.ppid,
      executable: process.execPath,
      argv: process.argv,
      origin,
      error: error && error.stack ? error.stack : String(error),
    }

    try {
      fs.appendFileSync(path, `${JSON.stringify(record)}\n`, 'utf8')
    } catch {}
  })
}
